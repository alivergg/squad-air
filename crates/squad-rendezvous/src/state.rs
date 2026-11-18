use dashmap::DashMap;
use squad_proto::{
    ClientUuid, PeerId, PeerRecord, RoomId, SignalingErrorCode, SignalingPacket, MAX_ROOM_PEERS,
};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, SocketAddrV4};
use std::time::{Duration, Instant};

const MAX_PACKETS_PER_SEC_PER_IP: f64 = 30.0;
const BUCKET_BURST: f64 = 45.0;
const RELAY_PACKETS_PER_SEC_PER_IP: f64 = 2500.0;
const RELAY_BUCKET_BURST: f64 = 500.0;
const STALE_BUCKET_TTL: Duration = Duration::from_secs(60);
const MAX_ACTIVE_ROOMS: usize = 4096;
const MAX_RATE_LIMIT_BUCKETS: usize = 65_536;

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

fn consume_token(
    buckets: &DashMap<IpAddr, TokenBucket>,
    ip: IpAddr,
    now: Instant,
    rate_per_sec: f64,
    burst: f64,
) -> bool {
    if !buckets.contains_key(&ip) && buckets.len() >= MAX_RATE_LIMIT_BUCKETS {
        return false;
    }
    let mut entry = buckets.entry(ip).or_insert(TokenBucket {
        tokens: burst,
        last_refill: now,
    });
    let elapsed = now
        .saturating_duration_since(entry.last_refill)
        .as_secs_f64();
    entry.tokens = (entry.tokens + elapsed * rate_per_sec).min(burst);
    entry.last_refill = now;

    if entry.tokens >= 1.0 {
        entry.tokens -= 1.0;
        true
    } else {
        false
    }
}

pub struct RendezvousState {
    rooms: DashMap<RoomId, RoomState>,
    rate_limits: DashMap<IpAddr, TokenBucket>,
    relay_rate_limits: DashMap<IpAddr, TokenBucket>,
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
            relay_rate_limits: DashMap::new(),
        }
    }

    pub fn check_rate_limit(&self, ip: IpAddr, now: Instant) -> bool {
        consume_token(
            &self.rate_limits,
            ip,
            now,
            MAX_PACKETS_PER_SEC_PER_IP,
            BUCKET_BURST,
        )
    }

    pub fn check_relay_rate_limit(&self, ip: IpAddr, now: Instant) -> bool {
        consume_token(
            &self.relay_rate_limits,
            ip,
            now,
            RELAY_PACKETS_PER_SEC_PER_IP,
            RELAY_BUCKET_BURST,
        )
    }

    /// Validates an inbound UDP relay request against active room membership and rate limits.
    /// Returns `(sender_peer_id, target_public_addr)` on success, or `None` for silent drop.
    pub fn authorize_relay_forward(
        &self,
        src_addr: SocketAddr,
        room_id: RoomId,
        target_peer_id: PeerId,
        now: Instant,
        ttl: Duration,
    ) -> Option<(PeerId, SocketAddr)> {
        let (sender_peer_id, target_addr) = {
            let room = self.rooms.get(&room_id)?;
            let sender = room.peers.values().find(|peer| {
                peer.public_addr == src_addr && now.saturating_duration_since(peer.last_seen) <= ttl
            })?;
            if target_peer_id == sender.peer_id {
                return None;
            }
            let target = room.peers.values().find(|peer| {
                peer.peer_id == target_peer_id
                    && now.saturating_duration_since(peer.last_seen) <= ttl
            })?;
            (sender.peer_id, target.public_addr)
        };

        if !self.check_relay_rate_limit(src_addr.ip(), now) {
            return None;
        }

        Some((sender_peer_id, target_addr))
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
        if !self.rooms.contains_key(&room_id) && self.rooms.len() >= MAX_ACTIVE_ROOMS {
            return vec![(
                src_addr,
                SignalingPacket::S2cError {
                    code: SignalingErrorCode::RoomFull,
                },
            )];
        }

        let mut room = self.rooms.entry(room_id).or_default();

        if let Some(existing) = room.peers.get(&client_uuid) {
            if existing.public_addr != src_addr
                && existing.public_addr.ip() != src_addr.ip()
                && now.saturating_duration_since(existing.last_seen) <= Duration::from_secs(2)
            {
                return Vec::new();
            }
        }

        let before_len = room.peers.len();
        room.peers
            .retain(|uuid, peer| *uuid == client_uuid || peer.public_addr != src_addr);
        let removed_collision = room.peers.len() != before_len;

        if let Some(existing) = room.peers.get_mut(&client_uuid) {
            let changed = removed_collision
                || existing.public_addr != src_addr
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
        src_addr: SocketAddr,
        room_id: RoomId,
        client_uuid: ClientUuid,
    ) -> Vec<(SocketAddr, SignalingPacket)> {
        let mut notifications = Vec::new();
        let mut should_remove_room = false;

        if let Some(mut room) = self.rooms.get_mut(&room_id) {
            if room
                .peers
                .get(&client_uuid)
                .is_some_and(|p| p.public_addr == src_addr)
            {
                room.peers.remove(&client_uuid);
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
    pub fn evict_expired(&self, now: Instant, ttl: Duration) -> Vec<(SocketAddr, SignalingPacket)> {
        let mut updates = Vec::new();
        let mut empty_rooms = Vec::new();

        for mut entry in self.rooms.iter_mut() {
            let room_id = *entry.key();
            let room = entry.value_mut();
            let before = room.peers.len();
            room.peers
                .retain(|_, peer| now.saturating_duration_since(peer.last_seen) <= ttl);

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
            .retain(|_, b| now.saturating_duration_since(b.last_refill) <= STALE_BUCKET_TTL);
        self.relay_rate_limits
            .retain(|_, b| now.saturating_duration_since(b.last_refill) <= STALE_BUCKET_TTL);

        updates
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn register_peer(
        state: &RendezvousState,
        src_addr: SocketAddr,
        room_id: RoomId,
        uuid: ClientUuid,
        name: &str,
        now: Instant,
    ) -> PeerId {
        let responses =
            state.handle_register(src_addr, room_id, uuid, name.to_string(), Vec::new(), now);
        for (dst, pkt) in responses {
            if dst == src_addr {
                if let SignalingPacket::S2cRoomState {
                    assigned_peer_id, ..
                } = pkt
                {
                    return assigned_peer_id;
                }
            }
        }
        panic!("expected S2cRoomState for {src_addr}");
    }

    #[test]
    fn test_authorize_relay_forward_same_room_and_rejects_self() {
        let state = RendezvousState::new();
        let now = Instant::now();
        let ttl = Duration::from_secs(6);
        let room_id = [0x11u8; 16];
        let addr_a: SocketAddr = "203.0.113.10:5001".parse().unwrap();
        let addr_b: SocketAddr = "203.0.113.11:5002".parse().unwrap();

        let peer_a_id = register_peer(&state, addr_a, room_id, [1u8; 16], "Alice", now);
        let peer_b_id = register_peer(&state, addr_b, room_id, [2u8; 16], "Bob", now);

        assert_eq!(
            state.authorize_relay_forward(addr_a, room_id, peer_b_id, now, ttl),
            Some((peer_a_id, addr_b))
        );
        assert_eq!(
            state.authorize_relay_forward(addr_b, room_id, peer_a_id, now, ttl),
            Some((peer_b_id, addr_a))
        );
        // Self-relay is rejected.
        assert_eq!(
            state.authorize_relay_forward(addr_a, room_id, peer_a_id, now, ttl),
            None
        );
    }

    #[test]
    fn test_authorize_relay_forward_rejects_spoofed_or_unregistered_src() {
        let state = RendezvousState::new();
        let now = Instant::now();
        let ttl = Duration::from_secs(6);
        let room_id = [0x22u8; 16];
        let addr_a: SocketAddr = "203.0.113.10:5001".parse().unwrap();
        let addr_b: SocketAddr = "203.0.113.11:5002".parse().unwrap();
        let spoofed_port: SocketAddr = "203.0.113.10:9999".parse().unwrap();
        let outsider: SocketAddr = "198.51.100.55:5001".parse().unwrap();

        let _peer_a_id = register_peer(&state, addr_a, room_id, [1u8; 16], "Alice", now);
        let peer_b_id = register_peer(&state, addr_b, room_id, [2u8; 16], "Bob", now);

        assert_eq!(
            state.authorize_relay_forward(spoofed_port, room_id, peer_b_id, now, ttl),
            None
        );
        assert_eq!(
            state.authorize_relay_forward(outsider, room_id, peer_b_id, now, ttl),
            None
        );
        assert_eq!(
            state.authorize_relay_forward(addr_a, [0xFFu8; 16], peer_b_id, now, ttl),
            None
        );
        assert_eq!(
            state.authorize_relay_forward(addr_a, room_id, 99, now, ttl),
            None
        );
        // Unauthenticated drops must not populate relay_rate_limits.
        assert!(!state.relay_rate_limits.contains_key(&outsider.ip()));
    }

    #[test]
    fn test_authorize_relay_forward_rejects_cross_room_target() {
        let state = RendezvousState::new();
        let now = Instant::now();
        let ttl = Duration::from_secs(6);
        let room_1 = [0x31u8; 16];
        let room_2 = [0x32u8; 16];
        let addr_a: SocketAddr = "203.0.113.10:5001".parse().unwrap();
        let addr_b: SocketAddr = "203.0.113.11:5002".parse().unwrap();
        let addr_c: SocketAddr = "203.0.113.12:5003".parse().unwrap();

        let _peer_a = register_peer(&state, addr_a, room_1, [1u8; 16], "Alice", now);
        let _peer_b = register_peer(&state, addr_b, room_2, [2u8; 16], "Bob", now);
        let peer_c = register_peer(&state, addr_c, room_2, [3u8; 16], "Carol", now);

        assert_eq!(
            state.authorize_relay_forward(addr_a, room_1, peer_c, now, ttl),
            None
        );
        assert_eq!(
            state.authorize_relay_forward(addr_a, room_2, peer_c, now, ttl),
            None
        );
    }

    #[test]
    fn test_authorize_relay_forward_rejects_expired_peer() {
        let state = RendezvousState::new();
        let t0 = Instant::now();
        let ttl = Duration::from_secs(6);
        let room_id = [0x44u8; 16];
        let addr_a: SocketAddr = "203.0.113.10:5001".parse().unwrap();
        let addr_b: SocketAddr = "203.0.113.11:5002".parse().unwrap();

        let peer_a_id = register_peer(&state, addr_a, room_id, [1u8; 16], "Alice", t0);
        let peer_b_id = register_peer(&state, addr_b, room_id, [2u8; 16], "Bob", t0);

        // Within TTL (t0 + 6s) succeeds.
        assert_eq!(
            state.authorize_relay_forward(addr_a, room_id, peer_b_id, t0 + ttl, ttl),
            Some((peer_a_id, addr_b))
        );

        // Past TTL (t0 + 7s) is rejected.
        let t_expired = t0 + Duration::from_secs(7);
        assert_eq!(
            state.authorize_relay_forward(addr_a, room_id, peer_b_id, t_expired, ttl),
            None
        );

        // Refresh only sender at t0 + 5s: target is still expired at t0 + 7s.
        register_peer(
            &state,
            addr_a,
            room_id,
            [1u8; 16],
            "Alice",
            t0 + Duration::from_secs(5),
        );
        assert_eq!(
            state.authorize_relay_forward(addr_a, room_id, peer_b_id, t_expired, ttl),
            None
        );
    }

    #[test]
    fn test_relay_rate_limit_burst_refill_and_100pps_voice() {
        let state = RendezvousState::new();
        let t0 = Instant::now();
        let ttl = Duration::from_secs(6);
        let room_id = [0x55u8; 16];
        let addr_a: SocketAddr = "203.0.113.10:5001".parse().unwrap();
        let addr_b: SocketAddr = "203.0.113.11:5002".parse().unwrap();

        let peer_a_id = register_peer(&state, addr_a, room_id, [1u8; 16], "Alice", t0);
        let peer_b_id = register_peer(&state, addr_b, room_id, [2u8; 16], "Bob", t0);

        // 100 pkt/s stream over 2s (10ms spacing) must never be false-blocked.
        for step in 0..200u64 {
            let now = t0 + Duration::from_millis(step * 10);
            assert_eq!(
                state.authorize_relay_forward(addr_a, room_id, peer_b_id, now, ttl),
                Some((peer_a_id, addr_b)),
                "100 pkt/s stream blocked at step {step}"
            );
        }

        // Wait 1s for full burst refill and verify burst ceiling (500 packets) at a single instant.
        let t_burst = t0 + Duration::from_secs(3);
        register_peer(&state, addr_a, room_id, [1u8; 16], "Alice", t_burst);
        register_peer(&state, addr_b, room_id, [2u8; 16], "Bob", t_burst);

        for i in 0..500 {
            assert_eq!(
                state.authorize_relay_forward(addr_a, room_id, peer_b_id, t_burst, ttl),
                Some((peer_a_id, addr_b)),
                "burst packet {i} should pass"
            );
        }
        assert_eq!(
            state.authorize_relay_forward(addr_a, room_id, peer_b_id, t_burst, ttl),
            None,
            "packet exceeding burst ceiling must be dropped"
        );

        // 20ms at 2500 pkt/s refills exactly 50 tokens.
        let t_refill = t_burst + Duration::from_millis(20);
        for i in 0..50 {
            assert_eq!(
                state.authorize_relay_forward(addr_a, room_id, peer_b_id, t_refill, ttl),
                Some((peer_a_id, addr_b)),
                "refilled packet {i} should pass"
            );
        }
        assert_eq!(
            state.authorize_relay_forward(addr_a, room_id, peer_b_id, t_refill, ttl),
            None
        );

        // Stale relay rate-limit buckets older than 60s are evicted.
        assert!(state.relay_rate_limits.contains_key(&addr_a.ip()));
        let _ = state.evict_expired(t_refill + Duration::from_secs(61), ttl);
        assert!(!state.relay_rate_limits.contains_key(&addr_a.ip()));
    }

    #[test]
    fn test_handle_leave_rejects_mismatched_src_addr() {
        let state = RendezvousState::new();
        let now = Instant::now();
        let room_id = [0x66u8; 16];
        let uuid_a = [1u8; 16];
        let uuid_b = [2u8; 16];
        let addr_a: SocketAddr = "203.0.113.10:5001".parse().unwrap();
        let addr_b: SocketAddr = "203.0.113.11:5002".parse().unwrap();
        let attacker_addr: SocketAddr = "198.51.100.77:9999".parse().unwrap();

        register_peer(&state, addr_a, room_id, uuid_a, "Alice", now);
        register_peer(&state, addr_b, room_id, uuid_b, "Bob", now);

        // Spoofed leave from another address must be rejected and leave peer intact.
        let spoofed = state.handle_leave(attacker_addr, room_id, uuid_a);
        assert!(spoofed.is_empty());
        assert_eq!(state.rooms.get(&room_id).unwrap().peers.len(), 2);

        // Spoofed leave from a different port on the same IP must also be rejected.
        let wrong_port: SocketAddr = "203.0.113.10:9999".parse().unwrap();
        assert!(state.handle_leave(wrong_port, room_id, uuid_a).is_empty());
        assert_eq!(state.rooms.get(&room_id).unwrap().peers.len(), 2);

        // Genuine leave from registered public_addr succeeds and notifies remaining peer.
        let legit = state.handle_leave(addr_a, room_id, uuid_a);
        assert_eq!(legit.len(), 1);
        assert_eq!(legit[0].0, addr_b);
        assert_eq!(state.rooms.get(&room_id).unwrap().peers.len(), 1);
    }

    #[test]
    fn test_handle_register_prevents_active_hijack_and_cleans_addr_collision() {
        let state = RendezvousState::new();
        let t0 = Instant::now();
        let room_id = [0x77u8; 16];
        let uuid_a = [1u8; 16];
        let uuid_a_new = [3u8; 16];
        let addr_a: SocketAddr = "203.0.113.10:5001".parse().unwrap();
        let attacker_addr: SocketAddr = "198.51.100.88:5001".parse().unwrap();

        register_peer(&state, addr_a, room_id, uuid_a, "Alice", t0);

        // Active session hijack from another IP within 2s is ignored.
        let hijack = state.handle_register(
            attacker_addr,
            room_id,
            uuid_a,
            "Hijacked".into(),
            Vec::new(),
            t0 + Duration::from_secs(1),
        );
        assert!(hijack.is_empty());
        assert_eq!(
            state.rooms.get(&room_id).unwrap().peers[&uuid_a].public_addr,
            addr_a
        );

        // Failover from another IP after >2s is allowed.
        let failover = state.handle_register(
            attacker_addr,
            room_id,
            uuid_a,
            "Alice".into(),
            Vec::new(),
            t0 + Duration::from_secs(3),
        );
        assert_eq!(failover.len(), 1);
        assert_eq!(
            state.rooms.get(&room_id).unwrap().peers[&uuid_a].public_addr,
            attacker_addr
        );

        // Restarting with a new UUID from the same socket address cleans up the stale UUID.
        let restart = state.handle_register(
            attacker_addr,
            room_id,
            uuid_a_new,
            "Alice2".into(),
            Vec::new(),
            t0 + Duration::from_secs(4),
        );
        assert_eq!(restart.len(), 1);
        let room = state.rooms.get(&room_id).unwrap();
        assert!(!room.peers.contains_key(&uuid_a));
        assert!(room.peers.contains_key(&uuid_a_new));
    }
}
