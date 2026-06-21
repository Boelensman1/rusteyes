use message_io::events::EventReceiver;
use message_io::network::{Endpoint, ResourceId, SendStatus, Transport};
use message_io::node::{self, NodeHandler, NodeTask, StoredNetEvent, StoredNodeEvent};
use std::fmt;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Duration;

pub(crate) struct TransportIo {
    handle: TransportIoHandle,
    listener_id: ResourceId,
    node_task: Option<NodeTask>,
}

impl TransportIo {
    pub(crate) fn listen(
        address: impl ToSocketAddrs,
    ) -> io::Result<(Self, TransportIoReceiver, SocketAddr)> {
        let (handler, listener) = node::split::<()>();
        let (listener_id, local_addr) = handler.network().listen(Transport::FramedTcp, address)?;
        let (node_task, event_receiver) = listener.enqueue();
        let handle = TransportIoHandle { handler };

        Ok((
            Self {
                handle,
                listener_id,
                node_task: Some(node_task),
            },
            TransportIoReceiver {
                receiver: event_receiver,
            },
            local_addr,
        ))
    }

    pub(crate) fn handle(&self) -> TransportIoHandle {
        self.handle.clone()
    }

    pub(crate) fn remove_listener(&self) {
        _ = self.handle.handler.network().remove(self.listener_id);
    }

    pub(crate) fn stop(&self) {
        self.handle.handler.stop();
    }

    pub(crate) fn wait(&mut self) {
        if let Some(mut node_task) = self.node_task.take() {
            node_task.wait();
        }
    }
}

#[derive(Clone)]
pub(crate) struct TransportIoHandle {
    handler: NodeHandler<()>,
}

impl TransportIoHandle {
    pub(crate) fn connect(&self, address: SocketAddr) -> io::Result<TransportEndpoint> {
        self.handler
            .network()
            .connect(Transport::FramedTcp, address)
            .map(|(endpoint, _)| TransportEndpoint(endpoint))
    }

    pub(crate) fn send(&self, endpoint: TransportEndpoint, payload: &[u8]) -> TransportSendStatus {
        self.handler.network().send(endpoint.0, payload).into()
    }

    pub(crate) fn remove(&self, endpoint: TransportEndpoint) {
        _ = self.handler.network().remove(endpoint.0.resource_id());
    }

    pub(crate) fn is_running(&self) -> bool {
        self.handler.is_running()
    }
}

pub(crate) struct TransportIoReceiver {
    receiver: EventReceiver<StoredNodeEvent<()>>,
}

impl TransportIoReceiver {
    pub(crate) fn receive_timeout(&mut self, timeout: Duration) -> Option<TransportIoEvent> {
        let event = self.receiver.receive_timeout(timeout)?;

        match event {
            StoredNodeEvent::Network(event) => Some(event.into()),
            StoredNodeEvent::Signal(()) => None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum TransportIoEvent {
    Connected(TransportEndpoint, bool),
    Accepted(TransportEndpoint),
    Message(TransportEndpoint, Vec<u8>),
    Disconnected(TransportEndpoint),
}

impl From<StoredNetEvent> for TransportIoEvent {
    fn from(event: StoredNetEvent) -> Self {
        match event {
            StoredNetEvent::Connected(endpoint, status) => {
                Self::Connected(TransportEndpoint(endpoint), status)
            }
            StoredNetEvent::Accepted(endpoint, _) => Self::Accepted(TransportEndpoint(endpoint)),
            StoredNetEvent::Message(endpoint, bytes) => {
                Self::Message(TransportEndpoint(endpoint), bytes)
            }
            StoredNetEvent::Disconnected(endpoint) => {
                Self::Disconnected(TransportEndpoint(endpoint))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TransportEndpoint(Endpoint);

impl fmt::Display for TransportEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransportSendStatus {
    Sent,
    MaxPacketSizeExceeded,
    ResourceNotFound,
    ResourceNotAvailable,
}

impl From<SendStatus> for TransportSendStatus {
    fn from(status: SendStatus) -> Self {
        match status {
            SendStatus::Sent => Self::Sent,
            SendStatus::MaxPacketSizeExceeded => Self::MaxPacketSizeExceeded,
            SendStatus::ResourceNotFound => Self::ResourceNotFound,
            SendStatus::ResourceNotAvailable => Self::ResourceNotAvailable,
        }
    }
}
