use dashmap::DashMap;
use squad_proto::{
    ClientUuid, FriendPresenceInfo, PeerId, PeerRecord, RoomId, SignalingErrorCode,
    SignalingPacket, MAX_FRIENDS_STATUS_PER_PACKET, MAX_FRIEND_INVITE_CIPHERTEXT, MAX_ROOM_PEERS,
    MAX_SPAKE2_MSG_LEN, SHORT_CODE_MAX,
};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, SocketAddrV4};
use std::time::{Duration, Instant};

const MAX_PACKETS_PER_SEC_PER_IP: f64 = 30.0;
const BUCKET_BURST: f64 = 45.0;
const RELAY_PACKETS_PER_SEC_PER_PEER: f64 = 150.0;
const RELAY_BUCKET_BURST: f64 = 150.0;
const STALE_BUCKET_TTL: Duration = Duration::from_secs(60);
const MAX_ACTIVE_ROOMS: usize = 4096;
const MAX_RATE_LIMIT_BUCKETS: usize = 65_536;
pub const PRESENCE_TTL: Duration = Duration::from_secs(15);
const MAX_PRESENCE_ENTRIES: usize = 65_536;
pub const SHORT_CODE_TTL_SECS: u16 = 600;
pub const SHORT_CODE_TTL: Duration = Duration::from_secs(600);
const MAX_SHORT_CODES: usize = 4096;
const SHORT_CODE_FAIL_WINDOW: Duration = Duration::from_secs(60);
const SHORT_CODE_MAX_FAILS: usize = 5;
const SHORT_CODE_BLOCK_DURATION: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct PresenceRecord {
    pub addr: SocketAddr,
    pub nickname: String,
    pub in_room: bool,
    pub last_seen: Instant,
}

#[derive(Debug, Clone)]
pub struct ShortCodeEntry {
    pub room_id: RoomId,
    pub host_pub_key: [u8; 32],
    pub host_addr: SocketAddr,
    pub host_lan_addrs: Vec<SocketAddrV4>,
    pub created_at: Instant,
}

#[derive(Debug, Default)]
struct ShortCodeRateState {
    failures: Vec<Instant>,
    blocked_until: Option<Instant>,
}

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

fn consume_token<K: Eq + std::hash::Hash>(
    buckets: &DashMap<K, TokenBucket>,
    key: K,
    now: Instant,
    rate_per_sec: f64,
    burst: f64,
) -> bool {
    if !buckets.contains_key(&key) && buckets.len() >= MAX_RATE_LIMIT_BUCKETS {
        return false;
    }
    let mut entry = buckets.entry(key).or_insert(TokenBucket {
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
    presence: DashMap<[u8; 32], PresenceRecord>,
    short_codes: DashMap<u32, ShortCodeEntry>,
    short_code_failures: DashMap<IpAddr, ShortCodeRateState>,
    rate_limits: DashMap<IpAddr, TokenBucket>,
    relay_rate_limits: DashMap<SocketAddr, TokenBucket>,
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
            presence: DashMap::new(),
            short_codes: DashMap::new(),
            short_code_failures: DashMap::new(),
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

    pub fn check_relay_rate_limit(&self, addr: SocketAddr, now: Instant) -> bool {
        consume_token(
            &self.relay_rate_limits,
            addr,
            now,
            RELAY_PACKETS_PER_SEC_PER_PEER,
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

        if !self.check_relay_rate_limit(src_addr, now) {
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
        self.short_codes.retain(|_, sc| {
            sc.room_id != room_id || (!should_remove_room && sc.host_addr != src_addr)
        });
        notifications
    }

    pub fn handle_presence_ping(
        &self,
        src_addr: SocketAddr,
        pub_key: [u8; 32],
        nickname: String,
        in_room: bool,
        now: Instant,
    ) {
        if !self.presence.contains_key(&pub_key) && self.presence.len() >= MAX_PRESENCE_ENTRIES {
            return;
        }
        if let Some(existing) = self.presence.get(&pub_key) {
            if existing.addr != src_addr
                && existing.addr.ip() != src_addr.ip()
                && now.saturating_duration_since(existing.last_seen) <= Duration::from_secs(2)
            {
                return;
            }
        }
        self.presence.insert(
            pub_key,
            PresenceRecord {
                addr: src_addr,
                nickname,
                in_room,
                last_seen: now,
            },
        );
    }

    pub fn handle_query_friends(
        &self,
        src_addr: SocketAddr,
        friend_pub_keys: Vec<[u8; 32]>,
        now: Instant,
    ) -> Vec<(SocketAddr, SignalingPacket)> {
        let mut online = Vec::new();
        for key in friend_pub_keys {
            if let Some(rec) = self.presence.get(&key) {
                if now.saturating_duration_since(rec.last_seen) <= PRESENCE_TTL {
                    online.push(FriendPresenceInfo {
                        pub_key: key,
                        nickname: rec.nickname.clone(),
                        in_room: rec.in_room,
                    });
                }
            }
        }
        if online.is_empty() {
            return vec![(
                src_addr,
                SignalingPacket::S2cFriendsStatus {
                    online_friends: Vec::new(),
                },
            )];
        }
        online
            .chunks(MAX_FRIENDS_STATUS_PER_PACKET)
            .map(|chunk| {
                (
                    src_addr,
                    SignalingPacket::S2cFriendsStatus {
                        online_friends: chunk.to_vec(),
                    },
                )
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn handle_relay_friend_invite(
        &self,
        src_addr: SocketAddr,
        sender_pub_key: [u8; 32],
        target_pub_key: [u8; 32],
        ephemeral_pub_key: [u8; 32],
        nonce: [u8; 12],
        ciphertext: Vec<u8>,
        now: Instant,
    ) -> Vec<(SocketAddr, SignalingPacket)> {
        if ciphertext.is_empty() || ciphertext.len() > MAX_FRIEND_INVITE_CIPHERTEXT {
            return vec![(
                src_addr,
                SignalingPacket::S2cError {
                    code: SignalingErrorCode::InvalidPacket,
                },
            )];
        }
        let target_addr = self.presence.get(&target_pub_key).and_then(|rec| {
            if now.saturating_duration_since(rec.last_seen) <= PRESENCE_TTL {
                Some(rec.addr)
            } else {
                None
            }
        });
        let Some(dst_addr) = target_addr else {
            return vec![(
                src_addr,
                SignalingPacket::S2cError {
                    code: SignalingErrorCode::FriendOffline,
                },
            )];
        };
        vec![(
            dst_addr,
            SignalingPacket::S2cIncomingFriendInvite {
                sender_pub_key,
                ephemeral_pub_key,
                nonce,
                ciphertext,
            },
        )]
    }

    pub fn handle_register_short_code(
        &self,
        src_addr: SocketAddr,
        room_id: RoomId,
        host_pub_key: [u8; 32],
        code: u32,
        now: Instant,
    ) -> Vec<(SocketAddr, SignalingPacket)> {
        if code > SHORT_CODE_MAX {
            return vec![(
                src_addr,
                SignalingPacket::S2cError {
                    code: SignalingErrorCode::InvalidShortCode,
                },
            )];
        }

        let host_lan_addrs = {
            let Some(room) = self.rooms.get(&room_id) else {
                return vec![(
                    src_addr,
                    SignalingPacket::S2cError {
                        code: SignalingErrorCode::InvalidPacket,
                    },
                )];
            };
            let Some(peer) = room.peers.values().find(|p| p.public_addr == src_addr) else {
                return vec![(
                    src_addr,
                    SignalingPacket::S2cError {
                        code: SignalingErrorCode::InvalidPacket,
                    },
                )];
            };
            peer.lan_addrs.clone()
        };

        self.short_codes.retain(|_, sc| sc.room_id != room_id);

        if let Some(existing) = self.short_codes.get(&code) {
            if now.saturating_duration_since(existing.created_at) <= SHORT_CODE_TTL
                && self.rooms.contains_key(&existing.room_id)
            {
                return vec![(
                    src_addr,
                    SignalingPacket::S2cError {
                        code: SignalingErrorCode::InvalidShortCode,
                    },
                )];
            }
        }

        if !self.short_codes.contains_key(&code) && self.short_codes.len() >= MAX_SHORT_CODES {
            return vec![(
                src_addr,
                SignalingPacket::S2cError {
                    code: SignalingErrorCode::RoomFull,
                },
            )];
        }

        self.short_codes.insert(
            code,
            ShortCodeEntry {
                room_id,
                host_pub_key,
                host_addr: src_addr,
                host_lan_addrs,
                created_at: now,
            },
        );

        vec![(
            src_addr,
            SignalingPacket::S2cShortCodeRegistered {
                code,
                expires_in_secs: SHORT_CODE_TTL_SECS,
            },
        )]
    }

    pub fn handle_resolve_short_code(
        &self,
        src_addr: SocketAddr,
        code: u32,
        joiner_pub_key: [u8; 32],
        joiner_lan_addrs: Vec<SocketAddrV4>,
        spake2_msg_a: Vec<u8>,
        now: Instant,
    ) -> Vec<(SocketAddr, SignalingPacket)> {
        let src_ip = src_addr.ip();
        if let Some(state) = self.short_code_failures.get(&src_ip) {
            if state.blocked_until.is_some_and(|until| now < until) {
                return vec![(
                    src_addr,
                    SignalingPacket::S2cError {
                        code: SignalingErrorCode::RateLimited,
                    },
                )];
            }
        }

        if code > SHORT_CODE_MAX
            || spake2_msg_a.is_empty()
            || spake2_msg_a.len() > MAX_SPAKE2_MSG_LEN
        {
            return self.record_short_code_failure(src_addr, now);
        }

        let entry = self.short_codes.get(&code).map(|e| e.value().clone());
        if let Some(sc) = entry {
            let not_expired = now.saturating_duration_since(sc.created_at) <= SHORT_CODE_TTL;
            let host_live = self
                .rooms
                .get(&sc.room_id)
                .is_some_and(|r| r.peers.values().any(|p| p.public_addr == sc.host_addr));
            if not_expired && host_live {
                return vec![
                    (
                        sc.host_addr,
                        SignalingPacket::S2cShortCodeChallenge {
                            code,
                            joiner_pub_key,
                            joiner_public_addr: src_addr,
                            joiner_lan_addrs,
                            spake2_msg_a,
                        },
                    ),
                    (
                        src_addr,
                        SignalingPacket::S2cShortCodeJoinerInfo {
                            code,
                            host_pub_key: sc.host_pub_key,
                            host_public_addr: sc.host_addr,
                            host_lan_addrs: sc.host_lan_addrs,
                        },
                    ),
                ];
            }
            self.short_codes.remove(&code);
        }

        self.record_short_code_failure(src_addr, now)
    }

    fn record_short_code_failure(
        &self,
        src_addr: SocketAddr,
        now: Instant,
    ) -> Vec<(SocketAddr, SignalingPacket)> {
        let ip = src_addr.ip();
        if !self.short_code_failures.contains_key(&ip)
            && self.short_code_failures.len() >= MAX_RATE_LIMIT_BUCKETS
        {
            return vec![(
                src_addr,
                SignalingPacket::S2cError {
                    code: SignalingErrorCode::RateLimited,
                },
            )];
        }
        let mut entry = self.short_code_failures.entry(ip).or_default();
        if entry.blocked_until.is_some_and(|until| now >= until) {
            entry.blocked_until = None;
            entry.failures.clear();
        }
        entry
            .failures
            .retain(|&t| now.saturating_duration_since(t) <= SHORT_CODE_FAIL_WINDOW);
        entry.failures.push(now);

        let err_code = if entry.failures.len() >= SHORT_CODE_MAX_FAILS {
            entry.blocked_until = Some(now + SHORT_CODE_BLOCK_DURATION);
            SignalingErrorCode::RateLimited
        } else {
            SignalingErrorCode::InvalidShortCode
        };

        vec![(src_addr, SignalingPacket::S2cError { code: err_code })]
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

        // Prune stale IP rate-limit buckets, presence records, short codes, and brute-force state
        self.presence
            .retain(|_, p| now.saturating_duration_since(p.last_seen) <= PRESENCE_TTL);
        self.short_codes.retain(|_, sc| {
            now.saturating_duration_since(sc.created_at) <= SHORT_CODE_TTL
                && self
                    .rooms
                    .get(&sc.room_id)
                    .is_some_and(|r| r.peers.values().any(|p| p.public_addr == sc.host_addr))
        });
        self.short_code_failures.retain(|_, st| {
            st.blocked_until.is_some_and(|u| now < u)
                || st
                    .failures
                    .iter()
                    .any(|&t| now.saturating_duration_since(t) <= SHORT_CODE_FAIL_WINDOW)
        });
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
        assert!(!state.relay_rate_limits.contains_key(&outsider));
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

        // Wait 1s for full burst refill and verify burst ceiling (150 packets) at a single instant.
        let t_burst = t0 + Duration::from_secs(3);
        register_peer(&state, addr_a, room_id, [1u8; 16], "Alice", t_burst);
        register_peer(&state, addr_b, room_id, [2u8; 16], "Bob", t_burst);

        for i in 0..150 {
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

        // 100ms at 150 pkt/s refills exactly 15 tokens.
        let t_refill = t_burst + Duration::from_millis(100);
        for i in 0..15 {
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
        assert!(state.relay_rate_limits.contains_key(&addr_a));
        let _ = state.evict_expired(t_refill + Duration::from_secs(61), ttl);
        assert!(!state.relay_rate_limits.contains_key(&addr_a));
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

    #[test]
    fn test_presence_ttl_query_privacy_and_invite_relay() {
        let state = RendezvousState::new();
        let t0 = Instant::now();
        let addr_a: SocketAddr = "203.0.113.10:5001".parse().unwrap();
        let addr_b: SocketAddr = "203.0.113.11:5002".parse().unwrap();
        let key_a = [0xA1; 32];
        let key_b = [0xB2; 32];
        let key_c = [0xC3; 32];

        state.handle_presence_ping(addr_a, key_a, "Alice".into(), false, t0);
        state.handle_presence_ping(addr_b, key_b, "Bob".into(), true, t0);

        let res =
            state.handle_query_friends(addr_a, vec![key_b, key_c], t0 + Duration::from_secs(5));
        assert_eq!(res.len(), 1);
        match &res[0].1 {
            SignalingPacket::S2cFriendsStatus { online_friends } => {
                assert_eq!(online_friends.len(), 1);
                assert_eq!(online_friends[0].pub_key, key_b);
                assert_eq!(online_friends[0].nickname, "Bob");
                assert!(online_friends[0].in_room);
            }
            other => panic!("expected S2cFriendsStatus, got {other:?}"),
        }

        let relay = state.handle_relay_friend_invite(
            addr_a,
            key_a,
            key_b,
            [0xE1; 32],
            [0x12; 12],
            vec![0x77; 48],
            t0 + Duration::from_secs(6),
        );
        assert_eq!(relay.len(), 1);
        assert_eq!(relay[0].0, addr_b);
        assert!(matches!(
            relay[0].1,
            SignalingPacket::S2cIncomingFriendInvite { sender_pub_key, .. } if sender_pub_key == key_a
        ));

        // After 16s (> 15s TTL), Bob expires and invite returns FriendOffline without leaking IP.
        let t_late = t0 + Duration::from_secs(16);
        let offline_relay = state.handle_relay_friend_invite(
            addr_a,
            key_a,
            key_b,
            [0xE2; 32],
            [0x13; 12],
            vec![0x77; 48],
            t_late,
        );
        assert_eq!(
            offline_relay,
            vec![(
                addr_a,
                SignalingPacket::S2cError {
                    code: SignalingErrorCode::FriendOffline
                }
            )]
        );
    }

    #[test]
    fn test_short_code_registration_resolution_eviction_and_rate_limit() {
        let state = RendezvousState::new();
        let t0 = Instant::now();
        let peer_ttl = Duration::from_secs(6);
        let room_id = [0x88; 16];
        let host_addr: SocketAddr = "203.0.113.10:5001".parse().unwrap();
        let joiner_addr: SocketAddr = "203.0.113.20:5002".parse().unwrap();
        let attacker_addr: SocketAddr = "198.51.100.99:5003".parse().unwrap();
        let host_pk = [0x11; 32];
        let joiner_pk = [0x22; 32];

        register_peer(&state, host_addr, room_id, [1u8; 16], "Host", t0);

        let reg = state.handle_register_short_code(host_addr, room_id, host_pk, 482_910, t0);
        assert_eq!(
            reg,
            vec![(
                host_addr,
                SignalingPacket::S2cShortCodeRegistered {
                    code: 482_910,
                    expires_in_secs: 600
                }
            )]
        );

        let res = state.handle_resolve_short_code(
            joiner_addr,
            482_910,
            joiner_pk,
            vec!["192.168.1.50:5002".parse().unwrap()],
            vec![0xAA; 33],
            t0 + Duration::from_secs(1),
        );
        assert_eq!(res.len(), 2);
        assert_eq!(res[0].0, host_addr);
        assert!(matches!(
            res[0].1,
            SignalingPacket::S2cShortCodeChallenge { code: 482_910, .. }
        ));
        assert_eq!(res[1].0, joiner_addr);
        assert!(matches!(
            res[1].1,
            SignalingPacket::S2cShortCodeJoinerInfo { code: 482_910, .. }
        ));

        // 4 failed attempts return InvalidShortCode; 5th failed attempt triggers 60s RateLimited lock.
        for attempt in 1..=4u32 {
            let err = state.handle_resolve_short_code(
                attacker_addr,
                100_000 + attempt,
                joiner_pk,
                Vec::new(),
                vec![0xBB; 33],
                t0 + Duration::from_secs(1),
            );
            assert_eq!(
                err,
                vec![(
                    attacker_addr,
                    SignalingPacket::S2cError {
                        code: SignalingErrorCode::InvalidShortCode
                    }
                )]
            );
        }
        let fifth = state.handle_resolve_short_code(
            attacker_addr,
            999_999,
            joiner_pk,
            Vec::new(),
            vec![0xBB; 33],
            t0 + Duration::from_secs(2),
        );
        assert_eq!(
            fifth,
            vec![(
                attacker_addr,
                SignalingPacket::S2cError {
                    code: SignalingErrorCode::RateLimited
                }
            )]
        );

        // Even guessing the VALID code while blocked must stay RateLimited for 60s.
        let blocked_valid = state.handle_resolve_short_code(
            attacker_addr,
            482_910,
            joiner_pk,
            Vec::new(),
            vec![0xBB; 33],
            t0 + Duration::from_secs(30),
        );
        assert_eq!(
            blocked_valid,
            vec![(
                attacker_addr,
                SignalingPacket::S2cError {
                    code: SignalingErrorCode::RateLimited
                }
            )]
        );

        // Evicting expired host automatically removes its room short code.
        let _ = state.evict_expired(t0 + Duration::from_secs(10), peer_ttl);
        assert!(!state.short_codes.contains_key(&482_910));
    }
}
