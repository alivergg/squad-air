use dashmap::DashMap;
use squad_proto::{
    ClientUuid, PeerId, PeerRecord, RoomId, SignalingErrorCode, SignalingPacket, MAX_ROOM_PEERS,
};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, SocketAddrV4};
use std::time::{Duration, Instant};

const MAX_PACKETS_PER_SEC_PER_IP: f64 = 30.0;
const BUCKET_BURST: f64 = 45.0;

#[derive(Debug, Clone)]
pub struct PeerEntry {
    pub peer_id: PeerId,
    pub client_uuid: ClientUuid,
    pub public_addr: SocketAddr,
    pub lan_addrs: Vec<SocketAddrV4>,
    pub nickname: String,
    pub last_seen: Instant,
}

#[derive(Debug, Default)]
pub struct RoomState {
    pub peers: HashMap<ClientUuid, PeerEntry>,
}

impl RoomState {
    fn next_available_peer_id(&self) -> Option<PeerId> {
        (1..=(MAX_ROOM_PEERS as PeerId))
            .find(|&candidate| !self.peers.values().any(|p| p.peer_id == candidate))
    }

    pub fn build_broadcast_packets(&self) -> Vec<(SocketAddr, SignalingPacket)> {
        let records: Vec<PeerRecord> = self
            .peers
            .values()
            .map(|p| PeerRecord {
                peer_id: p.peer_id,
                client_uuid: p.client_uuid,
                public_addr: p.public_addr,
                lan_addrs: p.lan_addrs.clone(),
                nickname: p.nickname.clone(),
            })
            .collect();

        self.peers
            .values()
            .map(|p| {
                (
                    p.public_addr,
                    SignalingPacket::S2cRoomState {
                        assigned_peer_id: p.peer_id,
                        peers: records.clone(),
                    },
                )
            })
            .collect()
    }
}

#[derive(Debug)]
struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
}

pub struct RendezvousState {
    rooms: DashMap<RoomId, RoomState>,
    rate_limits: DashMap<IpAddr, TokenBucket>,
}

impl Default for RendezvousState {
    fn default() -> Self {
        Self::new()
    }
}

impl RendezvousState {
    pub fn new() -> Self {
        Self {
            rooms: DashMap::new(),
            rate_limits: DashMap::new(),
        }
    }

    pub fn check_rate_limit(&self, ip: IpAddr, now: Instant) -> bool {
        let mut entry = self.rate_limits.entry(ip).or_insert(TokenBucket {
            tokens: BUCKET_BURST,
            last_refill: now,
        });
        let elapsed = now.duration_since(entry.last_refill).as_secs_f64();
        entry.tokens = (entry.tokens + elapsed * MAX_PACKETS_PER_SEC_PER_IP).min(BUCKET_BURST);
        entry.last_refill = now;

        if entry.tokens >= 1.0 {
            entry.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Handles `C2sRegister`. Returns outbound `(target_addr, SignalingPacket)` responses.
    pub fn handle_register(
        &self,
        src_addr: SocketAddr,
        room_id: RoomId,
        client_uuid: ClientUuid,
        nickname: String,
        lan_addrs: Vec<SocketAddrV4>,
        now: Instant,
    ) -> Vec<(SocketAddr, SignalingPacket)> {
        let mut room = self.rooms.entry(room_id).or_default();

        if let Some(existing) = room.peers.get_mut(&client_uuid) {
            let changed = existing.public_addr != src_addr
                || existing.nickname != nickname
                || existing.lan_addrs != lan_addrs;
            existing.public_addr = src_addr;
            existing.nickname = nickname;
            existing.lan_addrs = lan_addrs;
            existing.last_seen = now;

            let assigned_peer_id = existing.peer_id;
            if changed {
                return room.build_broadcast_packets();
            }
            // Reply only to the heartbeat sender if nothing changed
            let records: Vec<PeerRecord> = room
                .peers
                .values()
                .map(|p| PeerRecord {
                    peer_id: p.peer_id,
                    client_uuid: p.client_uuid,
                    public_addr: p.public_addr,
                    lan_addrs: p.lan_addrs.clone(),
                    nickname: p.nickname.clone(),
                })
                .collect();
            return vec![(
                src_addr,
                SignalingPacket::S2cRoomState {
                    assigned_peer_id,
                    peers: records,
                },
            )];
        }

        if room.peers.len() >= MAX_ROOM_PEERS {
            return vec![(
                src_addr,
                SignalingPacket::S2cError {
                    code: SignalingErrorCode::RoomFull,
                },
            )];
        }

        let Some(peer_id) = room.next_available_peer_id() else {
            return vec![(
                src_addr,
                SignalingPacket::S2cError {
                    code: SignalingErrorCode::RoomFull,
                },
            )];
        };

        room.peers.insert(
            client_uuid,
            PeerEntry {
                peer_id,
                client_uuid,
                public_addr: src_addr,
                lan_addrs,
                nickname,
                last_seen: now,
            },
        );

        room.build_broadcast_packets()
    }

    /// Handles `C2sLeave` and returns updated `S2cRoomState` broadcasts for remaining peers.
    pub fn handle_leave(
        &self,
        room_id: RoomId,
        client_uuid: ClientUuid,
    ) -> Vec<(SocketAddr, SignalingPacket)> {
        let mut notifications = Vec::new();
        let mut should_remove_room = false;

        if let Some(mut room) = self.rooms.get_mut(&room_id) {
            if room.peers.remove(&client_uuid).is_some() {
                if room.peers.is_empty() {
                    should_remove_room = true;
                } else {
                    notifications = room.build_broadcast_packets();
                }
            }
        }

        if should_remove_room {
            self.rooms.remove(&room_id);
        }
        notifications
    }

    /// Evicts expired peers (`now - last_seen > ttl`) and returns broadcast updates for affected rooms.
    pub fn evict_expired(
        &self,
        now: Instant,
        ttl: Duration,
    ) -> Vec<(SocketAddr, SignalingPacket)> {
        let mut updates = Vec::new();
        let mut empty_rooms = Vec::new();

        for mut entry in self.rooms.iter_mut() {
            let room_id = *entry.key();
            let room = entry.value_mut();
            let before = room.peers.len();
            room.peers
                .retain(|_, peer| now.duration_since(peer.last_seen) <= ttl);

            if room.peers.is_empty() {
                empty_rooms.push(room_id);
            } else if room.peers.len() != before {
                updates.extend(room.build_broadcast_packets());
            }
        }

        for room_id in empty_rooms {
            self.rooms.remove(&room_id);
        }

        // Prune stale IP rate-limit buckets older than 60s
        self.rate_limits
            .retain(|_, b| now.duration_since(b.last_refill) <= Duration::from_secs(60));

        updates
    }
}