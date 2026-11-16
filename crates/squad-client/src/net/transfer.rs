use squad_proto::{
    ClientUuid, P2pHeader, P2pPayload, PeerId, RoomCipher, BLOB_CHUNK_SIZE,
    INLINE_IMAGE_MAX_BYTES,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::{debug, info};

const HISTORY_LIMIT: usize = 30;
const RETRY_INTERVAL: Duration = Duration::from_millis(200);
const MAX_RETRIES: u8 = 10;
const CHUNK_REQ_INTERVAL: Duration = Duration::from_millis(40);
const WINDOW_RETRY_COOLDOWN: Duration = Duration::from_millis(160);
const QOS_MAX_CHUNKS_PER_TICK: usize = 15;
const MAX_OUTBOUND_CHUNK_QUEUE: usize = 4096;
const MAX_DOWNLOAD_FILE_BYTES: u64 = 512 * 1024 * 1024; // 512 MiB safety cap
const MAX_AVATAR_BYTES: usize = 64 * 1024;
const BLOB_KIND_INLINE_IMAGE: u8 = 1;
const BLOB_KIND_FILE: u8 = 2;
const BLOB_KIND_AVATAR: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlobTransferState {
    Available,
    Downloading {
        received_chunks: u32,
        total_chunks: u32,
    },
    Completed {
        saved_path: Option<PathBuf>,
    },
    Failed(String),
}

#[derive(Debug, Clone)]
pub enum ChatContentUi {
    Text(String),
    InlineImage {
        blob_id: [u8; 16],
        filename: String,
        total_size: u64,
        bytes: Option<Vec<u8>>,
        state: BlobTransferState,
    },
    File {
        blob_id: [u8; 16],
        filename: String,
        total_size: u64,
        state: BlobTransferState,
    },
}

#[derive(Debug, Clone)]
pub struct ChatItemUi {
    pub msg_id: u64,
    pub sender_uuid: ClientUuid,
    pub sender_nickname: String,
    pub timestamp_ms: u64,
    pub is_self: bool,
    pub content: ChatContentUi,
}

#[derive(Debug, Clone)]
pub enum TransferEvent {
    ChatItemReceived(ChatItemUi),
    BlobStateChanged {
        blob_id: [u8; 16],
        state: BlobTransferState,
        inline_bytes: Option<Vec<u8>>,
    },
    PeerAvatarLoaded {
        client_uuid: ClientUuid,
        avatar_hash: [u8; 32],
        jpeg_bytes: Option<Vec<u8>>,
    },
    Error(String),
}

pub struct TransferCtx<'a> {
    pub local_peer_id: PeerId,
    pub local_uuid: ClientUuid,
    pub local_nickname: &'a str,
    pub socket: &'a UdpSocket,
    pub cipher: &'a RoomCipher,
    pub room_prefix: [u8; 4],
    pub control_seq: &'a mut u64,
    pub seal_buf: &'a mut Vec<u8>,
    pub evt_tx: &'a mpsc::Sender<TransferEvent>,
}

impl TransferCtx<'_> {
    async fn send_p2p(&mut self, dst: SocketAddr, payload: &P2pPayload) {
        if self.local_peer_id == 0 {
            return;
        }
        *self.control_seq = self.control_seq.wrapping_add(1);
        let hdr = P2pHeader {
            msg_type: payload.msg_type(),
            room_prefix: self.room_prefix,
            sender_peer_id: self.local_peer_id,
            seq_num: *self.control_seq,
        };
        if self.cipher.seal_packet(&hdr, payload, self.seal_buf).is_ok() {
            let _ = self.socket.send_to(self.seal_buf, dst).await;
        }
    }
}

#[derive(Debug, Clone)]
enum HistoryPayload {
    Text(String),
    BlobOffer {
        blob_id: [u8; 16],
        kind: u8,
        auto_download: bool,
        filename: String,
        total_size: u64,
        chunk_size: u16,
        total_chunks: u32,
        blake3_root: [u8; 32],
    },
}

#[derive(Debug, Clone)]
struct HistoryEntry {
    sender_uuid: ClientUuid,
    sender_nickname: String,
    msg_id: u64,
    timestamp_ms: u64,
    payload: HistoryPayload,
}

struct PendingReliable {
    target_peer: PeerId,
    msg_id: u64,
    payload: P2pPayload,
    retries_left: u8,
    next_send_at: Instant,
}

enum LocalBlobSource {
    Memory(Vec<u8>),
    Disk { path: PathBuf, total_size: u64 },
}

#[derive(Debug, Clone)]
struct BlobMetadata {
    blob_id: [u8; 16],
    kind: u8,
    filename: String,
    total_size: u64,
    total_chunks: u32,
    blake3_root: [u8; 32],
}

struct ActiveDownload {
    received: Vec<bool>,
    received_count: u32,
    in_memory: Option<Vec<u8>>,
    part_path: Option<PathBuf>,
    part_file: Option<File>,
    window_requested_at: HashMap<u32, Instant>,
    last_reported_chunks: u32,
}

struct TrackedBlob {
    meta: BlobMetadata,
    seeders: HashSet<PeerId>,
    seeder_rr_cursor: usize,
    download: Option<ActiveDownload>,
    completed: bool,
}

pub struct SwarmChatManager {
    local_msg_counter: u64,
    history: VecDeque<HistoryEntry>,
    seen_messages: HashSet<(ClientUuid, u64)>,
    pending_reliable: Vec<PendingReliable>,
    local_blobs: HashMap<[u8; 16], LocalBlobSource>,
    remote_blobs: HashMap<[u8; 16], TrackedBlob>,
    outbound_chunks: VecDeque<(PeerId, [u8; 16], u32)>,
    queued_chunk_keys: HashSet<(PeerId, [u8; 16], u32)>,
    last_swarm_tick: Instant,
    local_avatar: Option<([u8; 32], Vec<u8>)>,
    avatar_targets: HashMap<[u8; 16], (ClientUuid, [u8; 32])>,
}

impl SwarmChatManager {
    pub fn new() -> Self {
        Self {
            local_msg_counter: 1,
            history: VecDeque::with_capacity(HISTORY_LIMIT),
            seen_messages: HashSet::new(),
            pending_reliable: Vec::new(),
            local_blobs: HashMap::new(),
            remote_blobs: HashMap::new(),
            outbound_chunks: VecDeque::new(),
            queued_chunk_keys: HashSet::new(),
            last_swarm_tick: Instant::now(),
            local_avatar: None,
            avatar_targets: HashMap::new(),
        }
    }

    pub fn set_initial_local_avatar(&mut self, avatar: Option<([u8; 32], Vec<u8>)>) {
        if let Some((hash, ref bytes)) = avatar {
            self.local_blobs
                .insert(avatar_blob_id(&hash), LocalBlobSource::Memory(bytes.clone()));
        }
        self.local_avatar = avatar;
    }

    pub async fn set_local_avatar(
        &mut self,
        avatar: Option<([u8; 32], Vec<u8>)>,
        connected_peers: &[(PeerId, SocketAddr)],
        ctx: &mut TransferCtx<'_>,
    ) {
        self.set_initial_local_avatar(avatar);
        let pkt = self.build_avatar_announce_payload();
        for &(_, addr) in connected_peers {
            ctx.send_p2p(addr, &pkt).await;
        }
    }

    pub async fn announce_avatar_to_peer(
        &self,
        peer_addr: SocketAddr,
        ctx: &mut TransferCtx<'_>,
    ) {
        if self.local_avatar.is_some() {
            let pkt = self.build_avatar_announce_payload();
            ctx.send_p2p(peer_addr, &pkt).await;
        }
    }

    fn build_avatar_announce_payload(&self) -> P2pPayload {
        if let Some((hash, ref bytes)) = self.local_avatar {
            P2pPayload::AvatarAnnounce {
                avatar_hash: hash,
                total_size: bytes.len() as u32,
                total_chunks: bytes.len().div_ceil(BLOB_CHUNK_SIZE) as u16,
            }
        } else {
            P2pPayload::AvatarAnnounce {
                avatar_hash: [0u8; 32],
                total_size: 0,
                total_chunks: 0,
            }
        }
    }

    fn next_msg_id(&mut self, local_peer_id: PeerId) -> u64 {
        let seq = self.local_msg_counter & 0x0000_FFFF_FFFF_FFFF;
        self.local_msg_counter = self.local_msg_counter.wrapping_add(1);
        ((local_peer_id as u64) << 48) | seq
    }

    fn push_history(&mut self, entry: HistoryEntry) {
        if self.history.len() >= HISTORY_LIMIT {
            self.history.pop_front();
        }
        self.history.push_back(entry);
    }

    pub async fn send_chat_text(
        &mut self,
        text: String,
        connected_peers: &[(PeerId, SocketAddr)],
        ctx: &mut TransferCtx<'_>,
    ) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let msg_id = self.next_msg_id(ctx.local_peer_id);
        let timestamp_ms = now_millis();
        let text_owned = trimmed.to_string();

        self.seen_messages.insert((ctx.local_uuid, msg_id));
        self.push_history(HistoryEntry {
            sender_uuid: ctx.local_uuid,
            sender_nickname: ctx.local_nickname.to_string(),
            msg_id,
            timestamp_ms,
            payload: HistoryPayload::Text(text_owned.clone()),
        });

        let _ = ctx
            .evt_tx
            .send(TransferEvent::ChatItemReceived(ChatItemUi {
                msg_id,
                sender_uuid: ctx.local_uuid,
                sender_nickname: ctx.local_nickname.to_string(),
                timestamp_ms,
                is_self: true,
                content: ChatContentUi::Text(text_owned.clone()),
            }))
            .await;

        let payload = P2pPayload::ChatText {
            sender_uuid: ctx.local_uuid,
            sender_nickname: ctx.local_nickname.to_string(),
            msg_id,
            timestamp_ms,
            text: text_owned,
        };
        self.broadcast_reliable(msg_id, &payload, connected_peers, ctx)
            .await;
    }

    pub async fn send_inline_image(
        &mut self,
        filename: String,
        bytes: Vec<u8>,
        connected_peers: &[(PeerId, SocketAddr)],
        ctx: &mut TransferCtx<'_>,
    ) {
        if bytes.is_empty() || bytes.len() >= INLINE_IMAGE_MAX_BYTES {
            let _ = ctx
                .evt_tx
                .send(TransferEvent::Error(
                    "Inline image must be non-empty and smaller than 200 KB".to_string(),
                ))
                .await;
            return;
        }

        let blob_id: [u8; 16] = rand::random();
        let blake3_root = *blake3::hash(&bytes).as_bytes();
        let total_size = bytes.len() as u64;
        let chunk_size = BLOB_CHUNK_SIZE as u16;
        let total_chunks = bytes.len().div_ceil(BLOB_CHUNK_SIZE) as u32;
        let msg_id = self.next_msg_id(ctx.local_peer_id);
        let timestamp_ms = now_millis();

        self.local_blobs
            .insert(blob_id, LocalBlobSource::Memory(bytes.clone()));
        self.seen_messages.insert((ctx.local_uuid, msg_id));
        self.push_history(HistoryEntry {
            sender_uuid: ctx.local_uuid,
            sender_nickname: ctx.local_nickname.to_string(),
            msg_id,
            timestamp_ms,
            payload: HistoryPayload::BlobOffer {
                blob_id,
                kind: 1,
                auto_download: true,
                filename: filename.clone(),
                total_size,
                chunk_size,
                total_chunks,
                blake3_root,
            },
        });

        let _ = ctx
            .evt_tx
            .send(TransferEvent::ChatItemReceived(ChatItemUi {
                msg_id,
                sender_uuid: ctx.local_uuid,
                sender_nickname: ctx.local_nickname.to_string(),
                timestamp_ms,
                is_self: true,
                content: ChatContentUi::InlineImage {
                    blob_id,
                    filename: filename.clone(),
                    total_size,
                    bytes: Some(bytes),
                    state: BlobTransferState::Completed { saved_path: None },
                },
            }))
            .await;

        let offer = P2pPayload::BlobOffer {
            sender_uuid: ctx.local_uuid,
            sender_nickname: ctx.local_nickname.to_string(),
            blob_id,
            msg_id,
            timestamp_ms,
            kind: 1,
            auto_download: true,
            filename,
            total_size,
            chunk_size,
            total_chunks,
            blake3_root,
        };
        self.broadcast_reliable(msg_id, &offer, connected_peers, ctx)
            .await;
    }

    pub async fn offer_file(
        &mut self,
        path: PathBuf,
        connected_peers: &[(PeerId, SocketAddr)],
        ctx: &mut TransferCtx<'_>,
    ) {
        let meta = match fs::metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                let _ = ctx
                    .evt_tx
                    .send(TransferEvent::Error(format!("Cannot read file: {e}")))
                    .await;
                return;
            }
        };
        let total_size = meta.len();
        if total_size == 0 || total_size > MAX_DOWNLOAD_FILE_BYTES {
            let _ = ctx
                .evt_tx
                .send(TransferEvent::Error(
                    "File must be between 1 byte and 512 MiB".to_string(),
                ))
                .await;
            return;
        }

        let blake3_root = match hash_file_blake3(&path) {
            Ok(h) => h,
            Err(e) => {
                let _ = ctx
                    .evt_tx
                    .send(TransferEvent::Error(format!("Failed to hash file: {e}")))
                    .await;
                return;
            }
        };

        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("file.bin")
            .to_string();

        let blob_id: [u8; 16] = rand::random();
        let chunk_size = BLOB_CHUNK_SIZE as u16;
        let total_chunks = total_size.div_ceil(BLOB_CHUNK_SIZE as u64) as u32;
        let msg_id = self.next_msg_id(ctx.local_peer_id);
        let timestamp_ms = now_millis();

        self.local_blobs.insert(
            blob_id,
            LocalBlobSource::Disk {
                path: path.clone(),
                total_size,
            },
        );
        self.seen_messages.insert((ctx.local_uuid, msg_id));
        self.push_history(HistoryEntry {
            sender_uuid: ctx.local_uuid,
            sender_nickname: ctx.local_nickname.to_string(),
            msg_id,
            timestamp_ms,
            payload: HistoryPayload::BlobOffer {
                blob_id,
                kind: 2,
                auto_download: false,
                filename: filename.clone(),
                total_size,
                chunk_size,
                total_chunks,
                blake3_root,
            },
        });

        let _ = ctx
            .evt_tx
            .send(TransferEvent::ChatItemReceived(ChatItemUi {
                msg_id,
                sender_uuid: ctx.local_uuid,
                sender_nickname: ctx.local_nickname.to_string(),
                timestamp_ms,
                is_self: true,
                content: ChatContentUi::File {
                    blob_id,
                    filename: filename.clone(),
                    total_size,
                    state: BlobTransferState::Completed {
                        saved_path: Some(path),
                    },
                },
            }))
            .await;

        let offer = P2pPayload::BlobOffer {
            sender_uuid: ctx.local_uuid,
            sender_nickname: ctx.local_nickname.to_string(),
            blob_id,
            msg_id,
            timestamp_ms,
            kind: 2,
            auto_download: false,
            filename,
            total_size,
            chunk_size,
            total_chunks,
            blake3_root,
        };
        self.broadcast_reliable(msg_id, &offer, connected_peers, ctx)
            .await;
    }

    pub fn remove_peer(&mut self, peer_id: PeerId) {
        self.pending_reliable.retain(|p| p.target_peer != peer_id);
        for blob in self.remote_blobs.values_mut() {
            blob.seeders.remove(&peer_id);
        }
        self.queued_chunk_keys.retain(|&(pid, _, _)| pid != peer_id);
        self.outbound_chunks.retain(|&(pid, _, _)| pid != peer_id);
    }

    pub async fn start_download(
        &mut self,
        blob_id: [u8; 16],
        evt_tx: &mpsc::Sender<TransferEvent>,
    ) {
        let Some(blob) = self.remote_blobs.get_mut(&blob_id) else {
            return;
        };
        if blob.completed || blob.download.is_some() {
            return;
        }
        let state = match init_download(&blob.meta) {
            Ok(dl) => {
                let s = BlobTransferState::Downloading {
                    received_chunks: 0,
                    total_chunks: blob.meta.total_chunks,
                };
                blob.download = Some(dl);
                s
            }
            Err(e) => BlobTransferState::Failed(e),
        };
        let _ = evt_tx
            .send(TransferEvent::BlobStateChanged {
                blob_id,
                state,
                inline_bytes: None,
            })
            .await;
    }

    pub async fn announce_haves_to_peer(
        &self,
        peer_addr: SocketAddr,
        ctx: &mut TransferCtx<'_>,
    ) {
        for &blob_id in self.local_blobs.keys() {
            ctx.send_p2p(peer_addr, &P2pPayload::BlobHave { blob_id })
                .await;
        }
    }

    pub async fn send_history_sync_req(
        &self,
        peer_addr: SocketAddr,
        ctx: &mut TransferCtx<'_>,
    ) {
        ctx.send_p2p(peer_addr, &P2pPayload::HistorySyncReq).await;
    }

    pub async fn handle_payload(
        &mut self,
        sender_peer_id: PeerId,
        sender_uuid: ClientUuid,
        sender_nickname: &str,
        sender_addr: SocketAddr,
        payload: P2pPayload,
        connected_peers: &[(PeerId, SocketAddr)],
        ctx: &mut TransferCtx<'_>,
    ) {
        match payload {
            P2pPayload::ChatText {
                sender_uuid,
                sender_nickname: pkt_nick,
                msg_id,
                timestamp_ms,
                text,
            } => {
                ctx.send_p2p(sender_addr, &P2pPayload::ChatAck { msg_id })
                    .await;

                if !self.seen_messages.insert((sender_uuid, msg_id)) {
                    return;
                }

                let display_nick = if pkt_nick.trim().is_empty() {
                    sender_nickname.to_string()
                } else {
                    pkt_nick
                };

                self.push_history(HistoryEntry {
                    sender_uuid,
                    sender_nickname: display_nick.clone(),
                    msg_id,
                    timestamp_ms,
                    payload: HistoryPayload::Text(text.clone()),
                });

                let _ = ctx
                    .evt_tx
                    .send(TransferEvent::ChatItemReceived(ChatItemUi {
                        msg_id,
                        sender_uuid,
                        sender_nickname: display_nick,
                        timestamp_ms,
                        is_self: sender_uuid == ctx.local_uuid,
                        content: ChatContentUi::Text(text),
                    }))
                    .await;
            }
            P2pPayload::ChatAck { msg_id } => {
                self.pending_reliable
                    .retain(|p| !(p.target_peer == sender_peer_id && p.msg_id == msg_id));
            }
            P2pPayload::HistorySyncReq => {
                let now = Instant::now();
                let entries: Vec<HistoryEntry> = self.history.iter().cloned().collect();
                for (idx, entry) in entries.into_iter().enumerate() {
                    let pkt = match entry.payload {
                        HistoryPayload::Text(text) => P2pPayload::ChatText {
                            sender_uuid: entry.sender_uuid,
                            sender_nickname: entry.sender_nickname,
                            msg_id: entry.msg_id,
                            timestamp_ms: entry.timestamp_ms,
                            text,
                        },
                        HistoryPayload::BlobOffer {
                            blob_id,
                            kind,
                            auto_download,
                            filename,
                            total_size,
                            chunk_size,
                            total_chunks,
                            blake3_root,
                        } => P2pPayload::BlobOffer {
                            sender_uuid: entry.sender_uuid,
                            sender_nickname: entry.sender_nickname,
                            blob_id,
                            msg_id: entry.msg_id,
                            timestamp_ms: entry.timestamp_ms,
                            kind,
                            auto_download,
                            filename,
                            total_size,
                            chunk_size,
                            total_chunks,
                            blake3_root,
                        },
                    };
                    let delay = Duration::from_millis((idx as u64) * 12);
                    self.pending_reliable.push(PendingReliable {
                        target_peer: sender_peer_id,
                        msg_id: entry.msg_id,
                        payload: pkt,
                        retries_left: MAX_RETRIES,
                        next_send_at: now + delay,
                    });
                }
                self.announce_haves_to_peer(sender_addr, ctx).await;
            }
            P2pPayload::BlobOffer {
                sender_uuid,
                sender_nickname: pkt_nick,
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
                ctx.send_p2p(sender_addr, &P2pPayload::ChatAck { msg_id })
                    .await;

                if chunk_size as usize != BLOB_CHUNK_SIZE
                    || total_size == 0
                    || total_size > MAX_DOWNLOAD_FILE_BYTES
                    || total_chunks == 0
                {
                    return;
                }
                let expected_chunks = total_size.div_ceil(BLOB_CHUNK_SIZE as u64);
                if expected_chunks != total_chunks as u64 {
                    return;
                }

                if let Some(blob) = self.remote_blobs.get_mut(&blob_id) {
                    blob.seeders.insert(sender_peer_id);
                }

                if !self.seen_messages.insert((sender_uuid, msg_id)) {
                    return;
                }

                let display_nick = if pkt_nick.trim().is_empty() {
                    sender_nickname.to_string()
                } else {
                    pkt_nick
                };

                let meta = BlobMetadata {
                    blob_id,
                    kind,
                    filename: filename.clone(),
                    total_size,
                    total_chunks,
                    blake3_root,
                };

                self.push_history(HistoryEntry {
                    sender_uuid,
                    sender_nickname: display_nick.clone(),
                    msg_id,
                    timestamp_ms,
                    payload: HistoryPayload::BlobOffer {
                        blob_id,
                        kind,
                        auto_download,
                        filename: filename.clone(),
                        total_size,
                        chunk_size,
                        total_chunks,
                        blake3_root,
                    },
                });

                let should_auto =
                    kind == 1 && auto_download && total_size < INLINE_IMAGE_MAX_BYTES as u64;
                let (download, init_state) = if should_auto {
                    match init_download(&meta) {
                        Ok(dl) => (
                            Some(dl),
                            BlobTransferState::Downloading {
                                received_chunks: 0,
                                total_chunks,
                            },
                        ),
                        Err(e) => (None, BlobTransferState::Failed(e)),
                    }
                } else {
                    (None, BlobTransferState::Available)
                };

                let mut seeders = HashSet::new();
                seeders.insert(sender_peer_id);

                self.remote_blobs.entry(blob_id).or_insert(TrackedBlob {
                    meta,
                    seeders,
                    seeder_rr_cursor: 0,
                    download,
                    completed: false,
                });

                let content = if kind == 1 {
                    ChatContentUi::InlineImage {
                        blob_id,
                        filename,
                        total_size,
                        bytes: None,
                        state: init_state,
                    }
                } else {
                    ChatContentUi::File {
                        blob_id,
                        filename,
                        total_size,
                        state: init_state,
                    }
                };

                let _ = ctx
                    .evt_tx
                    .send(TransferEvent::ChatItemReceived(ChatItemUi {
                        msg_id,
                        sender_uuid,
                        sender_nickname: display_nick,
                        timestamp_ms,
                        is_self: sender_uuid == ctx.local_uuid,
                        content,
                    }))
                    .await;
            }
            P2pPayload::AvatarAnnounce {
                avatar_hash,
                total_size,
                total_chunks,
            } => {
                if avatar_hash == [0u8; 32] {
                    let _ = ctx
                        .evt_tx
                        .send(TransferEvent::PeerAvatarLoaded {
                            client_uuid: sender_uuid,
                            avatar_hash,
                            jpeg_bytes: None,
                        })
                        .await;
                    return;
                }
                if total_size == 0
                    || (total_size as usize) > MAX_AVATAR_BYTES
                    || total_chunks == 0
                    || (total_chunks as u64) != (total_size as u64).div_ceil(BLOB_CHUNK_SIZE as u64)
                {
                    return;
                }
                if let Some(cached) = crate::ui::avatar::load_cached_avatar_jpeg(&avatar_hash) {
                    let _ = ctx
                        .evt_tx
                        .send(TransferEvent::PeerAvatarLoaded {
                            client_uuid: sender_uuid,
                            avatar_hash,
                            jpeg_bytes: Some(cached),
                        })
                        .await;
                    return;
                }
                let blob_id = avatar_blob_id(&avatar_hash);
                let meta = BlobMetadata {
                    blob_id,
                    kind: BLOB_KIND_AVATAR,
                    filename: "avatar.jpg".to_string(),
                    total_size: total_size as u64,
                    total_chunks: total_chunks as u32,
                    blake3_root: avatar_hash,
                };
                if let Ok(dl) = init_download(&meta) {
                    let mut seeders = HashSet::new();
                    seeders.insert(sender_peer_id);
                    self.remote_blobs.insert(
                        blob_id,
                        TrackedBlob {
                            meta,
                            seeders,
                            seeder_rr_cursor: 0,
                            download: Some(dl),
                            completed: false,
                        },
                    );
                    self.avatar_targets.insert(blob_id, (sender_uuid, avatar_hash));
                }
            }
            P2pPayload::BlobChunkReq {
                blob_id,
                start_chunk,
                bitmask,
            } => {
                if !self.local_blobs.contains_key(&blob_id) {
                    return;
                }
                for bit in 0..64u32 {
                    if ((bitmask >> bit) & 1) == 1 {
                        if self.outbound_chunks.len() >= MAX_OUTBOUND_CHUNK_QUEUE {
                            break;
                        }
                        let chunk_index = start_chunk.saturating_add(bit);
                        let key = (sender_peer_id, blob_id, chunk_index);
                        if self.queued_chunk_keys.insert(key) {
                            self.outbound_chunks.push_back(key);
                        }
                    }
                }
            }
            P2pPayload::BlobChunkData {
                blob_id,
                chunk_index,
                data,
            } => {
                self.handle_incoming_chunk(blob_id, chunk_index, &data, connected_peers, ctx)
                    .await;
            }
            P2pPayload::BlobHave { blob_id } => {
                if let Some(blob) = self.remote_blobs.get_mut(&blob_id) {
                    blob.seeders.insert(sender_peer_id);
                }
            }
            _ => {}
        }
    }

    async fn handle_incoming_chunk(
        &mut self,
        blob_id: [u8; 16],
        chunk_index: u32,
        data: &[u8],
        connected_peers: &[(PeerId, SocketAddr)],
        ctx: &mut TransferCtx<'_>,
    ) {
        let Some(blob) = self.remote_blobs.get_mut(&blob_id) else {
            return;
        };
        if blob.completed {
            return;
        }
        let Some(dl) = blob.download.as_mut() else {
            return;
        };
        let idx = chunk_index as usize;
        if idx >= dl.received.len() || dl.received[idx] {
            return;
        }

        let offset = (chunk_index as u64) * (BLOB_CHUNK_SIZE as u64);
        let expected_len = ((blob.meta.total_size - offset).min(BLOB_CHUNK_SIZE as u64)) as usize;
        if data.len() != expected_len {
            return;
        }

        if let Some(ref mut mem) = dl.in_memory {
            mem[offset as usize..offset as usize + expected_len].copy_from_slice(data);
        } else if let Some(ref mut file) = dl.part_file {
            if file.seek(SeekFrom::Start(offset)).is_err() || file.write_all(data).is_err() {
                blob.download = None;
                let _ = ctx
                    .evt_tx
                    .send(TransferEvent::BlobStateChanged {
                        blob_id,
                        state: BlobTransferState::Failed("Disk write failed".to_string()),
                        inline_bytes: None,
                    })
                    .await;
                return;
            }
        }

        dl.received[idx] = true;
        dl.received_count += 1;

        let total_chunks = blob.meta.total_chunks;
        if dl.received_count == total_chunks {
            let dl_done = blob.download.take().unwrap();
            let meta = blob.meta.clone();

            if let Some(mem) = dl_done.in_memory {
                if *blake3::hash(&mem).as_bytes() == meta.blake3_root {
                    blob.completed = true;
                    if meta.kind == BLOB_KIND_AVATAR {
                        let _ = crate::ui::avatar::save_cached_avatar_jpeg(&meta.blake3_root, &mem);
                        let (target_uuid, full_hash) = self
                            .avatar_targets
                            .remove(&blob_id)
                            .unwrap_or(([0u8; 16], meta.blake3_root));
                        let _ = ctx
                            .evt_tx
                            .send(TransferEvent::PeerAvatarLoaded {
                                client_uuid: target_uuid,
                                avatar_hash: full_hash,
                                jpeg_bytes: Some(mem),
                            })
                            .await;
                    } else {
                        self.local_blobs
                            .insert(blob_id, LocalBlobSource::Memory(mem.clone()));
                        info!("Swarm inline image verified: {}", meta.filename);
                        let _ = ctx
                            .evt_tx
                            .send(TransferEvent::BlobStateChanged {
                                blob_id,
                                state: BlobTransferState::Completed { saved_path: None },
                                inline_bytes: Some(mem),
                            })
                            .await;
                        self.broadcast_have(blob_id, connected_peers, ctx).await;
                    }
                } else {
                    if meta.kind == BLOB_KIND_AVATAR {
                        self.avatar_targets.remove(&blob_id);
                    } else {
                        let _ = ctx
                            .evt_tx
                            .send(TransferEvent::BlobStateChanged {
                                blob_id,
                                state: BlobTransferState::Failed(
                                    "BLAKE3 integrity check failed".to_string(),
                                ),
                                inline_bytes: None,
                            })
                            .await;
                    }
                }
            } else if let Some(part_path) = dl_done.part_path {
                drop(dl_done.part_file);
                match verify_and_finalize_file(&part_path, &meta) {
                    Ok(final_path) => {
                        blob.completed = true;
                        self.local_blobs.insert(
                            blob_id,
                            LocalBlobSource::Disk {
                                path: final_path.clone(),
                                total_size: meta.total_size,
                            },
                        );
                        info!("Swarm file download verified: {:?}", final_path);
                        let _ = ctx
                            .evt_tx
                            .send(TransferEvent::BlobStateChanged {
                                blob_id,
                                state: BlobTransferState::Completed {
                                    saved_path: Some(final_path),
                                },
                                inline_bytes: None,
                            })
                            .await;
                        self.broadcast_have(blob_id, connected_peers, ctx).await;
                    }
                    Err(e) => {
                        let _ = fs::remove_file(&part_path);
                        let _ = ctx
                            .evt_tx
                            .send(TransferEvent::BlobStateChanged {
                                blob_id,
                                state: BlobTransferState::Failed(e),
                                inline_bytes: None,
                            })
                            .await;
                    }
                }
            }
        } else if dl.received_count - dl.last_reported_chunks >= 8 || dl.received_count == 1 {
            dl.last_reported_chunks = dl.received_count;
            let _ = ctx
                .evt_tx
                .send(TransferEvent::BlobStateChanged {
                    blob_id,
                    state: BlobTransferState::Downloading {
                        received_chunks: dl.received_count,
                        total_chunks,
                    },
                    inline_bytes: None,
                })
                .await;
        }
    }

    /// Runs on every 10 ms QoS tick: drains <= 15 outbound BlobChunkData packets so audio stays unblocked,
    /// and every 40 ms runs reliable retransmissions and Swarm chunk requests.
    pub async fn tick_10ms(
        &mut self,
        peer_addrs: &HashMap<PeerId, SocketAddr>,
        ctx: &mut TransferCtx<'_>,
    ) {
        if ctx.local_peer_id == 0 {
            return;
        }

        let mut sent_chunks = 0;
        while sent_chunks < QOS_MAX_CHUNKS_PER_TICK {
            let Some(key @ (target_peer, blob_id, chunk_index)) =
                self.outbound_chunks.pop_front()
            else {
                break;
            };
            self.queued_chunk_keys.remove(&key);

            let Some(&dst_addr) = peer_addrs.get(&target_peer) else {
                continue;
            };
            let Some(source) = self.local_blobs.get(&blob_id) else {
                continue;
            };
            let Ok(chunk_data) = read_blob_chunk(source, chunk_index) else {
                continue;
            };

            let pkt = P2pPayload::BlobChunkData {
                blob_id,
                chunk_index,
                data: chunk_data,
            };
            ctx.send_p2p(dst_addr, &pkt).await;
            sent_chunks += 1;
        }

        let now = Instant::now();
        if now.duration_since(self.last_swarm_tick) < CHUNK_REQ_INTERVAL {
            return;
        }
        self.last_swarm_tick = now;

        let mut idx = 0;
        while idx < self.pending_reliable.len() {
            let due = now >= self.pending_reliable[idx].next_send_at;
            if due {
                if self.pending_reliable[idx].retries_left == 0 {
                    let dropped = self.pending_reliable.swap_remove(idx);
                    debug!(
                        "Dropping unacked msg {} for peer {}",
                        dropped.msg_id, dropped.target_peer
                    );
                    continue;
                }
                if let Some(&dst) = peer_addrs.get(&self.pending_reliable[idx].target_peer) {
                    let payload = self.pending_reliable[idx].payload.clone();
                    ctx.send_p2p(dst, &payload).await;
                }
                let item = &mut self.pending_reliable[idx];
                item.retries_left = item.retries_left.saturating_sub(1);
                item.next_send_at = now + RETRY_INTERVAL;
            }
            idx += 1;
        }

        let mut reqs: Vec<(SocketAddr, P2pPayload)> = Vec::new();
        for blob in self.remote_blobs.values_mut() {
            if blob.completed {
                continue;
            }
            let Some(dl) = blob.download.as_mut() else {
                continue;
            };
            let mut active_seeders: Vec<(PeerId, SocketAddr)> = blob
                .seeders
                .iter()
                .filter_map(|&pid| peer_addrs.get(&pid).map(|&addr| (pid, addr)))
                .collect();
            if active_seeders.is_empty() {
                continue;
            }
            active_seeders.sort_by_key(|(pid, _)| *pid);

            let total_chunks = dl.received.len();
            let mut cursor = 0usize;
            let mut scheduled = 0usize;
            while cursor < total_chunks && scheduled < 2 {
                let win_start = (cursor / 64) * 64;
                let mut bitmask = 0u64;
                for bit in 0..64 {
                    let ci = win_start + bit;
                    if ci >= total_chunks {
                        break;
                    }
                    if !dl.received[ci] {
                        bitmask |= 1u64 << bit;
                    }
                }
                cursor = win_start + 64;

                if bitmask == 0 {
                    continue;
                }

                let start_u32 = win_start as u32;
                let recently_requested = dl
                    .window_requested_at
                    .get(&start_u32)
                    .is_some_and(|t| now.duration_since(*t) < WINDOW_RETRY_COOLDOWN);
                if recently_requested {
                    continue;
                }

                dl.window_requested_at.insert(start_u32, now);
                let seeder_idx = blob.seeder_rr_cursor % active_seeders.len();
                blob.seeder_rr_cursor = blob.seeder_rr_cursor.wrapping_add(1);
                let (_, seeder_addr) = active_seeders[seeder_idx];

                reqs.push((
                    seeder_addr,
                    P2pPayload::BlobChunkReq {
                        blob_id: blob.meta.blob_id,
                        start_chunk: start_u32,
                        bitmask,
                    },
                ));
                scheduled += 1;
            }
        }

        for (dst, pkt) in reqs {
            ctx.send_p2p(dst, &pkt).await;
        }
    }

    async fn broadcast_reliable(
        &mut self,
        msg_id: u64,
        payload: &P2pPayload,
        connected_peers: &[(PeerId, SocketAddr)],
        ctx: &mut TransferCtx<'_>,
    ) {
        let now = Instant::now();
        for &(peer_id, addr) in connected_peers {
            ctx.send_p2p(addr, payload).await;
            self.pending_reliable.push(PendingReliable {
                target_peer: peer_id,
                msg_id,
                payload: payload.clone(),
                retries_left: MAX_RETRIES,
                next_send_at: now + RETRY_INTERVAL,
            });
        }
    }

    async fn broadcast_have(
        &self,
        blob_id: [u8; 16],
        connected_peers: &[(PeerId, SocketAddr)],
        ctx: &mut TransferCtx<'_>,
    ) {
        let pkt = P2pPayload::BlobHave { blob_id };
        for &(_, addr) in connected_peers {
            ctx.send_p2p(addr, &pkt).await;
        }
    }
}

fn init_download(meta: &BlobMetadata) -> Result<ActiveDownload, String> {
    let total_chunks = meta.total_chunks as usize;
    let received = vec![false; total_chunks];

    if (meta.kind == BLOB_KIND_INLINE_IMAGE || meta.kind == BLOB_KIND_AVATAR)
        && meta.total_size <= INLINE_IMAGE_MAX_BYTES as u64
    {
        return Ok(ActiveDownload {
            received,
            received_count: 0,
            in_memory: Some(vec![0u8; meta.total_size as usize]),
            part_path: None,
            part_file: None,
            window_requested_at: HashMap::new(),
            last_reported_chunks: 0,
        });
    }

    let dir = downloads_root_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("Cannot create downloads dir: {e}"))?;
    let hex_id = format!("{:032x}", u128::from_be_bytes(meta.blob_id));
    let part_path = dir.join(format!(".{hex_id}.part"));
    let file = File::options()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&part_path)
        .map_err(|e| format!("Cannot open temp file: {e}"))?;
    file.set_len(meta.total_size)
        .map_err(|e| format!("Cannot preallocate file: {e}"))?;

    Ok(ActiveDownload {
        received,
        received_count: 0,
        in_memory: None,
        part_path: Some(part_path),
        part_file: Some(file),
        window_requested_at: HashMap::new(),
        last_reported_chunks: 0,
    })
}

fn read_blob_chunk(source: &LocalBlobSource, chunk_index: u32) -> Result<Vec<u8>, String> {
    let offset = (chunk_index as u64) * (BLOB_CHUNK_SIZE as u64);
    match source {
        LocalBlobSource::Memory(bytes) => {
            let start = offset as usize;
            if start >= bytes.len() {
                return Err("Chunk out of bounds".to_string());
            }
            let end = (start + BLOB_CHUNK_SIZE).min(bytes.len());
            Ok(bytes[start..end].to_vec())
        }
        LocalBlobSource::Disk { path, total_size } => {
            if offset >= *total_size {
                return Err("Chunk out of bounds".to_string());
            }
            let to_read = ((*total_size - offset).min(BLOB_CHUNK_SIZE as u64)) as usize;
            let mut file = File::open(path).map_err(|e| e.to_string())?;
            file.seek(SeekFrom::Start(offset))
                .map_err(|e| e.to_string())?;
            let mut buf = vec![0u8; to_read];
            file.read_exact(&mut buf).map_err(|e| e.to_string())?;
            Ok(buf)
        }
    }
}

fn hash_file_blake3(path: &Path) -> Result<[u8; 32], std::io::Error> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(*hasher.finalize().as_bytes())
}

fn verify_and_finalize_file(part_path: &Path, meta: &BlobMetadata) -> Result<PathBuf, String> {
    let actual_hash = hash_file_blake3(part_path)
        .map_err(|e| format!("Failed to verify downloaded file: {e}"))?;
    if actual_hash != meta.blake3_root {
        return Err("BLAKE3 integrity check failed".to_string());
    }

    let dir = downloads_root_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("Cannot create downloads dir: {e}"))?;
    let final_path = unique_destination_path(&dir, &sanitize_filename(&meta.filename));
    fs::rename(part_path, &final_path).map_err(|e| format!("Failed to save file: {e}"))?;
    Ok(final_path)
}

pub fn downloads_root_dir() -> PathBuf {
    dirs::download_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Squad-Air")
}

fn sanitize_filename(raw: &str) -> String {
    let base = Path::new(raw)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("received_file")
        .trim();
    if base.is_empty() || base == "." || base == ".." {
        return "received_file".to_string();
    }
    base.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            other => other,
        })
        .collect()
}

fn unique_destination_path(dir: &Path, filename: &str) -> PathBuf {
    let candidate = dir.join(filename);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(filename);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("file");
    let ext = path.extension().and_then(|s| s.to_str());

    for n in 1..10_000u32 {
        let numbered = match ext {
            Some(e) if !e.is_empty() => format!("{stem} ({n}).{e}"),
            _ => format!("{stem} ({n})"),
        };
        let p = dir.join(numbered);
        if !p.exists() {
            return p;
        }
    }
    dir.join(format!("{stem}_{}", now_millis()))
}

#[inline]
fn avatar_blob_id(hash: &[u8; 32]) -> [u8; 16] {
    let mut id = [0u8; 16];
    id.copy_from_slice(&hash[..16]);
    id
}

#[inline]
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use squad_proto::{InviteLink, MAX_UDP_PAYLOAD};

    async fn pump_socket(
        mgr: &mut SwarmChatManager,
        peer_nicknames: &HashMap<PeerId, String>,
        connected_list: &[(PeerId, SocketAddr)],
        ctx: &mut TransferCtx<'_>,
    ) {
        let mut rx_buf = [0u8; MAX_UDP_PAYLOAD + 64];
        while let Ok(Ok((len, src))) =
            tokio::time::timeout(Duration::from_millis(5), ctx.socket.recv_from(&mut rx_buf))
                .await
        {
            if let Ok((hdr, payload)) = ctx.cipher.open_packet(&mut rx_buf[..len]) {
                let nick = peer_nicknames
                    .get(&hdr.sender_peer_id)
                    .map(String::as_str)
                    .unwrap_or("Peer");
                let sender_uuid = match hdr.sender_peer_id {
                    1 => [0x11; 16],
                    2 => [0x22; 16],
                    _ => [0x33; 16],
                };
                mgr.handle_payload(
                    hdr.sender_peer_id,
                    sender_uuid,
                    nick,
                    src,
                    payload,
                    connected_list,
                    ctx,
                )
                .await;
            }
        }
    }

    #[tokio::test]
    async fn test_swarm_chat_history_inline_image_and_file_transfer() {
        let invite = InviteLink::generate();
        let prefix = invite.room_prefix();
        let cipher = RoomCipher::from_secret(&invite.secret);

        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sock_c = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let addr_b = sock_b.local_addr().unwrap();
        let addr_c = sock_c.local_addr().unwrap();

        let mut mgr_a = SwarmChatManager::new();
        let mut mgr_b = SwarmChatManager::new();
        let mut mgr_c = SwarmChatManager::new();

        let (tx_a, mut _rx_a) = mpsc::channel(128);
        let (tx_b, mut rx_b) = mpsc::channel(128);
        let (tx_c, mut rx_c) = mpsc::channel(128);

        let mut seq_a = 1u64;
        let mut seq_b = 1u64;
        let mut seq_c = 1u64;
        let mut seal_a = Vec::with_capacity(MAX_UDP_PAYLOAD);
        let mut seal_b = Vec::with_capacity(MAX_UDP_PAYLOAD);
        let mut seal_c = Vec::with_capacity(MAX_UDP_PAYLOAD);

        let mut nicks = HashMap::new();
        nicks.insert(1, "Alpha".to_string());
        nicks.insert(2, "Bravo".to_string());
        nicks.insert(3, "Charlie".to_string());

        let ab_list = vec![(2, addr_b)];
        let ba_list = vec![(1, addr_a)];

        {
            let mut ctx_a = TransferCtx {
                local_peer_id: 1,
                local_uuid: [0x11; 16],
                local_nickname: "Alpha",
                socket: &sock_a,
                cipher: &cipher,
                room_prefix: prefix,
                control_seq: &mut seq_a,
                seal_buf: &mut seal_a,
                evt_tx: &tx_a,
            };
            mgr_a
                .send_chat_text("Smoke mid!".to_string(), &ab_list, &mut ctx_a)
                .await;
            let inline_bytes: Vec<u8> = (0..18_500u32).map(|i| (i % 251) as u8).collect();
            mgr_a
                .send_inline_image("strat.jpg".to_string(), inline_bytes, &ab_list, &mut ctx_a)
                .await;
        }

        let expected_inline: Vec<u8> = (0..18_500u32).map(|i| (i % 251) as u8).collect();
        let mut ab_map = HashMap::new();
        ab_map.insert(2, addr_b);
        let mut ba_map = HashMap::new();
        ba_map.insert(1, addr_a);

        let mut b_got_text = false;
        let mut b_got_img = false;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(12)).await;
            {
                let mut ctx_b = TransferCtx {
                    local_peer_id: 2,
                    local_uuid: [0x22; 16],
                    local_nickname: "Bravo",
                    socket: &sock_b,
                    cipher: &cipher,
                    room_prefix: prefix,
                    control_seq: &mut seq_b,
                    seal_buf: &mut seal_b,
                    evt_tx: &tx_b,
                };
                pump_socket(&mut mgr_b, &nicks, &ba_list, &mut ctx_b).await;
                mgr_b.tick_10ms(&ba_map, &mut ctx_b).await;
            }
            {
                let mut ctx_a = TransferCtx {
                    local_peer_id: 1,
                    local_uuid: [0x11; 16],
                    local_nickname: "Alpha",
                    socket: &sock_a,
                    cipher: &cipher,
                    room_prefix: prefix,
                    control_seq: &mut seq_a,
                    seal_buf: &mut seal_a,
                    evt_tx: &tx_a,
                };
                pump_socket(&mut mgr_a, &nicks, &ab_list, &mut ctx_a).await;
                mgr_a.tick_10ms(&ab_map, &mut ctx_a).await;
            }

            while let Ok(ev) = rx_b.try_recv() {
                match ev {
                    TransferEvent::ChatItemReceived(item) => {
                        if let ChatContentUi::Text(t) = item.content {
                            assert_eq!(t, "Smoke mid!");
                            b_got_text = true;
                        }
                    }
                    TransferEvent::BlobStateChanged {
                        state: BlobTransferState::Completed { saved_path: None },
                        inline_bytes: Some(bytes),
                        ..
                    } => {
                        assert_eq!(bytes, expected_inline);
                        b_got_img = true;
                    }
                    _ => {}
                }
            }
            if b_got_text && b_got_img {
                break;
            }
        }
        assert!(b_got_text && b_got_img);
        assert!(mgr_a.pending_reliable.is_empty());

        {
            let mut ctx_c = TransferCtx {
                local_peer_id: 3,
                local_uuid: [0x33; 16],
                local_nickname: "Charlie",
                socket: &sock_c,
                cipher: &cipher,
                room_prefix: prefix,
                control_seq: &mut seq_c,
                seal_buf: &mut seal_c,
                evt_tx: &tx_c,
            };
            mgr_c.send_history_sync_req(addr_b, &mut ctx_c).await;
        }

        let bc_list = vec![(1, addr_a), (3, addr_c)];
        let cb_list = vec![(1, addr_a), (2, addr_b)];
        let mut bc_map = HashMap::new();
        bc_map.insert(1, addr_a);
        bc_map.insert(3, addr_c);
        let mut cb_map = HashMap::new();
        cb_map.insert(1, addr_a);
        cb_map.insert(2, addr_b);

        let mut c_got_text = false;
        let mut c_got_img = false;
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_millis(12)).await;
            {
                let mut ctx_b = TransferCtx {
                    local_peer_id: 2,
                    local_uuid: [0x22; 16],
                    local_nickname: "Bravo",
                    socket: &sock_b,
                    cipher: &cipher,
                    room_prefix: prefix,
                    control_seq: &mut seq_b,
                    seal_buf: &mut seal_b,
                    evt_tx: &tx_b,
                };
                pump_socket(&mut mgr_b, &nicks, &bc_list, &mut ctx_b).await;
                mgr_b.tick_10ms(&bc_map, &mut ctx_b).await;
            }
            {
                let mut ctx_c = TransferCtx {
                    local_peer_id: 3,
                    local_uuid: [0x33; 16],
                    local_nickname: "Charlie",
                    socket: &sock_c,
                    cipher: &cipher,
                    room_prefix: prefix,
                    control_seq: &mut seq_c,
                    seal_buf: &mut seal_c,
                    evt_tx: &tx_c,
                };
                pump_socket(&mut mgr_c, &nicks, &cb_list, &mut ctx_c).await;
                mgr_c.tick_10ms(&cb_map, &mut ctx_c).await;
            }

            while let Ok(ev) = rx_c.try_recv() {
                match ev {
                    TransferEvent::ChatItemReceived(item) => {
                        if let ChatContentUi::Text(t) = item.content {
                            if t == "Smoke mid!" {
                                c_got_text = true;
                            }
                        }
                    }
                    TransferEvent::BlobStateChanged {
                        state: BlobTransferState::Completed { saved_path: None },
                        inline_bytes: Some(bytes),
                        ..
                    } => {
                        assert_eq!(bytes, expected_inline);
                        c_got_img = true;
                    }
                    _ => {}
                }
            }
            if c_got_text && c_got_img {
                break;
            }
        }
        assert!(c_got_text && c_got_img);

        let tmp_src = std::env::temp_dir().join("squad_air_test_offer.bin");
        let file_bytes: Vec<u8> = (0..12_345u32).map(|i| ((i * 7) % 256) as u8).collect();
        fs::write(&tmp_src, &file_bytes).unwrap();

        {
            let mut ctx_a = TransferCtx {
                local_peer_id: 1,
                local_uuid: [0x11; 16],
                local_nickname: "Alpha",
                socket: &sock_a,
                cipher: &cipher,
                room_prefix: prefix,
                control_seq: &mut seq_a,
                seal_buf: &mut seal_a,
                evt_tx: &tx_a,
            };
            mgr_a.offer_file(tmp_src.clone(), &ab_list, &mut ctx_a).await;
        }

        let mut offered_blob_id = None;
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(10)).await;
            {
                let mut ctx_b = TransferCtx {
                    local_peer_id: 2,
                    local_uuid: [0x22; 16],
                    local_nickname: "Bravo",
                    socket: &sock_b,
                    cipher: &cipher,
                    room_prefix: prefix,
                    control_seq: &mut seq_b,
                    seal_buf: &mut seal_b,
                    evt_tx: &tx_b,
                };
                pump_socket(&mut mgr_b, &nicks, &ba_list, &mut ctx_b).await;
            }
            while let Ok(ev) = rx_b.try_recv() {
                if let TransferEvent::ChatItemReceived(item) = ev {
                    if let ChatContentUi::File { blob_id, state, .. } = item.content {
                        assert_eq!(state, BlobTransferState::Available);
                        offered_blob_id = Some(blob_id);
                    }
                }
            }
            if offered_blob_id.is_some() {
                break;
            }
        }
        let bid = offered_blob_id.expect("Peer B should receive File BlobOffer");
        mgr_b.start_download(bid, &tx_b).await;

        let mut downloaded_path: Option<PathBuf> = None;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(12)).await;
            {
                let mut ctx_b = TransferCtx {
                    local_peer_id: 2,
                    local_uuid: [0x22; 16],
                    local_nickname: "Bravo",
                    socket: &sock_b,
                    cipher: &cipher,
                    room_prefix: prefix,
                    control_seq: &mut seq_b,
                    seal_buf: &mut seal_b,
                    evt_tx: &tx_b,
                };
                pump_socket(&mut mgr_b, &nicks, &ba_list, &mut ctx_b).await;
                mgr_b.tick_10ms(&ba_map, &mut ctx_b).await;
            }
            {
                let mut ctx_a = TransferCtx {
                    local_peer_id: 1,
                    local_uuid: [0x11; 16],
                    local_nickname: "Alpha",
                    socket: &sock_a,
                    cipher: &cipher,
                    room_prefix: prefix,
                    control_seq: &mut seq_a,
                    seal_buf: &mut seal_a,
                    evt_tx: &tx_a,
                };
                pump_socket(&mut mgr_a, &nicks, &ab_list, &mut ctx_a).await;
                mgr_a.tick_10ms(&ab_map, &mut ctx_a).await;
            }

            while let Ok(ev) = rx_b.try_recv() {
                if let TransferEvent::BlobStateChanged {
                    blob_id,
                    state: BlobTransferState::Completed { saved_path },
                    ..
                } = ev
                {
                    if blob_id == bid {
                        downloaded_path = saved_path;
                    }
                }
            }
            if downloaded_path.is_some() {
                break;
            }
        }

        let final_path = downloaded_path.expect("File download should complete with BLAKE3 check");
        assert_eq!(fs::read(&final_path).unwrap(), file_bytes);

        let _ = fs::remove_file(&tmp_src);
        let _ = fs::remove_file(&final_path);
    }
}
