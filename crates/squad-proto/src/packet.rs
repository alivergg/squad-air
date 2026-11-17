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
pub const MAX_STREAM_TITLE_BYTES: usize = 64;
pub const MAX_ENCODER_BACKEND_BYTES: usize = 48;
pub const MAX_STREAM_ERROR_BYTES: usize = 64;
pub const MAX_VIDEO_FRAG_BYTES: usize = 1100;
pub const VIDEO_FLAG_KEYFRAME: u8 = 0x01;
pub const VIDEO_FLAG_XOR_FEC: u8 = 0x02;
pub const OPCODE_RELAY: u8 = 0x05;
pub const RELAY_HEADER_LEN: usize = 22;
pub const MAX_RELAY_UDP_PAYLOAD: usize = MAX_UDP_PAYLOAD + RELAY_HEADER_LEN;

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

#[inline]
pub fn is_relay_datagram(datagram: &[u8]) -> bool {
    datagram.len() >= RELAY_HEADER_LEN + P2P_HEADER_LEN
        && datagram[0..2] == MAGIC
        && datagram[2] == PROTO_VERSION
        && datagram[3] == OPCODE_RELAY
}

/// Wraps an already-sealed P2P datagram into a 22-byte relay envelope:
/// `[MAGIC(2), PROTO_VERSION(1), OPCODE_RELAY(1), room_id(16), peer_id(2)]`.
pub fn encode_relay_datagram_into(
    room_id: &RoomId,
    peer_id: PeerId,
    inner_p2p: &[u8],
    out: &mut Vec<u8>,
) -> Result<(), ProtoError> {
    if inner_p2p.len() < P2P_HEADER_LEN {
        return Err(ProtoError::PacketTooShort {
            expected: P2P_HEADER_LEN,
            actual: inner_p2p.len(),
        });
    }
    if inner_p2p.len() > MAX_UDP_PAYLOAD {
        return Err(ProtoError::PacketTooLarge(
            RELAY_HEADER_LEN + inner_p2p.len(),
        ));
    }
    out.clear();
    out.reserve(RELAY_HEADER_LEN + inner_p2p.len());
    out.extend_from_slice(&MAGIC);
    out.push(PROTO_VERSION);
    out.push(OPCODE_RELAY);
    out.extend_from_slice(room_id);
    out.extend_from_slice(&peer_id.to_be_bytes());
    out.extend_from_slice(inner_p2p);
    Ok(())
}

pub fn parse_relay_header(datagram: &[u8]) -> Result<(RoomId, PeerId, &[u8]), ProtoError> {
    if datagram.len() < RELAY_HEADER_LEN + P2P_HEADER_LEN {
        return Err(ProtoError::PacketTooShort {
            expected: RELAY_HEADER_LEN + P2P_HEADER_LEN,
            actual: datagram.len(),
        });
    }
    if datagram.len() > MAX_RELAY_UDP_PAYLOAD {
        return Err(ProtoError::PacketTooLarge(datagram.len()));
    }
    if datagram[0..2] != MAGIC || datagram[2] != PROTO_VERSION || datagram[3] != OPCODE_RELAY {
        return Err(ProtoError::InvalidHeader);
    }
    let mut room_id = [0u8; 16];
    room_id.copy_from_slice(&datagram[4..20]);
    let peer_id = u16::from_be_bytes([datagram[20], datagram[21]]);
    Ok((room_id, peer_id, &datagram[RELAY_HEADER_LEN..]))
}

#[inline]
pub fn rewrite_relay_peer_id(datagram: &mut [u8], new_peer_id: PeerId) {
    if datagram.len() >= RELAY_HEADER_LEN {
        datagram[20..22].copy_from_slice(&new_peer_id.to_be_bytes());
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
    AvatarAnnounce = 0x44,
    StreamAnnounce = 0x50,
    StreamSubscribe = 0x51,
    StreamPli = 0x52,
    VideoFrameFrag = 0x53,
    StreamDecoderError = 0x54,
    StreamAudioOpus = 0x55,
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
            0x44 => Ok(Self::AvatarAnnounce),
            0x50 => Ok(Self::StreamAnnounce),
            0x51 => Ok(Self::StreamSubscribe),
            0x52 => Ok(Self::StreamPli),
            0x53 => Ok(Self::VideoFrameFrag),
            0x54 => Ok(Self::StreamDecoderError),
            0x55 => Ok(Self::StreamAudioOpus),
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
        sender_uuid: ClientUuid,
        sender_nickname: String,
        msg_id: u64,
        timestamp_ms: u64,
        text: String,
    },
    ChatAck {
        msg_id: u64,
    },
    HistorySyncReq,
    BlobOffer {
        sender_uuid: ClientUuid,
        sender_nickname: String,
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
    AvatarAnnounce {
        avatar_hash: [u8; 32],
        total_size: u32,
        total_chunks: u16,
    },
    StreamAnnounce {
        is_streaming: bool,
        codec: u8,
        has_audio: bool,
        width: u16,
        height: u16,
        fps: u8,
        source_title: String,
        encoder_backend: String,
    },
    StreamSubscribe {
        subscribe: bool,
    },
    StreamPli,
    VideoFrameFrag {
        frame_id: u32,
        frag_idx: u16,
        frag_count: u16,
        flags: u8,
        data: Vec<u8>,
    },
    StreamDecoderError {
        failed_codec: u8,
        reason: String,
    },
    StreamAudioOpus {
        opus_data: Vec<u8>,
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
            P2pPayload::AvatarAnnounce { .. } => P2pMsgType::AvatarAnnounce,
            P2pPayload::StreamAnnounce { .. } => P2pMsgType::StreamAnnounce,
            P2pPayload::StreamSubscribe { .. } => P2pMsgType::StreamSubscribe,
            P2pPayload::StreamPli => P2pMsgType::StreamPli,
            P2pPayload::VideoFrameFrag { .. } => P2pMsgType::VideoFrameFrag,
            P2pPayload::StreamDecoderError { .. } => P2pMsgType::StreamDecoderError,
            P2pPayload::StreamAudioOpus { .. } => P2pMsgType::StreamAudioOpus,
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
                sender_uuid,
                sender_nickname,
                msg_id,
                timestamp_ms,
                text,
            } => {
                let nick_bytes = truncate_utf8(sender_nickname, MAX_NICKNAME_LEN);
                let text_bytes = truncate_utf8(text, MAX_CHAT_TEXT_BYTES);
                out.extend_from_slice(sender_uuid);
                out.push(nick_bytes.len() as u8);
                out.extend_from_slice(nick_bytes);
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
                sender_uuid,
                sender_nickname,
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
                let nick_bytes = truncate_utf8(sender_nickname, MAX_NICKNAME_LEN);
                let name_bytes = truncate_utf8(filename, MAX_BLOB_FILENAME_BYTES);
                out.extend_from_slice(sender_uuid);
                out.push(nick_bytes.len() as u8);
                out.extend_from_slice(nick_bytes);
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
            P2pPayload::AvatarAnnounce {
                avatar_hash,
                total_size,
                total_chunks,
            } => {
                out.extend_from_slice(avatar_hash);
                out.extend_from_slice(&total_size.to_be_bytes());
                out.extend_from_slice(&total_chunks.to_be_bytes());
            }
            P2pPayload::StreamAnnounce {
                is_streaming,
                codec,
                has_audio,
                width,
                height,
                fps,
                source_title,
                encoder_backend,
            } => {
                let title_bytes = truncate_utf8(source_title, MAX_STREAM_TITLE_BYTES);
                let backend_bytes = truncate_utf8(encoder_backend, MAX_ENCODER_BACKEND_BYTES);
                out.push(u8::from(*is_streaming));
                out.push(*codec);
                out.push(u8::from(*has_audio));
                out.extend_from_slice(&width.to_be_bytes());
                out.extend_from_slice(&height.to_be_bytes());
                out.push(*fps);
                out.push(title_bytes.len() as u8);
                out.extend_from_slice(title_bytes);
                out.push(backend_bytes.len() as u8);
                out.extend_from_slice(backend_bytes);
            }
            P2pPayload::StreamSubscribe { subscribe } => {
                out.push(u8::from(*subscribe));
            }
            P2pPayload::StreamPli => {}
            P2pPayload::VideoFrameFrag {
                frame_id,
                frag_idx,
                frag_count,
                flags,
                data,
            } => {
                if data.len() > MAX_VIDEO_FRAG_BYTES {
                    return Err(ProtoError::PacketTooLarge(data.len()));
                }
                out.extend_from_slice(&frame_id.to_be_bytes());
                out.extend_from_slice(&frag_idx.to_be_bytes());
                out.extend_from_slice(&frag_count.to_be_bytes());
                out.push(*flags);
                out.extend_from_slice(&(data.len() as u16).to_be_bytes());
                out.extend_from_slice(data);
            }
            P2pPayload::StreamDecoderError {
                failed_codec,
                reason,
            } => {
                let reason_bytes = truncate_utf8(reason, MAX_STREAM_ERROR_BYTES);
                out.push(*failed_codec);
                out.push(reason_bytes.len() as u8);
                out.extend_from_slice(reason_bytes);
            }
            P2pPayload::StreamAudioOpus { opus_data } => {
                if opus_data.len() > 1024 {
                    return Err(ProtoError::PacketTooLarge(opus_data.len()));
                }
                out.extend_from_slice(&(opus_data.len() as u16).to_be_bytes());
                out.extend_from_slice(opus_data);
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
                let (sender_uuid, sender_nickname, off) = decode_sender_meta(body)?;
                need_bytes(body, off, 18)?;
                let msg_id = u64::from_be_bytes(body[off..off + 8].try_into().unwrap());
                let timestamp_ms = u64::from_be_bytes(body[off + 8..off + 16].try_into().unwrap());
                let len = u16::from_be_bytes([body[off + 16], body[off + 17]]) as usize;
                if len > MAX_CHAT_TEXT_BYTES {
                    return Err(ProtoError::PacketTooLarge(len));
                }
                need_bytes(body, off + 18, len)?;
                let text = std::str::from_utf8(&body[off + 18..off + 18 + len])
                    .map_err(|_| ProtoError::InvalidUtf8)?
                    .to_string();
                Ok(P2pPayload::ChatText {
                    sender_uuid,
                    sender_nickname,
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
                let (sender_uuid, sender_nickname, off) = decode_sender_meta(body)?;
                need_bytes(body, off, 35)?;
                let mut blob_id = [0u8; 16];
                blob_id.copy_from_slice(&body[off..off + 16]);
                let msg_id = u64::from_be_bytes(body[off + 16..off + 24].try_into().unwrap());
                let timestamp_ms = u64::from_be_bytes(body[off + 24..off + 32].try_into().unwrap());
                let kind = body[off + 32];
                let auto_download = body[off + 33] != 0;
                let name_len = body[off + 34] as usize;
                if name_len > MAX_BLOB_FILENAME_BYTES {
                    return Err(ProtoError::PacketTooLarge(name_len));
                }
                need_bytes(body, off + 35, name_len + 46)?;
                let name_end = off + 35 + name_len;
                let filename = std::str::from_utf8(&body[off + 35..name_end])
                    .map_err(|_| ProtoError::InvalidUtf8)?
                    .to_string();
                let total_size =
                    u64::from_be_bytes(body[name_end..name_end + 8].try_into().unwrap());
                let chunk_size =
                    u16::from_be_bytes(body[name_end + 8..name_end + 10].try_into().unwrap());
                let total_chunks =
                    u32::from_be_bytes(body[name_end + 10..name_end + 14].try_into().unwrap());
                let mut blake3_root = [0u8; 32];
                blake3_root.copy_from_slice(&body[name_end + 14..name_end + 46]);
                Ok(P2pPayload::BlobOffer {
                    sender_uuid,
                    sender_nickname,
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
            P2pMsgType::AvatarAnnounce => {
                need_bytes(body, 0, 38)?;
                let mut avatar_hash = [0u8; 32];
                avatar_hash.copy_from_slice(&body[0..32]);
                let total_size = u32::from_be_bytes(body[32..36].try_into().unwrap());
                let total_chunks = u16::from_be_bytes([body[36], body[37]]);
                Ok(P2pPayload::AvatarAnnounce {
                    avatar_hash,
                    total_size,
                    total_chunks,
                })
            }
            P2pMsgType::StreamAnnounce => {
                need_bytes(body, 0, 9)?;
                let is_streaming = body[0] != 0;
                let codec = body[1];
                let has_audio = body[2] != 0;
                let width = u16::from_be_bytes([body[3], body[4]]);
                let height = u16::from_be_bytes([body[5], body[6]]);
                let fps = body[7];
                let title_len = body[8] as usize;
                if title_len > MAX_STREAM_TITLE_BYTES {
                    return Err(ProtoError::PacketTooLarge(title_len));
                }
                need_bytes(body, 9, title_len + 1)?;
                let title_end = 9 + title_len;
                let source_title = std::str::from_utf8(&body[9..title_end])
                    .map_err(|_| ProtoError::InvalidUtf8)?
                    .to_string();
                let backend_len = body[title_end] as usize;
                if backend_len > MAX_ENCODER_BACKEND_BYTES {
                    return Err(ProtoError::PacketTooLarge(backend_len));
                }
                let backend_start = title_end + 1;
                need_bytes(body, backend_start, backend_len)?;
                let encoder_backend =
                    std::str::from_utf8(&body[backend_start..backend_start + backend_len])
                        .map_err(|_| ProtoError::InvalidUtf8)?
                        .to_string();
                Ok(P2pPayload::StreamAnnounce {
                    is_streaming,
                    codec,
                    has_audio,
                    width,
                    height,
                    fps,
                    source_title,
                    encoder_backend,
                })
            }
            P2pMsgType::StreamSubscribe => {
                need_bytes(body, 0, 1)?;
                Ok(P2pPayload::StreamSubscribe {
                    subscribe: body[0] != 0,
                })
            }
            P2pMsgType::StreamPli => Ok(P2pPayload::StreamPli),
            P2pMsgType::VideoFrameFrag => {
                need_bytes(body, 0, 11)?;
                let frame_id = u32::from_be_bytes(body[0..4].try_into().unwrap());
                let frag_idx = u16::from_be_bytes(body[4..6].try_into().unwrap());
                let frag_count = u16::from_be_bytes(body[6..8].try_into().unwrap());
                let flags = body[8];
                let data_len = u16::from_be_bytes([body[9], body[10]]) as usize;
                if data_len > MAX_VIDEO_FRAG_BYTES {
                    return Err(ProtoError::PacketTooLarge(data_len));
                }
                need_bytes(body, 11, data_len)?;
                Ok(P2pPayload::VideoFrameFrag {
                    frame_id,
                    frag_idx,
                    frag_count,
                    flags,
                    data: body[11..11 + data_len].to_vec(),
                })
            }
            P2pMsgType::StreamDecoderError => {
                need_bytes(body, 0, 2)?;
                let failed_codec = body[0];
                let reason_len = body[1] as usize;
                if reason_len > MAX_STREAM_ERROR_BYTES {
                    return Err(ProtoError::PacketTooLarge(reason_len));
                }
                need_bytes(body, 2, reason_len)?;
                let reason = std::str::from_utf8(&body[2..2 + reason_len])
                    .map_err(|_| ProtoError::InvalidUtf8)?
                    .to_string();
                Ok(P2pPayload::StreamDecoderError {
                    failed_codec,
                    reason,
                })
            }
            P2pMsgType::StreamAudioOpus => {
                need_bytes(body, 0, 2)?;
                let len = u16::from_be_bytes([body[0], body[1]]) as usize;
                if len > 1024 {
                    return Err(ProtoError::PacketTooLarge(len));
                }
                need_bytes(body, 2, len)?;
                Ok(P2pPayload::StreamAudioOpus {
                    opus_data: body[2..2 + len].to_vec(),
                })
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

fn decode_sender_meta(body: &[u8]) -> Result<(ClientUuid, String, usize), ProtoError> {
    need_bytes(body, 0, 17)?;
    let mut sender_uuid = [0u8; 16];
    sender_uuid.copy_from_slice(&body[0..16]);
    let nick_len = body[16] as usize;
    if nick_len > MAX_NICKNAME_LEN {
        return Err(ProtoError::InvalidHeader);
    }
    need_bytes(body, 17, nick_len)?;
    let sender_nickname = std::str::from_utf8(&body[17..17 + nick_len])
        .map_err(|_| ProtoError::InvalidUtf8)?
        .to_string();
    Ok((sender_uuid, sender_nickname, 17 + nick_len))
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
        let author_uuid: ClientUuid = [0x33; 16];
        let author_nick = "SwiftFalconVeryLongNicknameExceeding32Bytes".to_string();

        let long_text = "Привет мир! ".repeat(120);
        let chat_text = P2pPayload::ChatText {
            sender_uuid: author_uuid,
            sender_nickname: author_nick.clone(),
            msg_id: 9001,
            timestamp_ms: 1_700_000_001_234,
            text: long_text.clone(),
        };

        let mut packets = vec![
            (1u64, P2pPayload::ChatAck { msg_id: 9001 }),
            (2, P2pPayload::HistorySyncReq),
            (
                3,
                P2pPayload::BlobOffer {
                    sender_uuid: author_uuid,
                    sender_nickname: author_nick.clone(),
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
            (
                6,
                P2pPayload::BlobHave {
                    blob_id: [0x11; 16],
                },
            ),
            (
                7,
                P2pPayload::AvatarAnnounce {
                    avatar_hash: [0x7E; 32],
                    total_size: 8_420,
                    total_chunks: 9,
                },
            ),
            (
                8,
                P2pPayload::AvatarAnnounce {
                    avatar_hash: [0u8; 32],
                    total_size: 0,
                    total_chunks: 0,
                },
            ),
        ];
        packets.insert(0, (0, chat_text));

        for (seq, payload) in packets {
            let mut body = Vec::new();
            payload.encode_body(&mut body).unwrap();
            let decoded_body = P2pPayload::decode_body(payload.msg_type(), &body).unwrap();

            match (&payload, &decoded_body) {
                (
                    P2pPayload::ChatText {
                        sender_uuid,
                        msg_id,
                        timestamp_ms,
                        ..
                    },
                    P2pPayload::ChatText {
                        sender_uuid: d_uuid,
                        sender_nickname: d_nick,
                        msg_id: d_id,
                        timestamp_ms: d_ts,
                        text: d_text,
                    },
                ) => {
                    assert_eq!(sender_uuid, d_uuid);
                    assert!(d_nick.len() <= MAX_NICKNAME_LEN);
                    assert!(author_nick.starts_with(d_nick));
                    assert_eq!(msg_id, d_id);
                    assert_eq!(timestamp_ms, d_ts);
                    assert!(d_text.len() <= MAX_CHAT_TEXT_BYTES);
                    assert!(long_text.starts_with(d_text));
                }
                (
                    P2pPayload::BlobOffer {
                        sender_uuid,
                        filename,
                        ..
                    },
                    P2pPayload::BlobOffer {
                        sender_uuid: d_uuid,
                        sender_nickname: d_nick,
                        filename: d_name,
                        ..
                    },
                ) => {
                    assert_eq!(sender_uuid, d_uuid);
                    assert!(d_nick.len() <= MAX_NICKNAME_LEN);
                    assert!(author_nick.starts_with(d_nick));
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

    #[test]
    fn test_p2p_stream_and_video_packets_roundtrip_and_cipher() {
        use crate::crypto::RoomCipher;

        let cipher = RoomCipher::from_secret(&[0x77; 32]);
        let room_prefix = [0x12, 0x34, 0x56, 0x78];

        let packets = vec![
            (
                10u64,
                P2pPayload::StreamAnnounce {
                    is_streaming: true,
                    codec: 1,
                    has_audio: true,
                    width: 1920,
                    height: 1080,
                    fps: 60,
                    source_title: "Counter-Strike 2 (Matchmaking Live) ".repeat(3),
                    encoder_backend: "NVIDIA NVENC".to_string(),
                },
            ),
            (11, P2pPayload::StreamSubscribe { subscribe: true }),
            (12, P2pPayload::StreamPli),
            (
                13,
                P2pPayload::VideoFrameFrag {
                    frame_id: 42,
                    frag_idx: 2,
                    frag_count: 6,
                    flags: VIDEO_FLAG_KEYFRAME | VIDEO_FLAG_XOR_FEC,
                    data: vec![0xAB; MAX_VIDEO_FRAG_BYTES],
                },
            ),
            (
                14,
                P2pPayload::StreamDecoderError {
                    failed_codec: 1,
                    reason: "HEVC hardware decoder unavailable on viewer ".repeat(3),
                },
            ),
            (
                15,
                P2pPayload::StreamAudioOpus {
                    opus_data: vec![0x55; 96],
                },
            ),
        ];

        for (seq, payload) in packets {
            let mut body = Vec::new();
            payload.encode_body(&mut body).unwrap();
            let decoded_body = P2pPayload::decode_body(payload.msg_type(), &body).unwrap();

            match (&payload, &decoded_body) {
                (
                    P2pPayload::StreamAnnounce {
                        codec,
                        has_audio,
                        source_title,
                        encoder_backend,
                        ..
                    },
                    P2pPayload::StreamAnnounce {
                        codec: d_codec,
                        has_audio: d_audio,
                        source_title: d_title,
                        encoder_backend: d_backend,
                        ..
                    },
                ) => {
                    assert_eq!(codec, d_codec);
                    assert_eq!(has_audio, d_audio);
                    assert!(d_title.len() <= MAX_STREAM_TITLE_BYTES);
                    assert!(source_title.starts_with(d_title));
                    assert!(d_backend.len() <= MAX_ENCODER_BACKEND_BYTES);
                    assert!(encoder_backend.starts_with(d_backend));
                }
                (
                    P2pPayload::StreamDecoderError {
                        failed_codec,
                        reason,
                    },
                    P2pPayload::StreamDecoderError {
                        failed_codec: d_codec,
                        reason: d_reason,
                    },
                ) => {
                    assert_eq!(failed_codec, d_codec);
                    assert!(d_reason.len() <= MAX_STREAM_ERROR_BYTES);
                    assert!(reason.starts_with(d_reason));
                }
                _ => assert_eq!(payload, decoded_body),
            }

            let header = P2pHeader {
                msg_type: payload.msg_type(),
                room_prefix,
                sender_peer_id: 2,
                seq_num: seq,
            };
            let mut sealed = Vec::with_capacity(MAX_UDP_PAYLOAD);
            cipher.seal_packet(&header, &payload, &mut sealed).unwrap();
            assert!(sealed.len() <= MAX_UDP_PAYLOAD);
            let (hdr, opened) = cipher.open_packet(&mut sealed).unwrap();
            assert_eq!(hdr.msg_type, payload.msg_type());
            assert_eq!(opened, decoded_body);
        }
    }

    #[test]
    fn test_relay_datagram_roundtrip_rewrite_and_bounds() {
        let secret = [0x22u8; 32];
        let cipher = crate::crypto::RoomCipher::from_secret(&secret);
        let room_id = [0xABu8; 16];
        let room_prefix = [0xAB, 0xAB, 0xAB, 0xAB];
        let hdr = P2pHeader {
            msg_type: P2pMsgType::AudioOpus,
            room_prefix,
            sender_peer_id: 1,
            seq_num: 42,
        };
        let payload = P2pPayload::AudioOpus {
            opus_data: vec![1, 2, 3, 4, 5],
        };
        let mut inner = Vec::new();
        cipher.seal_packet(&hdr, &payload, &mut inner).unwrap();

        let mut wire = Vec::new();
        encode_relay_datagram_into(&room_id, 7, &inner, &mut wire).unwrap();
        assert!(is_relay_datagram(&wire));

        let (parsed_room, target_pid, parsed_inner) = parse_relay_header(&wire).unwrap();
        assert_eq!(parsed_room, room_id);
        assert_eq!(target_pid, 7);
        assert_eq!(parsed_inner, inner.as_slice());

        rewrite_relay_peer_id(&mut wire, 1);
        let (parsed_room2, sender_pid, _) = parse_relay_header(&wire).unwrap();
        assert_eq!(parsed_room2, room_id);
        assert_eq!(sender_pid, 1);

        let oversized = vec![0u8; MAX_UDP_PAYLOAD + 1];
        assert!(matches!(
            encode_relay_datagram_into(&room_id, 2, &oversized, &mut wire),
            Err(ProtoError::PacketTooLarge(_))
        ));
    }
}
