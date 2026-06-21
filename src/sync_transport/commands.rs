use super::SyncTransportError;
use crate::sync_protocol::{PeerId, SyncEvent};
use std::sync::mpsc;

pub(super) enum TransportCommand {
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
