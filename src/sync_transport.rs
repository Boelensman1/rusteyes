use crate::config::{SharedSecret, SyncConfig};
use crate::sync_discovery::{DiscoveredPeer, LanDiscovery, SyncDiscoveryError};
use crate::sync_protocol::{
    PeerId, SyncEvent, SyncMessage, SyncProtocolError, decode_authenticated, encode_authenticated,
};
use message_io::events::EventReceiver;
use message_io::network::{Endpoint, SendStatus, Transport};
use message_io::node::{self, NodeHandler, NodeTask, StoredNetEvent, StoredNodeEvent};
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
    handler: NodeHandler<()>,
    node_task: Option<NodeTask>,
    listener_id: message_io::network::ResourceId,
    #[cfg(test)]
    local_addr: SocketAddr,
    worker_thread: Option<JoinHandle<()>>,
    discovery_thread: Option<JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
}

impl SyncTransport {
    pub(crate) fn start(sync: SyncConfig) -> Result<Option<Self>, SyncTransportError> {
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
    ) -> Result<Self, SyncTransportError> {
        let hello = peer_hello_payload(self_id, &shared_secret)
            .map_err(|error| sync_protocol_error(&error))?;
        let (handler, listener) = node::split::<()>();
        let (listener_id, local_addr) = handler
            .network()
            .listen(Transport::FramedTcp, listen_addr)
            .map_err(|error| sync_listen_error(&error))?;
        let (mut node_task, event_receiver) = listener.enqueue();
        let shutdown = Arc::new(AtomicBool::new(false));

        let discovery = match discovery_mode {
            DiscoveryMode::Advertise => {
                match LanDiscovery::start(self_id, shared_secret.clone(), local_addr.port()) {
                    Ok(discovery) => Some(discovery),
                    Err(error) => {
                        shutdown.store(true, Ordering::Relaxed);
                        handler.stop();
                        node_task.wait();
                        return Err(SyncTransportError::Discovery(error));
                    }
                }
            }
            #[cfg(test)]
            DiscoveryMode::Disabled => None,
        };

        let worker_thread = spawn_worker_thread(
            self_id,
            shared_secret,
            hello,
            handler.clone(),
            event_receiver,
            shutdown.clone(),
            observer,
        );
        let discovery_thread = discovery.map(|discovery| {
            spawn_discovery_thread(self_id, discovery, handler.clone(), shutdown.clone())
        });

        info!(
            peer_id = %self_id,
            listen_addr = %local_addr,
            discovery = discovery_mode.as_str(),
            "started Resteyes sync transport"
        );

        Ok(Self {
            handler,
            node_task: Some(node_task),
            listener_id,
            #[cfg(test)]
            local_addr,
            worker_thread: Some(worker_thread),
            discovery_thread,
            shutdown,
        })
    }

    #[cfg(test)]
    fn start_for_test(
        self_id: PeerId,
        shared_secret: SharedSecret,
    ) -> Result<(Self, mpsc::Receiver<TransportNotification>), SyncTransportError> {
        let (sender, receiver) = mpsc::channel();
        let transport = Self::start_internal(
            self_id,
            shared_secret,
            "127.0.0.1:0",
            DiscoveryMode::Disabled,
            Some(sender),
        )?;

        Ok((transport, receiver))
    }

    #[cfg(test)]
    fn connect_for_test(&self, address: SocketAddr) -> io::Result<Endpoint> {
        self.handler
            .network()
            .connect(Transport::FramedTcp, address)
            .map(|(endpoint, _)| endpoint)
    }

    #[cfg(test)]
    fn local_addr_for_test(&self) -> SocketAddr {
        self.local_addr
    }
}

impl Drop for SyncTransport {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        _ = self.handler.network().remove(self.listener_id);
        self.handler.stop();

        if let Some(handle) = self.discovery_thread.take() {
            _ = handle.join();
        }

        if let Some(handle) = self.worker_thread.take() {
            _ = handle.join();
        }

        if let Some(mut node_task) = self.node_task.take() {
            node_task.wait();
        }
    }
}

fn spawn_discovery_thread(
    self_id: PeerId,
    discovery: LanDiscovery,
    handler: NodeHandler<()>,
    shutdown: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        while !shutdown.load(Ordering::Relaxed) {
            if let Some(peer) = discovery.next_peer_timeout(Instant::now(), DISCOVERY_POLL_INTERVAL)
            {
                connect_discovered_peer(&handler, &peer);
            }
        }

        trace!(peer_id = %self_id, "stopped Resteyes sync discovery thread");
    })
}

fn connect_discovered_peer(handler: &NodeHandler<()>, peer: &DiscoveredPeer) {
    match handler
        .network()
        .connect(Transport::FramedTcp, peer.address)
    {
        Ok((endpoint, _)) => {
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
    self_id: PeerId,
    shared_secret: SharedSecret,
    hello: Vec<u8>,
    handler: NodeHandler<()>,
    mut event_receiver: EventReceiver<StoredNodeEvent<()>>,
    shutdown: Arc<AtomicBool>,
    observer: Option<mpsc::Sender<TransportNotification>>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut tracker = ConnectionTracker::default();

        while !shutdown.load(Ordering::Relaxed) && handler.is_running() {
            let Some(event) = event_receiver.receive_timeout(NODE_EVENT_POLL_INTERVAL) else {
                continue;
            };

            match event {
                StoredNodeEvent::Network(event) => handle_network_event(
                    self_id,
                    &shared_secret,
                    &hello,
                    &handler,
                    event,
                    &mut tracker,
                    observer.as_ref(),
                ),
                StoredNodeEvent::Signal(()) => {}
            }
        }

        for endpoint in tracker.endpoints() {
            _ = handler.network().remove(endpoint.resource_id());
        }

        trace!(peer_id = %self_id, "stopped Resteyes sync transport worker");
    })
}

fn handle_network_event(
    self_id: PeerId,
    shared_secret: &SharedSecret,
    hello: &[u8],
    handler: &NodeHandler<()>,
    event: StoredNetEvent,
    tracker: &mut ConnectionTracker<Endpoint>,
    observer: Option<&mpsc::Sender<TransportNotification>>,
) {
    match event {
        StoredNetEvent::Connected(endpoint, true) => {
            tracker.record_endpoint(endpoint, ConnectionDirection::Outgoing);
            send_hello(handler, endpoint, hello);
        }
        StoredNetEvent::Connected(endpoint, false) => {
            tracker.remove_endpoint(endpoint);
            warn!(endpoint = %endpoint, "sync peer connection failed");
        }
        StoredNetEvent::Accepted(endpoint, _) => {
            tracker.record_endpoint(endpoint, ConnectionDirection::Incoming);
            send_hello(handler, endpoint, hello);
        }
        StoredNetEvent::Message(endpoint, bytes) => {
            handle_peer_message(
                self_id,
                shared_secret,
                handler,
                endpoint,
                &bytes,
                tracker,
                observer,
            );
        }
        StoredNetEvent::Disconnected(endpoint) => {
            if let Some(peer_id) = tracker.remove_endpoint(endpoint) {
                info!(peer_id = %peer_id, endpoint = %endpoint, "sync peer disconnected");
                notify(observer, TransportNotification::PeerDisconnected(peer_id));
            }
        }
    }
}

fn handle_peer_message(
    self_id: PeerId,
    shared_secret: &SharedSecret,
    handler: &NodeHandler<()>,
    endpoint: Endpoint,
    bytes: &[u8],
    tracker: &mut ConnectionTracker<Endpoint>,
    observer: Option<&mpsc::Sender<TransportNotification>>,
) {
    let input = match str::from_utf8(bytes) {
        Ok(input) => input,
        Err(error) => {
            warn!(endpoint = %endpoint, %error, "sync peer sent non-UTF-8 frame");
            remove_endpoint(handler, tracker, endpoint);
            return;
        }
    };

    let message = match decode_authenticated(input, shared_secret) {
        Ok(message) => message,
        Err(error) => {
            warn!(endpoint = %endpoint, %error, "sync peer message authentication failed");
            remove_endpoint(handler, tracker, endpoint);
            return;
        }
    };

    match message.event {
        SyncEvent::PeerHello => {
            let result = tracker.bind_peer(self_id, endpoint, message.sender);
            remove_endpoints(handler, result.remove_endpoints);

            match result.status {
                BindPeerStatus::Accepted { peer_connected } if peer_connected => {
                    info!(
                        peer_id = %message.sender,
                        endpoint = %endpoint,
                        "authenticated Resteyes sync peer"
                    );
                    notify(
                        observer,
                        TransportNotification::PeerAuthenticated(message.sender),
                    );
                }
                BindPeerStatus::Accepted { .. } => {}
                BindPeerStatus::RejectedSelf => {
                    warn!(
                        peer_id = %message.sender,
                        endpoint = %endpoint,
                        "rejected sync connection from local peer id"
                    );
                }
                BindPeerStatus::RejectedUnknownEndpoint => {
                    warn!(
                        peer_id = %message.sender,
                        endpoint = %endpoint,
                        "rejected sync hello from unknown endpoint"
                    );
                }
            }
        }
        event if tracker.peer_for_endpoint(endpoint).is_some() => {
            trace!(endpoint = %endpoint, ?event, "ignored authenticated sync event before runtime sync wiring");
        }
        event => {
            warn!(
                endpoint = %endpoint,
                ?event,
                "sync peer sent data before authenticated hello"
            );
            remove_endpoint(handler, tracker, endpoint);
        }
    }
}

fn send_hello(handler: &NodeHandler<()>, endpoint: Endpoint, hello: &[u8]) {
    match handler.network().send(endpoint, hello) {
        SendStatus::Sent => {
            trace!(endpoint = %endpoint, "sent sync peer hello");
        }
        status => {
            warn!(endpoint = %endpoint, ?status, "failed to send sync peer hello");
        }
    }
}

fn remove_endpoint(
    handler: &NodeHandler<()>,
    tracker: &mut ConnectionTracker<Endpoint>,
    endpoint: Endpoint,
) {
    tracker.remove_endpoint(endpoint);
    _ = handler.network().remove(endpoint.resource_id());
}

fn remove_endpoints(handler: &NodeHandler<()>, endpoints: Vec<Endpoint>) {
    for endpoint in endpoints {
        _ = handler.network().remove(endpoint.resource_id());
    }
}

fn notify(observer: Option<&mpsc::Sender<TransportNotification>>, event: TransportNotification) {
    if let Some(observer) = observer {
        _ = observer.send(event);
    }
}

fn peer_hello_payload(
    self_id: PeerId,
    shared_secret: &SharedSecret,
) -> Result<Vec<u8>, SyncProtocolError> {
    encode_authenticated(
        &SyncMessage::new(self_id, 0, SyncEvent::PeerHello),
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
        }
    }
}

impl std::error::Error for SyncTransportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Discovery(error) => Some(error),
            Self::MissingSharedSecret | Self::Protocol { .. } | Self::Listen { .. } => None,
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
