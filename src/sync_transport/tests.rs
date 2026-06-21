use super::{
    BindPeerStatus, ConnectionDirection, ConnectionTracker, SyncTransport, TransportNotification,
    peer_hello_payload,
};
use crate::config::SharedSecret;
use crate::sync_protocol::{PeerId, SyncEvent, SyncProtocolError, decode_authenticated};
use std::error::Error;
use std::str;
use std::str::FromStr;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const LOCAL_PEER: &str = "00112233445566778899aabbccddeeff";
const REMOTE_PEER: &str = "ffeeddccbbaa99887766554433221100";
const SHARED_SECRET: &str = "0123456789abcdef0123456789abcdef";
const WRONG_SHARED_SECRET: &str = "fedcba9876543210fedcba9876543210";

#[test]
fn peer_hello_uses_authenticated_sequence_zero_frame() -> Result<(), Box<dyn Error>> {
    let payload = peer_hello_payload(local_peer()?, &shared_secret())?;
    let input = str::from_utf8(&payload)?;
    let message = decode_authenticated(input, &shared_secret())?;

    assert_eq!(message.sender, local_peer()?);
    assert_eq!(message.sequence, 0);
    assert_eq!(message.event, SyncEvent::PeerHello);

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
    let mut tracker = ConnectionTracker::default();
    tracker.record_endpoint(1, ConnectionDirection::Incoming);

    let result = tracker.bind_peer(local_peer()?, 1, remote_peer()?);

    assert_eq!(
        result.status,
        BindPeerStatus::Accepted {
            peer_connected: true,
        }
    );
    assert!(result.remove_endpoints.is_empty());
    assert_eq!(tracker.peer_for_endpoint(1), Some(remote_peer()?));

    Ok(())
}

#[test]
fn tracker_rejects_self_peer_id() -> Result<(), Box<dyn Error>> {
    let mut tracker = ConnectionTracker::default();
    tracker.record_endpoint(1, ConnectionDirection::Incoming);

    let result = tracker.bind_peer(local_peer()?, 1, local_peer()?);

    assert_eq!(result.status, BindPeerStatus::RejectedSelf);
    assert_eq!(result.remove_endpoints, vec![1]);
    assert!(tracker.endpoints().is_empty());

    Ok(())
}

#[test]
fn tracker_rejects_unknown_endpoint() -> Result<(), Box<dyn Error>> {
    let mut tracker = ConnectionTracker::default();

    let result = tracker.bind_peer(local_peer()?, 1, remote_peer()?);

    assert_eq!(result.status, BindPeerStatus::RejectedUnknownEndpoint);
    assert_eq!(result.remove_endpoints, vec![1]);

    Ok(())
}

#[test]
fn lower_peer_keeps_outgoing_duplicate_connection() -> Result<(), Box<dyn Error>> {
    let mut tracker = ConnectionTracker::default();
    tracker.record_endpoint(1, ConnectionDirection::Incoming);
    tracker.bind_peer(local_peer()?, 1, remote_peer()?);
    tracker.record_endpoint(2, ConnectionDirection::Outgoing);

    let result = tracker.bind_peer(local_peer()?, 2, remote_peer()?);

    assert_eq!(
        result.status,
        BindPeerStatus::Accepted {
            peer_connected: false,
        }
    );
    assert_eq!(result.remove_endpoints, vec![1]);
    assert_eq!(tracker.peer_for_endpoint(1), None);
    assert_eq!(tracker.peer_for_endpoint(2), Some(remote_peer()?));

    Ok(())
}

#[test]
fn higher_peer_keeps_incoming_duplicate_connection() -> Result<(), Box<dyn Error>> {
    let mut tracker = ConnectionTracker::default();
    tracker.record_endpoint(1, ConnectionDirection::Incoming);
    tracker.bind_peer(remote_peer()?, 1, local_peer()?);
    tracker.record_endpoint(2, ConnectionDirection::Outgoing);

    let result = tracker.bind_peer(remote_peer()?, 2, local_peer()?);

    assert_eq!(
        result.status,
        BindPeerStatus::Accepted {
            peer_connected: false,
        }
    );
    assert_eq!(result.remove_endpoints, vec![2]);
    assert_eq!(tracker.peer_for_endpoint(1), Some(local_peer()?));
    assert_eq!(tracker.peer_for_endpoint(2), None);

    Ok(())
}

#[test]
fn disconnect_removes_endpoint_peer_binding() -> Result<(), Box<dyn Error>> {
    let mut tracker = ConnectionTracker::default();
    tracker.record_endpoint(1, ConnectionDirection::Incoming);
    tracker.bind_peer(local_peer()?, 1, remote_peer()?);

    assert_eq!(tracker.remove_endpoint(1), Some(remote_peer()?));
    assert!(tracker.endpoints().is_empty());

    Ok(())
}

#[test]
fn loopback_transports_authenticate_after_hello_exchange() -> Result<(), Box<dyn Error>> {
    let (left, left_events) = SyncTransport::start_for_test(local_peer()?, shared_secret())?;
    let (right, right_events) = SyncTransport::start_for_test(remote_peer()?, shared_secret())?;

    left.connect_for_test(right.local_addr_for_test())?;

    expect_peer_authenticated(&left_events, remote_peer()?)?;
    expect_peer_authenticated(&right_events, local_peer()?)?;

    Ok(())
}

fn expect_peer_authenticated(
    receiver: &mpsc::Receiver<TransportNotification>,
    peer_id: PeerId,
) -> Result<(), Box<dyn Error>> {
    let deadline = Instant::now() + Duration::from_secs(2);

    loop {
        let now = Instant::now();
        let remaining = deadline.saturating_duration_since(now);
        if remaining.is_zero() {
            return Err(format!("timed out waiting for authenticated peer {peer_id}").into());
        }

        match receiver.recv_timeout(remaining)? {
            TransportNotification::PeerAuthenticated(actual) if actual == peer_id => {
                return Ok(());
            }
            _ => {}
        }
    }
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
