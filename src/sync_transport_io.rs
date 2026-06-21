use message_io::events::EventReceiver;
use message_io::network::{Endpoint, ResourceId, SendStatus, Transport};
use message_io::node::{self, NodeHandler, NodeTask, StoredNetEvent, StoredNodeEvent};
use std::fmt;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
#[cfg(test)]
use std::time::Duration;

pub(crate) struct TransportIo {
    handle: TransportIoHandle,
    listener_id: Option<ResourceId>,
    node_task: Option<NodeTask>,
}

impl TransportIo {
    pub(crate) fn listen(address: impl ToSocketAddrs) -> io::Result<TransportBinding> {
        let (handler, listener) = node::split::<()>();
        let (listener_id, local_addr) = handler.network().listen(Transport::FramedTcp, address)?;
        let (node_task, event_receiver) = listener.enqueue();
        let handle = TransportIoHandle { handler };

        Ok(TransportBinding {
            io: Self {
                handle,
                listener_id: Some(listener_id),
                node_task: Some(node_task),
            },
            event_receiver: TransportIoReceiver {
                receiver: event_receiver,
            },
            local_addr,
        })
    }

    pub(crate) fn handle(&self) -> TransportIoHandle {
        self.handle.clone()
    }

    pub(crate) fn remove_listener(&mut self) {
        if let Some(listener_id) = self.listener_id.take() {
            _ = self.handle.handler.network().remove(listener_id);
        }
    }

    pub(crate) fn stop(&self) {
        self.handle.handler.stop();
    }

    pub(crate) fn wait(&mut self) {
        if let Some(mut node_task) = self.node_task.take() {
            node_task.wait();
        }
    }

    pub(crate) fn shutdown(&mut self) {
        self.remove_listener();
        self.stop();
        self.wait();
    }
}

pub(crate) struct TransportBinding {
    pub(crate) io: TransportIo,
    pub(crate) event_receiver: TransportIoReceiver,
    pub(crate) local_addr: SocketAddr,
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

    pub(crate) fn wake(&self) {
        self.handler.signals().send(());
    }

    pub(crate) fn is_running(&self) -> bool {
        self.handler.is_running()
    }
}

pub(crate) struct TransportIoReceiver {
    receiver: EventReceiver<StoredNodeEvent<()>>,
}

impl TransportIoReceiver {
    pub(crate) fn receive(&mut self) -> TransportIoEvent {
        match self.receiver.receive() {
            StoredNodeEvent::Network(event) => event.into(),
            StoredNodeEvent::Signal(()) => TransportIoEvent::Wake,
        }
    }

    #[cfg(test)]
    pub(crate) fn receive_timeout(&mut self, timeout: Duration) -> Option<TransportIoEvent> {
        let event = self.receiver.receive_timeout(timeout)?;

        match event {
            StoredNodeEvent::Network(event) => Some(event.into()),
            StoredNodeEvent::Signal(()) => Some(TransportIoEvent::Wake),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum TransportIoEvent {
    Wake,
    Connected(TransportEndpoint),
    ConnectFailed(TransportEndpoint),
    Accepted(TransportEndpoint),
    Message(TransportEndpoint, Vec<u8>),
    Disconnected(TransportEndpoint),
}

impl From<StoredNetEvent> for TransportIoEvent {
    fn from(event: StoredNetEvent) -> Self {
        match event {
            StoredNetEvent::Connected(endpoint, true) => {
                Self::Connected(TransportEndpoint(endpoint))
            }
            StoredNetEvent::Connected(endpoint, false) => {
                Self::ConnectFailed(TransportEndpoint(endpoint))
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
