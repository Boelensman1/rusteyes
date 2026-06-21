use crate::config::{SharedSecret, SyncConfig};
use crate::sync_discovery::{DiscoveredPeer, LanDiscovery, SyncDiscoveryError};
use crate::sync_protocol::{
    PeerId, SyncEvent, SyncFramePayload, SyncMessage, SyncProtocolError, TransportControlFrame,
    decode_authenticated, encode_authenticated,
};
use crate::sync_transport_io::{
    TransportEndpoint, TransportIo, TransportIoEvent, TransportIoHandle, TransportIoReceiver,
    TransportSendStatus,
};
use std::fmt;
use std::io;
#[cfg(test)]
use std::net::SocketAddr;
use std::net::ToSocketAddrs;
use std::str;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tracing::{info, trace, warn};

const PRODUCTION_LISTEN_ADDR: &str = "0.0.0.0:0";
const DISCOVERY_POLL_INTERVAL: Duration = Duration::from_millis(250);
const NODE_EVENT_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(crate) struct SyncTransport {
    io: TransportIo,
    command_sender: mpsc::Sender<TransportCommand>,
    #[cfg(test)]
    local_addr: SocketAddr,
    worker_thread: Option<JoinHandle<()>>,
    discovery_thread: Option<JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
}

impl SyncTransport {
    pub(crate) fn start(
        sync: SyncConfig,
    ) -> Result<Option<(Self, SyncInboundReceiver)>, SyncTransportError> {
        if !sync.enabled {
            return Ok(None);
        }

        let Some(shared_secret) = sync.shared_secret else {
            return Err(SyncTransportError::MissingSharedSecret);
        };

        let self_id = PeerId::generate().map_err(|error| sync_protocol_error(&error))?;
        Self::start_internal(
            self_id,
            shared_secret,
            PRODUCTION_LISTEN_ADDR,
            DiscoveryMode::Advertise,
            None,
        )
        .map(Some)
    }

    fn start_internal(
        self_id: PeerId,
        shared_secret: SharedSecret,
        listen_addr: impl ToSocketAddrs,
        discovery_mode: DiscoveryMode,
        observer: Option<mpsc::Sender<TransportNotification>>,
    ) -> Result<(Self, SyncInboundReceiver), SyncTransportError> {
        let hello = peer_hello_payload(self_id, &shared_secret)
            .map_err(|error| sync_protocol_error(&error))?;
        let (mut io, event_receiver, local_addr) =
            TransportIo::listen(listen_addr).map_err(|error| sync_listen_error(&error))?;
        let handle = io.handle();
        let (command_sender, command_receiver) = mpsc::channel();
        let (inbound_sender, inbound_receiver) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));

        let discovery = match discovery_mode {
            DiscoveryMode::Advertise => {
                match LanDiscovery::start(self_id, shared_secret.clone(), local_addr.port()) {
                    Ok(discovery) => Some(discovery),
                    Err(error) => {
                        shutdown.store(true, Ordering::Relaxed);
                        io.remove_listener();
                        io.stop();
                        io.wait();
                        return Err(SyncTransportError::Discovery(error));
                    }
                }
            }
            #[cfg(test)]
            DiscoveryMode::Disabled => None,
        };

        let worker = WorkerState::new(
            self_id,
            shared_secret,
            hello,
            handle.clone(),
            inbound_sender,
            observer,
        );
        let worker_thread =
            spawn_worker_thread(worker, event_receiver, command_receiver, shutdown.clone());
        let discovery_thread = discovery.map(|discovery| {
            spawn_discovery_thread(self_id, discovery, handle.clone(), shutdown.clone())
        });

        info!(
            peer_id = %self_id,
            listen_addr = %local_addr,
            discovery = discovery_mode.as_str(),
            "started Resteyes sync transport"
        );

        Ok((
            Self {
                io,
                command_sender,
                #[cfg(test)]
                local_addr,
                worker_thread: Some(worker_thread),
                discovery_thread,
                shutdown,
            },
            SyncInboundReceiver {
                receiver: inbound_receiver,
            },
        ))
    }

    #[allow(dead_code)]
    pub(crate) fn broadcast(&self, event: SyncEvent) -> Result<usize, SyncTransportError> {
        let (reply_sender, reply_receiver) = mpsc::channel();
        self.command_sender
            .send(TransportCommand::Broadcast {
                event,
                reply: reply_sender,
            })
            .map_err(|_| SyncTransportError::WorkerStopped)?;

        reply_receiver
            .recv()
            .map_err(|_| SyncTransportError::WorkerStopped)?
    }

    #[allow(dead_code)]
    pub(crate) fn send(
        &self,
        peer_id: PeerId,
        event: SyncEvent,
    ) -> Result<bool, SyncTransportError> {
        let (reply_sender, reply_receiver) = mpsc::channel();
        self.command_sender
            .send(TransportCommand::Send {
                peer_id,
                event,
                reply: reply_sender,
            })
            .map_err(|_| SyncTransportError::WorkerStopped)?;

        reply_receiver
            .recv()
            .map_err(|_| SyncTransportError::WorkerStopped)?
    }

    #[cfg(test)]
    fn start_for_test(
        self_id: PeerId,
        shared_secret: SharedSecret,
    ) -> Result<
        (
            Self,
            SyncInboundReceiver,
            mpsc::Receiver<TransportNotification>,
        ),
        SyncTransportError,
    > {
        let (sender, receiver) = mpsc::channel();
        let (transport, inbound_receiver) = Self::start_internal(
            self_id,
            shared_secret,
            "127.0.0.1:0",
            DiscoveryMode::Disabled,
            Some(sender),
        )?;

        Ok((transport, inbound_receiver, receiver))
    }

    #[cfg(test)]
    fn connect_for_test(&self, address: SocketAddr) -> io::Result<TransportEndpoint> {
        self.io.handle().connect(address)
    }

    #[cfg(test)]
    fn local_addr_for_test(&self) -> SocketAddr {
        self.local_addr
    }
}

impl Drop for SyncTransport {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        self.io.remove_listener();
        self.io.stop();

        if let Some(handle) = self.discovery_thread.take() {
            _ = handle.join();
        }

        if let Some(handle) = self.worker_thread.take() {
            _ = handle.join();
        }

        self.io.wait();
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncInboundEvent {
    pub(crate) sender: PeerId,
    pub(crate) sequence: u64,
    pub(crate) event: SyncEvent,
}

#[allow(dead_code)]
pub(crate) struct SyncInboundReceiver {
    receiver: mpsc::Receiver<SyncInboundEvent>,
}

#[allow(dead_code)]
impl SyncInboundReceiver {
    pub(crate) fn try_recv(&self) -> Result<SyncInboundEvent, mpsc::TryRecvError> {
        self.receiver.try_recv()
    }

    pub(crate) fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<SyncInboundEvent, mpsc::RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }
}

enum TransportCommand {
    Broadcast {
        event: SyncEvent,
        reply: mpsc::Sender<Result<usize, SyncTransportError>>,
    },
    Send {
        peer_id: PeerId,
        event: SyncEvent,
        reply: mpsc::Sender<Result<bool, SyncTransportError>>,
    },
}

fn spawn_discovery_thread(
    self_id: PeerId,
    discovery: LanDiscovery,
    handle: TransportIoHandle,
    shutdown: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        while !shutdown.load(Ordering::Relaxed) {
            if let Some(peer) = discovery.next_peer_timeout(Instant::now(), DISCOVERY_POLL_INTERVAL)
            {
                connect_discovered_peer(&handle, &peer);
            }
        }

        trace!(peer_id = %self_id, "stopped Resteyes sync discovery thread");
    })
}

fn connect_discovered_peer(handle: &TransportIoHandle, peer: &DiscoveredPeer) {
    match handle.connect(peer.address) {
        Ok(endpoint) => {
            trace!(
                peer_id = %peer.peer_id,
                address = %peer.address,
                endpoint = %endpoint,
                "connecting to authenticated Resteyes peer"
            );
        }
        Err(error) => {
            warn!(
                peer_id = %peer.peer_id,
                address = %peer.address,
                %error,
                "failed to connect to discovered Resteyes peer"
            );
        }
    }
}

fn spawn_worker_thread(
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

struct WorkerState {
    self_id: PeerId,
    shared_secret: SharedSecret,
    hello: Vec<u8>,
    handle: TransportIoHandle,
    tracker: ConnectionTracker<TransportEndpoint>,
    next_sequence: u64,
    inbound_sender: mpsc::Sender<SyncInboundEvent>,
    observer: Option<mpsc::Sender<TransportNotification>>,
}

impl WorkerState {
    fn new(
        self_id: PeerId,
        shared_secret: SharedSecret,
        hello: Vec<u8>,
        handle: TransportIoHandle,
        inbound_sender: mpsc::Sender<SyncInboundEvent>,
        observer: Option<mpsc::Sender<TransportNotification>>,
    ) -> Self {
        Self {
            self_id,
            shared_secret,
            hello,
            handle,
            tracker: ConnectionTracker::default(),
            next_sequence: 1,
            inbound_sender,
            observer,
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
            .map_err(|error| sync_protocol_error(&error))?;
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
            TransportIoEvent::Connected(endpoint, true) => {
                self.tracker
                    .record_endpoint(endpoint, ConnectionDirection::Outgoing);
                self.send_hello(endpoint);
            }
            TransportIoEvent::Connected(endpoint, false) => {
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
                    self.notify(TransportNotification::PeerDisconnected(peer_id));
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
        let BindPeerResult {
            status,
            remove_endpoints,
        } = self.tracker.bind_peer(self.self_id, endpoint, sender);
        self.remove_endpoints(remove_endpoints);

        match status {
            BindPeerStatus::Accepted { peer_connected } if peer_connected => {
                info!(
                    peer_id = %sender,
                    endpoint = %endpoint,
                    "authenticated Resteyes sync peer"
                );
                self.notify(TransportNotification::PeerAuthenticated(sender));
            }
            BindPeerStatus::Accepted { .. } => {}
            BindPeerStatus::RejectedSelf => {
                warn!(
                    peer_id = %sender,
                    endpoint = %endpoint,
                    "rejected sync connection from local peer id"
                );
            }
            BindPeerStatus::RejectedUnknownEndpoint => {
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
        let Some(peer_id) = self.tracker.peer_for_endpoint(endpoint) else {
            warn!(
                endpoint = %endpoint,
                ?event,
                "sync peer sent domain event before authenticated hello"
            );
            self.remove_endpoint(endpoint);
            return;
        };

        if peer_id != sender {
            warn!(
                endpoint = %endpoint,
                authenticated_peer_id = %peer_id,
                frame_sender = %sender,
                "sync peer sent frame with mismatched sender"
            );
            self.remove_endpoint(endpoint);
            return;
        }

        _ = self.inbound_sender.send(SyncInboundEvent {
            sender,
            sequence,
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
            self.notify(TransportNotification::PeerDisconnected(peer_id));
        }

        self.handle.remove(endpoint);
    }

    fn remove_endpoints(&mut self, endpoints: Vec<TransportEndpoint>) {
        for endpoint in endpoints {
            self.remove_endpoint(endpoint);
        }
    }

    fn notify(&self, event: TransportNotification) {
        if let Some(observer) = &self.observer {
            _ = observer.send(event);
        }
    }
}

fn peer_hello_payload(
    self_id: PeerId,
    shared_secret: &SharedSecret,
) -> Result<Vec<u8>, SyncProtocolError> {
    encode_authenticated(
        &SyncMessage::control(self_id, 0, TransportControlFrame::PeerHello),
        shared_secret,
    )
    .map(String::into_bytes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiscoveryMode {
    Advertise,
    #[cfg(test)]
    Disabled,
}

impl DiscoveryMode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Advertise => "advertise",
            #[cfg(test)]
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionDirection {
    Incoming,
    Outgoing,
}

#[derive(Debug)]
struct ConnectionTracker<E> {
    connections: Vec<Connection<E>>,
}

impl<E> Default for ConnectionTracker<E> {
    fn default() -> Self {
        Self {
            connections: Vec::new(),
        }
    }
}

impl<E> ConnectionTracker<E>
where
    E: Copy + Eq,
{
    fn record_endpoint(&mut self, endpoint: E, direction: ConnectionDirection) {
        if let Some(connection) = self.connection_mut(endpoint) {
            connection.direction = direction;
            return;
        }

        self.connections.push(Connection {
            endpoint,
            direction,
            peer_id: None,
        });
    }

    fn bind_peer(&mut self, self_id: PeerId, endpoint: E, peer_id: PeerId) -> BindPeerResult<E> {
        if peer_id == self_id {
            self.remove_endpoint(endpoint);
            return BindPeerResult {
                status: BindPeerStatus::RejectedSelf,
                remove_endpoints: vec![endpoint],
            };
        }

        let had_peer = self.peer_is_connected(peer_id);
        let Some(connection) = self.connection_mut(endpoint) else {
            return BindPeerResult {
                status: BindPeerStatus::RejectedUnknownEndpoint,
                remove_endpoints: vec![endpoint],
            };
        };

        connection.peer_id = Some(peer_id);
        let remove_endpoints = self.collapse_duplicate_peer_connections(self_id, peer_id);
        let peer_connected = !had_peer
            && self
                .connection(endpoint)
                .is_some_and(|connection| connection.peer_id == Some(peer_id));

        BindPeerResult {
            status: BindPeerStatus::Accepted { peer_connected },
            remove_endpoints,
        }
    }

    fn remove_endpoint(&mut self, endpoint: E) -> Option<PeerId> {
        let index = self
            .connections
            .iter()
            .position(|connection| connection.endpoint == endpoint)?;
        self.connections.remove(index).peer_id
    }

    fn peer_for_endpoint(&self, endpoint: E) -> Option<PeerId> {
        self.connection(endpoint)
            .and_then(|connection| connection.peer_id)
    }

    fn endpoint_for_peer(&self, peer_id: PeerId) -> Option<E> {
        self.connections
            .iter()
            .find(|connection| connection.peer_id == Some(peer_id))
            .map(|connection| connection.endpoint)
    }

    fn authenticated_endpoints(&self) -> Vec<E> {
        self.connections
            .iter()
            .filter(|connection| connection.peer_id.is_some())
            .map(|connection| connection.endpoint)
            .collect()
    }

    fn endpoints(&self) -> Vec<E> {
        self.connections
            .iter()
            .map(|connection| connection.endpoint)
            .collect()
    }

    fn peer_is_connected(&self, peer_id: PeerId) -> bool {
        self.connections
            .iter()
            .any(|connection| connection.peer_id == Some(peer_id))
    }

    fn collapse_duplicate_peer_connections(&mut self, self_id: PeerId, peer_id: PeerId) -> Vec<E> {
        let Some(keep_endpoint) = self.endpoint_to_keep(self_id, peer_id) else {
            return Vec::new();
        };

        let mut remove_endpoints = Vec::new();
        self.connections.retain(|connection| {
            let should_remove =
                connection.peer_id == Some(peer_id) && connection.endpoint != keep_endpoint;

            if should_remove {
                remove_endpoints.push(connection.endpoint);
            }

            !should_remove
        });

        remove_endpoints
    }

    fn endpoint_to_keep(&self, self_id: PeerId, peer_id: PeerId) -> Option<E> {
        let desired_direction = desired_connection_direction(self_id, peer_id);

        self.connections
            .iter()
            .find(|connection| {
                connection.peer_id == Some(peer_id) && connection.direction == desired_direction
            })
            .or_else(|| {
                self.connections
                    .iter()
                    .find(|connection| connection.peer_id == Some(peer_id))
            })
            .map(|connection| connection.endpoint)
    }

    fn connection(&self, endpoint: E) -> Option<&Connection<E>> {
        self.connections
            .iter()
            .find(|connection| connection.endpoint == endpoint)
    }

    fn connection_mut(&mut self, endpoint: E) -> Option<&mut Connection<E>> {
        self.connections
            .iter_mut()
            .find(|connection| connection.endpoint == endpoint)
    }
}

#[derive(Debug, Clone, Copy)]
struct Connection<E> {
    endpoint: E,
    direction: ConnectionDirection,
    peer_id: Option<PeerId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BindPeerResult<E> {
    status: BindPeerStatus,
    remove_endpoints: Vec<E>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BindPeerStatus {
    Accepted { peer_connected: bool },
    RejectedSelf,
    RejectedUnknownEndpoint,
}

fn desired_connection_direction(self_id: PeerId, peer_id: PeerId) -> ConnectionDirection {
    if self_id < peer_id {
        ConnectionDirection::Outgoing
    } else {
        ConnectionDirection::Incoming
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TransportNotification {
    PeerAuthenticated(PeerId),
    PeerDisconnected(PeerId),
}

#[derive(Debug)]
pub(crate) enum SyncTransportError {
    MissingSharedSecret,
    Protocol { message: String },
    Listen { message: String },
    Discovery(SyncDiscoveryError),
    WorkerStopped,
    SequenceExhausted,
}

impl fmt::Display for SyncTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSharedSecret => {
                formatter.write_str("sync shared_secret is required when sync transport is enabled")
            }
            Self::Protocol { message } => {
                write!(formatter, "sync transport protocol setup failed: {message}")
            }
            Self::Listen { message } => {
                write!(formatter, "sync transport listener setup failed: {message}")
            }
            Self::Discovery(error) => write!(formatter, "{error}"),
            Self::WorkerStopped => formatter.write_str("sync transport worker stopped"),
            Self::SequenceExhausted => formatter.write_str("sync transport sequence exhausted"),
        }
    }
}

impl std::error::Error for SyncTransportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Discovery(error) => Some(error),
            Self::MissingSharedSecret
            | Self::Protocol { .. }
            | Self::Listen { .. }
            | Self::WorkerStopped
            | Self::SequenceExhausted => None,
        }
    }
}

fn sync_protocol_error(error: &SyncProtocolError) -> SyncTransportError {
    SyncTransportError::Protocol {
        message: error.to_string(),
    }
}

fn sync_listen_error(error: &io::Error) -> SyncTransportError {
    SyncTransportError::Listen {
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests;
