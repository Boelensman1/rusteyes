mod commands;
mod connections;
mod worker;

use self::commands::TransportCommand;
use self::worker::{WorkerState, peer_hello_payload, spawn_worker_thread};
use crate::config::{SharedSecret, SyncConfig};
use crate::sync_discovery::{DiscoveredPeer, LanDiscovery, SyncDiscoveryError};
use crate::sync_protocol::{PeerId, SyncEvent, SyncProtocolError};
#[cfg(test)]
use crate::sync_transport_io::TransportEndpoint;
use crate::sync_transport_io::{TransportIo, TransportIoHandle};
use std::fmt;
use std::io;
#[cfg(test)]
use std::net::SocketAddr;
use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tracing::{info, trace, warn};

const PRODUCTION_LISTEN_ADDR: &str = "0.0.0.0:0";
const DISCOVERY_POLL_INTERVAL: Duration = Duration::from_millis(250);

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
