use super::commands::TransportCommand;
use super::connections::{
    ConnectionDirection, ConnectionTracker, InboundEventAcceptance, PeerBindResult,
};
use super::{SyncTransportError, SyncTransportEvent};
use crate::config::SharedSecret;
use crate::sync_protocol::{
    PeerId, SyncEvent, SyncFramePayload, SyncMessage, SyncProtocolError, TransportControlFrame,
    decode_authenticated, encode_authenticated,
};
use crate::sync_transport_io::{
    TransportEndpoint, TransportIoEvent, TransportIoHandle, TransportIoReceiver,
    TransportSendStatus,
};
use std::str;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tracing::{info, trace, warn};

const NODE_EVENT_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(super) fn spawn_worker_thread(
    mut worker: WorkerState,
    mut event_receiver: TransportIoReceiver,
    command_receiver: mpsc::Receiver<TransportCommand>,
    shutdown: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        while !shutdown.load(Ordering::Relaxed) && worker.handle.is_running() {
            worker.handle_transport_commands(&command_receiver);

            let Some(event) = event_receiver.receive_timeout(NODE_EVENT_POLL_INTERVAL) else {
                continue;
            };

            worker.handle_network_event(event);
        }

        for endpoint in worker.tracker.endpoints() {
            worker.handle.remove(endpoint);
        }

        trace!(peer_id = %worker.self_id, "stopped Resteyes sync transport worker");
    })
}

pub(super) struct WorkerState {
    self_id: PeerId,
    shared_secret: SharedSecret,
    hello: Vec<u8>,
    handle: TransportIoHandle,
    tracker: ConnectionTracker<TransportEndpoint>,
    next_sequence: u64,
    event_sender: mpsc::Sender<SyncTransportEvent>,
}

impl WorkerState {
    pub(super) fn new(
        self_id: PeerId,
        shared_secret: SharedSecret,
        hello: Vec<u8>,
        handle: TransportIoHandle,
        event_sender: mpsc::Sender<SyncTransportEvent>,
    ) -> Self {
        Self {
            self_id,
            shared_secret,
            hello,
            handle,
            tracker: ConnectionTracker::new(self_id),
            next_sequence: 1,
            event_sender,
        }
    }

    fn handle_transport_commands(&mut self, command_receiver: &mpsc::Receiver<TransportCommand>) {
        for command in command_receiver.try_iter() {
            match command {
                TransportCommand::Broadcast { event, reply } => {
                    _ = reply.send(self.broadcast_domain_event(event));
                }
                TransportCommand::Send {
                    peer_id,
                    event,
                    reply,
                } => {
                    _ = reply.send(self.send_domain_event(peer_id, event));
                }
            }
        }
    }

    fn broadcast_domain_event(&mut self, event: SyncEvent) -> Result<usize, SyncTransportError> {
        let payload = self.domain_event_payload(event)?;
        let mut sent_count = 0;

        for endpoint in self.tracker.authenticated_endpoints() {
            if self.send_payload(endpoint, &payload) {
                sent_count += 1;
            }
        }

        Ok(sent_count)
    }

    fn send_domain_event(
        &mut self,
        peer_id: PeerId,
        event: SyncEvent,
    ) -> Result<bool, SyncTransportError> {
        let Some(endpoint) = self.tracker.endpoint_for_peer(peer_id) else {
            return Ok(false);
        };

        let payload = self.domain_event_payload(event)?;
        Ok(self.send_payload(endpoint, &payload))
    }

    fn domain_event_payload(&mut self, event: SyncEvent) -> Result<Vec<u8>, SyncTransportError> {
        let sequence = self.next_sequence;
        if sequence == u64::MAX {
            return Err(SyncTransportError::SequenceExhausted);
        }

        let message = SyncMessage::event(self.self_id, sequence, event);
        let payload = encode_authenticated(&message, &self.shared_secret)
            .map_err(SyncTransportError::Protocol)?;
        self.next_sequence += 1;

        Ok(payload.into_bytes())
    }

    fn send_payload(&mut self, endpoint: TransportEndpoint, payload: &[u8]) -> bool {
        match self.handle.send(endpoint, payload) {
            TransportSendStatus::Sent => true,
            status => {
                warn!(endpoint = %endpoint, ?status, "failed to send sync domain event");
                self.remove_endpoint(endpoint);
                false
            }
        }
    }

    fn handle_network_event(&mut self, event: TransportIoEvent) {
        match event {
            TransportIoEvent::Connected(endpoint) => {
                self.tracker
                    .record_endpoint(endpoint, ConnectionDirection::Outgoing);
                self.send_hello(endpoint);
            }
            TransportIoEvent::ConnectFailed(endpoint) => {
                self.tracker.remove_endpoint(endpoint);
                warn!(endpoint = %endpoint, "sync peer connection failed");
            }
            TransportIoEvent::Accepted(endpoint) => {
                self.tracker
                    .record_endpoint(endpoint, ConnectionDirection::Incoming);
                self.send_hello(endpoint);
            }
            TransportIoEvent::Message(endpoint, bytes) => {
                self.handle_peer_message(endpoint, &bytes);
            }
            TransportIoEvent::Disconnected(endpoint) => {
                if let Some(peer_id) = self.tracker.remove_endpoint(endpoint) {
                    info!(peer_id = %peer_id, endpoint = %endpoint, "sync peer disconnected");
                    self.emit(SyncTransportEvent::PeerDisconnected(peer_id));
                }
            }
        }
    }

    fn handle_peer_message(&mut self, endpoint: TransportEndpoint, bytes: &[u8]) {
        let input = match str::from_utf8(bytes) {
            Ok(input) => input,
            Err(error) => {
                warn!(endpoint = %endpoint, %error, "sync peer sent non-UTF-8 frame");
                self.remove_endpoint(endpoint);
                return;
            }
        };

        let message = match decode_authenticated(input, &self.shared_secret) {
            Ok(message) => message,
            Err(error) => {
                warn!(endpoint = %endpoint, %error, "sync peer message authentication failed");
                self.remove_endpoint(endpoint);
                return;
            }
        };

        let sender = message.sender;
        let sequence = message.sequence;

        match message.payload {
            SyncFramePayload::Control {
                control: TransportControlFrame::PeerHello,
            } => self.handle_peer_hello(endpoint, sender),
            SyncFramePayload::Event { event } => {
                self.handle_domain_event(endpoint, sender, sequence, event);
            }
        }
    }

    fn handle_peer_hello(&mut self, endpoint: TransportEndpoint, sender: PeerId) {
        let update = self.tracker.bind_peer(endpoint, sender);
        self.remove_endpoints(update.disconnect);

        match update.result {
            PeerBindResult::Authenticated { peer_connected } => {
                if peer_connected {
                    info!(
                        peer_id = %sender,
                        endpoint = %endpoint,
                        "authenticated Resteyes sync peer"
                    );
                    self.emit(SyncTransportEvent::PeerAuthenticated(sender));
                }
            }
            PeerBindResult::RejectedSelf => {
                warn!(
                    peer_id = %sender,
                    endpoint = %endpoint,
                    "rejected sync connection from local peer id"
                );
            }
            PeerBindResult::RejectedUnknownEndpoint => {
                warn!(
                    peer_id = %sender,
                    endpoint = %endpoint,
                    "rejected sync hello from unknown endpoint"
                );
            }
        }
    }

    fn handle_domain_event(
        &mut self,
        endpoint: TransportEndpoint,
        sender: PeerId,
        sequence: u64,
        event: SyncEvent,
    ) {
        match self
            .tracker
            .accept_inbound_event(endpoint, sender, sequence)
        {
            InboundEventAcceptance::Accepted => {}
            InboundEventAcceptance::UnauthenticatedEndpoint => {
                warn!(
                    endpoint = %endpoint,
                    ?event,
                    "sync peer sent domain event before authenticated hello"
                );
                self.remove_endpoint(endpoint);
                return;
            }
            InboundEventAcceptance::SenderMismatch {
                authenticated_peer_id,
            } => {
                warn!(
                    endpoint = %endpoint,
                    authenticated_peer_id = %authenticated_peer_id,
                    frame_sender = %sender,
                    "sync peer sent frame with mismatched sender"
                );
                self.remove_endpoint(endpoint);
                return;
            }
            InboundEventAcceptance::Replayed { highest_seen } => {
                warn!(
                    endpoint = %endpoint,
                    peer_id = %sender,
                    sequence,
                    highest_seen,
                    ?event,
                    "rejected stale sync domain event sequence"
                );
                return;
            }
        }

        _ = self.event_sender.send(SyncTransportEvent::Domain {
            peer_id: sender,
            event,
        });
    }

    fn send_hello(&self, endpoint: TransportEndpoint) {
        match self.handle.send(endpoint, &self.hello) {
            TransportSendStatus::Sent => {
                trace!(endpoint = %endpoint, "sent sync peer hello");
            }
            status => {
                warn!(
                    endpoint = %endpoint,
                    ?status,
                    "failed to send sync peer hello"
                );
            }
        }
    }

    fn remove_endpoint(&mut self, endpoint: TransportEndpoint) {
        if let Some(peer_id) = self.tracker.remove_endpoint(endpoint) {
            self.emit(SyncTransportEvent::PeerDisconnected(peer_id));
        }

        self.handle.remove(endpoint);
    }

    fn remove_endpoints(&mut self, endpoints: Vec<TransportEndpoint>) {
        for endpoint in endpoints {
            self.remove_endpoint(endpoint);
        }
    }

    fn emit(&self, event: SyncTransportEvent) {
        _ = self.event_sender.send(event);
    }
}

pub(super) fn peer_hello_payload(
    self_id: PeerId,
    shared_secret: &SharedSecret,
) -> Result<Vec<u8>, SyncProtocolError> {
    encode_authenticated(
        &SyncMessage::control(self_id, 0, TransportControlFrame::PeerHello),
        shared_secret,
    )
    .map(String::into_bytes)
}
