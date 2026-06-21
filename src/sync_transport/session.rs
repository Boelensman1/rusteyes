use super::SyncTransportError;
use crate::config::SharedSecret;
use crate::sync_protocol::{
    PeerId, SyncEvent, SyncMessage, SyncProtocolError, TransportControlFrame, decode_authenticated,
    encode_authenticated,
};
use std::fmt;
use std::str::{self, Utf8Error};

const HELLO_SEQUENCE: u64 = 0;
const FIRST_EVENT_SEQUENCE: u64 = 1;

pub(super) struct TransportSession {
    self_id: PeerId,
    shared_secret: SharedSecret,
    hello_payload: Vec<u8>,
    next_sequence: u64,
}

impl TransportSession {
    pub(super) fn new(
        self_id: PeerId,
        shared_secret: SharedSecret,
    ) -> Result<Self, SyncProtocolError> {
        let hello_payload = peer_hello_payload(self_id, &shared_secret)?;

        Ok(Self {
            self_id,
            shared_secret,
            hello_payload,
            next_sequence: FIRST_EVENT_SEQUENCE,
        })
    }

    pub(super) const fn self_id(&self) -> PeerId {
        self.self_id
    }

    pub(super) fn hello_payload(&self) -> &[u8] {
        &self.hello_payload
    }

    pub(super) fn encode_event(&mut self, event: SyncEvent) -> Result<Vec<u8>, SyncTransportError> {
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

    pub(super) fn decode_message(&self, bytes: &[u8]) -> Result<SyncMessage, TransportFrameError> {
        let input = str::from_utf8(bytes).map_err(TransportFrameError::NonUtf8)?;
        decode_authenticated(input, &self.shared_secret).map_err(TransportFrameError::Protocol)
    }
}

pub(super) fn peer_hello_payload(
    self_id: PeerId,
    shared_secret: &SharedSecret,
) -> Result<Vec<u8>, SyncProtocolError> {
    encode_authenticated(
        &SyncMessage::control(self_id, HELLO_SEQUENCE, TransportControlFrame::PeerHello),
        shared_secret,
    )
    .map(String::into_bytes)
}

#[derive(Debug)]
pub(super) enum TransportFrameError {
    NonUtf8(Utf8Error),
    Protocol(SyncProtocolError),
}

impl fmt::Display for TransportFrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonUtf8(error) => write!(formatter, "{error}"),
            Self::Protocol(error) => write!(formatter, "{error}"),
        }
    }
}
