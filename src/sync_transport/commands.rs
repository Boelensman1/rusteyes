use super::SyncTransportError;
use crate::sync_protocol::{PeerId, SyncEvent};

pub(super) enum TransportCommand {
    Broadcast {
        event: SyncEvent,
        reply: flume::Sender<Result<usize, SyncTransportError>>,
    },
    Send {
        peer_id: PeerId,
        event: SyncEvent,
        reply: flume::Sender<Result<bool, SyncTransportError>>,
    },
}
