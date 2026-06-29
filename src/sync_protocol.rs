use crate::config::{Config, SharedSecret};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::Sha256;
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use std::time::Duration;

const PROTOCOL_VERSION: u8 = 4;
const PEER_ID_BYTES: usize = 16;
const MAC_BYTES: usize = 32;
const COMPATIBILITY_FINGERPRINT_DOMAIN: &[u8] = b"rusteyes-sync-config-compatibility-v1";

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct PeerId([u8; PEER_ID_BYTES]);

impl PeerId {
    pub(crate) fn generate() -> Result<Self, SyncProtocolError> {
        let mut bytes = [0; PEER_ID_BYTES];
        getrandom::fill(&mut bytes).map_err(|error| SyncProtocolError::Random {
            message: error.to_string(),
        })?;
        Ok(Self(bytes))
    }
}

impl fmt::Display for PeerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&encode_hex(&self.0))
    }
}

impl FromStr for PeerId {
    type Err = SyncProtocolError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self(decode_hex_exact::<PEER_ID_BYTES>(value, "sender")?))
    }
}

impl Serialize for PeerId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for PeerId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_str(&value).map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SyncCompatibilityFingerprint([u8; MAC_BYTES]);

impl SyncCompatibilityFingerprint {
    pub(crate) fn from_config(
        config: &Config,
        shared_secret: &SharedSecret,
    ) -> Result<Self, SyncProtocolError> {
        let profile = SyncCompatibilityProfile::from_config(config)?;
        let profile_json = serde_json::to_vec(&profile).map_err(|error| json_error(&error))?;
        let mut mac = hmac_sha256(shared_secret)?;

        mac.update(COMPATIBILITY_FINGERPRINT_DOMAIN);
        mac.update(&[0]);
        mac.update(&profile_json);

        Ok(Self(finish_hmac(mac)))
    }

    #[cfg(test)]
    pub(crate) const fn for_test(bytes: [u8; MAC_BYTES]) -> Self {
        Self(bytes)
    }
}

impl fmt::Display for SyncCompatibilityFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&encode_hex(&self.0))
    }
}

impl Serialize for SyncCompatibilityFingerprint {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for SyncCompatibilityFingerprint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        decode_hex_exact::<MAC_BYTES>(&value, "compatibility")
            .map(Self)
            .map_err(de::Error::custom)
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncCompatibilityProfile {
    breaks_after_active_ms: u64,
    breaks_reset_after_idle_ms: Option<u64>,
    break_types: BTreeMap<String, SyncCompatibilityBreakType>,
    disable_preset_ms: Vec<u64>,
}

impl SyncCompatibilityProfile {
    fn from_config(config: &Config) -> Result<Self, SyncProtocolError> {
        let mut disable_preset_ms = config
            .disable_presets
            .iter()
            .map(|duration| duration_millis_u64(*duration, "disablePresets"))
            .collect::<Result<Vec<_>, _>>()?;
        disable_preset_ms.sort_unstable();

        Ok(Self {
            breaks_after_active_ms: duration_millis_u64(
                config.breaks.after_active,
                "breaks.afterActive",
            )?,
            breaks_reset_after_idle_ms: config
                .breaks
                .reset_after_idle
                .map(|duration| duration_millis_u64(duration, "breaks.resetAfterIdle"))
                .transpose()?,
            break_types: config
                .breaks
                .types
                .iter()
                .map(|(name, break_type)| {
                    Ok((
                        name.clone(),
                        SyncCompatibilityBreakType {
                            interval: break_type.interval,
                            duration_ms: duration_millis_u64(
                                break_type.duration,
                                "breaks.types.duration",
                            )?,
                            messages: break_type.messages.clone(),
                            autolock: break_type.autolock,
                        },
                    ))
                })
                .collect::<Result<BTreeMap<_, _>, SyncProtocolError>>()?,
            disable_preset_ms,
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncCompatibilityBreakType {
    interval: usize,
    duration_ms: u64,
    messages: Vec<String>,
    autolock: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SyncMessage {
    pub(crate) version: u8,
    pub(crate) sender: PeerId,
    pub(crate) sequence: u64,
    #[serde(flatten)]
    pub(crate) payload: SyncFramePayload,
}

impl SyncMessage {
    pub(crate) fn control(sender: PeerId, sequence: u64, control: TransportControlFrame) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            sender,
            sequence,
            payload: SyncFramePayload::Control { control },
        }
    }

    pub(crate) fn event(sender: PeerId, sequence: u64, event: SyncEvent) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            sender,
            sequence,
            payload: SyncFramePayload::Event { event },
        }
    }

    fn validate(&self) -> Result<(), SyncProtocolError> {
        if self.version != PROTOCOL_VERSION {
            return Err(SyncProtocolError::UnsupportedVersion {
                version: self.version,
            });
        }

        match &self.payload {
            SyncFramePayload::Control {
                control: TransportControlFrame::PeerHello { .. },
            } if self.sequence != 0 => Err(SyncProtocolError::InvalidHelloSequence {
                sequence: self.sequence,
            }),
            SyncFramePayload::Control { .. } => Ok(()),
            SyncFramePayload::Event { .. } if self.sequence == 0 => {
                Err(SyncProtocolError::InvalidEventSequence {
                    sequence: self.sequence,
                })
            }
            SyncFramePayload::Event { event } => event.validate(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum SyncFramePayload {
    Control { control: TransportControlFrame },
    Event { event: SyncEvent },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(crate) enum TransportControlFrame {
    PeerHello {
        compatibility: SyncCompatibilityFingerprint,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(crate) enum SyncEvent {
    ActiveTimeElapsed {
        #[serde(rename = "elapsedMs", with = "duration_millis")]
        elapsed: Duration,
    },
    BreakStarted {
        name: String,
        message: String,
        #[serde(rename = "startedAtMs")]
        started_at_ms: u64,
        origin: SyncBreakOrigin,
    },
    SchedulerState {
        slot: usize,
        #[serde(rename = "activeElapsedMs", with = "duration_millis")]
        active_elapsed: Duration,
        #[serde(rename = "activeBreak")]
        active_break: Option<SyncActiveBreak>,
    },
    DisableFor {
        #[serde(rename = "durationMs", with = "duration_millis")]
        duration: Duration,
    },
    DisableUntilRestart,
    Enable,
    LockAfterCurrentBreak,
}

impl SyncEvent {
    fn validate(&self) -> Result<(), SyncProtocolError> {
        match self {
            Self::ActiveTimeElapsed { elapsed } if elapsed.is_zero() => {
                Err(SyncProtocolError::ZeroDuration { field: "elapsedMs" })
            }
            Self::DisableFor { duration } if duration.is_zero() => {
                Err(SyncProtocolError::ZeroDuration {
                    field: "durationMs",
                })
            }
            Self::BreakStarted { name, .. } if name.trim() != name || name.is_empty() => {
                Err(SyncProtocolError::InvalidBreakName { name: name.clone() })
            }
            Self::BreakStarted {
                origin: SyncBreakOrigin::Scheduled { slot: 0 },
                ..
            } => Err(SyncProtocolError::InvalidBreakSlot { slot: 0 }),
            Self::SchedulerState {
                active_break: Some(active_break),
                ..
            } => active_break.validate(),
            Self::ActiveTimeElapsed { .. }
            | Self::BreakStarted { .. }
            | Self::SchedulerState { .. }
            | Self::DisableFor { .. }
            | Self::DisableUntilRestart
            | Self::Enable
            | Self::LockAfterCurrentBreak => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncActiveBreak {
    pub(crate) name: String,
    pub(crate) message: String,
    #[serde(rename = "startedAtMs")]
    pub(crate) started_at_ms: u64,
    pub(crate) origin: SyncBreakOrigin,
    #[serde(rename = "lockAfter")]
    pub(crate) lock_after: bool,
}

impl SyncActiveBreak {
    fn validate(&self) -> Result<(), SyncProtocolError> {
        if self.name.trim() != self.name || self.name.is_empty() {
            return Err(SyncProtocolError::InvalidBreakName {
                name: self.name.clone(),
            });
        }

        if let SyncBreakOrigin::Scheduled { slot: 0 } = self.origin {
            return Err(SyncProtocolError::InvalidBreakSlot { slot: 0 });
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(crate) enum SyncBreakOrigin {
    Manual,
    Scheduled { slot: usize },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct AuthenticatedSyncMessage {
    #[serde(flatten)]
    message: SyncMessage,
    mac: String,
}

pub(crate) fn encode_authenticated(
    message: &SyncMessage,
    shared_secret: &SharedSecret,
) -> Result<String, SyncProtocolError> {
    message.validate()?;
    let mac = authenticate_message(message, shared_secret)?;
    let envelope = AuthenticatedSyncMessage {
        message: message.clone(),
        mac: encode_hex(&mac),
    };

    serde_json::to_string(&envelope).map_err(|error| json_error(&error))
}

pub(crate) fn decode_authenticated(
    input: &str,
    shared_secret: &SharedSecret,
) -> Result<SyncMessage, SyncProtocolError> {
    let envelope = serde_json::from_str::<AuthenticatedSyncMessage>(input)
        .map_err(|error| json_error(&error))?;
    let expected_mac = authenticate_message(&envelope.message, shared_secret)?;
    let actual_mac = decode_hex_exact::<MAC_BYTES>(&envelope.mac, "mac")?;

    if !constant_time_eq(&expected_mac, &actual_mac) {
        return Err(SyncProtocolError::AuthenticationFailed);
    }

    envelope.message.validate()?;
    Ok(envelope.message)
}

fn authenticate_message(
    message: &SyncMessage,
    shared_secret: &SharedSecret,
) -> Result<[u8; MAC_BYTES], SyncProtocolError> {
    let payload = serde_json::to_vec(message).map_err(|error| json_error(&error))?;
    let mut mac = hmac_sha256(shared_secret)?;

    mac.update(&payload);
    Ok(finish_hmac(mac))
}

fn hmac_sha256(shared_secret: &SharedSecret) -> Result<HmacSha256, SyncProtocolError> {
    HmacSha256::new_from_slice(shared_secret.as_bytes()).map_err(|error| {
        SyncProtocolError::MacKey {
            message: error.to_string(),
        }
    })
}

fn finish_hmac(mac: HmacSha256) -> [u8; MAC_BYTES] {
    let bytes = mac.finalize().into_bytes();
    let mut output = [0; MAC_BYTES];
    output.copy_from_slice(&bytes);
    output
}

fn constant_time_eq(left: &[u8; MAC_BYTES], right: &[u8; MAC_BYTES]) -> bool {
    let mut difference = 0;

    for (left_byte, right_byte) in left.iter().zip(right) {
        difference |= *left_byte ^ *right_byte;
    }

    difference == 0
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn decode_hex_exact<const N: usize>(
    value: &str,
    field: &'static str,
) -> Result<[u8; N], SyncProtocolError> {
    let expected_len = N * 2;
    if value.len() != expected_len {
        return Err(SyncProtocolError::InvalidHexLength {
            field,
            expected: expected_len,
            actual: value.len(),
        });
    }

    let mut output = [0; N];
    let bytes = value.as_bytes();

    for (index, output_byte) in output.iter_mut().enumerate() {
        let high = hex_nibble(bytes[index * 2], field)?;
        let low = hex_nibble(bytes[index * 2 + 1], field)?;
        *output_byte = (high << 4) | low;
    }

    Ok(output)
}

fn hex_nibble(byte: u8, field: &'static str) -> Result<u8, SyncProtocolError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(SyncProtocolError::InvalidHex { field }),
    }
}

fn json_error(error: &serde_json::Error) -> SyncProtocolError {
    SyncProtocolError::Json {
        message: error.to_string(),
    }
}

fn duration_millis_u64(duration: Duration, field: &'static str) -> Result<u64, SyncProtocolError> {
    u64::try_from(duration.as_millis()).map_err(|_| SyncProtocolError::DurationOutOfRange { field })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SyncProtocolError {
    Json {
        message: String,
    },
    Random {
        message: String,
    },
    MacKey {
        message: String,
    },
    InvalidHexLength {
        field: &'static str,
        expected: usize,
        actual: usize,
    },
    InvalidHex {
        field: &'static str,
    },
    UnsupportedVersion {
        version: u8,
    },
    ZeroDuration {
        field: &'static str,
    },
    InvalidBreakName {
        name: String,
    },
    InvalidBreakSlot {
        slot: usize,
    },
    InvalidHelloSequence {
        sequence: u64,
    },
    InvalidEventSequence {
        sequence: u64,
    },
    DurationOutOfRange {
        field: &'static str,
    },
    AuthenticationFailed,
}

impl fmt::Display for SyncProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json { message } => write!(formatter, "invalid sync message JSON: {message}"),
            Self::Random { message } => {
                write!(formatter, "failed to generate sync peer id: {message}")
            }
            Self::MacKey { message } => write!(formatter, "invalid sync MAC key: {message}"),
            Self::InvalidHexLength {
                field,
                expected,
                actual,
            } => write!(
                formatter,
                "sync field {field} must be {expected} hex characters, got {actual}"
            ),
            Self::InvalidHex { field } => {
                write!(formatter, "sync field {field} must contain only hex")
            }
            Self::UnsupportedVersion { version } => {
                write!(formatter, "unsupported sync protocol version {version}")
            }
            Self::ZeroDuration { field } => {
                write!(formatter, "sync field {field} must be greater than zero")
            }
            Self::InvalidBreakName { name } => write!(
                formatter,
                "sync break name {name:?} must not be empty or contain surrounding whitespace"
            ),
            Self::InvalidBreakSlot { slot } => {
                write!(
                    formatter,
                    "sync scheduled break slot must be greater than zero, got {slot}"
                )
            }
            Self::InvalidHelloSequence { sequence } => write!(
                formatter,
                "sync peer hello sequence must be 0, got {sequence}"
            ),
            Self::InvalidEventSequence { sequence } => write!(
                formatter,
                "sync domain event sequence must be greater than 0, got {sequence}"
            ),
            Self::DurationOutOfRange { field } => {
                write!(formatter, "sync field {field} duration is too large")
            }
            Self::AuthenticationFailed => formatter.write_str("sync message authentication failed"),
        }
    }
}

impl std::error::Error for SyncProtocolError {}

mod duration_millis {
    use serde::{Deserialize, Deserializer, Serializer, ser};
    use std::time::Duration;

    pub(super) fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let millis = u64::try_from(duration.as_millis()).map_err(ser::Error::custom)?;
        serializer.serialize_u64(millis)
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let millis = u64::deserialize(deserializer)?;
        Ok(Duration::from_millis(millis))
    }
}

#[cfg(test)]
mod tests;
