use crate::packet::{need_bytes, sanitize_display_str, truncate_utf8};
use crate::{
    ClientUuid, PeerId, ProtoError, RoomId, MAGIC, MAX_LAN_ADDRS, MAX_NICKNAME_LEN, MAX_ROOM_PEERS,
    MAX_UDP_PAYLOAD, PROTO_VERSION,
};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4};

pub const MAX_QUERY_FRIENDS_PER_PACKET: usize = 32;
pub const MAX_FRIENDS_STATUS_PER_PACKET: usize = 16;
pub const MAX_FRIEND_INVITE_CIPHERTEXT: usize = 256;
pub const MAX_SPAKE2_MSG_LEN: usize = 64;
pub const SHORT_CODE_MAX: u32 = 999_999;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SignalingErrorCode {
    RoomFull = 1,
    RateLimited = 2,
    InvalidPacket = 3,
    FriendOffline = 4,
    InvalidShortCode = 5,
}

impl SignalingErrorCode {
    pub fn from_u8(val: u8) -> Result<Self, ProtoError> {
        match val {
            1 => Ok(Self::RoomFull),
            2 => Ok(Self::RateLimited),
            3 => Ok(Self::InvalidPacket),
            4 => Ok(Self::FriendOffline),
            5 => Ok(Self::InvalidShortCode),
            other => Err(ProtoError::UnknownOpcode(other)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRecord {
    pub peer_id: PeerId,
    pub client_uuid: ClientUuid,
    pub public_addr: SocketAddr,
    pub lan_addrs: Vec<SocketAddrV4>,
    pub nickname: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FriendPresenceInfo {
    pub pub_key: [u8; 32],
    pub nickname: String,
    pub in_room: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignalingPacket {
    /// `0x01` Client -> Rendezvous
    C2sRegister {
        room_id: RoomId,
        client_uuid: ClientUuid,
        nickname: String,
        lan_addrs: Vec<SocketAddrV4>,
    },
    /// `0x02` Rendezvous -> Client
    S2cRoomState {
        assigned_peer_id: PeerId,
        peers: Vec<PeerRecord>,
    },
    /// `0x03` Client -> Rendezvous
    C2sLeave {
        room_id: RoomId,
        client_uuid: ClientUuid,
    },
    /// `0x04` Rendezvous -> Client
    S2cError { code: SignalingErrorCode },
    /// `0x70` Client -> Rendezvous
    C2sPresencePing {
        pub_key: [u8; 32],
        nickname: String,
        in_room: bool,
    },
    /// `0x71` Client -> Rendezvous
    C2sQueryFriends { friend_pub_keys: Vec<[u8; 32]> },
    /// `0x72` Rendezvous -> Client
    S2cFriendsStatus {
        online_friends: Vec<FriendPresenceInfo>,
    },
    /// `0x73` Client -> Rendezvous
    C2sRelayFriendInvite {
        sender_pub_key: [u8; 32],
        target_pub_key: [u8; 32],
        ephemeral_pub_key: [u8; 32],
        nonce: [u8; 12],
        ciphertext: Vec<u8>,
    },
    /// `0x74` Rendezvous -> Client
    S2cIncomingFriendInvite {
        sender_pub_key: [u8; 32],
        ephemeral_pub_key: [u8; 32],
        nonce: [u8; 12],
        ciphertext: Vec<u8>,
    },
    /// `0x75` Client -> Rendezvous
    C2sRegisterShortCode {
        room_id: RoomId,
        host_pub_key: [u8; 32],
        code: u32,
    },
    /// `0x76` Rendezvous -> Client
    S2cShortCodeRegistered { code: u32, expires_in_secs: u16 },
    /// `0x77` Client -> Rendezvous
    C2sResolveShortCode {
        code: u32,
        joiner_pub_key: [u8; 32],
        joiner_lan_addrs: Vec<SocketAddrV4>,
        spake2_msg_a: Vec<u8>,
    },
    /// `0x78` Rendezvous -> Host
    S2cShortCodeChallenge {
        code: u32,
        joiner_pub_key: [u8; 32],
        joiner_public_addr: SocketAddr,
        joiner_lan_addrs: Vec<SocketAddrV4>,
        spake2_msg_a: Vec<u8>,
    },
    /// `0x79` Rendezvous -> Joiner
    S2cShortCodeJoinerInfo {
        code: u32,
        host_pub_key: [u8; 32],
        host_public_addr: SocketAddr,
        host_lan_addrs: Vec<SocketAddrV4>,
    },
}

fn encode_socket_addr(buf: &mut Vec<u8>, addr: SocketAddr) {
    match addr.ip() {
        IpAddr::V4(ip4) => {
            buf.push(4);
            buf.extend_from_slice(&ip4.octets());
        }
        IpAddr::V6(ip6) => {
            buf.push(6);
            buf.extend_from_slice(&ip6.octets());
        }
    }
    buf.extend_from_slice(&addr.port().to_be_bytes());
}

fn decode_socket_addr(data: &[u8], cursor: &mut usize) -> Result<SocketAddr, ProtoError> {
    need_bytes(data, *cursor, 1)?;
    let family = data[*cursor];
    *cursor += 1;
    let ip = match family {
        4 => {
            need_bytes(data, *cursor, 4)?;
            let ip4 = Ipv4Addr::new(
                data[*cursor],
                data[*cursor + 1],
                data[*cursor + 2],
                data[*cursor + 3],
            );
            *cursor += 4;
            IpAddr::V4(ip4)
        }
        6 => {
            need_bytes(data, *cursor, 16)?;
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&data[*cursor..*cursor + 16]);
            *cursor += 16;
            IpAddr::V6(Ipv6Addr::from(octets))
        }
        _ => return Err(ProtoError::InvalidHeader),
    };
    need_bytes(data, *cursor, 2)?;
    let port = u16::from_be_bytes([data[*cursor], data[*cursor + 1]]);
    *cursor += 2;
    Ok(SocketAddr::new(ip, port))
}

fn encode_lan_addrs(buf: &mut Vec<u8>, lan_addrs: &[SocketAddrV4]) {
    let count = lan_addrs.len().min(MAX_LAN_ADDRS);
    buf.push(count as u8);
    for addr in lan_addrs.iter().take(count) {
        buf.extend_from_slice(&addr.ip().octets());
        buf.extend_from_slice(&addr.port().to_be_bytes());
    }
}

fn decode_lan_addrs(data: &[u8], cursor: &mut usize) -> Result<Vec<SocketAddrV4>, ProtoError> {
    need_bytes(data, *cursor, 1)?;
    let count = data[*cursor] as usize;
    *cursor += 1;
    if count > MAX_LAN_ADDRS {
        return Err(ProtoError::InvalidHeader);
    }
    need_bytes(data, *cursor, count * 6)?;
    let mut lan_addrs = Vec::with_capacity(count);
    for _ in 0..count {
        let ip = Ipv4Addr::new(
            data[*cursor],
            data[*cursor + 1],
            data[*cursor + 2],
            data[*cursor + 3],
        );
        let port = u16::from_be_bytes([data[*cursor + 4], data[*cursor + 5]]);
        lan_addrs.push(SocketAddrV4::new(ip, port));
        *cursor += 6;
    }
    Ok(lan_addrs)
}

fn decode_nick(data: &[u8], cursor: &mut usize) -> Result<String, ProtoError> {
    need_bytes(data, *cursor, 1)?;
    let nick_len = data[*cursor] as usize;
    *cursor += 1;
    if nick_len > MAX_NICKNAME_LEN {
        return Err(ProtoError::InvalidHeader);
    }
    need_bytes(data, *cursor, nick_len)?;
    let raw = std::str::from_utf8(&data[*cursor..*cursor + nick_len])
        .map_err(|_| ProtoError::InvalidUtf8)?;
    *cursor += nick_len;
    Ok(sanitize_display_str(raw, MAX_NICKNAME_LEN))
}

fn decode_bytes_32(data: &[u8], cursor: &mut usize) -> Result<[u8; 32], ProtoError> {
    need_bytes(data, *cursor, 32)?;
    let mut out = [0u8; 32];
    out.copy_from_slice(&data[*cursor..*cursor + 32]);
    *cursor += 32;
    Ok(out)
}

impl SignalingPacket {
    pub fn encode(&self) -> Result<Vec<u8>, ProtoError> {
        let mut buf = Vec::with_capacity(256);
        buf.extend_from_slice(&MAGIC);
        buf.push(PROTO_VERSION);

        match self {
            SignalingPacket::C2sRegister {
                room_id,
                client_uuid,
                nickname,
                lan_addrs,
            } => {
                buf.push(0x01);
                buf.extend_from_slice(room_id);
                buf.extend_from_slice(client_uuid);
                let nick_bytes = truncate_utf8(nickname, MAX_NICKNAME_LEN);
                buf.push(nick_bytes.len() as u8);
                buf.extend_from_slice(nick_bytes);
                encode_lan_addrs(&mut buf, lan_addrs);
            }
            SignalingPacket::S2cRoomState {
                assigned_peer_id,
                peers,
            } => {
                buf.push(0x02);
                buf.extend_from_slice(&assigned_peer_id.to_be_bytes());
                let peer_count = peers.len().min(MAX_ROOM_PEERS);
                buf.push(peer_count as u8);

                for peer in peers.iter().take(peer_count) {
                    buf.extend_from_slice(&peer.peer_id.to_be_bytes());
                    buf.extend_from_slice(&peer.client_uuid);
                    encode_socket_addr(&mut buf, peer.public_addr);
                    encode_lan_addrs(&mut buf, &peer.lan_addrs);
                    let nick_bytes = truncate_utf8(&peer.nickname, MAX_NICKNAME_LEN);
                    buf.push(nick_bytes.len() as u8);
                    buf.extend_from_slice(nick_bytes);
                }
            }
            SignalingPacket::C2sLeave {
                room_id,
                client_uuid,
            } => {
                buf.push(0x03);
                buf.extend_from_slice(room_id);
                buf.extend_from_slice(client_uuid);
            }
            SignalingPacket::S2cError { code } => {
                buf.push(0x04);
                buf.push(*code as u8);
            }
            SignalingPacket::C2sPresencePing {
                pub_key,
                nickname,
                in_room,
            } => {
                buf.push(0x70);
                buf.extend_from_slice(pub_key);
                buf.push(u8::from(*in_room));
                let nick_bytes = truncate_utf8(nickname, MAX_NICKNAME_LEN);
                buf.push(nick_bytes.len() as u8);
                buf.extend_from_slice(nick_bytes);
            }
            SignalingPacket::C2sQueryFriends { friend_pub_keys } => {
                if friend_pub_keys.len() > MAX_QUERY_FRIENDS_PER_PACKET {
                    return Err(ProtoError::PacketTooLarge(friend_pub_keys.len() * 32));
                }
                buf.push(0x71);
                buf.push(friend_pub_keys.len() as u8);
                for key in friend_pub_keys {
                    buf.extend_from_slice(key);
                }
            }
            SignalingPacket::S2cFriendsStatus { online_friends } => {
                if online_friends.len() > MAX_FRIENDS_STATUS_PER_PACKET {
                    return Err(ProtoError::PacketTooLarge(online_friends.len()));
                }
                buf.push(0x72);
                buf.push(online_friends.len() as u8);
                for friend in online_friends {
                    buf.extend_from_slice(&friend.pub_key);
                    buf.push(u8::from(friend.in_room));
                    let nick_bytes = truncate_utf8(&friend.nickname, MAX_NICKNAME_LEN);
                    buf.push(nick_bytes.len() as u8);
                    buf.extend_from_slice(nick_bytes);
                }
            }
            SignalingPacket::C2sRelayFriendInvite {
                sender_pub_key,
                target_pub_key,
                ephemeral_pub_key,
                nonce,
                ciphertext,
            } => {
                if ciphertext.len() > MAX_FRIEND_INVITE_CIPHERTEXT {
                    return Err(ProtoError::PacketTooLarge(ciphertext.len()));
                }
                buf.push(0x73);
                buf.extend_from_slice(sender_pub_key);
                buf.extend_from_slice(target_pub_key);
                buf.extend_from_slice(ephemeral_pub_key);
                buf.extend_from_slice(nonce);
                buf.extend_from_slice(&(ciphertext.len() as u16).to_be_bytes());
                buf.extend_from_slice(ciphertext);
            }
            SignalingPacket::S2cIncomingFriendInvite {
                sender_pub_key,
                ephemeral_pub_key,
                nonce,
                ciphertext,
            } => {
                if ciphertext.len() > MAX_FRIEND_INVITE_CIPHERTEXT {
                    return Err(ProtoError::PacketTooLarge(ciphertext.len()));
                }
                buf.push(0x74);
                buf.extend_from_slice(sender_pub_key);
                buf.extend_from_slice(ephemeral_pub_key);
                buf.extend_from_slice(nonce);
                buf.extend_from_slice(&(ciphertext.len() as u16).to_be_bytes());
                buf.extend_from_slice(ciphertext);
            }
            SignalingPacket::C2sRegisterShortCode {
                room_id,
                host_pub_key,
                code,
            } => {
                if *code > SHORT_CODE_MAX {
                    return Err(ProtoError::InvalidHeader);
                }
                buf.push(0x75);
                buf.extend_from_slice(room_id);
                buf.extend_from_slice(host_pub_key);
                buf.extend_from_slice(&code.to_be_bytes());
            }
            SignalingPacket::S2cShortCodeRegistered {
                code,
                expires_in_secs,
            } => {
                if *code > SHORT_CODE_MAX {
                    return Err(ProtoError::InvalidHeader);
                }
                buf.push(0x76);
                buf.extend_from_slice(&code.to_be_bytes());
                buf.extend_from_slice(&expires_in_secs.to_be_bytes());
            }
            SignalingPacket::C2sResolveShortCode {
                code,
                joiner_pub_key,
                joiner_lan_addrs,
                spake2_msg_a,
            } => {
                if *code > SHORT_CODE_MAX {
                    return Err(ProtoError::InvalidHeader);
                }
                if spake2_msg_a.is_empty() || spake2_msg_a.len() > MAX_SPAKE2_MSG_LEN {
                    return Err(ProtoError::InvalidHeader);
                }
                buf.push(0x77);
                buf.extend_from_slice(&code.to_be_bytes());
                buf.extend_from_slice(joiner_pub_key);
                encode_lan_addrs(&mut buf, joiner_lan_addrs);
                buf.push(spake2_msg_a.len() as u8);
                buf.extend_from_slice(spake2_msg_a);
            }
            SignalingPacket::S2cShortCodeChallenge {
                code,
                joiner_pub_key,
                joiner_public_addr,
                joiner_lan_addrs,
                spake2_msg_a,
            } => {
                if *code > SHORT_CODE_MAX {
                    return Err(ProtoError::InvalidHeader);
                }
                if spake2_msg_a.is_empty() || spake2_msg_a.len() > MAX_SPAKE2_MSG_LEN {
                    return Err(ProtoError::InvalidHeader);
                }
                buf.push(0x78);
                buf.extend_from_slice(&code.to_be_bytes());
                buf.extend_from_slice(joiner_pub_key);
                encode_socket_addr(&mut buf, *joiner_public_addr);
                encode_lan_addrs(&mut buf, joiner_lan_addrs);
                buf.push(spake2_msg_a.len() as u8);
                buf.extend_from_slice(spake2_msg_a);
            }
            SignalingPacket::S2cShortCodeJoinerInfo {
                code,
                host_pub_key,
                host_public_addr,
                host_lan_addrs,
            } => {
                if *code > SHORT_CODE_MAX {
                    return Err(ProtoError::InvalidHeader);
                }
                buf.push(0x79);
                buf.extend_from_slice(&code.to_be_bytes());
                buf.extend_from_slice(host_pub_key);
                encode_socket_addr(&mut buf, *host_public_addr);
                encode_lan_addrs(&mut buf, host_lan_addrs);
            }
        }

        if buf.len() > MAX_UDP_PAYLOAD {
            return Err(ProtoError::PacketTooLarge(buf.len()));
        }
        Ok(buf)
    }

    pub fn decode(data: &[u8]) -> Result<Self, ProtoError> {
        if data.len() > MAX_UDP_PAYLOAD {
            return Err(ProtoError::PacketTooLarge(data.len()));
        }
        if data.len() < 4 {
            return Err(ProtoError::PacketTooShort {
                expected: 4,
                actual: data.len(),
            });
        }
        if data[0..2] != MAGIC || data[2] != PROTO_VERSION {
            return Err(ProtoError::InvalidHeader);
        }

        let opcode = data[3];
        let mut cursor = 4usize;

        match opcode {
            0x01 => {
                need_bytes(data, cursor, 32)?;
                let mut room_id = [0u8; 16];
                room_id.copy_from_slice(&data[cursor..cursor + 16]);
                cursor += 16;

                let mut client_uuid = [0u8; 16];
                client_uuid.copy_from_slice(&data[cursor..cursor + 16]);
                cursor += 16;

                let nickname = decode_nick(data, &mut cursor)?;
                let lan_addrs = decode_lan_addrs(data, &mut cursor)?;

                Ok(SignalingPacket::C2sRegister {
                    room_id,
                    client_uuid,
                    nickname,
                    lan_addrs,
                })
            }
            0x02 => {
                need_bytes(data, cursor, 3)?;
                let assigned_peer_id = u16::from_be_bytes([data[cursor], data[cursor + 1]]);
                let peer_count = data[cursor + 2] as usize;
                cursor += 3;

                if peer_count > MAX_ROOM_PEERS {
                    return Err(ProtoError::InvalidHeader);
                }

                let mut peers = Vec::with_capacity(peer_count);
                for _ in 0..peer_count {
                    need_bytes(data, cursor, 18)?;
                    let peer_id = u16::from_be_bytes([data[cursor], data[cursor + 1]]);
                    cursor += 2;

                    let mut client_uuid = [0u8; 16];
                    client_uuid.copy_from_slice(&data[cursor..cursor + 16]);
                    cursor += 16;

                    let public_addr = decode_socket_addr(data, &mut cursor)?;
                    let lan_addrs = decode_lan_addrs(data, &mut cursor)?;
                    let nickname = decode_nick(data, &mut cursor)?;

                    peers.push(PeerRecord {
                        peer_id,
                        client_uuid,
                        public_addr,
                        lan_addrs,
                        nickname,
                    });
                }

                Ok(SignalingPacket::S2cRoomState {
                    assigned_peer_id,
                    peers,
                })
            }
            0x03 => {
                need_bytes(data, cursor, 32)?;
                let mut room_id = [0u8; 16];
                room_id.copy_from_slice(&data[cursor..cursor + 16]);
                let mut client_uuid = [0u8; 16];
                client_uuid.copy_from_slice(&data[cursor + 16..cursor + 32]);
                Ok(SignalingPacket::C2sLeave {
                    room_id,
                    client_uuid,
                })
            }
            0x04 => {
                need_bytes(data, cursor, 1)?;
                let code = SignalingErrorCode::from_u8(data[cursor])?;
                Ok(SignalingPacket::S2cError { code })
            }
            0x70 => {
                let pub_key = decode_bytes_32(data, &mut cursor)?;
                need_bytes(data, cursor, 1)?;
                let in_room = data[cursor] != 0;
                cursor += 1;
                let nickname = decode_nick(data, &mut cursor)?;
                Ok(SignalingPacket::C2sPresencePing {
                    pub_key,
                    nickname,
                    in_room,
                })
            }
            0x71 => {
                need_bytes(data, cursor, 1)?;
                let count = data[cursor] as usize;
                cursor += 1;
                if count > MAX_QUERY_FRIENDS_PER_PACKET {
                    return Err(ProtoError::PacketTooLarge(count * 32));
                }
                need_bytes(data, cursor, count * 32)?;
                let mut friend_pub_keys = Vec::with_capacity(count);
                for _ in 0..count {
                    friend_pub_keys.push(decode_bytes_32(data, &mut cursor)?);
                }
                Ok(SignalingPacket::C2sQueryFriends { friend_pub_keys })
            }
            0x72 => {
                need_bytes(data, cursor, 1)?;
                let count = data[cursor] as usize;
                cursor += 1;
                if count > MAX_FRIENDS_STATUS_PER_PACKET {
                    return Err(ProtoError::InvalidHeader);
                }
                let mut online_friends = Vec::with_capacity(count);
                for _ in 0..count {
                    let pub_key = decode_bytes_32(data, &mut cursor)?;
                    need_bytes(data, cursor, 1)?;
                    let in_room = data[cursor] != 0;
                    cursor += 1;
                    let nickname = decode_nick(data, &mut cursor)?;
                    online_friends.push(FriendPresenceInfo {
                        pub_key,
                        nickname,
                        in_room,
                    });
                }
                Ok(SignalingPacket::S2cFriendsStatus { online_friends })
            }
            0x73 => {
                let sender_pub_key = decode_bytes_32(data, &mut cursor)?;
                let target_pub_key = decode_bytes_32(data, &mut cursor)?;
                let ephemeral_pub_key = decode_bytes_32(data, &mut cursor)?;
                need_bytes(data, cursor, 14)?;
                let mut nonce = [0u8; 12];
                nonce.copy_from_slice(&data[cursor..cursor + 12]);
                let ct_len = u16::from_be_bytes([data[cursor + 12], data[cursor + 13]]) as usize;
                cursor += 14;
                if ct_len > MAX_FRIEND_INVITE_CIPHERTEXT {
                    return Err(ProtoError::PacketTooLarge(ct_len));
                }
                need_bytes(data, cursor, ct_len)?;
                Ok(SignalingPacket::C2sRelayFriendInvite {
                    sender_pub_key,
                    target_pub_key,
                    ephemeral_pub_key,
                    nonce,
                    ciphertext: data[cursor..cursor + ct_len].to_vec(),
                })
            }
            0x74 => {
                let sender_pub_key = decode_bytes_32(data, &mut cursor)?;
                let ephemeral_pub_key = decode_bytes_32(data, &mut cursor)?;
                need_bytes(data, cursor, 14)?;
                let mut nonce = [0u8; 12];
                nonce.copy_from_slice(&data[cursor..cursor + 12]);
                let ct_len = u16::from_be_bytes([data[cursor + 12], data[cursor + 13]]) as usize;
                cursor += 14;
                if ct_len > MAX_FRIEND_INVITE_CIPHERTEXT {
                    return Err(ProtoError::PacketTooLarge(ct_len));
                }
                need_bytes(data, cursor, ct_len)?;
                Ok(SignalingPacket::S2cIncomingFriendInvite {
                    sender_pub_key,
                    ephemeral_pub_key,
                    nonce,
                    ciphertext: data[cursor..cursor + ct_len].to_vec(),
                })
            }
            0x75 => {
                need_bytes(data, cursor, 52)?;
                let mut room_id = [0u8; 16];
                room_id.copy_from_slice(&data[cursor..cursor + 16]);
                cursor += 16;
                let host_pub_key = decode_bytes_32(data, &mut cursor)?;
                let code = u32::from_be_bytes([
                    data[cursor],
                    data[cursor + 1],
                    data[cursor + 2],
                    data[cursor + 3],
                ]);
                if code > SHORT_CODE_MAX {
                    return Err(ProtoError::InvalidHeader);
                }
                Ok(SignalingPacket::C2sRegisterShortCode {
                    room_id,
                    host_pub_key,
                    code,
                })
            }
            0x76 => {
                need_bytes(data, cursor, 6)?;
                let code = u32::from_be_bytes([
                    data[cursor],
                    data[cursor + 1],
                    data[cursor + 2],
                    data[cursor + 3],
                ]);
                let expires_in_secs = u16::from_be_bytes([data[cursor + 4], data[cursor + 5]]);
                if code > SHORT_CODE_MAX {
                    return Err(ProtoError::InvalidHeader);
                }
                Ok(SignalingPacket::S2cShortCodeRegistered {
                    code,
                    expires_in_secs,
                })
            }
            0x77 => {
                need_bytes(data, cursor, 4)?;
                let code = u32::from_be_bytes([
                    data[cursor],
                    data[cursor + 1],
                    data[cursor + 2],
                    data[cursor + 3],
                ]);
                cursor += 4;
                if code > SHORT_CODE_MAX {
                    return Err(ProtoError::InvalidHeader);
                }
                let joiner_pub_key = decode_bytes_32(data, &mut cursor)?;
                let joiner_lan_addrs = decode_lan_addrs(data, &mut cursor)?;
                need_bytes(data, cursor, 1)?;
                let msg_len = data[cursor] as usize;
                cursor += 1;
                if msg_len == 0 || msg_len > MAX_SPAKE2_MSG_LEN {
                    return Err(ProtoError::InvalidHeader);
                }
                need_bytes(data, cursor, msg_len)?;
                Ok(SignalingPacket::C2sResolveShortCode {
                    code,
                    joiner_pub_key,
                    joiner_lan_addrs,
                    spake2_msg_a: data[cursor..cursor + msg_len].to_vec(),
                })
            }
            0x78 => {
                need_bytes(data, cursor, 4)?;
                let code = u32::from_be_bytes([
                    data[cursor],
                    data[cursor + 1],
                    data[cursor + 2],
                    data[cursor + 3],
                ]);
                cursor += 4;
                if code > SHORT_CODE_MAX {
                    return Err(ProtoError::InvalidHeader);
                }
                let joiner_pub_key = decode_bytes_32(data, &mut cursor)?;
                let joiner_public_addr = decode_socket_addr(data, &mut cursor)?;
                let joiner_lan_addrs = decode_lan_addrs(data, &mut cursor)?;
                need_bytes(data, cursor, 1)?;
                let msg_len = data[cursor] as usize;
                cursor += 1;
                if msg_len == 0 || msg_len > MAX_SPAKE2_MSG_LEN {
                    return Err(ProtoError::InvalidHeader);
                }
                need_bytes(data, cursor, msg_len)?;
                Ok(SignalingPacket::S2cShortCodeChallenge {
                    code,
                    joiner_pub_key,
                    joiner_public_addr,
                    joiner_lan_addrs,
                    spake2_msg_a: data[cursor..cursor + msg_len].to_vec(),
                })
            }
            0x79 => {
                need_bytes(data, cursor, 4)?;
                let code = u32::from_be_bytes([
                    data[cursor],
                    data[cursor + 1],
                    data[cursor + 2],
                    data[cursor + 3],
                ]);
                cursor += 4;
                if code > SHORT_CODE_MAX {
                    return Err(ProtoError::InvalidHeader);
                }
                let host_pub_key = decode_bytes_32(data, &mut cursor)?;
                let host_public_addr = decode_socket_addr(data, &mut cursor)?;
                let host_lan_addrs = decode_lan_addrs(data, &mut cursor)?;
                Ok(SignalingPacket::S2cShortCodeJoinerInfo {
                    code,
                    host_pub_key,
                    host_public_addr,
                    host_lan_addrs,
                })
            }
            other => Err(ProtoError::UnknownOpcode(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extended_signaling_packets_roundtrip() {
        let packets = vec![
            SignalingPacket::S2cError {
                code: SignalingErrorCode::FriendOffline,
            },
            SignalingPacket::S2cError {
                code: SignalingErrorCode::InvalidShortCode,
            },
            SignalingPacket::C2sPresencePing {
                pub_key: [0x11; 32],
                nickname: " \u{202E}Aliver ".to_string(),
                in_room: true,
            },
            SignalingPacket::C2sQueryFriends {
                friend_pub_keys: vec![[0x22; 32], [0x33; 32]],
            },
            SignalingPacket::S2cFriendsStatus {
                online_friends: vec![
                    FriendPresenceInfo {
                        pub_key: [0x22; 32],
                        nickname: "FriendOne".to_string(),
                        in_room: false,
                    },
                    FriendPresenceInfo {
                        pub_key: [0x33; 32],
                        nickname: "FriendTwo".to_string(),
                        in_room: true,
                    },
                ],
            },
            SignalingPacket::C2sRelayFriendInvite {
                sender_pub_key: [0x11; 32],
                target_pub_key: [0x22; 32],
                ephemeral_pub_key: [0x44; 32],
                nonce: [0x55; 12],
                ciphertext: vec![0xAB; 84],
            },
            SignalingPacket::S2cIncomingFriendInvite {
                sender_pub_key: [0x11; 32],
                ephemeral_pub_key: [0x44; 32],
                nonce: [0x55; 12],
                ciphertext: vec![0xAB; 84],
            },
            SignalingPacket::C2sRegisterShortCode {
                room_id: [0x77; 16],
                host_pub_key: [0x11; 32],
                code: 482_910,
            },
            SignalingPacket::S2cShortCodeRegistered {
                code: 482_910,
                expires_in_secs: 600,
            },
            SignalingPacket::C2sResolveShortCode {
                code: 482_910,
                joiner_pub_key: [0x22; 32],
                joiner_lan_addrs: vec![SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 20), 40000)],
                spake2_msg_a: vec![0x99; 33],
            },
            SignalingPacket::S2cShortCodeChallenge {
                code: 482_910,
                joiner_pub_key: [0x22; 32],
                joiner_public_addr: "203.0.113.5:50001".parse().unwrap(),
                joiner_lan_addrs: vec![SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 20), 40000)],
                spake2_msg_a: vec![0x99; 33],
            },
            SignalingPacket::S2cShortCodeJoinerInfo {
                code: 482_910,
                host_pub_key: [0x11; 32],
                host_public_addr: "198.51.100.8:50002".parse().unwrap(),
                host_lan_addrs: vec![SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 5), 50002)],
            },
        ];

        for pkt in packets {
            let encoded = pkt.encode().unwrap();
            assert!(encoded.len() <= MAX_UDP_PAYLOAD);
            let decoded = SignalingPacket::decode(&encoded).unwrap();
            match (&pkt, &decoded) {
                (
                    SignalingPacket::C2sPresencePing {
                        pub_key, in_room, ..
                    },
                    SignalingPacket::C2sPresencePing {
                        pub_key: d_pub,
                        nickname: d_nick,
                        in_room: d_room,
                    },
                ) => {
                    assert_eq!(pub_key, d_pub);
                    assert_eq!(in_room, d_room);
                    assert_eq!(d_nick, "Aliver");
                }
                _ => assert_eq!(pkt, decoded),
            }
        }

        assert!(SignalingPacket::C2sRegisterShortCode {
            room_id: [0; 16],
            host_pub_key: [0; 32],
            code: 1_000_000,
        }
        .encode()
        .is_err());

        assert!(SignalingPacket::C2sQueryFriends {
            friend_pub_keys: vec![[0u8; 32]; MAX_QUERY_FRIENDS_PER_PACKET + 1],
        }
        .encode()
        .is_err());
    }
}
