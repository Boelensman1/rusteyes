mod commands;
mod connections;
mod session;
mod worker;

use self::commands::TransportCommand;
use self::session::TransportSession;
use self::worker::{WorkerState, spawn_worker_thread};
use crate::config::{SharedSecret, SyncConfig};
use crate::sync_discovery::{DiscoveredPeer, DiscoveryEvent, LanDiscovery, SyncDiscoveryError};
use crate::sync_protocol::{PeerId, SyncEvent, SyncProtocolError};
#[cfg(test)]
use crate::sync_transport_io::{TransportEndpoint, TransportSendStatus};
use crate::sync_transport_io::{TransportIo, TransportIoHandle};
use std::fmt;
use std::io;
#[cfg(test)]
use std::net::SocketAddr;
use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
#[cfg(test)]
use std::time::Duration;
use tracing::{info, trace, warn};

const PRODUCTION_LISTEN_ADDR: &str = "0.0.0.0:0";

pub(crate) struct SyncTransport {
    state: SyncTransportState,
}

enum SyncTransportState {
    Inactive,
    Active(Box<ActiveSyncTransport>),
}

struct ActiveSyncTransport {
    io: TransportIo,
    command_sender: flume::Sender<TransportCommand>,
    event_receiver: flume::Receiver<SyncTransportEvent>,
    #[cfg(test)]
    local_addr: SocketAddr,
    worker_thread: Option<JoinHandle<()>>,
    discovery_thread: Option<JoinHandle<()>>,
    discovery_shutdown_sender: Option<flume::Sender<()>>,
    shutdown: Arc<AtomicBool>,
}

impl SyncTransport {
    pub(crate) fn start(sync: SyncConfig) -> Result<Self, SyncTransportError> {
        if !sync.enabled {
            return Ok(Self {
                state: SyncTransportState::Inactive,
            });
        }

        let Some(shared_secret) = sync.shared_secret else {
            return Err(SyncTransportError::MissingSharedSecret);
        };

        let self_id = PeerId::generate().map_err(SyncTransportError::Protocol)?;
        Self::start_internal(
            self_id,
            &shared_secret,
            PRODUCTION_LISTEN_ADDR,
            DiscoveryMode::Advertise,
        )
    }

    fn start_internal(
        self_id: PeerId,
        shared_secret: &SharedSecret,
        listen_addr: impl ToSocketAddrs,
        discovery_mode: DiscoveryMode,
    ) -> Result<Self, SyncTransportError> {
        let session = TransportSession::new(self_id, shared_secret.clone())
            .map_err(SyncTransportError::Protocol)?;
        let mut binding = TransportIo::listen(listen_addr).map_err(SyncTransportError::Listen)?;
        let handle = binding.io.handle();
        let (command_sender, command_receiver) = flume::unbounded();
        let (event_sender, event_receiver) = flume::unbounded();
        let shutdown = Arc::new(AtomicBool::new(false));

        let discovery = match discovery_mode {
            DiscoveryMode::Advertise => {
                match LanDiscovery::start(self_id, shared_secret.clone(), binding.local_addr.port())
                {
                    Ok(discovery) => Some(discovery),
                    Err(error) => {
                        binding.io.shutdown();
                        return Err(SyncTransportError::Discovery(error));
                    }
                }
            }
            #[cfg(test)]
            DiscoveryMode::Disabled => None,
        };

        let worker = WorkerState::new(session, handle.clone(), event_sender);
        let worker_thread = spawn_worker_thread(
            worker,
            binding.event_receiver,
            command_receiver,
            shutdown.clone(),
        );
        let (discovery_thread, discovery_shutdown_sender) = match discovery {
            Some(discovery) => {
                let (shutdown_sender, shutdown_receiver) = flume::bounded(1);
                (
                    Some(spawn_discovery_thread(
                        self_id,
                        discovery,
                        handle.clone(),
                        shutdown_receiver,
                    )),
                    Some(shutdown_sender),
                )
            }
            None => (None, None),
        };

        info!(
            peer_id = %self_id,
            listen_addr = %binding.local_addr,
            discovery = discovery_mode.as_str(),
            "started Resteyes sync transport"
        );

        Ok(Self {
            state: SyncTransportState::Active(Box::new(ActiveSyncTransport {
                io: binding.io,
                command_sender,
                event_receiver,
                #[cfg(test)]
                local_addr: binding.local_addr,
                worker_thread: Some(worker_thread),
                discovery_thread,
                discovery_shutdown_sender,
                shutdown,
            })),
        })
    }

    #[allow(dead_code)]
    pub(crate) fn broadcast_event(&self, event: SyncEvent) -> Result<usize, SyncTransportError> {
        let SyncTransportState::Active(active) = &self.state else {
            return Ok(0);
        };

        active.broadcast_event(event)
    }

    #[allow(dead_code)]
    pub(crate) fn send_event(
        &self,
        peer_id: PeerId,
        event: SyncEvent,
    ) -> Result<bool, SyncTransportError> {
        let SyncTransportState::Active(active) = &self.state else {
            return Ok(false);
        };

        active.send_event(peer_id, event)
    }

    #[allow(dead_code)]
    pub(crate) fn try_recv_event(&self) -> Result<Option<SyncTransportEvent>, SyncTransportError> {
        let SyncTransportState::Active(active) = &self.state else {
            return Ok(None);
        };

        active.try_recv_event()
    }

    #[allow(dead_code)]
    pub(crate) fn drain_events(&self) -> Result<Vec<SyncTransportEvent>, SyncTransportError> {
        let SyncTransportState::Active(active) = &self.state else {
            return Ok(Vec::new());
        };

        active.drain_events()
    }

    #[cfg(test)]
    fn start_for_test(
        self_id: PeerId,
        shared_secret: &SharedSecret,
    ) -> Result<Self, SyncTransportError> {
        Self::start_internal(
            self_id,
            shared_secret,
            "127.0.0.1:0",
            DiscoveryMode::Disabled,
        )
    }

    #[cfg(test)]
    fn connect_for_test(&self, address: SocketAddr) -> io::Result<TransportEndpoint> {
        self.active_for_test().io.handle().connect(address)
    }

    #[cfg(test)]
    fn send_raw_for_test(
        &self,
        endpoint: TransportEndpoint,
        payload: &[u8],
    ) -> TransportSendStatus {
        self.active_for_test().io.handle().send(endpoint, payload)
    }

    #[cfg(test)]
    fn local_addr_for_test(&self) -> SocketAddr {
        self.active_for_test().local_addr
    }

    #[cfg(test)]
    fn recv_event_timeout_for_test(
        &self,
        timeout: Duration,
    ) -> Result<Option<SyncTransportEvent>, SyncTransportError> {
        self.active_for_test().recv_event_timeout(timeout)
    }

    #[cfg(test)]
    fn active_for_test(&self) -> &ActiveSyncTransport {
        match &self.state {
            SyncTransportState::Active(active) => active,
            SyncTransportState::Inactive => panic!("test transport should be active"),
        }
    }
}

impl ActiveSyncTransport {
    fn broadcast_event(&self, event: SyncEvent) -> Result<usize, SyncTransportError> {
        let (reply_sender, reply_receiver) = flume::bounded(1);
        self.command_sender
            .send(TransportCommand::Broadcast {
                event,
                reply: reply_sender,
            })
            .map_err(|_| SyncTransportError::WorkerStopped)?;
        self.io.handle().wake();

        reply_receiver
            .recv()
            .map_err(|_| SyncTransportError::WorkerStopped)?
    }

    fn send_event(&self, peer_id: PeerId, event: SyncEvent) -> Result<bool, SyncTransportError> {
        let (reply_sender, reply_receiver) = flume::bounded(1);
        self.command_sender
            .send(TransportCommand::Send {
                peer_id,
                event,
                reply: reply_sender,
            })
            .map_err(|_| SyncTransportError::WorkerStopped)?;
        self.io.handle().wake();

        reply_receiver
            .recv()
            .map_err(|_| SyncTransportError::WorkerStopped)?
    }

    fn try_recv_event(&self) -> Result<Option<SyncTransportEvent>, SyncTransportError> {
        match self.event_receiver.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(flume::TryRecvError::Empty) => Ok(None),
            Err(flume::TryRecvError::Disconnected) => Err(SyncTransportError::WorkerStopped),
        }
    }

    fn drain_events(&self) -> Result<Vec<SyncTransportEvent>, SyncTransportError> {
        let mut events = Vec::new();

        while let Some(event) = self.try_recv_event()? {
            events.push(event);
        }

        Ok(events)
    }

    #[cfg(test)]
    fn recv_event_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Option<SyncTransportEvent>, SyncTransportError> {
        match self.event_receiver.recv_timeout(timeout) {
            Ok(event) => Ok(Some(event)),
            Err(flume::RecvTimeoutError::Timeout) => Ok(None),
            Err(flume::RecvTimeoutError::Disconnected) => Err(SyncTransportError::WorkerStopped),
        }
    }

    fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        self.io.handle().wake();

        if let Some(sender) = self.discovery_shutdown_sender.take() {
            _ = sender.send(());
        }

        self.io.remove_listener();

        if let Some(handle) = self.discovery_thread.take() {
            _ = handle.join();
        }

        if let Some(handle) = self.worker_thread.take() {
            _ = handle.join();
        }

        self.io.stop();
        self.io.wait();
    }
}

impl Drop for SyncTransport {
    fn drop(&mut self) {
        if let SyncTransportState::Active(active) = &mut self.state {
            active.shutdown();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SyncTransportEvent {
    PeerAuthenticated(PeerId),
    PeerDisconnected(PeerId),
    Domain { peer_id: PeerId, event: SyncEvent },
}

fn spawn_discovery_thread(
    self_id: PeerId,
    discovery: LanDiscovery,
    handle: TransportIoHandle,
    shutdown_receiver: flume::Receiver<()>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        while let DiscoveryEvent::Peer(peer) = discovery.next_event(&shutdown_receiver) {
            connect_discovered_peer(&handle, &peer);
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

#[derive(Debug)]
pub(crate) enum SyncTransportError {
    MissingSharedSecret,
    Protocol(SyncProtocolError),
    Listen(io::Error),
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
            Self::Protocol(error) => {
                write!(formatter, "sync transport protocol setup failed: {error}")
            }
            Self::Listen(error) => {
                write!(formatter, "sync transport listener setup failed: {error}")
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
            Self::Protocol(error) => Some(error),
            Self::Listen(error) => Some(error),
            Self::Discovery(error) => Some(error),
            Self::MissingSharedSecret | Self::WorkerStopped | Self::SequenceExhausted => None,
        }
    }
}

#[cfg(test)]
mod tests;
