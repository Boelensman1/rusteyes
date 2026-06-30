use super::{
    AuthenticatedSyncMessage, PeerId, SyncActiveBreak, SyncBreakOrigin,
    SyncCompatibilityFingerprint, SyncEvent, SyncFramePayload, SyncMessage, SyncProtocolError,
    TransportControlFrame, authenticate_message, decode_authenticated, encode_authenticated,
    encode_hex,
};
use crate::config::{BreakTypeConfig, Config, LockConfig, SharedSecret, StartupConfig};
use serde_json::{Value, json};
use std::collections::BTreeMap;
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
            message: String::from("Rest your eyes"),
            started_at_ms: 1_700_000_000_000,
            origin: SyncBreakOrigin::Scheduled { slot: 1 },
        },
        SyncEvent::SchedulerState {
            slot: 1,
            active_elapsed: Duration::from_millis(500),
            active_break: None,
        },
        SyncEvent::SchedulerReset,
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
    let message = SyncMessage::control(peer_id()?, 0, peer_hello_control());
    let encoded = encode_authenticated(&message, &shared_secret())?;

    assert_eq!(decode_authenticated(&encoded, &shared_secret())?, message);

    Ok(())
}

#[test]
fn rejects_peer_hello_with_non_zero_sequence() -> Result<(), Box<dyn Error>> {
    let message = SyncMessage::control(peer_id()?, 1, peer_hello_control());

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

    assert_eq!(value["version"], json!(5));
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
fn wire_json_carries_break_message_and_started_at() -> Result<(), Box<dyn Error>> {
    let message = SyncMessage::event(
        peer_id()?,
        7,
        SyncEvent::BreakStarted {
            name: String::from("short"),
            message: String::from("Rest your eyes"),
            started_at_ms: 1_700_000_000_000,
            origin: SyncBreakOrigin::Scheduled { slot: 1 },
        },
    );
    let encoded = encode_authenticated(&message, &shared_secret())?;
    let value = serde_json::from_str::<Value>(&encoded)?;

    assert_eq!(value["event"]["type"], json!("breakStarted"));
    assert_eq!(value["event"]["name"], json!("short"));
    assert_eq!(value["event"]["message"], json!("Rest your eyes"));
    assert_eq!(value["event"]["startedAtMs"], json!(1_700_000_000_000_u64));
    assert_eq!(value["event"]["origin"]["type"], json!("scheduled"));
    assert_eq!(value["event"]["origin"]["slot"], json!(1));

    assert_eq!(decode_authenticated(&encoded, &shared_secret())?, message);

    Ok(())
}

#[test]
fn wire_json_carries_scheduler_state_snapshot() -> Result<(), Box<dyn Error>> {
    let message = SyncMessage::event(
        peer_id()?,
        8,
        SyncEvent::SchedulerState {
            slot: 2,
            active_elapsed: Duration::from_millis(1_500),
            active_break: Some(SyncActiveBreak {
                name: String::from("long"),
                message: String::from("Look away"),
                started_at_ms: 1_700_000_000_000,
                origin: SyncBreakOrigin::Scheduled { slot: 2 },
                lock_after: true,
            }),
        },
    );
    let encoded = encode_authenticated(&message, &shared_secret())?;
    let value = serde_json::from_str::<Value>(&encoded)?;

    assert_eq!(value["event"]["type"], json!("schedulerState"));
    assert_eq!(value["event"]["slot"], json!(2));
    assert_eq!(value["event"]["activeElapsedMs"], json!(1_500));
    assert_eq!(value["event"]["activeBreak"]["name"], json!("long"));
    assert_eq!(value["event"]["activeBreak"]["message"], json!("Look away"));
    assert_eq!(
        value["event"]["activeBreak"]["startedAtMs"],
        json!(1_700_000_000_000_u64)
    );
    assert_eq!(
        value["event"]["activeBreak"]["origin"]["type"],
        json!("scheduled")
    );
    assert_eq!(value["event"]["activeBreak"]["origin"]["slot"], json!(2));
    assert_eq!(value["event"]["activeBreak"]["lockAfter"], json!(true));

    assert_eq!(decode_authenticated(&encoded, &shared_secret())?, message);

    Ok(())
}

#[test]
fn wire_json_carries_scheduler_reset() -> Result<(), Box<dyn Error>> {
    let message = SyncMessage::event(peer_id()?, 9, SyncEvent::SchedulerReset);
    let encoded = encode_authenticated(&message, &shared_secret())?;
    let value = serde_json::from_str::<Value>(&encoded)?;

    assert_eq!(value["event"]["type"], json!("schedulerReset"));
    assert_eq!(decode_authenticated(&encoded, &shared_secret())?, message);

    Ok(())
}

#[test]
fn wire_json_uses_control_field_for_peer_hello() -> Result<(), Box<dyn Error>> {
    let message = SyncMessage::control(peer_id()?, 0, peer_hello_control());
    let encoded = encode_authenticated(&message, &shared_secret())?;
    let value = serde_json::from_str::<Value>(&encoded)?;

    assert_eq!(value["version"], json!(5));
    assert_eq!(value["sender"], json!(PEER_ID));
    assert_eq!(value["sequence"], json!(0));
    assert_eq!(value["control"]["type"], json!("peerHello"));
    assert_eq!(
        value["control"]["compatibility"],
        json!(compatibility_fingerprint().to_string())
    );
    assert!(value["control"].get("breaks").is_none());
    assert!(value["control"].get("disablePresets").is_none());
    assert!(value.get("event").is_none());
    assert_eq!(
        value["mac"].as_str().map(str::len),
        Some(64),
        "HMAC-SHA256 should be encoded as 32 bytes of hex"
    );

    Ok(())
}

#[test]
fn compatibility_fingerprint_ignores_lock_command() -> Result<(), Box<dyn Error>> {
    let mut left = compatibility_config();
    let mut right = compatibility_config();
    left.lock = LockConfig {
        command: Some(vec![String::from("loginctl"), String::from("lock-session")]),
    };
    right.lock = LockConfig {
        command: Some(vec![String::from("custom-lock")]),
    };

    assert_eq!(
        SyncCompatibilityFingerprint::from_config(&left, &shared_secret())?,
        SyncCompatibilityFingerprint::from_config(&right, &shared_secret())?
    );
    Ok(())
}

#[test]
fn compatibility_fingerprint_ignores_sync_config_fields() -> Result<(), Box<dyn Error>> {
    let mut left = compatibility_config();
    let mut right = compatibility_config();
    left.sync.enabled = true;
    left.sync.shared_secret = Some(SharedSecret::new(String::from(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )));
    right.sync.shared_secret = Some(SharedSecret::new(String::from(
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    )));

    assert_eq!(fingerprint_for(&left)?, fingerprint_for(&right)?);
    Ok(())
}

#[test]
fn compatibility_fingerprint_ignores_startup_config() -> Result<(), Box<dyn Error>> {
    let mut left = compatibility_config();
    let mut right = compatibility_config();
    left.startup.open_at_login = Some(true);
    right.startup.open_at_login = Some(false);

    assert_eq!(fingerprint_for(&left)?, fingerprint_for(&right)?);
    Ok(())
}

#[test]
fn compatibility_fingerprint_changes_for_synced_behavior() -> Result<(), Box<dyn Error>> {
    let base = compatibility_config();

    let mut changed_after_active = base.clone();
    changed_after_active.breaks.after_active = Duration::from_secs(11);
    assert_ne!(
        fingerprint_for(&base)?,
        fingerprint_for(&changed_after_active)?,
        "break cadence should affect compatibility"
    );

    let mut changed_reset = base.clone();
    changed_reset.breaks.reset_after_idle = None;
    assert_ne!(
        fingerprint_for(&base)?,
        fingerprint_for(&changed_reset)?,
        "idle reset should affect compatibility"
    );

    let mut changed_break = base.clone();
    changed_break.breaks.types.insert(
        String::from("short"),
        BreakTypeConfig {
            interval: 1,
            duration: Duration::from_secs(21),
            messages: vec![String::from("Rest your eyes")],
            autolock: false,
        },
    );
    assert_ne!(
        fingerprint_for(&base)?,
        fingerprint_for(&changed_break)?,
        "break duration should affect compatibility"
    );

    let mut changed_message = base.clone();
    changed_message.breaks.types.insert(
        String::from("short"),
        BreakTypeConfig {
            interval: 1,
            duration: Duration::from_secs(20),
            messages: vec![String::from("Look away")],
            autolock: false,
        },
    );
    assert_ne!(
        fingerprint_for(&base)?,
        fingerprint_for(&changed_message)?,
        "break messages should affect compatibility"
    );

    let mut changed_autolock = base.clone();
    changed_autolock.breaks.types.insert(
        String::from("short"),
        BreakTypeConfig {
            interval: 1,
            duration: Duration::from_secs(20),
            messages: vec![String::from("Rest your eyes")],
            autolock: true,
        },
    );
    assert_ne!(
        fingerprint_for(&base)?,
        fingerprint_for(&changed_autolock)?,
        "autolock should affect compatibility"
    );

    let mut changed_presets = base.clone();
    changed_presets.disable_presets = vec![Duration::from_mins(1)];
    assert_ne!(
        fingerprint_for(&base)?,
        fingerprint_for(&changed_presets)?,
        "disable presets should affect compatibility"
    );

    Ok(())
}

#[test]
fn compatibility_fingerprint_normalizes_disable_preset_order() -> Result<(), Box<dyn Error>> {
    let mut left = compatibility_config();
    let mut right = compatibility_config();
    left.disable_presets = vec![Duration::from_secs(30), Duration::from_mins(1)];
    right.disable_presets = vec![Duration::from_mins(1), Duration::from_secs(30)];

    assert_eq!(fingerprint_for(&left)?, fingerprint_for(&right)?);
    Ok(())
}

#[test]
fn rejects_tampered_payload() -> Result<(), Box<dyn Error>> {
    let message = SyncMessage::event(
        peer_id()?,
        7,
        SyncEvent::BreakStarted {
            name: String::from("short"),
            message: String::from("Rest your eyes"),
            started_at_ms: 1_700_000_000_000,
            origin: SyncBreakOrigin::Scheduled { slot: 1 },
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
    message.version = 6;
    let encoded = authenticated_json(&message)?;

    assert_eq!(
        decode_authenticated(&encoded, &shared_secret()),
        Err(SyncProtocolError::UnsupportedVersion { version: 6 })
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
                message: String::from("Rest your eyes"),
                started_at_ms: 1_700_000_000_000,
                origin: SyncBreakOrigin::Scheduled { slot: 1 },
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
fn rejects_zero_scheduled_break_slot() -> Result<(), Box<dyn Error>> {
    let break_started = SyncMessage::event(
        peer_id()?,
        16,
        SyncEvent::BreakStarted {
            name: String::from("short"),
            message: String::from("Rest your eyes"),
            started_at_ms: 1_700_000_000_000,
            origin: SyncBreakOrigin::Scheduled { slot: 0 },
        },
    );
    assert_eq!(
        encode_authenticated(&break_started, &shared_secret()),
        Err(SyncProtocolError::InvalidBreakSlot { slot: 0 })
    );

    let scheduler_state = SyncMessage::event(
        peer_id()?,
        17,
        SyncEvent::SchedulerState {
            slot: 1,
            active_elapsed: Duration::ZERO,
            active_break: Some(SyncActiveBreak {
                name: String::from("short"),
                message: String::from("Rest your eyes"),
                started_at_ms: 1_700_000_000_000,
                origin: SyncBreakOrigin::Scheduled { slot: 0 },
                lock_after: false,
            }),
        },
    );
    assert_eq!(
        encode_authenticated(&scheduler_state, &shared_secret()),
        Err(SyncProtocolError::InvalidBreakSlot { slot: 0 })
    );

    Ok(())
}

#[test]
fn decoded_domain_event_keeps_event_payload_separate() -> Result<(), Box<dyn Error>> {
    let message = SyncMessage::event(peer_id()?, 18, SyncEvent::Enable);
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

fn peer_hello_control() -> TransportControlFrame {
    TransportControlFrame::PeerHello {
        compatibility: compatibility_fingerprint(),
    }
}

fn compatibility_fingerprint() -> SyncCompatibilityFingerprint {
    SyncCompatibilityFingerprint::for_test([0x11; 32])
}

fn fingerprint_for(config: &Config) -> Result<SyncCompatibilityFingerprint, SyncProtocolError> {
    SyncCompatibilityFingerprint::from_config(config, &shared_secret())
}

fn compatibility_config() -> Config {
    Config {
        breaks: crate::config::Breaks {
            after_active: Duration::from_secs(10),
            reset_after_idle: Some(Duration::from_mins(5)),
            types: [(
                String::from("short"),
                BreakTypeConfig {
                    interval: 1,
                    duration: Duration::from_secs(20),
                    messages: vec![String::from("Rest your eyes")],
                    autolock: false,
                },
            )]
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
        },
        disable_presets: vec![Duration::from_secs(30)],
        lock: LockConfig::default(),
        startup: StartupConfig::default(),
        sync: crate::config::SyncConfig::default(),
    }
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
