use crate::config::SharedSecret;
use crate::sync_protocol::PeerId;
use hmac::{Hmac, Mac};
use mdns_sd::{Receiver, ResolvedService, ScopedIp, ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::Serialize;
use sha2::Sha256;
use std::fmt;
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
use std::str::FromStr;
use std::time::Instant;

pub(crate) const SERVICE_TYPE: &str = "_resteyes-sync._udp.local.";

const DISCOVERY_VERSION: u8 = 1;
const KEY_VERSION: &str = "version";
const KEY_PEER: &str = "peer";
const KEY_PORT: &str = "port";
const KEY_MAC: &str = "mac";
const MAC_BYTES: usize = 32;

type HmacSha256 = Hmac<Sha256>;

pub(crate) struct LanDiscovery {
    daemon: ServiceDaemon,
    events: Receiver<ServiceEvent>,
    self_id: PeerId,
    shared_secret: SharedSecret,
    service_fullname: String,
}

impl LanDiscovery {
    pub(crate) fn start(
        self_id: PeerId,
        shared_secret: SharedSecret,
        transport_port: u16,
    ) -> Result<Self, SyncDiscoveryError> {
        validate_port(transport_port)?;

        let daemon = ServiceDaemon::new().map_err(|error| mdns_error(&error))?;
        let service = discovery_service_info(self_id, transport_port, &shared_secret)?;
        let service_fullname = service.get_fullname().to_owned();
        let events = daemon
            .browse(SERVICE_TYPE)
            .map_err(|error| mdns_error(&error))?;

        daemon
            .register(service)
            .map_err(|error| mdns_error(&error))?;

        Ok(Self {
            daemon,
            events,
            self_id,
            shared_secret,
            service_fullname,
        })
    }

    pub(crate) fn next_peer(
        &self,
        observed_at: Instant,
    ) -> Result<DiscoveredPeer, SyncDiscoveryError> {
        loop {
            let event = self
                .events
                .recv()
                .map_err(|error| SyncDiscoveryError::Mdns {
                    message: error.to_string(),
                })?;

            if let Some(peer) =
                discovered_peer_from_event(&event, self.self_id, &self.shared_secret, observed_at)?
            {
                return Ok(peer);
            }
        }
    }
}

impl Drop for LanDiscovery {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.service_fullname);
        let _ = self.daemon.shutdown();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscoveredPeer {
    pub(crate) peer_id: PeerId,
    pub(crate) address: SocketAddr,
    pub(crate) observed_at: Instant,
}

fn discovered_peer_from_event(
    event: &ServiceEvent,
    self_id: PeerId,
    shared_secret: &SharedSecret,
    observed_at: Instant,
) -> Result<Option<DiscoveredPeer>, SyncDiscoveryError> {
    match event {
        ServiceEvent::ServiceResolved(service) => {
            discovered_peer_from_resolved_service(service, self_id, shared_secret, observed_at)
        }
        _ => Ok(None),
    }
}

fn discovered_peer_from_resolved_service(
    service: &ResolvedService,
    self_id: PeerId,
    shared_secret: &SharedSecret,
    observed_at: Instant,
) -> Result<Option<DiscoveredPeer>, SyncDiscoveryError> {
    if service.ty_domain != SERVICE_TYPE {
        return Ok(None);
    }

    let metadata = DiscoveryMetadata::from_resolved_service(service, shared_secret)?;

    if metadata.peer_id == self_id {
        return Ok(None);
    }

    if service.get_port() != metadata.transport_port {
        return Err(SyncDiscoveryError::PortMismatch {
            txt_port: metadata.transport_port,
            srv_port: service.get_port(),
        });
    }

    let address =
        service_address(service, metadata.transport_port).ok_or(SyncDiscoveryError::NoAddress)?;

    Ok(Some(DiscoveredPeer {
        peer_id: metadata.peer_id,
        address,
        observed_at,
    }))
}

fn discovery_service_info(
    self_id: PeerId,
    transport_port: u16,
    shared_secret: &SharedSecret,
) -> Result<ServiceInfo, SyncDiscoveryError> {
    let properties = discovery_txt_properties(self_id, transport_port, shared_secret)?;
    let instance_name = instance_name(self_id);
    let host_name = host_name(self_id);
    let service = ServiceInfo::new(
        SERVICE_TYPE,
        &instance_name,
        &host_name,
        (),
        transport_port,
        &properties[..],
    )
    .map_err(|error| mdns_error(&error))?
    .enable_addr_auto();

    Ok(service)
}

fn service_address(service: &ResolvedService, port: u16) -> Option<SocketAddr> {
    service
        .get_addresses()
        .iter()
        .filter_map(|address| socket_addr(address, port))
        .min()
}

fn socket_addr(address: &ScopedIp, port: u16) -> Option<SocketAddr> {
    match address {
        ScopedIp::V4(address) => Some(SocketAddr::V4(SocketAddrV4::new(*address.addr(), port))),
        ScopedIp::V6(address) => Some(SocketAddr::V6(SocketAddrV6::new(
            *address.addr(),
            port,
            0,
            address.scope_id().index,
        ))),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DiscoveryMetadata {
    peer_id: PeerId,
    transport_port: u16,
}

impl DiscoveryMetadata {
    fn from_resolved_service(
        service: &ResolvedService,
        shared_secret: &SharedSecret,
    ) -> Result<Self, SyncDiscoveryError> {
        let version = required_txt(service, KEY_VERSION)?;
        let peer = required_txt(service, KEY_PEER)?;
        let port = required_txt(service, KEY_PORT)?;
        let mac = required_txt(service, KEY_MAC)?;

        let version = parse_version(version)?;
        let peer_id =
            PeerId::from_str(peer).map_err(|error| SyncDiscoveryError::InvalidPeerId {
                message: error.to_string(),
            })?;
        let transport_port = parse_port(port)?;
        let payload = DiscoveryPayload {
            version,
            peer: peer_id,
            port: transport_port,
        };
        let expected_mac = authenticate_payload(&payload, shared_secret)?;
        let actual_mac = decode_hex_mac(mac)?;

        if !constant_time_eq(&expected_mac, &actual_mac) {
            return Err(SyncDiscoveryError::AuthenticationFailed);
        }

        if version != DISCOVERY_VERSION {
            return Err(SyncDiscoveryError::UnsupportedVersion { version });
        }

        validate_port(transport_port)?;

        Ok(Self {
            peer_id,
            transport_port,
        })
    }
}

#[derive(Clone, Copy, Serialize)]
struct DiscoveryPayload {
    version: u8,
    peer: PeerId,
    port: u16,
}

fn discovery_txt_properties(
    self_id: PeerId,
    transport_port: u16,
    shared_secret: &SharedSecret,
) -> Result<Vec<(String, String)>, SyncDiscoveryError> {
    validate_port(transport_port)?;

    let payload = DiscoveryPayload {
        version: DISCOVERY_VERSION,
        peer: self_id,
        port: transport_port,
    };
    let mac = encode_hex(&authenticate_payload(&payload, shared_secret)?);

    Ok(vec![
        (String::from(KEY_VERSION), payload.version.to_string()),
        (String::from(KEY_PEER), payload.peer.to_string()),
        (String::from(KEY_PORT), payload.port.to_string()),
        (String::from(KEY_MAC), mac),
    ])
}

fn required_txt<'a>(
    service: &'a ResolvedService,
    key: &'static str,
) -> Result<&'a str, SyncDiscoveryError> {
    service
        .get_property_val_str(key)
        .ok_or(SyncDiscoveryError::MissingTxt { key })
}

fn parse_version(value: &str) -> Result<u8, SyncDiscoveryError> {
    value
        .parse::<u8>()
        .map_err(|_| SyncDiscoveryError::InvalidVersion {
            value: value.to_owned(),
        })
}

fn parse_port(value: &str) -> Result<u16, SyncDiscoveryError> {
    value
        .parse::<u16>()
        .map_err(|_| SyncDiscoveryError::InvalidPort {
            value: value.to_owned(),
        })
}

fn validate_port(port: u16) -> Result<(), SyncDiscoveryError> {
    if port == 0 {
        return Err(SyncDiscoveryError::ZeroPort);
    }

    Ok(())
}

fn authenticate_payload(
    payload: &DiscoveryPayload,
    shared_secret: &SharedSecret,
) -> Result<[u8; MAC_BYTES], SyncDiscoveryError> {
    let bytes = serde_json::to_vec(payload).map_err(|error| SyncDiscoveryError::Json {
        message: error.to_string(),
    })?;
    let mut mac = HmacSha256::new_from_slice(shared_secret.as_bytes()).map_err(|error| {
        SyncDiscoveryError::MacKey {
            message: error.to_string(),
        }
    })?;

    mac.update(&bytes);
    let bytes = mac.finalize().into_bytes();
    let mut output = [0; MAC_BYTES];
    output.copy_from_slice(&bytes);
    Ok(output)
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

fn decode_hex_mac(value: &str) -> Result<[u8; MAC_BYTES], SyncDiscoveryError> {
    let expected_len = MAC_BYTES * 2;
    if value.len() != expected_len {
        return Err(SyncDiscoveryError::InvalidMacLength {
            expected: expected_len,
            actual: value.len(),
        });
    }

    let mut output = [0; MAC_BYTES];
    let bytes = value.as_bytes();

    for (index, output_byte) in output.iter_mut().enumerate() {
        let high = hex_nibble(bytes[index * 2])?;
        let low = hex_nibble(bytes[index * 2 + 1])?;
        *output_byte = (high << 4) | low;
    }

    Ok(output)
}

fn hex_nibble(byte: u8) -> Result<u8, SyncDiscoveryError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(SyncDiscoveryError::InvalidMacHex),
    }
}

fn constant_time_eq(left: &[u8; MAC_BYTES], right: &[u8; MAC_BYTES]) -> bool {
    let mut difference = 0;

    for (left_byte, right_byte) in left.iter().zip(right) {
        difference |= *left_byte ^ *right_byte;
    }

    difference == 0
}

fn instance_name(peer_id: PeerId) -> String {
    format!("resteyes-{peer_id}")
}

fn host_name(peer_id: PeerId) -> String {
    format!("resteyes-{peer_id}.local.")
}

fn mdns_error(error: &mdns_sd::Error) -> SyncDiscoveryError {
    SyncDiscoveryError::Mdns {
        message: error.to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SyncDiscoveryError {
    Mdns { message: String },
    Json { message: String },
    MacKey { message: String },
    MissingTxt { key: &'static str },
    InvalidVersion { value: String },
    UnsupportedVersion { version: u8 },
    InvalidPeerId { message: String },
    InvalidPort { value: String },
    ZeroPort,
    PortMismatch { txt_port: u16, srv_port: u16 },
    InvalidMacLength { expected: usize, actual: usize },
    InvalidMacHex,
    AuthenticationFailed,
    NoAddress,
}

impl fmt::Display for SyncDiscoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mdns { message } => write!(formatter, "mDNS discovery failed: {message}"),
            Self::Json { message } => write!(formatter, "invalid discovery JSON: {message}"),
            Self::MacKey { message } => write!(formatter, "invalid discovery MAC key: {message}"),
            Self::MissingTxt { key } => write!(formatter, "missing discovery TXT key {key}"),
            Self::InvalidVersion { value } => {
                write!(formatter, "invalid discovery version {value:?}")
            }
            Self::UnsupportedVersion { version } => {
                write!(formatter, "unsupported discovery version {version}")
            }
            Self::InvalidPeerId { message } => {
                write!(formatter, "invalid discovery peer id: {message}")
            }
            Self::InvalidPort { value } => write!(formatter, "invalid discovery port {value:?}"),
            Self::ZeroPort => formatter.write_str("discovery port must be greater than zero"),
            Self::PortMismatch { txt_port, srv_port } => write!(
                formatter,
                "discovery TXT port {txt_port} does not match SRV port {srv_port}"
            ),
            Self::InvalidMacLength { expected, actual } => write!(
                formatter,
                "discovery MAC must be {expected} hex characters, got {actual}"
            ),
            Self::InvalidMacHex => formatter.write_str("discovery MAC must contain only hex"),
            Self::AuthenticationFailed => {
                formatter.write_str("discovery metadata authentication failed")
            }
            Self::NoAddress => formatter.write_str("discovery service has no usable address"),
        }
    }
}

impl std::error::Error for SyncDiscoveryError {}

#[cfg(test)]
mod tests;
