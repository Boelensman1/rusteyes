use super::{
    DISCOVERY_VERSION, DiscoveredPeer, DiscoveryEvent, DiscoveryMetadata, DiscoveryPayload,
    KEY_MAC, KEY_PEER, KEY_PORT, KEY_VERSION, SERVICE_TYPE, SyncDiscoveryError,
    authenticate_payload, discovered_peer_from_resolved_service, discovery_txt_properties,
    encode_hex, host_name, instance_name, receive_discovery_event, service_address,
};
use crate::config::SharedSecret;
use crate::sync_protocol::PeerId;
use mdns_sd::{ResolvedService, ServiceEvent, ServiceInfo};
use std::error::Error;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::time::Instant;

const LOCAL_PEER: &str = "00112233445566778899aabbccddeeff";
const REMOTE_PEER: &str = "ffeeddccbbaa99887766554433221100";
const SHARED_SECRET: &str = "0123456789abcdef0123456789abcdef";
const WRONG_SHARED_SECRET: &str = "fedcba9876543210fedcba9876543210";

#[test]
fn service_type_advertises_tcp_transport() {
    assert_eq!(SERVICE_TYPE, "_rusteyes-sync._tcp.local.");
}

#[test]
fn discovery_shutdown_event_wakes_receive() -> Result<(), Box<dyn Error>> {
    let (_service_sender, service_receiver) = flume::bounded::<ServiceEvent>(1);
    let (shutdown_sender, shutdown_receiver) = flume::bounded(1);

    shutdown_sender.send(())?;

    assert_eq!(
        receive_discovery_event(
            &service_receiver,
            &shutdown_receiver,
            local_peer()?,
            &shared_secret(),
        ),
        DiscoveryEvent::Shutdown,
    );

    Ok(())
}

#[test]
fn discovery_service_event_maps_to_peer() -> Result<(), Box<dyn Error>> {
    let (service_sender, service_receiver) = flume::bounded(1);
    let (_shutdown_sender, shutdown_receiver) = flume::bounded(1);
    let service = resolved_service(remote_peer()?, 7821, &shared_secret(), remote_ip())?;

    service_sender.send(ServiceEvent::ServiceResolved(Box::new(service)))?;

    let observed_after = Instant::now();
    let event = receive_discovery_event(
        &service_receiver,
        &shutdown_receiver,
        local_peer()?,
        &shared_secret(),
    );

    let DiscoveryEvent::Peer(peer) = event else {
        return Err(format!("expected discovered peer, got {event:?}").into());
    };

    assert_eq!(peer.peer_id, remote_peer()?);
    assert_eq!(peer.address, SocketAddr::from((remote_ip(), 7821)));
    assert!(peer.observed_at >= observed_after);

    Ok(())
}

#[test]
fn discovery_txt_metadata_round_trips() -> Result<(), Box<dyn Error>> {
    let service = resolved_service(remote_peer()?, 7821, &shared_secret(), remote_ip())?;
    let metadata = DiscoveryMetadata::from_resolved_service(&service, &shared_secret())?;

    assert_eq!(metadata.peer_id, remote_peer()?);
    assert_eq!(metadata.transport_port, 7821);

    Ok(())
}

#[test]
fn discovery_txt_metadata_rejects_wrong_shared_secret() -> Result<(), Box<dyn Error>> {
    let service = resolved_service(remote_peer()?, 7821, &shared_secret(), remote_ip())?;

    assert_eq!(
        DiscoveryMetadata::from_resolved_service(&service, &wrong_shared_secret()),
        Err(SyncDiscoveryError::AuthenticationFailed)
    );

    Ok(())
}

#[test]
fn discovery_txt_metadata_rejects_unsupported_version() -> Result<(), Box<dyn Error>> {
    let payload = DiscoveryPayload {
        version: DISCOVERY_VERSION + 1,
        peer: remote_peer()?,
        port: 7821,
    };
    let service = resolved_service_with_properties(
        7821,
        remote_ip(),
        &signed_properties(payload, &shared_secret())?,
    )?;

    assert_eq!(
        DiscoveryMetadata::from_resolved_service(&service, &shared_secret()),
        Err(SyncDiscoveryError::UnsupportedVersion {
            version: DISCOVERY_VERSION + 1,
        })
    );

    Ok(())
}

#[test]
fn discovery_txt_metadata_rejects_invalid_port() -> Result<(), Box<dyn Error>> {
    let payload = DiscoveryPayload {
        version: DISCOVERY_VERSION,
        peer: remote_peer()?,
        port: 0,
    };
    let service = resolved_service_with_properties(
        7821,
        remote_ip(),
        &signed_properties(payload, &shared_secret())?,
    )?;

    assert_eq!(
        DiscoveryMetadata::from_resolved_service(&service, &shared_secret()),
        Err(SyncDiscoveryError::ZeroPort)
    );

    Ok(())
}

#[test]
fn discovery_txt_metadata_rejects_bad_mac_hex() -> Result<(), Box<dyn Error>> {
    let properties = vec![
        (String::from(KEY_VERSION), DISCOVERY_VERSION.to_string()),
        (String::from(KEY_PEER), REMOTE_PEER.to_owned()),
        (String::from(KEY_PORT), String::from("7821")),
        (String::from(KEY_MAC), String::from("not-hex")),
    ];
    let service = resolved_service_with_properties(7821, remote_ip(), &properties)?;

    assert_eq!(
        DiscoveryMetadata::from_resolved_service(&service, &shared_secret()),
        Err(SyncDiscoveryError::InvalidMacLength {
            expected: 64,
            actual: 7,
        })
    );

    Ok(())
}

#[test]
fn resolved_self_announcement_is_ignored() -> Result<(), Box<dyn Error>> {
    let service = resolved_service(local_peer()?, 7821, &shared_secret(), remote_ip())?;

    assert_eq!(
        discovered_peer_from_resolved_service(
            &service,
            local_peer()?,
            &shared_secret(),
            Instant::now()
        )?,
        None
    );

    Ok(())
}

#[test]
fn resolved_service_converts_to_discovered_peer() -> Result<(), Box<dyn Error>> {
    let observed_at = Instant::now();
    let service = resolved_service(remote_peer()?, 7821, &shared_secret(), remote_ip())?;

    assert_eq!(
        discovered_peer_from_resolved_service(
            &service,
            local_peer()?,
            &shared_secret(),
            observed_at,
        )?,
        Some(DiscoveredPeer {
            peer_id: remote_peer()?,
            address: SocketAddr::from((remote_ip(), 7821)),
            observed_at,
        })
    );

    Ok(())
}

#[test]
fn resolved_service_rejects_port_mismatch() -> Result<(), Box<dyn Error>> {
    let properties = discovery_txt_properties(remote_peer()?, 7821, &shared_secret())?;
    let service = resolved_service_with_properties(99, remote_ip(), &properties)?;

    assert_eq!(
        discovered_peer_from_resolved_service(
            &service,
            local_peer()?,
            &shared_secret(),
            Instant::now()
        ),
        Err(SyncDiscoveryError::PortMismatch {
            txt_port: 7821,
            srv_port: 99,
        })
    );

    Ok(())
}

#[test]
fn resolved_service_requires_address() -> Result<(), Box<dyn Error>> {
    let properties = discovery_txt_properties(remote_peer()?, 7821, &shared_secret())?;
    let service = resolved_service_with_properties(7821, (), &properties)?;

    assert_eq!(service_address(&service, 7821), None);
    assert_eq!(
        discovered_peer_from_resolved_service(
            &service,
            local_peer()?,
            &shared_secret(),
            Instant::now()
        ),
        Err(SyncDiscoveryError::NoAddress)
    );

    Ok(())
}

fn resolved_service(
    peer_id: PeerId,
    port: u16,
    shared_secret: &SharedSecret,
    ip: impl mdns_sd::AsIpAddrs,
) -> Result<ResolvedService, Box<dyn Error>> {
    let properties = discovery_txt_properties(peer_id, port, shared_secret)?;
    resolved_service_with_properties(port, ip, &properties)
}

fn resolved_service_with_properties(
    port: u16,
    ip: impl mdns_sd::AsIpAddrs,
    properties: &[(String, String)],
) -> Result<ResolvedService, Box<dyn Error>> {
    let service = ServiceInfo::new(
        SERVICE_TYPE,
        &instance_name(remote_peer()?),
        &host_name(remote_peer()?),
        ip,
        port,
        properties,
    )?;

    Ok(service.as_resolved_service())
}

fn signed_properties(
    payload: DiscoveryPayload,
    shared_secret: &SharedSecret,
) -> Result<Vec<(String, String)>, SyncDiscoveryError> {
    let mac = encode_hex(&authenticate_payload(&payload, shared_secret)?);

    Ok(vec![
        (String::from(KEY_VERSION), payload.version.to_string()),
        (String::from(KEY_PEER), payload.peer.to_string()),
        (String::from(KEY_PORT), payload.port.to_string()),
        (String::from(KEY_MAC), mac),
    ])
}

fn local_peer() -> Result<PeerId, Box<dyn Error>> {
    Ok(PeerId::from_str(LOCAL_PEER)?)
}

fn remote_peer() -> Result<PeerId, Box<dyn Error>> {
    Ok(PeerId::from_str(REMOTE_PEER)?)
}

fn shared_secret() -> SharedSecret {
    SharedSecret::new(String::from(SHARED_SECRET))
}

fn wrong_shared_secret() -> SharedSecret {
    SharedSecret::new(String::from(WRONG_SHARED_SECRET))
}

const fn remote_ip() -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10))
}
