use crate::sync_protocol::PeerId;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConnectionDirection {
    Incoming,
    Outgoing,
}

#[derive(Debug)]
pub(super) struct ConnectionTracker<E> {
    self_id: PeerId,
    connections: Vec<Connection<E>>,
    highest_accepted_sequences: BTreeMap<PeerId, u64>,
}

impl<E> ConnectionTracker<E> {
    pub(super) fn new(self_id: PeerId) -> Self {
        Self {
            self_id,
            connections: Vec::new(),
            highest_accepted_sequences: BTreeMap::new(),
        }
    }
}

impl<E> ConnectionTracker<E>
where
    E: Copy + Eq,
{
    pub(super) fn record_endpoint(&mut self, endpoint: E, direction: ConnectionDirection) {
        if let Some(connection) = self.connection_mut(endpoint) {
            connection.direction = direction;
            return;
        }

        self.connections.push(Connection {
            endpoint,
            direction,
            peer_id: None,
        });
    }

    pub(super) fn bind_peer(&mut self, endpoint: E, peer_id: PeerId) -> BindPeerOutcome<E> {
        if peer_id == self.self_id {
            self.remove_endpoint(endpoint);
            return BindPeerOutcome::RejectedSelf {
                close_endpoints: vec![endpoint],
            };
        }

        let had_peer = self.peer_is_connected(peer_id);
        let Some(connection) = self.connection_mut(endpoint) else {
            return BindPeerOutcome::RejectedUnknownEndpoint {
                close_endpoints: vec![endpoint],
            };
        };

        connection.peer_id = Some(peer_id);
        let close_endpoints = self.collapse_duplicate_peer_connections(peer_id);
        let peer_connected = !had_peer
            && self
                .connection(endpoint)
                .is_some_and(|connection| connection.peer_id == Some(peer_id));

        BindPeerOutcome::Authenticated {
            peer_connected,
            close_endpoints,
        }
    }

    pub(super) fn remove_endpoint(&mut self, endpoint: E) -> Option<PeerId> {
        let index = self
            .connections
            .iter()
            .position(|connection| connection.endpoint == endpoint)?;
        self.connections.remove(index).peer_id
    }

    pub(super) fn peer_for_endpoint(&self, endpoint: E) -> Option<PeerId> {
        self.connection(endpoint)
            .and_then(|connection| connection.peer_id)
    }

    pub(super) fn endpoint_for_peer(&self, peer_id: PeerId) -> Option<E> {
        self.connections
            .iter()
            .find(|connection| connection.peer_id == Some(peer_id))
            .map(|connection| connection.endpoint)
    }

    pub(super) fn authenticated_endpoints(&self) -> Vec<E> {
        self.connections
            .iter()
            .filter(|connection| connection.peer_id.is_some())
            .map(|connection| connection.endpoint)
            .collect()
    }

    pub(super) fn endpoints(&self) -> Vec<E> {
        self.connections
            .iter()
            .map(|connection| connection.endpoint)
            .collect()
    }

    pub(super) fn accept_inbound_event(
        &mut self,
        endpoint: E,
        sender: PeerId,
        sequence: u64,
    ) -> InboundEventAcceptance {
        let Some(peer_id) = self.peer_for_endpoint(endpoint) else {
            return InboundEventAcceptance::UnauthenticatedEndpoint;
        };

        if peer_id != sender {
            return InboundEventAcceptance::SenderMismatch {
                authenticated_peer_id: peer_id,
            };
        }

        if let Some(&highest_seen) = self.highest_accepted_sequences.get(&peer_id)
            && sequence <= highest_seen
        {
            return InboundEventAcceptance::Replayed { highest_seen };
        }

        self.highest_accepted_sequences.insert(peer_id, sequence);
        InboundEventAcceptance::Accepted
    }

    fn peer_is_connected(&self, peer_id: PeerId) -> bool {
        self.connections
            .iter()
            .any(|connection| connection.peer_id == Some(peer_id))
    }

    fn collapse_duplicate_peer_connections(&mut self, peer_id: PeerId) -> Vec<E> {
        let Some(keep_endpoint) = self.endpoint_to_keep(peer_id) else {
            return Vec::new();
        };

        let mut remove_endpoints = Vec::new();
        self.connections.retain(|connection| {
            let should_remove =
                connection.peer_id == Some(peer_id) && connection.endpoint != keep_endpoint;

            if should_remove {
                remove_endpoints.push(connection.endpoint);
            }

            !should_remove
        });

        remove_endpoints
    }

    fn endpoint_to_keep(&self, peer_id: PeerId) -> Option<E> {
        let desired_direction = desired_connection_direction(self.self_id, peer_id);

        self.connections
            .iter()
            .find(|connection| {
                connection.peer_id == Some(peer_id) && connection.direction == desired_direction
            })
            .or_else(|| {
                self.connections
                    .iter()
                    .find(|connection| connection.peer_id == Some(peer_id))
            })
            .map(|connection| connection.endpoint)
    }

    fn connection(&self, endpoint: E) -> Option<&Connection<E>> {
        self.connections
            .iter()
            .find(|connection| connection.endpoint == endpoint)
    }

    fn connection_mut(&mut self, endpoint: E) -> Option<&mut Connection<E>> {
        self.connections
            .iter_mut()
            .find(|connection| connection.endpoint == endpoint)
    }
}

#[derive(Debug, Clone, Copy)]
struct Connection<E> {
    endpoint: E,
    direction: ConnectionDirection,
    peer_id: Option<PeerId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BindPeerOutcome<E> {
    Authenticated {
        peer_connected: bool,
        close_endpoints: Vec<E>,
    },
    RejectedSelf {
        close_endpoints: Vec<E>,
    },
    RejectedUnknownEndpoint {
        close_endpoints: Vec<E>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InboundEventAcceptance {
    Accepted,
    UnauthenticatedEndpoint,
    SenderMismatch { authenticated_peer_id: PeerId },
    Replayed { highest_seen: u64 },
}

fn desired_connection_direction(self_id: PeerId, peer_id: PeerId) -> ConnectionDirection {
    if self_id < peer_id {
        ConnectionDirection::Outgoing
    } else {
        ConnectionDirection::Incoming
    }
}
