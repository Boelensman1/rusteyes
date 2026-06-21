use super::connections::{
    ConnectionDirection, ConnectionTracker, EndpointRemoval, InboundEventAcceptance,
    PeerAuthentication, PeerAuthenticationResult,
};
use super::session::peer_hello_payload;
use super::{SyncTransport, SyncTransportEvent};
use crate::config::{SharedSecret, SyncConfig};
use crate::sync_protocol::{
    PeerId, SyncEvent, SyncFramePayload, SyncMessage, SyncProtocolError, TransportControlFrame,
    decode_authenticated, encode_authenticated,
};
use crate::sync_transport_io::{TransportIo, TransportIoEvent, TransportSendStatus};
use std::error::Error;
use std::str;
use std::str::FromStr;
use std::time::{Duration, Instant};

const LOCAL_PEER: &str = "00112233445566778899aabbccddeeff";
const REMOTE_PEER: &str = "ffeeddccbbaa99887766554433221100";
const THIRD_PEER: &str = "11112222333344445555666677778888";
const SHARED_SECRET: &str = "0123456789abcdef0123456789abcdef";
const WRONG_SHARED_SECRET: &str = "fedcba9876543210fedcba9876543210";

#[test]
fn peer_hello_uses_authenticated_sequence_zero_frame() -> Result<(), Box<dyn Error>> {
    let payload = peer_hello_payload(local_peer()?, &shared_secret())?;
    let input = str::from_utf8(&payload)?;
    let message = decode_authenticated(input, &shared_secret())?;

    assert_eq!(message.sender, local_peer()?);
    assert_eq!(message.sequence, 0);
    assert_eq!(
        message.payload,
        SyncFramePayload::Control {
            control: TransportControlFrame::PeerHello,
        }
    );

    Ok(())
}

#[test]
fn peer_hello_rejects_wrong_shared_secret() -> Result<(), Box<dyn Error>> {
    let payload = peer_hello_payload(local_peer()?, &shared_secret())?;
    let input = str::from_utf8(&payload)?;

    assert_eq!(
        decode_authenticated(input, &wrong_shared_secret()),
        Err(SyncProtocolError::AuthenticationFailed)
    );

    Ok(())
}

#[test]
fn tracker_binds_endpoint_to_authenticated_peer() -> Result<(), Box<dyn Error>> {
    let mut tracker = ConnectionTracker::new(local_peer()?);
    tracker.record_endpoint(1, ConnectionDirection::Incoming);

    let outcome = tracker.authenticate_peer(1, remote_peer()?);

    assert_eq!(
        outcome,
        PeerAuthentication {
            result: PeerAuthenticationResult::AuthenticatedNewPeer {
                peer_id: remote_peer()?,
            },
            endpoints_to_close: vec![],
        }
    );
    assert_eq!(tracker.peer_for_endpoint(1), Some(remote_peer()?));

    Ok(())
}

#[test]
fn tracker_rejects_self_peer_id() -> Result<(), Box<dyn Error>> {
    let mut tracker = ConnectionTracker::new(local_peer()?);
    tracker.record_endpoint(1, ConnectionDirection::Incoming);

    let outcome = tracker.authenticate_peer(1, local_peer()?);

    assert_eq!(
        outcome,
        PeerAuthentication {
            result: PeerAuthenticationResult::RejectedSelf {
                peer_id: local_peer()?,
            },
            endpoints_to_close: vec![1],
        }
    );
    assert!(tracker.endpoints().is_empty());

    Ok(())
}

#[test]
fn tracker_rejects_unknown_endpoint() -> Result<(), Box<dyn Error>> {
    let mut tracker = ConnectionTracker::new(local_peer()?);

    let outcome = tracker.authenticate_peer(1, remote_peer()?);

    assert_eq!(
        outcome,
        PeerAuthentication {
            result: PeerAuthenticationResult::RejectedUnknownEndpoint {
                peer_id: remote_peer()?,
            },
            endpoints_to_close: vec![1],
        }
    );

    Ok(())
}

#[test]
fn lower_peer_keeps_outgoing_duplicate_connection() -> Result<(), Box<dyn Error>> {
    let mut tracker = ConnectionTracker::new(local_peer()?);
    tracker.record_endpoint(1, ConnectionDirection::Incoming);
    tracker.authenticate_peer(1, remote_peer()?);
    tracker.record_endpoint(2, ConnectionDirection::Outgoing);

    let outcome = tracker.authenticate_peer(2, remote_peer()?);

    assert_eq!(
        outcome,
        PeerAuthentication {
            result: PeerAuthenticationResult::AuthenticatedExistingPeer {
                peer_id: remote_peer()?,
            },
            endpoints_to_close: vec![1],
        }
    );
    assert_eq!(tracker.peer_for_endpoint(1), None);
    assert_eq!(tracker.peer_for_endpoint(2), Some(remote_peer()?));

    Ok(())
}

#[test]
fn higher_peer_keeps_incoming_duplicate_connection() -> Result<(), Box<dyn Error>> {
    let mut tracker = ConnectionTracker::new(remote_peer()?);
    tracker.record_endpoint(1, ConnectionDirection::Incoming);
    tracker.authenticate_peer(1, local_peer()?);
    tracker.record_endpoint(2, ConnectionDirection::Outgoing);

    let outcome = tracker.authenticate_peer(2, local_peer()?);

    assert_eq!(
        outcome,
        PeerAuthentication {
            result: PeerAuthenticationResult::AuthenticatedExistingPeer {
                peer_id: local_peer()?,
            },
            endpoints_to_close: vec![2],
        }
    );
    assert_eq!(tracker.peer_for_endpoint(1), Some(local_peer()?));
    assert_eq!(tracker.peer_for_endpoint(2), None);

    Ok(())
}

#[test]
fn disconnect_removes_endpoint_peer_binding() -> Result<(), Box<dyn Error>> {
    let mut tracker = ConnectionTracker::new(local_peer()?);
    tracker.record_endpoint(1, ConnectionDirection::Incoming);
    tracker.authenticate_peer(1, remote_peer()?);

    assert_eq!(
        tracker.remove_endpoint(1),
        EndpointRemoval::PeerDisconnected {
            peer_id: remote_peer()?,
        }
    );
    assert!(tracker.endpoints().is_empty());

    Ok(())
}

#[test]
fn disconnect_reports_unauthenticated_endpoint() -> Result<(), Box<dyn Error>> {
    let mut tracker = ConnectionTracker::new(local_peer()?);
    tracker.record_endpoint(1, ConnectionDirection::Incoming);

    assert_eq!(
        tracker.remove_endpoint(1),
        EndpointRemoval::UnauthenticatedEndpoint
    );
    assert!(tracker.endpoints().is_empty());

    Ok(())
}

#[test]
fn tracker_rejects_stale_event_sequences_per_peer() -> Result<(), Box<dyn Error>> {
    let remote = remote_peer()?;
    let third = third_peer()?;
    let mut tracker: ConnectionTracker<u8> = ConnectionTracker::new(local_peer()?);
    tracker.record_endpoint(1, ConnectionDirection::Incoming);
    tracker.authenticate_peer(1, remote);
    tracker.record_endpoint(2, ConnectionDirection::Incoming);
    tracker.authenticate_peer(2, third);

    assert_eq!(
        tracker.accept_inbound_event(1, remote, 1),
        InboundEventAcceptance::Accepted
    );
    assert_eq!(
        tracker.accept_inbound_event(1, remote, 1),
        InboundEventAcceptance::Replayed { highest_seen: 1 }
    );
    assert_eq!(
        tracker.accept_inbound_event(1, remote, 0),
        InboundEventAcceptance::Replayed { highest_seen: 1 }
    );
    assert_eq!(
        tracker.accept_inbound_event(1, remote, 2),
        InboundEventAcceptance::Accepted
    );
    assert_eq!(
        tracker.accept_inbound_event(2, third, 1),
        InboundEventAcceptance::Accepted
    );

    Ok(())
}

#[test]
fn tracker_rejects_inbound_event_sender_mismatch() -> Result<(), Box<dyn Error>> {
    let remote = remote_peer()?;
    let third = third_peer()?;
    let mut tracker = ConnectionTracker::new(local_peer()?);
    tracker.record_endpoint(1, ConnectionDirection::Incoming);
    tracker.authenticate_peer(1, remote);

    assert_eq!(
        tracker.accept_inbound_event(1, third, 1),
        InboundEventAcceptance::SenderMismatch {
            authenticated_peer_id: remote,
        }
    );

    Ok(())
}

#[test]
fn tracker_preserves_sequence_state_after_reconnect() -> Result<(), Box<dyn Error>> {
    let local = local_peer()?;
    let remote = remote_peer()?;
    let mut tracker = ConnectionTracker::new(local);

    tracker.record_endpoint(1, ConnectionDirection::Incoming);
    tracker.authenticate_peer(1, remote);
    assert_eq!(
        tracker.accept_inbound_event(1, remote, 7),
        InboundEventAcceptance::Accepted
    );
    assert_eq!(
        tracker.remove_endpoint(1),
        EndpointRemoval::PeerDisconnected { peer_id: remote }
    );

    tracker.record_endpoint(2, ConnectionDirection::Incoming);
    tracker.authenticate_peer(2, remote);
    assert_eq!(
        tracker.accept_inbound_event(2, remote, 7),
        InboundEventAcceptance::Replayed { highest_seen: 7 }
    );
    assert_eq!(
        tracker.accept_inbound_event(2, remote, 8),
        InboundEventAcceptance::Accepted
    );

    Ok(())
}

#[test]
fn disabled_transport_is_inert() -> Result<(), Box<dyn Error>> {
    let transport = SyncTransport::start(SyncConfig::default())?;

    assert_eq!(
        transport.broadcast_event(SyncEvent::ActiveTimeElapsed {
            elapsed: Duration::from_millis(1),
        })?,
        0
    );
    assert!(!transport.send_event(remote_peer()?, SyncEvent::Enable)?);
    assert_eq!(transport.try_recv_event()?, None);
    assert!(transport.drain_events()?.is_empty());

    Ok(())
}

#[test]
fn loopback_transports_authenticate_after_hello_exchange() -> Result<(), Box<dyn Error>> {
    let left = SyncTransport::start_for_test(local_peer()?, &shared_secret())?;
    let right = SyncTransport::start_for_test(remote_peer()?, &shared_secret())?;

    left.connect_for_test(right.local_addr_for_test())?;

    expect_peer_authenticated(&left, remote_peer()?)?;
    expect_peer_authenticated(&right, local_peer()?)?;

    Ok(())
}

#[test]
fn broadcast_sends_domain_event_to_authenticated_peer() -> Result<(), Box<dyn Error>> {
    let left = SyncTransport::start_for_test(local_peer()?, &shared_secret())?;
    let right = SyncTransport::start_for_test(remote_peer()?, &shared_secret())?;

    left.connect_for_test(right.local_addr_for_test())?;
    expect_peer_authenticated(&left, remote_peer()?)?;
    expect_peer_authenticated(&right, local_peer()?)?;

    assert_eq!(
        left.broadcast_event(SyncEvent::ActiveTimeElapsed {
            elapsed: Duration::from_millis(1_500),
        })?,
        1
    );

    assert_eq!(
        expect_domain_event(&right)?,
        SyncTransportEvent::Domain {
            peer_id: local_peer()?,
            event: SyncEvent::ActiveTimeElapsed {
                elapsed: Duration::from_millis(1_500),
            },
        }
    );

    Ok(())
}

#[test]
fn directed_send_delivers_only_to_requested_authenticated_peer() -> Result<(), Box<dyn Error>> {
    let left = SyncTransport::start_for_test(local_peer()?, &shared_secret())?;
    let right = SyncTransport::start_for_test(remote_peer()?, &shared_secret())?;
    let third = SyncTransport::start_for_test(third_peer()?, &shared_secret())?;

    left.connect_for_test(right.local_addr_for_test())?;
    left.connect_for_test(third.local_addr_for_test())?;
    expect_peers_authenticated(&left, &[remote_peer()?, third_peer()?])?;
    expect_peer_authenticated(&right, local_peer()?)?;
    expect_peer_authenticated(&third, local_peer()?)?;

    assert!(left.send_event(remote_peer()?, SyncEvent::Enable)?);

    assert_eq!(
        expect_domain_event(&right)?,
        SyncTransportEvent::Domain {
            peer_id: local_peer()?,
            event: SyncEvent::Enable,
        }
    );
    assert!(
        third
            .recv_event_timeout_for_test(Duration::from_millis(100))?
            .is_none()
    );

    Ok(())
}

#[test]
fn directed_send_returns_false_for_unknown_peer() -> Result<(), Box<dyn Error>> {
    let left = SyncTransport::start_for_test(local_peer()?, &shared_secret())?;

    assert!(!left.send_event(remote_peer()?, SyncEvent::Enable)?);

    Ok(())
}

#[test]
fn domain_event_before_authenticated_hello_is_rejected() -> Result<(), Box<dyn Error>> {
    let server = SyncTransport::start_for_test(local_peer()?, &shared_secret())?;
    let mut client_binding = TransportIo::listen("127.0.0.1:0")?;
    let client_handle = client_binding.io.handle();
    client_handle.connect(server.local_addr_for_test())?;

    let endpoint = expect_client_connected(&mut client_binding.event_receiver)?;
    let payload = encode_authenticated(
        &SyncMessage::event(remote_peer()?, 1, SyncEvent::Enable),
        &shared_secret(),
    )?;

    assert_eq!(
        client_handle.send(endpoint, payload.as_bytes()),
        TransportSendStatus::Sent
    );
    expect_client_disconnected(&mut client_binding.event_receiver)?;
    assert!(
        server
            .recv_event_timeout_for_test(Duration::from_millis(100))?
            .is_none()
    );

    client_binding.io.shutdown();

    Ok(())
}

#[test]
fn authenticated_endpoint_rejects_spoofed_sender() -> Result<(), Box<dyn Error>> {
    let left = SyncTransport::start_for_test(local_peer()?, &shared_secret())?;
    let right = SyncTransport::start_for_test(remote_peer()?, &shared_secret())?;

    let right_endpoint = right.connect_for_test(left.local_addr_for_test())?;
    expect_peer_authenticated(&left, remote_peer()?)?;
    expect_peer_authenticated(&right, local_peer()?)?;

    let payload = encode_authenticated(
        &SyncMessage::event(local_peer()?, 1, SyncEvent::Enable),
        &shared_secret(),
    )?;

    assert_eq!(
        right.send_raw_for_test(right_endpoint, payload.as_bytes()),
        TransportSendStatus::Sent
    );
    expect_peer_disconnected(&left, remote_peer()?)?;
    assert!(
        left.recv_event_timeout_for_test(Duration::from_millis(100))?
            .is_none()
    );

    Ok(())
}

#[test]
fn authenticated_endpoint_rejects_replayed_sequence() -> Result<(), Box<dyn Error>> {
    let left = SyncTransport::start_for_test(local_peer()?, &shared_secret())?;
    let right = SyncTransport::start_for_test(remote_peer()?, &shared_secret())?;

    let right_endpoint = right.connect_for_test(left.local_addr_for_test())?;
    expect_peer_authenticated(&left, remote_peer()?)?;
    expect_peer_authenticated(&right, local_peer()?)?;

    let replayed_payload = encode_authenticated(
        &SyncMessage::event(remote_peer()?, 1, SyncEvent::Enable),
        &shared_secret(),
    )?;

    assert_eq!(
        right.send_raw_for_test(right_endpoint, replayed_payload.as_bytes()),
        TransportSendStatus::Sent
    );
    assert_eq!(
        expect_domain_event(&left)?,
        SyncTransportEvent::Domain {
            peer_id: remote_peer()?,
            event: SyncEvent::Enable,
        }
    );

    assert_eq!(
        right.send_raw_for_test(right_endpoint, replayed_payload.as_bytes()),
        TransportSendStatus::Sent
    );
    assert!(
        left.recv_event_timeout_for_test(Duration::from_millis(100))?
            .is_none()
    );

    let fresh_payload = encode_authenticated(
        &SyncMessage::event(remote_peer()?, 2, SyncEvent::DisableUntilRestart),
        &shared_secret(),
    )?;

    assert_eq!(
        right.send_raw_for_test(right_endpoint, fresh_payload.as_bytes()),
        TransportSendStatus::Sent
    );
    assert_eq!(
        expect_domain_event(&left)?,
        SyncTransportEvent::Domain {
            peer_id: remote_peer()?,
            event: SyncEvent::DisableUntilRestart,
        }
    );

    Ok(())
}

fn expect_peer_authenticated(
    transport: &SyncTransport,
    peer_id: PeerId,
) -> Result<(), Box<dyn Error>> {
    let deadline = Instant::now() + Duration::from_secs(2);

    loop {
        let now = Instant::now();
        let remaining = deadline.saturating_duration_since(now);
        if remaining.is_zero() {
            return Err(format!("timed out waiting for authenticated peer {peer_id}").into());
        }

        match transport.recv_event_timeout_for_test(remaining)? {
            Some(SyncTransportEvent::PeerAuthenticated(actual)) if actual == peer_id => {
                return Ok(());
            }
            _ => {}
        }
    }
}

fn expect_peers_authenticated(
    transport: &SyncTransport,
    peer_ids: &[PeerId],
) -> Result<(), Box<dyn Error>> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut remaining_peers = peer_ids.to_vec();

    while !remaining_peers.is_empty() {
        let now = Instant::now();
        let remaining = deadline.saturating_duration_since(now);
        if remaining.is_zero() {
            return Err(
                format!("timed out waiting for authenticated peers {remaining_peers:?}").into(),
            );
        }

        if let Some(SyncTransportEvent::PeerAuthenticated(actual)) =
            transport.recv_event_timeout_for_test(remaining)?
        {
            remaining_peers.retain(|peer_id| *peer_id != actual);
        }
    }

    Ok(())
}

fn expect_peer_disconnected(
    transport: &SyncTransport,
    peer_id: PeerId,
) -> Result<(), Box<dyn Error>> {
    let deadline = Instant::now() + Duration::from_secs(2);

    loop {
        let now = Instant::now();
        let remaining = deadline.saturating_duration_since(now);
        if remaining.is_zero() {
            return Err(format!("timed out waiting for disconnected peer {peer_id}").into());
        }

        match transport.recv_event_timeout_for_test(remaining)? {
            Some(SyncTransportEvent::PeerDisconnected(actual)) if actual == peer_id => {
                return Ok(());
            }
            _ => {}
        }
    }
}

fn expect_domain_event(transport: &SyncTransport) -> Result<SyncTransportEvent, Box<dyn Error>> {
    let event = transport
        .recv_event_timeout_for_test(Duration::from_secs(2))?
        .ok_or_else(|| "timed out waiting for inbound sync event".to_owned())?;

    match event {
        SyncTransportEvent::Domain { .. } => Ok(event),
        other => Err(format!("expected inbound sync domain event, got {other:?}").into()),
    }
}

fn expect_client_connected(
    receiver: &mut crate::sync_transport_io::TransportIoReceiver,
) -> Result<crate::sync_transport_io::TransportEndpoint, Box<dyn Error>> {
    let deadline = Instant::now() + Duration::from_secs(2);

    loop {
        let now = Instant::now();
        let remaining = deadline.saturating_duration_since(now);
        if remaining.is_zero() {
            return Err("timed out waiting for raw client connection".into());
        }

        match receiver.receive_timeout(remaining) {
            Some(TransportIoEvent::Connected(endpoint)) => return Ok(endpoint),
            Some(_) => {}
            None => return Err("timed out waiting for raw client connection".into()),
        }
    }
}

fn expect_client_disconnected(
    receiver: &mut crate::sync_transport_io::TransportIoReceiver,
) -> Result<(), Box<dyn Error>> {
    let deadline = Instant::now() + Duration::from_secs(2);

    loop {
        let now = Instant::now();
        let remaining = deadline.saturating_duration_since(now);
        if remaining.is_zero() {
            return Err("timed out waiting for raw client disconnect".into());
        }

        match receiver.receive_timeout(remaining) {
            Some(TransportIoEvent::Disconnected(_)) => return Ok(()),
            Some(_) => {}
            None => return Err("timed out waiting for raw client disconnect".into()),
        }
    }
}

fn local_peer() -> Result<PeerId, Box<dyn Error>> {
    Ok(PeerId::from_str(LOCAL_PEER)?)
}

fn remote_peer() -> Result<PeerId, Box<dyn Error>> {
    Ok(PeerId::from_str(REMOTE_PEER)?)
}

fn third_peer() -> Result<PeerId, Box<dyn Error>> {
    Ok(PeerId::from_str(THIRD_PEER)?)
}

fn shared_secret() -> SharedSecret {
    SharedSecret::new(String::from(SHARED_SECRET))
}

fn wrong_shared_secret() -> SharedSecret {
    SharedSecret::new(String::from(WRONG_SHARED_SECRET))
}
