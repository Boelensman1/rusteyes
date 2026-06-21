use super::commands::TransportCommand;
use super::connections::{
    ConnectionDirection, ConnectionTracker, EndpointRemoval, InboundEventAcceptance,
    PeerAuthenticationResult,
};
use super::session::{TransportFrameError, TransportSession};
use super::{SyncTransportError, SyncTransportEvent};
use crate::sync_protocol::{PeerId, SyncEvent, SyncFramePayload, TransportControlFrame};
use crate::sync_transport_io::{
    TransportEndpoint, TransportIoEvent, TransportIoHandle, TransportIoReceiver,
    TransportSendStatus,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use tracing::{info, trace, warn};

pub(super) fn spawn_worker_thread(
    mut worker: WorkerState,
    mut event_receiver: TransportIoReceiver,
    command_receiver: flume::Receiver<TransportCommand>,
    shutdown: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        while !shutdown.load(Ordering::Relaxed) && worker.handle.is_running() {
            match event_receiver.receive() {
                TransportIoEvent::Wake => {
                    worker.handle_transport_commands(&command_receiver);
                }
                event => {
                    worker.handle_network_event(event);
                }
            }
        }

        for endpoint in worker.tracker.endpoints() {
            worker.handle.remove(endpoint);
        }

        trace!(peer_id = %worker.self_id(), "stopped Resteyes sync transport worker");
    })
}

pub(super) struct WorkerState {
    session: TransportSession,
    handle: TransportIoHandle,
    tracker: ConnectionTracker<TransportEndpoint>,
    event_sender: flume::Sender<SyncTransportEvent>,
}

impl WorkerState {
    pub(super) fn new(
        session: TransportSession,
        handle: TransportIoHandle,
        event_sender: flume::Sender<SyncTransportEvent>,
    ) -> Self {
        let self_id = session.self_id();

        Self {
            session,
            handle,
            tracker: ConnectionTracker::new(self_id),
            event_sender,
        }
    }

    fn self_id(&self) -> PeerId {
        self.session.self_id()
    }

    fn handle_transport_commands(&mut self, command_receiver: &flume::Receiver<TransportCommand>) {
        for command in command_receiver.try_iter() {
            match command {
                TransportCommand::Broadcast { event, reply } => {
                    _ = reply.send(self.broadcast_domain_event(event));
                }
                #[cfg(test)]
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
        let payload = self.session.encode_event(event)?;
        let mut sent_count = 0;

        for endpoint in self.tracker.authenticated_endpoints() {
            if self.send_payload(endpoint, &payload) {
                sent_count += 1;
            }
        }

        Ok(sent_count)
    }

    #[cfg(test)]
    fn send_domain_event(
        &mut self,
        peer_id: PeerId,
        event: SyncEvent,
    ) -> Result<bool, SyncTransportError> {
        let Some(endpoint) = self.tracker.endpoint_for_peer(peer_id) else {
            return Ok(false);
        };

        let payload = self.session.encode_event(event)?;
        Ok(self.send_payload(endpoint, &payload))
    }

    fn send_payload(&mut self, endpoint: TransportEndpoint, payload: &[u8]) -> bool {
        match self.handle.send(endpoint, payload) {
            TransportSendStatus::Sent => true,
            status => {
                warn!(endpoint = %endpoint, ?status, "failed to send sync domain event");
                self.close_endpoint(endpoint);
                false
            }
        }
    }

    fn handle_network_event(&mut self, event: TransportIoEvent) {
        match event {
            TransportIoEvent::Wake => {}
            TransportIoEvent::Connected(endpoint) => {
                self.tracker
                    .record_endpoint(endpoint, ConnectionDirection::Outgoing);
                self.send_hello(endpoint);
            }
            TransportIoEvent::ConnectFailed(endpoint) => {
                self.close_endpoint(endpoint);
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
                self.close_endpoint(endpoint);
            }
        }
    }

    fn handle_peer_message(&mut self, endpoint: TransportEndpoint, bytes: &[u8]) {
        let message = match self.session.decode_message(bytes) {
            Ok(message) => message,
            Err(TransportFrameError::NonUtf8(error)) => {
                warn!(endpoint = %endpoint, %error, "sync peer sent non-UTF-8 frame");
                self.close_endpoint(endpoint);
                return;
            }
            Err(TransportFrameError::Protocol(error)) => {
                warn!(endpoint = %endpoint, %error, "sync peer message authentication failed");
                self.close_endpoint(endpoint);
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
        let authentication = self.tracker.authenticate_peer(endpoint, sender);
        self.close_untracked_endpoints(authentication.endpoints_to_close);

        match authentication.result {
            PeerAuthenticationResult::AuthenticatedNewPeer { peer_id } => {
                info!(
                    peer_id = %peer_id,
                    endpoint = %endpoint,
                    "authenticated Resteyes sync peer"
                );
                self.emit(SyncTransportEvent::PeerAuthenticated(peer_id));
            }
            PeerAuthenticationResult::AuthenticatedExistingPeer { .. } => {}
            PeerAuthenticationResult::RejectedSelf { peer_id } => {
                warn!(
                    peer_id = %peer_id,
                    endpoint = %endpoint,
                    "rejected sync connection from local peer id"
                );
            }
            PeerAuthenticationResult::RejectedUnknownEndpoint { peer_id } => {
                warn!(
                    peer_id = %peer_id,
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
                self.close_endpoint(endpoint);
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
                self.close_endpoint(endpoint);
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

    fn send_hello(&mut self, endpoint: TransportEndpoint) {
        match self.handle.send(endpoint, self.session.hello_payload()) {
            TransportSendStatus::Sent => {
                trace!(endpoint = %endpoint, "sent sync peer hello");
            }
            status => {
                warn!(
                    endpoint = %endpoint,
                    ?status,
                    "failed to send sync peer hello"
                );
                self.close_endpoint(endpoint);
            }
        }
    }

    fn close_endpoint(&mut self, endpoint: TransportEndpoint) {
        if let EndpointRemoval::PeerDisconnected { peer_id } =
            self.tracker.remove_endpoint(endpoint)
        {
            info!(peer_id = %peer_id, endpoint = %endpoint, "sync peer disconnected");
            self.emit(SyncTransportEvent::PeerDisconnected(peer_id));
        }

        self.handle.remove(endpoint);
    }

    fn close_untracked_endpoints(&self, endpoints: Vec<TransportEndpoint>) {
        for endpoint in endpoints {
            self.handle.remove(endpoint);
        }
    }

    fn emit(&self, event: SyncTransportEvent) {
        _ = self.event_sender.send(event);
    }
}
