use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4};
use thiserror::Error;

pub const MAGIC: [u8; 2] = [0x53, 0x51]; // "SQ"
pub const PROTO_VERSION: u8 = 0x01;
pub const MAX_ROOM_PEERS: usize = 10;
pub const MAX_NICKNAME_LEN: usize = 32;
pub const MAX_LAN_ADDRS: usize = 4;
pub const MAX_UDP_PAYLOAD: usize = 1200;
pub const P2P_HEADER_LEN: usize = 18;
pub const MAX_CHAT_TEXT_BYTES: usize = 900;
pub const MAX_BLOB_FILENAME_BYTES: usize = 128;
pub const BLOB_CHUNK_SIZE: usize = 1024;
pub const INLINE_IMAGE_MAX_BYTES: usize = 200 * 1024;

pub type RoomId = [u8; 16];
pub type ClientUuid = [u8; 16];
pub type PeerId = u16;

#[derive(Debug, Error)]
pub enum ProtoError {
    #[error("packet too short: expected at least {expected} bytes, got {actual}")]
    PacketTooShort { expected: usize, actual: usize },
    #[error("packet exceeds max UDP payload ({0} bytes)")]
    PacketTooLarge(usize),
    #[error("invalid magic bytes or protocol version")]
    InvalidHeader,
    #[error("unknown message opcode: 0x{0:02x}")]
    UnknownOpcode(u8),
    #[error("invalid UTF-8 string in packet")]
    InvalidUtf8,
    #[error("AEAD authentication or decryption failed")]
    CryptoFailure,
    #[error("invalid invite URI: {0}")]
    InvalidInvite(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SignalingErrorCode {
    RoomFull = 1,
    RateLimited = 2,
    InvalidPacket = 3,
}

impl SignalingErrorCode {
    pub fn from_u8(val: u8) -> Result<Self, ProtoError> {
        match val {
            1 => Ok(Self::RoomFull),
            2 => Ok(Self::RateLimited),
            3 => Ok(Self::InvalidPacket),
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

                let count = lan_addrs.len().min(MAX_LAN_ADDRS);
                buf.push(count as u8);
                for addr in lan_addrs.iter().take(count) {
                    buf.extend_from_slice(&addr.ip().octets());
                    buf.extend_from_slice(&addr.port().to_be_bytes());
                }
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
                    match peer.public_addr.ip() {
                        IpAddr::V4(ip4) => {
                            buf.push(4);
                            buf.extend_from_slice(&ip4.octets());
                        }
                        IpAddr::V6(ip6) => {
                            buf.push(6);
                            buf.extend_from_slice(&ip6.octets());
                        }
                    }
                    buf.extend_from_slice(&peer.public_addr.port().to_be_bytes());

                    let lan_count = peer.lan_addrs.len().min(MAX_LAN_ADDRS);
                    buf.push(lan_count as u8);
                    for lan in peer.lan_addrs.iter().take(lan_count) {
                        buf.extend_from_slice(&lan.ip().octets());
                        buf.extend_from_slice(&lan.port().to_be_bytes());
                    }

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
        }

        if buf.len() > MAX_UDP_PAYLOAD {
            return Err(ProtoError::PacketTooLarge(buf.len()));
        }
        Ok(buf)
    }

    pub fn decode(data: &[u8]) -> Result<Self, ProtoError> {
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
                need_bytes(data, cursor, 33)?;
                let mut room_id = [0u8; 16];
                room_id.copy_from_slice(&data[cursor..cursor + 16]);
                cursor += 16;

                let mut client_uuid = [0u8; 16];
                client_uuid.copy_from_slice(&data[cursor..cursor + 16]);
                cursor += 16;

                let nick_len = data[cursor] as usize;
                cursor += 1;
                if nick_len > MAX_NICKNAME_LEN {
                    return Err(ProtoError::InvalidHeader);
                }
                need_bytes(data, cursor, nick_len + 1)?;
                let nickname = std::str::from_utf8(&data[cursor..cursor + nick_len])
                    .map_err(|_| ProtoError::InvalidUtf8)?
                    .to_string();
                cursor += nick_len;

                let lan_count = data[cursor] as usize;
                cursor += 1;
                if lan_count > MAX_LAN_ADDRS {
                    return Err(ProtoError::InvalidHeader);
                }
                need_bytes(data, cursor, lan_count * 6)?;
                let mut lan_addrs = Vec::with_capacity(lan_count);
                for _ in 0..lan_count {
                    let ip = Ipv4Addr::new(
                        data[cursor],
                        data[cursor + 1],
                        data[cursor + 2],
                        data[cursor + 3],
                    );
                    let port = u16::from_be_bytes([data[cursor + 4], data[cursor + 5]]);
                    lan_addrs.push(SocketAddrV4::new(ip, port));
                    cursor += 6;
                }

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
                    need_bytes(data, cursor, 19)?;
                    let peer_id = u16::from_be_bytes([data[cursor], data[cursor + 1]]);
                    cursor += 2;

                    let mut client_uuid = [0u8; 16];
                    client_uuid.copy_from_slice(&data[cursor..cursor + 16]);
                    cursor += 16;

                    let family = data[cursor];
                    cursor += 1;

                    let ip = match family {
                        4 => {
                            need_bytes(data, cursor, 4)?;
                            let ip4 = Ipv4Addr::new(
                                data[cursor],
                                data[cursor + 1],
                                data[cursor + 2],
                                data[cursor + 3],
                            );
                            cursor += 4;
                            IpAddr::V4(ip4)
                        }
                        6 => {
                            need_bytes(data, cursor, 16)?;
                            let mut octets = [0u8; 16];
                            octets.copy_from_slice(&data[cursor..cursor + 16]);
                            cursor += 16;
                            IpAddr::V6(Ipv6Addr::from(octets))
                        }
                        _ => return Err(ProtoError::InvalidHeader),
                    };

                    need_bytes(data, cursor, 3)?;
                    let port = u16::from_be_bytes([data[cursor], data[cursor + 1]]);
                    let public_addr = SocketAddr::new(ip, port);
                    let lan_count = data[cursor + 2] as usize;
                    cursor += 3;

                    if lan_count > MAX_LAN_ADDRS {
                        return Err(ProtoError::InvalidHeader);
                    }
                    need_bytes(data, cursor, lan_count * 6 + 1)?;
                    let mut lan_addrs = Vec::with_capacity(lan_count);
                    for _ in 0..lan_count {
                        let lip = Ipv4Addr::new(
                            data[cursor],
                            data[cursor + 1],
                            data[cursor + 2],
                            data[cursor + 3],
                        );
                        let lport = u16::from_be_bytes([data[cursor + 4], data[cursor + 5]]);
                        lan_addrs.push(SocketAddrV4::new(lip, lport));
                        cursor += 6;
                    }

                    let nick_len = data[cursor] as usize;
                    cursor += 1;
                    if nick_len > MAX_NICKNAME_LEN {
                        return Err(ProtoError::InvalidHeader);
                    }
                    need_bytes(data, cursor, nick_len)?;
                    let nickname = std::str::from_utf8(&data[cursor..cursor + nick_len])
                        .map_err(|_| ProtoError::InvalidUtf8)?
                        .to_string();
                    cursor += nick_len;

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
            other => Err(ProtoError::UnknownOpcode(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum P2pMsgType {
    PunchSyn = 0x10,
    PunchAck = 0x11,
    Ping = 0x12,
    Pong = 0x13,
    AudioOpus = 0x20,
    AudioDtxKeepalive = 0x21,
    ChatText = 0x30,
    ChatAck = 0x31,
    HistorySyncReq = 0x32,
    BlobOffer = 0x40,
    BlobChunkReq = 0x41,
    BlobChunkData = 0x42,
    BlobHave = 0x43,
}

impl P2pMsgType {
    pub fn from_u8(val: u8) -> Result<Self, ProtoError> {
        match val {
            0x10 => Ok(Self::PunchSyn),
            0x11 => Ok(Self::PunchAck),
            0x12 => Ok(Self::Ping),
            0x13 => Ok(Self::Pong),
            0x20 => Ok(Self::AudioOpus),
            0x21 => Ok(Self::AudioDtxKeepalive),
            0x30 => Ok(Self::ChatText),
            0x31 => Ok(Self::ChatAck),
            0x32 => Ok(Self::HistorySyncReq),
            0x40 => Ok(Self::BlobOffer),
            0x41 => Ok(Self::BlobChunkReq),
            0x42 => Ok(Self::BlobChunkData),
            0x43 => Ok(Self::BlobHave),
            other => Err(ProtoError::UnknownOpcode(other)),
        }
    }
}

/// 18-byte cleartext header used as AEAD Additional Authenticated Data (AAD).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2pHeader {
    pub msg_type: P2pMsgType,
    pub room_prefix: [u8; 4],
    pub sender_peer_id: PeerId,
    pub seq_num: u64,
}

impl P2pHeader {
    #[inline]
    pub fn encode(&self) -> [u8; P2P_HEADER_LEN] {
        let mut out = [0u8; P2P_HEADER_LEN];
        out[0..2].copy_from_slice(&MAGIC);
        out[2] = PROTO_VERSION;
        out[3] = self.msg_type as u8;
        out[4..8].copy_from_slice(&self.room_prefix);
        out[8..10].copy_from_slice(&self.sender_peer_id.to_be_bytes());
        out[10..18].copy_from_slice(&self.seq_num.to_be_bytes());
        out
    }

    #[inline]
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtoError> {
        if bytes.len() < P2P_HEADER_LEN {
            return Err(ProtoError::PacketTooShort {
                expected: P2P_HEADER_LEN,
                actual: bytes.len(),
            });
        }
        if bytes[0..2] != MAGIC || bytes[2] != PROTO_VERSION {
            return Err(ProtoError::InvalidHeader);
        }
        let msg_type = P2pMsgType::from_u8(bytes[3])?;
        let mut room_prefix = [0u8; 4];
        room_prefix.copy_from_slice(&bytes[4..8]);
        let sender_peer_id = u16::from_be_bytes([bytes[8], bytes[9]]);
        let seq_num = u64::from_be_bytes([
            bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15], bytes[16], bytes[17],
        ]);
        Ok(Self {
            msg_type,
            room_prefix,
            sender_peer_id,
            seq_num,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum P2pPayload {
    PunchSyn {
        client_uuid: ClientUuid,
        timestamp_us: u64,
    },
    PunchAck {
        client_uuid: ClientUuid,
        echo_timestamp_us: u64,
    },
    Ping {
        timestamp_us: u64,
    },
    Pong {
        echo_timestamp_us: u64,
    },
    AudioOpus {
        opus_data: Vec<u8>,
    },
    AudioDtxKeepalive,
    ChatText {
        msg_id: u64,
        timestamp_ms: u64,
        text: String,
    },
    ChatAck {
        msg_id: u64,
    },
    HistorySyncReq,
    BlobOffer {
        blob_id: [u8; 16],
        msg_id: u64,
        timestamp_ms: u64,
        kind: u8,
        auto_download: bool,
        filename: String,
        total_size: u64,
        chunk_size: u16,
        total_chunks: u32,
        blake3_root: [u8; 32],
    },
    BlobChunkReq {
        blob_id: [u8; 16],
        start_chunk: u32,
        bitmask: u64,
    },
    BlobChunkData {
        blob_id: [u8; 16],
        chunk_index: u32,
        data: Vec<u8>,
    },
    BlobHave {
        blob_id: [u8; 16],
    },
}

impl P2pPayload {
    pub fn msg_type(&self) -> P2pMsgType {
        match self {
            P2pPayload::PunchSyn { .. } => P2pMsgType::PunchSyn,
            P2pPayload::PunchAck { .. } => P2pMsgType::PunchAck,
            P2pPayload::Ping { .. } => P2pMsgType::Ping,
            P2pPayload::Pong { .. } => P2pMsgType::Pong,
            P2pPayload::AudioOpus { .. } => P2pMsgType::AudioOpus,
            P2pPayload::AudioDtxKeepalive => P2pMsgType::AudioDtxKeepalive,
            P2pPayload::ChatText { .. } => P2pMsgType::ChatText,
            P2pPayload::ChatAck { .. } => P2pMsgType::ChatAck,
            P2pPayload::HistorySyncReq => P2pMsgType::HistorySyncReq,
            P2pPayload::BlobOffer { .. } => P2pMsgType::BlobOffer,
            P2pPayload::BlobChunkReq { .. } => P2pMsgType::BlobChunkReq,
            P2pPayload::BlobChunkData { .. } => P2pMsgType::BlobChunkData,
            P2pPayload::BlobHave { .. } => P2pMsgType::BlobHave,
        }
    }

    pub fn encode_body(&self, out: &mut Vec<u8>) -> Result<(), ProtoError> {
        match self {
            P2pPayload::PunchSyn {
                client_uuid,
                timestamp_us,
            } => {
                out.extend_from_slice(client_uuid);
                out.extend_from_slice(&timestamp_us.to_be_bytes());
            }
            P2pPayload::PunchAck {
                client_uuid,
                echo_timestamp_us,
            } => {
                out.extend_from_slice(client_uuid);
                out.extend_from_slice(&echo_timestamp_us.to_be_bytes());
            }
            P2pPayload::Ping { timestamp_us } => {
                out.extend_from_slice(&timestamp_us.to_be_bytes());
            }
            P2pPayload::Pong { echo_timestamp_us } => {
                out.extend_from_slice(&echo_timestamp_us.to_be_bytes());
            }
            P2pPayload::AudioOpus { opus_data } => {
                if opus_data.len() > 1024 {
                    return Err(ProtoError::PacketTooLarge(opus_data.len()));
                }
                out.extend_from_slice(&(opus_data.len() as u16).to_be_bytes());
                out.extend_from_slice(opus_data);
            }
            P2pPayload::AudioDtxKeepalive => {}
            P2pPayload::ChatText {
                msg_id,
                timestamp_ms,
                text,
            } => {
                let text_bytes = truncate_utf8(text, MAX_CHAT_TEXT_BYTES);
                out.extend_from_slice(&msg_id.to_be_bytes());
                out.extend_from_slice(&timestamp_ms.to_be_bytes());
                out.extend_from_slice(&(text_bytes.len() as u16).to_be_bytes());
                out.extend_from_slice(text_bytes);
            }
            P2pPayload::ChatAck { msg_id } => {
                out.extend_from_slice(&msg_id.to_be_bytes());
            }
            P2pPayload::HistorySyncReq => {}
            P2pPayload::BlobOffer {
                blob_id,
                msg_id,
                timestamp_ms,
                kind,
                auto_download,
                filename,
                total_size,
                chunk_size,
                total_chunks,
                blake3_root,
            } => {
                let name_bytes = truncate_utf8(filename, MAX_BLOB_FILENAME_BYTES);
                out.extend_from_slice(blob_id);
                out.extend_from_slice(&msg_id.to_be_bytes());
                out.extend_from_slice(&timestamp_ms.to_be_bytes());
                out.push(*kind);
                out.push(u8::from(*auto_download));
                out.push(name_bytes.len() as u8);
                out.extend_from_slice(name_bytes);
                out.extend_from_slice(&total_size.to_be_bytes());
                out.extend_from_slice(&chunk_size.to_be_bytes());
                out.extend_from_slice(&total_chunks.to_be_bytes());
                out.extend_from_slice(blake3_root);
            }
            P2pPayload::BlobChunkReq {
                blob_id,
                start_chunk,
                bitmask,
            } => {
                out.extend_from_slice(blob_id);
                out.extend_from_slice(&start_chunk.to_be_bytes());
                out.extend_from_slice(&bitmask.to_be_bytes());
            }
            P2pPayload::BlobChunkData {
                blob_id,
                chunk_index,
                data,
            } => {
                if data.len() > BLOB_CHUNK_SIZE {
                    return Err(ProtoError::PacketTooLarge(data.len()));
                }
                out.extend_from_slice(blob_id);
                out.extend_from_slice(&chunk_index.to_be_bytes());
                out.extend_from_slice(&(data.len() as u16).to_be_bytes());
                out.extend_from_slice(data);
            }
            P2pPayload::BlobHave { blob_id } => {
                out.extend_from_slice(blob_id);
            }
        }
        Ok(())
    }

    pub fn decode_body(msg_type: P2pMsgType, body: &[u8]) -> Result<Self, ProtoError> {
        match msg_type {
            P2pMsgType::PunchSyn => {
                need_bytes(body, 0, 24)?;
                let mut client_uuid = [0u8; 16];
                client_uuid.copy_from_slice(&body[0..16]);
                let timestamp_us = u64::from_be_bytes(body[16..24].try_into().unwrap());
                Ok(P2pPayload::PunchSyn {
                    client_uuid,
                    timestamp_us,
                })
            }
            P2pMsgType::PunchAck => {
                need_bytes(body, 0, 24)?;
                let mut client_uuid = [0u8; 16];
                client_uuid.copy_from_slice(&body[0..16]);
                let echo_timestamp_us = u64::from_be_bytes(body[16..24].try_into().unwrap());
                Ok(P2pPayload::PunchAck {
                    client_uuid,
                    echo_timestamp_us,
                })
            }
            P2pMsgType::Ping => {
                need_bytes(body, 0, 8)?;
                let timestamp_us = u64::from_be_bytes(body[0..8].try_into().unwrap());
                Ok(P2pPayload::Ping { timestamp_us })
            }
            P2pMsgType::Pong => {
                need_bytes(body, 0, 8)?;
                let echo_timestamp_us = u64::from_be_bytes(body[0..8].try_into().unwrap());
                Ok(P2pPayload::Pong { echo_timestamp_us })
            }
            P2pMsgType::AudioOpus => {
                need_bytes(body, 0, 2)?;
                let len = u16::from_be_bytes([body[0], body[1]]) as usize;
                need_bytes(body, 2, len)?;
                Ok(P2pPayload::AudioOpus {
                    opus_data: body[2..2 + len].to_vec(),
                })
            }
            P2pMsgType::AudioDtxKeepalive => Ok(P2pPayload::AudioDtxKeepalive),
            P2pMsgType::ChatText => {
                need_bytes(body, 0, 18)?;
                let msg_id = u64::from_be_bytes(body[0..8].try_into().unwrap());
                let timestamp_ms = u64::from_be_bytes(body[8..16].try_into().unwrap());
                let len = u16::from_be_bytes([body[16], body[17]]) as usize;
                if len > MAX_CHAT_TEXT_BYTES {
                    return Err(ProtoError::PacketTooLarge(len));
                }
                need_bytes(body, 18, len)?;
                let text = std::str::from_utf8(&body[18..18 + len])
                    .map_err(|_| ProtoError::InvalidUtf8)?
                    .to_string();
                Ok(P2pPayload::ChatText {
                    msg_id,
                    timestamp_ms,
                    text,
                })
            }
            P2pMsgType::ChatAck => {
                need_bytes(body, 0, 8)?;
                let msg_id = u64::from_be_bytes(body[0..8].try_into().unwrap());
                Ok(P2pPayload::ChatAck { msg_id })
            }
            P2pMsgType::HistorySyncReq => Ok(P2pPayload::HistorySyncReq),
            P2pMsgType::BlobOffer => {
                need_bytes(body, 0, 35)?;
                let mut blob_id = [0u8; 16];
                blob_id.copy_from_slice(&body[0..16]);
                let msg_id = u64::from_be_bytes(body[16..24].try_into().unwrap());
                let timestamp_ms = u64::from_be_bytes(body[24..32].try_into().unwrap());
                let kind = body[32];
                let auto_download = body[33] != 0;
                let name_len = body[34] as usize;
                if name_len > MAX_BLOB_FILENAME_BYTES {
                    return Err(ProtoError::PacketTooLarge(name_len));
                }
                need_bytes(body, 35, name_len + 46)?;
                let name_end = 35 + name_len;
                let filename = std::str::from_utf8(&body[35..name_end])
                    .map_err(|_| ProtoError::InvalidUtf8)?
                    .to_string();
                let total_size = u64::from_be_bytes(body[name_end..name_end + 8].try_into().unwrap());
                let chunk_size =
                    u16::from_be_bytes(body[name_end + 8..name_end + 10].try_into().unwrap());
                let total_chunks =
                    u32::from_be_bytes(body[name_end + 10..name_end + 14].try_into().unwrap());
                let mut blake3_root = [0u8; 32];
                blake3_root.copy_from_slice(&body[name_end + 14..name_end + 46]);
                Ok(P2pPayload::BlobOffer {
                    blob_id,
                    msg_id,
                    timestamp_ms,
                    kind,
                    auto_download,
                    filename,
                    total_size,
                    chunk_size,
                    total_chunks,
                    blake3_root,
                })
            }
            P2pMsgType::BlobChunkReq => {
                need_bytes(body, 0, 28)?;
                let mut blob_id = [0u8; 16];
                blob_id.copy_from_slice(&body[0..16]);
                let start_chunk = u32::from_be_bytes(body[16..20].try_into().unwrap());
                let bitmask = u64::from_be_bytes(body[20..28].try_into().unwrap());
                Ok(P2pPayload::BlobChunkReq {
                    blob_id,
                    start_chunk,
                    bitmask,
                })
            }
            P2pMsgType::BlobChunkData => {
                need_bytes(body, 0, 22)?;
                let mut blob_id = [0u8; 16];
                blob_id.copy_from_slice(&body[0..16]);
                let chunk_index = u32::from_be_bytes(body[16..20].try_into().unwrap());
                let len = u16::from_be_bytes([body[20], body[21]]) as usize;
                if len > BLOB_CHUNK_SIZE {
                    return Err(ProtoError::PacketTooLarge(len));
                }
                need_bytes(body, 22, len)?;
                Ok(P2pPayload::BlobChunkData {
                    blob_id,
                    chunk_index,
                    data: body[22..22 + len].to_vec(),
                })
            }
            P2pMsgType::BlobHave => {
                need_bytes(body, 0, 16)?;
                let mut blob_id = [0u8; 16];
                blob_id.copy_from_slice(&body[0..16]);
                Ok(P2pPayload::BlobHave { blob_id })
            }
        }
    }
}

#[inline]
fn need_bytes(buf: &[u8], offset: usize, count: usize) -> Result<(), ProtoError> {
    if buf.len() < offset + count {
        Err(ProtoError::PacketTooShort {
            expected: offset + count,
            actual: buf.len(),
        })
    } else {
        Ok(())
    }
}

fn truncate_utf8(s: &str, max_bytes: usize) -> &[u8] {
    if s.len() <= max_bytes {
        return s.as_bytes();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s.as_bytes()[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signaling_packets_roundtrip() {
        let reg = SignalingPacket::C2sRegister {
            room_id: [0x11; 16],
            client_uuid: [0x22; 16],
            nickname: "SwiftFalcon-742".to_string(),
            lan_addrs: vec![SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 50), 54321)],
        };
        let encoded = reg.encode().unwrap();
        let decoded = SignalingPacket::decode(&encoded).unwrap();
        assert_eq!(reg, decoded);

        let state = SignalingPacket::S2cRoomState {
            assigned_peer_id: 2,
            peers: vec![PeerRecord {
                peer_id: 1,
                client_uuid: [0xAA; 16],
                public_addr: "203.0.113.10:61000".parse().unwrap(),
                lan_addrs: vec!["10.0.0.15:61000".parse().unwrap()],
                nickname: "AlphaWolf-101".to_string(),
            }],
        };
        let encoded_state = state.encode().unwrap();
        let decoded_state = SignalingPacket::decode(&encoded_state).unwrap();
        assert_eq!(state, decoded_state);
    }

    #[test]
    fn test_p2p_chat_and_blob_packets_roundtrip_and_cipher() {
        use crate::crypto::RoomCipher;

        let cipher = RoomCipher::from_secret(&[0x42; 32]);
        let room_prefix = [0xAA, 0xBB, 0xCC, 0xDD];

        let long_text = "Привет мир! ".repeat(120);
        let chat_text = P2pPayload::ChatText {
            msg_id: 9001,
            timestamp_ms: 1_700_000_001_234,
            text: long_text.clone(),
        };

        let mut packets = vec![
            (
                1u64,
                P2pPayload::ChatAck { msg_id: 9001 },
            ),
            (2, P2pPayload::HistorySyncReq),
            (
                3,
                P2pPayload::BlobOffer {
                    blob_id: [0x11; 16],
                    msg_id: 9002,
                    timestamp_ms: 1_700_000_002_000,
                    kind: 1,
                    auto_download: true,
                    filename: "screenshot_very_long_".repeat(10) + ".png",
                    total_size: 120_000,
                    chunk_size: BLOB_CHUNK_SIZE as u16,
                    total_chunks: 118,
                    blake3_root: [0x99; 32],
                },
            ),
            (
                4,
                P2pPayload::BlobChunkReq {
                    blob_id: [0x11; 16],
                    start_chunk: 64,
                    bitmask: 0xFFFF_0000_0000_000F,
                },
            ),
            (
                5,
                P2pPayload::BlobChunkData {
                    blob_id: [0x11; 16],
                    chunk_index: 64,
                    data: vec![0x5Au8; BLOB_CHUNK_SIZE],
                },
            ),
            (6, P2pPayload::BlobHave { blob_id: [0x11; 16] }),
        ];
        packets.insert(0, (0, chat_text));

        for (seq, payload) in packets {
            let mut body = Vec::new();
            payload.encode_body(&mut body).unwrap();
            let decoded_body = P2pPayload::decode_body(payload.msg_type(), &body).unwrap();

            match (&payload, &decoded_body) {
                (
                    P2pPayload::ChatText { msg_id, timestamp_ms, .. },
                    P2pPayload::ChatText { msg_id: d_id, timestamp_ms: d_ts, text: d_text },
                ) => {
                    assert_eq!(msg_id, d_id);
                    assert_eq!(timestamp_ms, d_ts);
                    assert!(d_text.len() <= MAX_CHAT_TEXT_BYTES);
                    assert!(long_text.starts_with(d_text));
                }
                (
                    P2pPayload::BlobOffer { filename, .. },
                    P2pPayload::BlobOffer { filename: d_name, .. },
                ) => {
                    assert!(d_name.len() <= MAX_BLOB_FILENAME_BYTES);
                    assert!(filename.starts_with(d_name));
                }
                _ => assert_eq!(payload, decoded_body),
            }

            let header = P2pHeader {
                msg_type: payload.msg_type(),
                room_prefix,
                sender_peer_id: 1,
                seq_num: seq,
            };
            let mut sealed = Vec::with_capacity(MAX_UDP_PAYLOAD);
            cipher.seal_packet(&header, &payload, &mut sealed).unwrap();
            assert!(sealed.len() <= MAX_UDP_PAYLOAD);
            let (hdr, opened_payload) = cipher.open_packet(&mut sealed).unwrap();
            assert_eq!(hdr.msg_type, payload.msg_type());
            assert_eq!(hdr.seq_num, seq);
            assert_eq!(opened_payload, decoded_body);
        }
    }
}