use super::{
    AuthenticatedSyncMessage, PeerId, SyncEvent, SyncFramePayload, SyncMessage, SyncProtocolError,
    TransportControlFrame, authenticate_message, decode_authenticated, encode_authenticated,
    encode_hex,
};
use crate::config::SharedSecret;
use serde_json::{Value, json};
use std::error::Error;
use std::str::FromStr;
use std::time::Duration;

const PEER_ID: &str = "00112233445566778899aabbccddeeff";
const SHARED_SECRET: &str = "0123456789abcdef0123456789abcdef";
const WRONG_SHARED_SECRET: &str = "fedcba9876543210fedcba9876543210";

#[test]
fn generated_peer_id_is_valid_protocol_id() -> Result<(), Box<dyn Error>> {
    let peer_id = PeerId::generate()?;
    let encoded = peer_id.to_string();

    assert_eq!(encoded.len(), 32);
    assert_eq!(PeerId::from_str(&encoded)?, peer_id);

    Ok(())
}

#[test]
fn authenticates_all_sync_event_variants() -> Result<(), Box<dyn Error>> {
    let events = [
        SyncEvent::ActiveTimeElapsed {
            elapsed: Duration::from_millis(1_500),
        },
        SyncEvent::BreakStarted {
            name: String::from("short"),
        },
        SyncEvent::DisableFor {
            duration: Duration::from_secs(30),
        },
        SyncEvent::DisableUntilRestart,
        SyncEvent::Enable,
        SyncEvent::LockAfterCurrentBreak,
    ];

    for (index, event) in events.into_iter().enumerate() {
        let sequence = u64::try_from(index)? + 1;
        let message = SyncMessage::event(peer_id()?, sequence, event);
        let encoded = encode_authenticated(&message, &shared_secret())?;

        assert_eq!(decode_authenticated(&encoded, &shared_secret())?, message);
    }

    Ok(())
}

#[test]
fn authenticates_peer_hello_control_frame() -> Result<(), Box<dyn Error>> {
    let message = SyncMessage::control(peer_id()?, 0, TransportControlFrame::PeerHello);
    let encoded = encode_authenticated(&message, &shared_secret())?;

    assert_eq!(decode_authenticated(&encoded, &shared_secret())?, message);

    Ok(())
}

#[test]
fn rejects_peer_hello_with_non_zero_sequence() -> Result<(), Box<dyn Error>> {
    let message = SyncMessage::control(peer_id()?, 1, TransportControlFrame::PeerHello);

    assert_eq!(
        encode_authenticated(&message, &shared_secret()),
        Err(SyncProtocolError::InvalidHelloSequence { sequence: 1 })
    );

    Ok(())
}

#[test]
fn rejects_domain_event_with_zero_sequence() -> Result<(), Box<dyn Error>> {
    let message = SyncMessage::event(peer_id()?, 0, SyncEvent::Enable);

    assert_eq!(
        encode_authenticated(&message, &shared_secret()),
        Err(SyncProtocolError::InvalidEventSequence { sequence: 0 })
    );

    Ok(())
}

#[test]
fn wire_json_uses_expected_version_sender_sequence_event_and_mac() -> Result<(), Box<dyn Error>> {
    let message = SyncMessage::event(
        peer_id()?,
        42,
        SyncEvent::ActiveTimeElapsed {
            elapsed: Duration::from_millis(1_500),
        },
    );
    let encoded = encode_authenticated(&message, &shared_secret())?;
    let value = serde_json::from_str::<Value>(&encoded)?;

    assert_eq!(value["version"], json!(1));
    assert_eq!(value["sender"], json!(PEER_ID));
    assert_eq!(value["sequence"], json!(42));
    assert_eq!(value["event"]["type"], json!("activeTimeElapsed"));
    assert_eq!(value["event"]["elapsedMs"], json!(1_500));
    assert_eq!(
        value["mac"].as_str().map(str::len),
        Some(64),
        "HMAC-SHA256 should be encoded as 32 bytes of hex"
    );

    Ok(())
}

#[test]
fn wire_json_uses_control_field_for_peer_hello() -> Result<(), Box<dyn Error>> {
    let message = SyncMessage::control(peer_id()?, 0, TransportControlFrame::PeerHello);
    let encoded = encode_authenticated(&message, &shared_secret())?;
    let value = serde_json::from_str::<Value>(&encoded)?;

    assert_eq!(value["version"], json!(1));
    assert_eq!(value["sender"], json!(PEER_ID));
    assert_eq!(value["sequence"], json!(0));
    assert_eq!(value["control"]["type"], json!("peerHello"));
    assert!(value.get("event").is_none());
    assert_eq!(
        value["mac"].as_str().map(str::len),
        Some(64),
        "HMAC-SHA256 should be encoded as 32 bytes of hex"
    );

    Ok(())
}

#[test]
fn rejects_tampered_payload() -> Result<(), Box<dyn Error>> {
    let message = SyncMessage::event(
        peer_id()?,
        7,
        SyncEvent::BreakStarted {
            name: String::from("short"),
        },
    );
    let encoded = encode_authenticated(&message, &shared_secret())?;
    let mut value = serde_json::from_str::<Value>(&encoded)?;
    value["event"]["name"] = json!("long");
    let tampered = serde_json::to_string(&value)?;

    assert_eq!(
        decode_authenticated(&tampered, &shared_secret()),
        Err(SyncProtocolError::AuthenticationFailed)
    );

    Ok(())
}

#[test]
fn rejects_tampered_mac() -> Result<(), Box<dyn Error>> {
    let message = SyncMessage::event(peer_id()?, 8, SyncEvent::Enable);
    let encoded = encode_authenticated(&message, &shared_secret())?;
    let mut value = serde_json::from_str::<Value>(&encoded)?;
    value["mac"] = json!("0000000000000000000000000000000000000000000000000000000000000000");
    let tampered = serde_json::to_string(&value)?;

    assert_eq!(
        decode_authenticated(&tampered, &shared_secret()),
        Err(SyncProtocolError::AuthenticationFailed)
    );

    Ok(())
}

#[test]
fn rejects_wrong_shared_secret() -> Result<(), Box<dyn Error>> {
    let message = SyncMessage::event(peer_id()?, 9, SyncEvent::DisableUntilRestart);
    let encoded = encode_authenticated(&message, &shared_secret())?;

    assert_eq!(
        decode_authenticated(&encoded, &wrong_shared_secret()),
        Err(SyncProtocolError::AuthenticationFailed)
    );

    Ok(())
}

#[test]
fn rejects_invalid_mac_hex() -> Result<(), Box<dyn Error>> {
    let message = SyncMessage::event(peer_id()?, 10, SyncEvent::Enable);
    let encoded = encode_authenticated(&message, &shared_secret())?;
    let mut value = serde_json::from_str::<Value>(&encoded)?;
    value["mac"] = json!("not-hex");
    let invalid = serde_json::to_string(&value)?;

    assert_eq!(
        decode_authenticated(&invalid, &shared_secret()),
        Err(SyncProtocolError::InvalidHexLength {
            field: "mac",
            expected: 64,
            actual: 7,
        })
    );

    Ok(())
}

#[test]
fn rejects_unsupported_version_after_authentication() -> Result<(), Box<dyn Error>> {
    let mut message = SyncMessage::event(peer_id()?, 11, SyncEvent::Enable);
    message.version = 2;
    let encoded = authenticated_json(&message)?;

    assert_eq!(
        decode_authenticated(&encoded, &shared_secret()),
        Err(SyncProtocolError::UnsupportedVersion { version: 2 })
    );

    Ok(())
}

#[test]
fn rejects_bad_sender_id() -> Result<(), Box<dyn Error>> {
    let message = SyncMessage::event(peer_id()?, 12, SyncEvent::Enable);
    let encoded = encode_authenticated(&message, &shared_secret())?;
    let mut value = serde_json::from_str::<Value>(&encoded)?;
    value["sender"] = json!("bad-sender");
    let invalid = serde_json::to_string(&value)?;

    assert!(matches!(
        decode_authenticated(&invalid, &shared_secret()),
        Err(SyncProtocolError::Json { .. })
    ));

    Ok(())
}

#[test]
fn rejects_zero_active_time_duration() -> Result<(), Box<dyn Error>> {
    let message = SyncMessage::event(
        peer_id()?,
        13,
        SyncEvent::ActiveTimeElapsed {
            elapsed: Duration::ZERO,
        },
    );
    let encoded = authenticated_json(&message)?;

    assert_eq!(
        decode_authenticated(&encoded, &shared_secret()),
        Err(SyncProtocolError::ZeroDuration { field: "elapsedMs" })
    );

    Ok(())
}

#[test]
fn rejects_zero_disable_duration() -> Result<(), Box<dyn Error>> {
    let message = SyncMessage::event(
        peer_id()?,
        14,
        SyncEvent::DisableFor {
            duration: Duration::ZERO,
        },
    );
    let encoded = authenticated_json(&message)?;

    assert_eq!(
        decode_authenticated(&encoded, &shared_secret()),
        Err(SyncProtocolError::ZeroDuration {
            field: "durationMs"
        })
    );

    Ok(())
}

#[test]
fn rejects_invalid_break_names() -> Result<(), Box<dyn Error>> {
    for name in ["", " short", "short "] {
        let message = SyncMessage::event(
            peer_id()?,
            15,
            SyncEvent::BreakStarted {
                name: String::from(name),
            },
        );

        assert_eq!(
            encode_authenticated(&message, &shared_secret()),
            Err(SyncProtocolError::InvalidBreakName {
                name: String::from(name),
            })
        );
    }

    Ok(())
}

#[test]
fn decoded_domain_event_keeps_event_payload_separate() -> Result<(), Box<dyn Error>> {
    let message = SyncMessage::event(peer_id()?, 16, SyncEvent::Enable);
    let encoded = encode_authenticated(&message, &shared_secret())?;

    assert_eq!(
        decode_authenticated(&encoded, &shared_secret())?.payload,
        SyncFramePayload::Event {
            event: SyncEvent::Enable,
        }
    );

    Ok(())
}

fn authenticated_json(message: &SyncMessage) -> Result<String, SyncProtocolError> {
    let mac = authenticate_message(message, &shared_secret())?;
    serde_json::to_string(&AuthenticatedSyncMessage {
        message: message.clone(),
        mac: encode_hex(&mac),
    })
    .map_err(|error| SyncProtocolError::Json {
        message: error.to_string(),
    })
}

fn peer_id() -> Result<PeerId, SyncProtocolError> {
    PeerId::from_str(PEER_ID)
}

fn shared_secret() -> SharedSecret {
    SharedSecret::new(String::from(SHARED_SECRET))
}

fn wrong_shared_secret() -> SharedSecret {
    SharedSecret::new(String::from(WRONG_SHARED_SECRET))
}
