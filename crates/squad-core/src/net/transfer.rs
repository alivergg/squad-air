use squad_proto::{
    encode_peer_relay_datagram_into, encode_relay_datagram_into, ClientUuid, P2pHeader, P2pPayload,
    PeerId, RoomCipher, RoomId, BLOB_CHUNK_SIZE, INLINE_IMAGE_MAX_BYTES, MAX_RELAY_UDP_PAYLOAD,
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
const RETRY_INTERVAL: Duration = Duration::from_millis(250);
const MAX_RETRIES: u8 = 20;
const DELIVERY_MAX_AGE: Duration = Duration::from_secs(5);
const CHUNK_REQ_INTERVAL: Duration = Duration::from_millis(40);
const WINDOW_RETRY_COOLDOWN: Duration = Duration::from_millis(160);
const QOS_MAX_CHUNKS_PER_TICK: usize = 15;
const MAX_OUTBOUND_CHUNK_QUEUE: usize = 4096;
const MAX_PENDING_RELIABLE: usize = 256;
const MAX_ACTIVE_DOWNLOADS: usize = 8;
const MAX_SEEN_MESSAGES: usize = 2048;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerEndpoint {
    Direct(SocketAddr),
    PeerRelay {
        relay_addr: SocketAddr,
        room_id: RoomId,
        target_peer_id: PeerId,
    },
    Relay {
        server: SocketAddr,
        room_id: RoomId,
        peer_id: PeerId,
    },
}

impl From<SocketAddr> for PeerEndpoint {
    fn from(addr: SocketAddr) -> Self {
        Self::Direct(addr)
    }
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
    async fn send_p2p(&mut self, dst: PeerEndpoint, payload: &P2pPayload) {
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
        if self
            .cipher
            .seal_packet(&hdr, payload, self.seal_buf)
            .is_ok()
        {
            match dst {
                PeerEndpoint::Direct(addr) => {
                    let _ = self.socket.send_to(self.seal_buf, addr).await;
                }
                PeerEndpoint::PeerRelay {
                    relay_addr,
                    room_id,
                    target_peer_id,
                } => {
                    let mut relay_buf = Vec::with_capacity(MAX_RELAY_UDP_PAYLOAD);
                    if encode_peer_relay_datagram_into(
                        &room_id,
                        target_peer_id,
                        self.seal_buf,
                        &mut relay_buf,
                    )
                    .is_ok()
                    {
                        let _ = self.socket.send_to(&relay_buf, relay_addr).await;
                    }
                }
                PeerEndpoint::Relay {
                    server,
                    room_id,
                    peer_id,
                } => {
                    if !payload.msg_type().is_server_relay_allowed() {
                        return;
                    }
                    let mut relay_buf = Vec::with_capacity(MAX_RELAY_UDP_PAYLOAD);
                    if encode_relay_datagram_into(&room_id, peer_id, self.seal_buf, &mut relay_buf)
                        .is_ok()
                    {
                        let _ = self.socket.send_to(&relay_buf, server).await;
                    }
                }
            }
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

struct PendingRoomDelivery {
    msg_id: u64,
    payload: P2pPayload,
    acked_by: HashSet<ClientUuid>,
    created_at: Instant,
    next_send_at: Instant,
    attempts_remaining: u8,
}

#[derive(Debug, Clone)]
struct DownloadedBlobRecord {
    blob_id: [u8; 16],
    path: PathBuf,
    total_size: u64,
    blake3_root: [u8; 32],
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
    pending_deliveries: Vec<PendingRoomDelivery>,
    history_sync_attempts_left: u8,
    history_sync_next_at: Instant,
    room_id: Option<RoomId>,
    data_root: Option<PathBuf>,
    downloaded_index: HashMap<[u8; 16], DownloadedBlobRecord>,
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
            pending_deliveries: Vec::new(),
            history_sync_attempts_left: MAX_RETRIES,
            history_sync_next_at: Instant::now(),
            room_id: None,
            data_root: None,
            downloaded_index: HashMap::new(),
            local_blobs: HashMap::new(),
            remote_blobs: HashMap::new(),
            outbound_chunks: VecDeque::new(),
            queued_chunk_keys: HashSet::new(),
            last_swarm_tick: Instant::now(),
            local_avatar: None,
            avatar_targets: HashMap::new(),
        }
    }

    pub fn set_room_persistence(&mut self, room_id: RoomId, data_root: PathBuf) {
        self.room_id = Some(room_id);
        self.data_root = Some(data_root);
    }

    pub fn has_pending_work(&self) -> bool {
        !self.outbound_chunks.is_empty()
            || !self.pending_deliveries.is_empty()
            || !self.pending_reliable.is_empty()
            || self.history_sync_attempts_left > 0
            || self
                .remote_blobs
                .values()
                .any(|b| !b.completed && b.download.is_some())
    }

    pub fn trim_memory(&mut self) {
        if let Some(ref root) = self.data_root {
            for (blob_id, src) in &mut self.local_blobs {
                if let LocalBlobSource::Memory(bytes) = src {
                    let path = inline_cache_path(root, blob_id);
                    if path.is_file() {
                        *src = LocalBlobSource::Disk {
                            path,
                            total_size: bytes.len() as u64,
                        };
                    }
                }
            }
        }
        self.outbound_chunks.shrink_to_fit();
        self.queued_chunk_keys.shrink_to_fit();
        self.pending_reliable.shrink_to_fit();
        self.pending_deliveries.shrink_to_fit();
        self.seen_messages.shrink_to_fit();
    }

    pub async fn load_room_from_disk(
        &mut self,
        local_uuid: ClientUuid,
        evt_tx: &mpsc::Sender<TransferEvent>,
    ) {
        let Some(root) = self.data_root.clone() else {
            return;
        };

        // Load downloaded files index and re-seed files whose BLAKE3 still matches.
        for rec in read_downloaded_blobs_index(&root) {
            if rec.path.is_file() {
                if let Ok(actual) = hash_file_blake3(&rec.path) {
                    if actual == rec.blake3_root {
                        self.local_blobs.insert(
                            rec.blob_id,
                            LocalBlobSource::Disk {
                                path: rec.path.clone(),
                                total_size: rec.total_size,
                            },
                        );
                        self.downloaded_index.insert(rec.blob_id, rec);
                    }
                }
            }
        }

        let Some(room_id) = self.room_id else {
            return;
        };
        let mut entries = read_room_history_file(&root, &room_id);
        entries.sort_by_key(|e| (e.timestamp_ms, e.msg_id));
        if entries.len() > HISTORY_LIMIT {
            let drop_n = entries.len() - HISTORY_LIMIT;
            entries.drain(0..drop_n);
        }

        for entry in entries {
            if !self.mark_seen_message((entry.sender_uuid, entry.msg_id)) {
                continue;
            }
            let is_self = entry.sender_uuid == local_uuid;
            let content = match &entry.payload {
                HistoryPayload::Text(text) => ChatContentUi::Text(text.clone()),
                HistoryPayload::BlobOffer {
                    blob_id,
                    kind,
                    filename,
                    total_size,
                    total_chunks,
                    blake3_root,
                    ..
                } => {
                    if *kind == BLOB_KIND_INLINE_IMAGE {
                        let cached = load_cached_inline_image(&root, blob_id, blake3_root);
                        if let Some(ref bytes) = cached {
                            self.local_blobs
                                .insert(*blob_id, LocalBlobSource::Memory(bytes.clone()));
                            ChatContentUi::InlineImage {
                                blob_id: *blob_id,
                                filename: filename.clone(),
                                total_size: *total_size,
                                bytes: Some(bytes.clone()),
                                state: BlobTransferState::Completed { saved_path: None },
                            }
                        } else {
                            self.remote_blobs.entry(*blob_id).or_insert(TrackedBlob {
                                meta: BlobMetadata {
                                    blob_id: *blob_id,
                                    kind: *kind,
                                    filename: filename.clone(),
                                    total_size: *total_size,
                                    total_chunks: *total_chunks,
                                    blake3_root: *blake3_root,
                                },
                                seeders: HashSet::new(),
                                seeder_rr_cursor: 0,
                                download: None,
                                completed: false,
                            });
                            ChatContentUi::InlineImage {
                                blob_id: *blob_id,
                                filename: filename.clone(),
                                total_size: *total_size,
                                bytes: None,
                                state: BlobTransferState::Available,
                            }
                        }
                    } else if let Some(rec) = self.downloaded_index.get(blob_id) {
                        ChatContentUi::File {
                            blob_id: *blob_id,
                            filename: filename.clone(),
                            total_size: *total_size,
                            state: BlobTransferState::Completed {
                                saved_path: Some(rec.path.clone()),
                            },
                        }
                    } else {
                        self.remote_blobs.entry(*blob_id).or_insert(TrackedBlob {
                            meta: BlobMetadata {
                                blob_id: *blob_id,
                                kind: *kind,
                                filename: filename.clone(),
                                total_size: *total_size,
                                total_chunks: *total_chunks,
                                blake3_root: *blake3_root,
                            },
                            seeders: HashSet::new(),
                            seeder_rr_cursor: 0,
                            download: None,
                            completed: false,
                        });
                        ChatContentUi::File {
                            blob_id: *blob_id,
                            filename: filename.clone(),
                            total_size: *total_size,
                            state: BlobTransferState::Available,
                        }
                    }
                }
            };

            let item = ChatItemUi {
                msg_id: entry.msg_id,
                sender_uuid: entry.sender_uuid,
                sender_nickname: entry.sender_nickname.clone(),
                timestamp_ms: entry.timestamp_ms,
                is_self,
                content,
            };
            self.history.push_back(entry);
            let _ = evt_tx.send(TransferEvent::ChatItemReceived(item)).await;
        }
    }

    pub fn set_initial_local_avatar(&mut self, avatar: Option<([u8; 32], Vec<u8>)>) {
        if let Some((hash, ref bytes)) = avatar {
            self.local_blobs.insert(
                avatar_blob_id(&hash),
                LocalBlobSource::Memory(bytes.clone()),
            );
        }
        self.local_avatar = avatar;
    }

    pub async fn set_local_avatar(
        &mut self,
        avatar: Option<([u8; 32], Vec<u8>)>,
        connected_peers: &[(PeerId, PeerEndpoint)],
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
        peer_addr: PeerEndpoint,
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

    fn mark_seen_message(&mut self, key: (ClientUuid, u64)) -> bool {
        if self.seen_messages.contains(&key) {
            return false;
        }
        if self.seen_messages.len() >= MAX_SEEN_MESSAGES {
            self.seen_messages.clear();
            for entry in &self.history {
                self.seen_messages.insert((entry.sender_uuid, entry.msg_id));
            }
        }
        self.seen_messages.insert(key)
    }

    fn active_download_count(&self) -> usize {
        self.remote_blobs
            .values()
            .filter(|b| b.download.is_some())
            .count()
    }

    fn clear_pending_avatar_for_sender(
        &mut self,
        sender_uuid: ClientUuid,
        keep_blob_id: Option<[u8; 16]>,
    ) {
        let stale_ids: Vec<[u8; 16]> = self
            .avatar_targets
            .iter()
            .filter(|(&id, (uuid, _))| *uuid == sender_uuid && Some(id) != keep_blob_id)
            .map(|(&id, _)| id)
            .collect();
        for stale_id in stale_ids {
            self.avatar_targets.remove(&stale_id);
            if self
                .remote_blobs
                .get(&stale_id)
                .is_some_and(|b| !b.completed)
            {
                self.remote_blobs.remove(&stale_id);
            }
        }
    }

    fn next_msg_id(&mut self, local_peer_id: PeerId) -> u64 {
        let seq = self.local_msg_counter & 0x0000_FFFF_FFFF_FFFF;
        self.local_msg_counter = self.local_msg_counter.wrapping_add(1);
        ((local_peer_id as u64) << 48) | seq
    }

    fn push_history(&mut self, entry: HistoryEntry) {
        if self
            .history
            .iter()
            .any(|e| e.sender_uuid == entry.sender_uuid && e.msg_id == entry.msg_id)
        {
            return;
        }
        let mut list: Vec<HistoryEntry> = self.history.drain(..).collect();
        list.push(entry);
        list.sort_by_key(|e| (e.timestamp_ms, e.msg_id));
        if list.len() > HISTORY_LIMIT {
            let drop_n = list.len() - HISTORY_LIMIT;
            list.drain(0..drop_n);
        }
        self.history = VecDeque::from(list);
        self.persist_history_to_disk();
    }

    fn persist_history_to_disk(&self) {
        let (Some(root), Some(room_id)) = (&self.data_root, &self.room_id) else {
            return;
        };
        let entries: Vec<HistoryEntry> = self.history.iter().cloned().collect();
        let _ = write_room_history_file(root, room_id, &entries);
    }

    fn record_downloaded_blob(
        &mut self,
        blob_id: [u8; 16],
        path: PathBuf,
        total_size: u64,
        blake3_root: [u8; 32],
    ) {
        self.downloaded_index.insert(
            blob_id,
            DownloadedBlobRecord {
                blob_id,
                path,
                total_size,
                blake3_root,
            },
        );
        if let Some(root) = &self.data_root {
            let records: Vec<DownloadedBlobRecord> =
                self.downloaded_index.values().cloned().collect();
            let _ = write_downloaded_blobs_index(root, &records);
        }
    }

    pub async fn send_chat_text(
        &mut self,
        text: String,
        connected_peers: &[(PeerId, PeerEndpoint)],
        ctx: &mut TransferCtx<'_>,
    ) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let msg_id = self.next_msg_id(ctx.local_peer_id);
        let timestamp_ms = now_millis();
        let text_owned = trimmed.to_string();

        self.mark_seen_message((ctx.local_uuid, msg_id));
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
        connected_peers: &[(PeerId, PeerEndpoint)],
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
        if let Some(root) = &self.data_root {
            let _ = save_cached_inline_image(root, &blob_id, &bytes);
        }
        self.mark_seen_message((ctx.local_uuid, msg_id));
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
        connected_peers: &[(PeerId, PeerEndpoint)],
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
        self.record_downloaded_blob(blob_id, path.clone(), total_size, blake3_root);
        self.mark_seen_message((ctx.local_uuid, msg_id));
        self.push_history(HistoryEntry {
            sender_uuid: ctx.local_uuid,
            sender_nickname: ctx.local_nickname.to_string(),
            msg_id,
            timestamp_ms,
            payload: HistoryPayload::BlobOffer {
                blob_id,
                kind: BLOB_KIND_FILE,
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
            kind: BLOB_KIND_FILE,
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

    pub async fn announce_haves_to_peer(&self, peer_addr: PeerEndpoint, ctx: &mut TransferCtx<'_>) {
        for &blob_id in self.local_blobs.keys() {
            ctx.send_p2p(peer_addr, &P2pPayload::BlobHave { blob_id })
                .await;
        }
    }

    pub async fn send_history_sync_req(&self, peer_addr: PeerEndpoint, ctx: &mut TransferCtx<'_>) {
        ctx.send_p2p(peer_addr, &P2pPayload::HistorySyncReq).await;
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn handle_payload(
        &mut self,
        sender_peer_id: PeerId,
        sender_uuid: ClientUuid,
        sender_nickname: &str,
        sender_addr: PeerEndpoint,
        payload: P2pPayload,
        connected_peers: &[(PeerId, PeerEndpoint)],
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

                if !self.mark_seen_message((sender_uuid, msg_id)) {
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
                for delivery in &mut self.pending_deliveries {
                    if delivery.msg_id == msg_id {
                        delivery.acked_by.insert(sender_uuid);
                    }
                }
            }
            P2pPayload::HistorySyncReq => {
                if self.pending_reliable.len() >= MAX_PENDING_RELIABLE
                    || self
                        .pending_reliable
                        .iter()
                        .any(|p| p.target_peer == sender_peer_id)
                {
                    return;
                }
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

                if !self.mark_seen_message((sender_uuid, msg_id)) {
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

                let should_auto = kind == 1
                    && auto_download
                    && total_size < INLINE_IMAGE_MAX_BYTES as u64
                    && self.active_download_count() < MAX_ACTIVE_DOWNLOADS;
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
                    self.clear_pending_avatar_for_sender(sender_uuid, None);
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
                if let Some(cached) = crate::avatar_core::load_cached_avatar_jpeg(&avatar_hash) {
                    self.clear_pending_avatar_for_sender(sender_uuid, None);
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
                self.clear_pending_avatar_for_sender(sender_uuid, Some(blob_id));
                let active_downloads = self.active_download_count();
                if let Some(existing) = self.remote_blobs.get_mut(&blob_id) {
                    existing.seeders.insert(sender_peer_id);
                    if !existing.completed {
                        if existing.download.is_none() && active_downloads < MAX_ACTIVE_DOWNLOADS {
                            existing.download = init_download(&existing.meta).ok();
                        }
                        self.avatar_targets
                            .insert(blob_id, (sender_uuid, avatar_hash));
                    }
                    return;
                }
                let meta = BlobMetadata {
                    blob_id,
                    kind: BLOB_KIND_AVATAR,
                    filename: "avatar.jpg".to_string(),
                    total_size: total_size as u64,
                    total_chunks: total_chunks as u32,
                    blake3_root: avatar_hash,
                };
                let download = if active_downloads < MAX_ACTIVE_DOWNLOADS {
                    init_download(&meta).ok()
                } else {
                    None
                };
                let mut seeders = HashSet::new();
                seeders.insert(sender_peer_id);
                self.remote_blobs.insert(
                    blob_id,
                    TrackedBlob {
                        meta,
                        seeders,
                        seeder_rr_cursor: 0,
                        download,
                        completed: false,
                    },
                );
                self.avatar_targets
                    .insert(blob_id, (sender_uuid, avatar_hash));
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
                self.handle_incoming_chunk(
                    sender_peer_id,
                    blob_id,
                    chunk_index,
                    &data,
                    connected_peers,
                    ctx,
                )
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
        sender_peer_id: PeerId,
        blob_id: [u8; 16],
        chunk_index: u32,
        data: &[u8],
        connected_peers: &[(PeerId, PeerEndpoint)],
        ctx: &mut TransferCtx<'_>,
    ) {
        let Some(blob) = self.remote_blobs.get_mut(&blob_id) else {
            return;
        };
        if blob.completed || !blob.seeders.contains(&sender_peer_id) {
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
        let Some(rem) = blob.meta.total_size.checked_sub(offset) else {
            return;
        };
        let expected_len = (rem.min(BLOB_CHUNK_SIZE as u64)) as usize;
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
                        let _ = crate::avatar_core::save_cached_avatar_jpeg(&meta.blake3_root, &mem);
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
                        if let Some(root) = &self.data_root {
                            let _ = save_cached_inline_image(root, &blob_id, &mem);
                        }
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
                        self.record_downloaded_blob(
                            blob_id,
                            final_path.clone(),
                            meta.total_size,
                            meta.blake3_root,
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
        peer_addrs: &HashMap<PeerId, PeerEndpoint>,
        peer_uuids: &HashMap<PeerId, ClientUuid>,
        expected_room_uuids: &[ClientUuid],
        ctx: &mut TransferCtx<'_>,
    ) {
        if ctx.local_peer_id == 0 {
            return;
        }

        let mut sent_chunks = 0;
        while sent_chunks < QOS_MAX_CHUNKS_PER_TICK {
            let Some(key @ (target_peer, blob_id, chunk_index)) = self.outbound_chunks.pop_front()
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

        self.pending_deliveries.retain(|d| {
            let all_acked = !expected_room_uuids.is_empty()
                && expected_room_uuids.iter().all(|u| d.acked_by.contains(u));
            !all_acked
                && d.attempts_remaining > 0
                && now.duration_since(d.created_at) < DELIVERY_MAX_AGE
        });

        for idx in 0..self.pending_deliveries.len() {
            if now >= self.pending_deliveries[idx].next_send_at {
                let payload = self.pending_deliveries[idx].payload.clone();
                let acked_by = &self.pending_deliveries[idx].acked_by;
                for (&pid, &dst) in peer_addrs {
                    let already_acked = peer_uuids.get(&pid).is_some_and(|u| acked_by.contains(u));
                    if !already_acked {
                        ctx.send_p2p(dst, &payload).await;
                    }
                }
                let item = &mut self.pending_deliveries[idx];
                item.attempts_remaining = item.attempts_remaining.saturating_sub(1);
                item.next_send_at = now + RETRY_INTERVAL;
            }
        }

        if self.history_sync_attempts_left > 0
            && !peer_addrs.is_empty()
            && now >= self.history_sync_next_at
        {
            for &dst in peer_addrs.values() {
                ctx.send_p2p(dst, &P2pPayload::HistorySyncReq).await;
            }
            self.history_sync_attempts_left = self.history_sync_attempts_left.saturating_sub(1);
            self.history_sync_next_at = now + RETRY_INTERVAL;
        }

        let mut idx = 0;
        while idx < self.pending_reliable.len() {
            let due = now >= self.pending_reliable[idx].next_send_at;
            if due {
                if self.pending_reliable[idx].retries_left == 0 {
                    let dropped = self.pending_reliable.swap_remove(idx);
                    debug!(
                        "Dropping unacked sync msg {} for peer {}",
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

        let mut reqs: Vec<(PeerEndpoint, P2pPayload)> = Vec::new();
        for blob in self.remote_blobs.values_mut() {
            if blob.completed {
                continue;
            }
            let Some(dl) = blob.download.as_mut() else {
                continue;
            };
            let mut active_seeders: Vec<(PeerId, PeerEndpoint)> = blob
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
        connected_peers: &[(PeerId, PeerEndpoint)],
        ctx: &mut TransferCtx<'_>,
    ) {
        let now = Instant::now();
        for &(_, addr) in connected_peers {
            ctx.send_p2p(addr, payload).await;
        }
        self.pending_deliveries.push(PendingRoomDelivery {
            msg_id,
            payload: payload.clone(),
            acked_by: HashSet::new(),
            created_at: now,
            next_send_at: now + RETRY_INTERVAL,
            attempts_remaining: MAX_RETRIES,
        });
    }

    async fn broadcast_have(
        &self,
        blob_id: [u8; 16],
        connected_peers: &[(PeerId, PeerEndpoint)],
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

pub fn app_data_root_dir() -> PathBuf {
    crate::storage_paths::app_config_root_dir()
}

pub fn downloads_root_dir() -> PathBuf {
    crate::storage_paths::app_downloads_root_dir()
}

fn room_history_path(root: &Path, room_id: &RoomId) -> PathBuf {
    root.join("history")
        .join(format!("{}.json", bs58::encode(room_id).into_string()))
}

fn inline_cache_path(root: &Path, blob_id: &[u8; 16]) -> PathBuf {
    root.join("cache")
        .join("inline")
        .join(format!("{}.bin", bytes_to_hex(blob_id)))
}

fn downloaded_blobs_index_path(root: &Path) -> PathBuf {
    root.join("history").join("downloaded_blobs.json")
}

fn save_cached_inline_image(root: &Path, blob_id: &[u8; 16], bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() || bytes.len() >= INLINE_IMAGE_MAX_BYTES {
        return Ok(());
    }
    let path = inline_cache_path(root, blob_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(path, bytes).map_err(|e| e.to_string())
}

fn load_cached_inline_image(
    root: &Path,
    blob_id: &[u8; 16],
    blake3_root: &[u8; 32],
) -> Option<Vec<u8>> {
    let bytes = fs::read(inline_cache_path(root, blob_id)).ok()?;
    if bytes.is_empty() || bytes.len() >= INLINE_IMAGE_MAX_BYTES {
        return None;
    }
    if *blake3::hash(&bytes).as_bytes() == *blake3_root {
        Some(bytes)
    } else {
        None
    }
}

fn write_downloaded_blobs_index(
    root: &Path,
    records: &[DownloadedBlobRecord],
) -> Result<(), String> {
    let path = downloaded_blobs_index_path(root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut out = String::from("[\n");
    for (idx, rec) in records.iter().enumerate() {
        if idx > 0 {
            out.push_str(",\n");
        }
        let path_str = rec.path.to_string_lossy();
        out.push_str(&format!(
            "  {{\"blob_id\":\"{}\",\"path\":\"{}\",\"total_size\":{},\"blake3_root\":\"{}\"}}",
            bytes_to_hex(&rec.blob_id),
            json_escape(&path_str),
            rec.total_size,
            bytes_to_hex(&rec.blake3_root),
        ));
    }
    out.push_str("\n]\n");
    fs::write(path, out).map_err(|e| e.to_string())
}

fn read_downloaded_blobs_index(root: &Path) -> Vec<DownloadedBlobRecord> {
    let Ok(raw) = fs::read_to_string(downloaded_blobs_index_path(root)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for obj in parse_json_object_array(&raw) {
        let Some(blob_hex) = obj.get("blob_id").and_then(JsonVal::as_str) else {
            continue;
        };
        let Some(path_str) = obj.get("path").and_then(JsonVal::as_str) else {
            continue;
        };
        let Some(total_size) = obj.get("total_size").and_then(JsonVal::as_u64) else {
            continue;
        };
        let Some(hash_hex) = obj.get("blake3_root").and_then(JsonVal::as_str) else {
            continue;
        };
        if let (Some(blob_id), Some(blake3_root)) = (hex_to_16(blob_hex), hex_to_32(hash_hex)) {
            out.push(DownloadedBlobRecord {
                blob_id,
                path: PathBuf::from(path_str),
                total_size,
                blake3_root,
            });
        }
    }
    out
}

fn write_room_history_file(
    root: &Path,
    room_id: &RoomId,
    entries: &[HistoryEntry],
) -> Result<(), String> {
    let path = room_history_path(root, room_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut out = String::from("[\n");
    for (idx, e) in entries.iter().enumerate() {
        if idx > 0 {
            out.push_str(",\n");
        }
        let sender_hex = bytes_to_hex(&e.sender_uuid);
        let nick_esc = json_escape(&e.sender_nickname);
        match &e.payload {
            HistoryPayload::Text(text) => {
                out.push_str(&format!(
                    "  {{\"msg_id\":{},\"sender_uuid\":\"{}\",\"sender_nickname\":\"{}\",\"timestamp_ms\":{},\"kind\":0,\"text\":\"{}\"}}",
                    e.msg_id,
                    sender_hex,
                    nick_esc,
                    e.timestamp_ms,
                    json_escape(text),
                ));
            }
            HistoryPayload::BlobOffer {
                blob_id,
                kind,
                auto_download,
                filename,
                total_size,
                chunk_size,
                total_chunks,
                blake3_root,
            } => {
                out.push_str(&format!(
                    "  {{\"msg_id\":{},\"sender_uuid\":\"{}\",\"sender_nickname\":\"{}\",\"timestamp_ms\":{},\"kind\":{},\"blob_id\":\"{}\",\"auto_download\":{},\"filename\":\"{}\",\"total_size\":{},\"chunk_size\":{},\"total_chunks\":{},\"blake3_root\":\"{}\"}}",
                    e.msg_id,
                    sender_hex,
                    nick_esc,
                    e.timestamp_ms,
                    kind,
                    bytes_to_hex(blob_id),
                    auto_download,
                    json_escape(filename),
                    total_size,
                    chunk_size,
                    total_chunks,
                    bytes_to_hex(blake3_root),
                ));
            }
        }
    }
    out.push_str("\n]\n");
    fs::write(path, out).map_err(|e| e.to_string())
}

fn read_room_history_file(root: &Path, room_id: &RoomId) -> Vec<HistoryEntry> {
    let Ok(raw) = fs::read_to_string(room_history_path(root, room_id)) else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for obj in parse_json_object_array(&raw) {
        let Some(msg_id) = obj.get("msg_id").and_then(JsonVal::as_u64) else {
            continue;
        };
        let Some(sender_uuid) = obj
            .get("sender_uuid")
            .and_then(JsonVal::as_str)
            .and_then(hex_to_16)
        else {
            continue;
        };
        let sender_nickname = obj
            .get("sender_nickname")
            .and_then(JsonVal::as_str)
            .unwrap_or("Peer")
            .to_string();
        let Some(timestamp_ms) = obj.get("timestamp_ms").and_then(JsonVal::as_u64) else {
            continue;
        };
        let Some(raw_kind) = obj.get("kind").and_then(JsonVal::as_u64) else {
            continue;
        };

        let payload = if raw_kind == 0 {
            let text = obj
                .get("text")
                .and_then(JsonVal::as_str)
                .unwrap_or("")
                .to_string();
            HistoryPayload::Text(text)
        } else {
            let Ok(kind) = u8::try_from(raw_kind) else {
                continue;
            };
            let Some(blob_id) = obj
                .get("blob_id")
                .and_then(JsonVal::as_str)
                .and_then(hex_to_16)
            else {
                continue;
            };
            let auto_download = obj
                .get("auto_download")
                .and_then(JsonVal::as_bool)
                .unwrap_or(kind == 1);
            let filename = obj
                .get("filename")
                .and_then(JsonVal::as_str)
                .unwrap_or("attachment")
                .to_string();
            let Some(total_size) = obj.get("total_size").and_then(JsonVal::as_u64) else {
                continue;
            };
            let raw_chunk_size = obj
                .get("chunk_size")
                .and_then(JsonVal::as_u64)
                .unwrap_or(BLOB_CHUNK_SIZE as u64);
            let Ok(chunk_size) = u16::try_from(raw_chunk_size) else {
                continue;
            };
            let raw_total_chunks = obj
                .get("total_chunks")
                .and_then(JsonVal::as_u64)
                .unwrap_or(1);
            let Ok(total_chunks) = u32::try_from(raw_total_chunks) else {
                continue;
            };
            if total_size == 0
                || total_size > MAX_DOWNLOAD_FILE_BYTES
                || (chunk_size as usize) != BLOB_CHUNK_SIZE
                || (total_chunks as u64) != total_size.div_ceil(BLOB_CHUNK_SIZE as u64)
            {
                continue;
            }
            let Some(blake3_root) = obj
                .get("blake3_root")
                .and_then(JsonVal::as_str)
                .and_then(hex_to_32)
            else {
                continue;
            };
            HistoryPayload::BlobOffer {
                blob_id,
                kind,
                auto_download,
                filename,
                total_size,
                chunk_size,
                total_chunks,
                blake3_root,
            }
        };

        entries.push(HistoryEntry {
            sender_uuid,
            sender_nickname,
            msg_id,
            timestamp_ms,
            payload,
        });
    }
    entries
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut s, "{b:02x}");
    }
    s
}

fn hex_to_16(hex: &str) -> Option<[u8; 16]> {
    let t = hex.trim();
    if t.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(t.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

fn hex_to_32(hex: &str) -> Option<[u8; 32]> {
    let t = hex.trim();
    if t.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(t.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                use std::fmt::Write;
                let _ = write!(&mut out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

enum JsonVal {
    Str(String),
    Num(u64),
    Bool(bool),
}

impl JsonVal {
    fn as_str(&self) -> Option<&str> {
        match self {
            JsonVal::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }
    fn as_u64(&self) -> Option<u64> {
        match self {
            JsonVal::Num(n) => Some(*n),
            _ => None,
        }
    }
    fn as_bool(&self) -> Option<bool> {
        match self {
            JsonVal::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

fn parse_json_object_array(input: &str) -> Vec<HashMap<String, JsonVal>> {
    let bytes = input.as_bytes();
    let mut i = 0usize;
    let skip_ws = |idx: &mut usize| {
        while *idx < bytes.len() && bytes[*idx].is_ascii_whitespace() {
            *idx += 1;
        }
    };

    skip_ws(&mut i);
    if i >= bytes.len() || bytes[i] != b'[' {
        return Vec::new();
    }
    i += 1;

    let mut objs = Vec::new();
    loop {
        skip_ws(&mut i);
        if i >= bytes.len() || bytes[i] == b']' {
            break;
        }
        if bytes[i] == b',' {
            i += 1;
            continue;
        }
        if bytes[i] != b'{' {
            break;
        }
        i += 1;
        let mut map = HashMap::new();
        loop {
            skip_ws(&mut i);
            if i >= bytes.len() || bytes[i] == b'}' {
                i += 1;
                break;
            }
            if bytes[i] == b',' {
                i += 1;
                continue;
            }
            let Some(key) = parse_json_string(input, &mut i) else {
                return objs;
            };
            skip_ws(&mut i);
            if i >= bytes.len() || bytes[i] != b':' {
                return objs;
            }
            i += 1;
            skip_ws(&mut i);
            if i >= bytes.len() {
                return objs;
            }
            let val = if bytes[i] == b'"' {
                let Some(s) = parse_json_string(input, &mut i) else {
                    return objs;
                };
                JsonVal::Str(s)
            } else if input[i..].starts_with("true") {
                i += 4;
                JsonVal::Bool(true)
            } else if input[i..].starts_with("false") {
                i += 5;
                JsonVal::Bool(false)
            } else {
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i == start {
                    return objs;
                }
                let n = input[start..i].parse::<u64>().unwrap_or(0);
                JsonVal::Num(n)
            };
            map.insert(key, val);
        }
        objs.push(map);
    }
    objs
}

fn parse_json_string(input: &str, idx: &mut usize) -> Option<String> {
    let rest = input.get(*idx..)?;
    if !rest.starts_with('"') {
        return None;
    }
    let mut chars = rest[1..].char_indices();
    let mut out = String::new();
    while let Some((off, ch)) = chars.next() {
        match ch {
            '"' => {
                *idx += 1 + off + 1;
                return Some(out);
            }
            '\\' => {
                let (_, esc) = chars.next()?;
                match esc {
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    '/' => out.push('/'),
                    'b' => out.push('\u{0008}'),
                    'f' => out.push('\u{000C}'),
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    'u' => {
                        let mut hex = [0u8; 4];
                        for slot in &mut hex {
                            let (_, h) = chars.next()?;
                            if !h.is_ascii_hexdigit() {
                                return None;
                            }
                            *slot = h as u8;
                        }
                        let hex_str = std::str::from_utf8(&hex).ok()?;
                        let code = u32::from_str_radix(hex_str, 16).ok()?;
                        if let Some(decoded) = char::from_u32(code) {
                            out.push(decoded);
                        }
                    }
                    other => out.push(other),
                }
            }
            other => out.push(other),
        }
    }
    None
}

fn is_windows_reserved_stem(stem: &str) -> bool {
    let upper = stem.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn sanitize_filename(raw: &str) -> String {
    let base = Path::new(raw)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let cleaned: String = base
        .chars()
        .filter(|&c| {
            !matches!(
                c,
                '\u{202A}'..='\u{202E}'
                    | '\u{2066}'..='\u{2069}'
                    | '\u{200E}'
                    | '\u{200F}'
                    | '\u{FEFF}'
            )
        })
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            other => other,
        })
        .collect();
    let trimmed = cleaned.trim_matches(|c| c == '.' || c == ' ');
    if trimmed.is_empty() {
        return "received_file".to_string();
    }
    let stem = trimmed
        .split('.')
        .next()
        .unwrap_or(trimmed)
        .trim_end_matches(' ');
    if is_windows_reserved_stem(stem) {
        format!("file_{trimmed}")
    } else {
        trimmed.to_string()
    }
}

fn unique_destination_path(dir: &Path, filename: &str) -> PathBuf {
    let candidate = dir.join(filename);
    if candidate.symlink_metadata().is_err() {
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
        if p.symlink_metadata().is_err() {
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
        connected_list: &[(PeerId, PeerEndpoint)],
        ctx: &mut TransferCtx<'_>,
    ) {
        let mut rx_buf = [0u8; MAX_UDP_PAYLOAD + 64];
        while let Ok(Ok((len, src))) =
            tokio::time::timeout(Duration::from_millis(5), ctx.socket.recv_from(&mut rx_buf)).await
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
                    PeerEndpoint::Direct(src),
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

        let ab_list = vec![(2, PeerEndpoint::Direct(addr_b))];
        let ba_list = vec![(1, PeerEndpoint::Direct(addr_a))];

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
        ab_map.insert(2, PeerEndpoint::Direct(addr_b));
        let mut ba_map = HashMap::new();
        ba_map.insert(1, PeerEndpoint::Direct(addr_a));
        let mut uuids = HashMap::new();
        uuids.insert(1u16, [0x11; 16]);
        uuids.insert(2u16, [0x22; 16]);
        uuids.insert(3u16, [0x33; 16]);

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
                mgr_b
                    .tick_10ms(&ba_map, &uuids, &[[0x11; 16]], &mut ctx_b)
                    .await;
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
                mgr_a
                    .tick_10ms(&ab_map, &uuids, &[[0x22; 16]], &mut ctx_a)
                    .await;
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
        assert!(mgr_a.pending_deliveries.is_empty());

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
            mgr_c
                .send_history_sync_req(PeerEndpoint::Direct(addr_b), &mut ctx_c)
                .await;
        }

        let bc_list = vec![
            (1, PeerEndpoint::Direct(addr_a)),
            (3, PeerEndpoint::Direct(addr_c)),
        ];
        let cb_list = vec![
            (1, PeerEndpoint::Direct(addr_a)),
            (2, PeerEndpoint::Direct(addr_b)),
        ];
        let mut bc_map = HashMap::new();
        bc_map.insert(1, PeerEndpoint::Direct(addr_a));
        bc_map.insert(3, PeerEndpoint::Direct(addr_c));
        let mut cb_map = HashMap::new();
        cb_map.insert(1, PeerEndpoint::Direct(addr_a));
        cb_map.insert(2, PeerEndpoint::Direct(addr_b));

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
                mgr_b
                    .tick_10ms(&bc_map, &uuids, &[[0x11; 16], [0x33; 16]], &mut ctx_b)
                    .await;
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
                mgr_c
                    .tick_10ms(&cb_map, &uuids, &[[0x11; 16], [0x22; 16]], &mut ctx_c)
                    .await;
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
            mgr_a
                .offer_file(tmp_src.clone(), &ab_list, &mut ctx_a)
                .await;
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
                mgr_b
                    .tick_10ms(&ba_map, &uuids, &[[0x11; 16]], &mut ctx_b)
                    .await;
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
                mgr_a
                    .tick_10ms(&ab_map, &uuids, &[[0x22; 16]], &mut ctx_a)
                    .await;
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

    async fn pump_socket_with_uuids(
        mgr: &mut SwarmChatManager,
        peer_nicknames: &HashMap<PeerId, String>,
        peer_uuids: &HashMap<PeerId, ClientUuid>,
        connected_list: &[(PeerId, PeerEndpoint)],
        ctx: &mut TransferCtx<'_>,
    ) {
        let mut rx_buf = [0u8; MAX_UDP_PAYLOAD + 64];
        while let Ok(Ok((len, src))) =
            tokio::time::timeout(Duration::from_millis(5), ctx.socket.recv_from(&mut rx_buf)).await
        {
            if let Ok((hdr, payload)) = ctx.cipher.open_packet(&mut rx_buf[..len]) {
                let nick = peer_nicknames
                    .get(&hdr.sender_peer_id)
                    .map(String::as_str)
                    .unwrap_or("Peer");
                let sender_uuid = peer_uuids
                    .get(&hdr.sender_peer_id)
                    .copied()
                    .unwrap_or([0x99; 16]);
                mgr.handle_payload(
                    hdr.sender_peer_id,
                    sender_uuid,
                    nick,
                    PeerEndpoint::Direct(src),
                    payload,
                    connected_list,
                    ctx,
                )
                .await;
            }
        }
    }

    #[tokio::test]
    async fn test_hybrid_avatar_p2p_sync_and_blake3_cache() {
        let img = image::RgbaImage::from_fn(160, 160, |x, y| {
            image::Rgba([(x % 255) as u8, (y % 255) as u8, 180, 255])
        });
        let mut raw_png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut raw_png),
                image::ImageFormat::Png,
            )
            .unwrap();
        let (jpeg_bytes, avatar_hash, _rgba128) =
            crate::avatar_core::process_custom_avatar_image(&raw_png).unwrap();
        let _ = fs::remove_file(crate::avatar_core::avatar_disk_path(&avatar_hash));

        let invite = InviteLink::generate();
        let prefix = invite.room_prefix();
        let cipher = RoomCipher::from_secret(&invite.secret);

        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let uuid_a = [0xA1; 16];
        let uuid_b = [0xB2; 16];

        let mut mgr_a = SwarmChatManager::new();
        let mut mgr_b = SwarmChatManager::new();

        let (tx_a, mut _rx_a) = mpsc::channel(64);
        let (tx_b, mut rx_b) = mpsc::channel(64);
        let mut seq_a = 1u64;
        let mut seq_b = 1u64;
        let mut seal_a = Vec::with_capacity(MAX_UDP_PAYLOAD);
        let mut seal_b = Vec::with_capacity(MAX_UDP_PAYLOAD);

        let ab_list = vec![(2, PeerEndpoint::Direct(addr_b))];
        let ba_list = vec![(1, PeerEndpoint::Direct(addr_a))];
        let mut ab_map = HashMap::new();
        ab_map.insert(2, PeerEndpoint::Direct(addr_b));
        let mut ba_map = HashMap::new();
        ba_map.insert(1, PeerEndpoint::Direct(addr_a));

        let mut nicks = HashMap::new();
        nicks.insert(1, "Alpha".to_string());
        nicks.insert(2, "Bravo".to_string());

        let mut peer_uuids = HashMap::new();
        peer_uuids.insert(1, uuid_a);
        peer_uuids.insert(2, uuid_b);

        {
            let mut ctx_a = TransferCtx {
                local_peer_id: 1,
                local_uuid: uuid_a,
                local_nickname: "Alpha",
                socket: &sock_a,
                cipher: &cipher,
                room_prefix: prefix,
                control_seq: &mut seq_a,
                seal_buf: &mut seal_a,
                evt_tx: &tx_a,
            };
            mgr_a
                .set_local_avatar(
                    Some((avatar_hash, jpeg_bytes.clone())),
                    &ab_list,
                    &mut ctx_a,
                )
                .await;
        }

        let mut loaded_custom = None;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(12)).await;
            {
                let mut ctx_b = TransferCtx {
                    local_peer_id: 2,
                    local_uuid: uuid_b,
                    local_nickname: "Bravo",
                    socket: &sock_b,
                    cipher: &cipher,
                    room_prefix: prefix,
                    control_seq: &mut seq_b,
                    seal_buf: &mut seal_b,
                    evt_tx: &tx_b,
                };
                pump_socket_with_uuids(&mut mgr_b, &nicks, &peer_uuids, &ba_list, &mut ctx_b).await;
                mgr_b
                    .tick_10ms(&ba_map, &peer_uuids, &[uuid_a], &mut ctx_b)
                    .await;
            }
            {
                let mut ctx_a = TransferCtx {
                    local_peer_id: 1,
                    local_uuid: uuid_a,
                    local_nickname: "Alpha",
                    socket: &sock_a,
                    cipher: &cipher,
                    room_prefix: prefix,
                    control_seq: &mut seq_a,
                    seal_buf: &mut seal_a,
                    evt_tx: &tx_a,
                };
                pump_socket_with_uuids(&mut mgr_a, &nicks, &peer_uuids, &ab_list, &mut ctx_a).await;
                mgr_a
                    .tick_10ms(&ab_map, &peer_uuids, &[uuid_b], &mut ctx_a)
                    .await;
            }

            while let Ok(ev) = rx_b.try_recv() {
                if let TransferEvent::PeerAvatarLoaded {
                    client_uuid,
                    avatar_hash: h,
                    jpeg_bytes: Some(bytes),
                } = ev
                {
                    loaded_custom = Some((client_uuid, h, bytes));
                }
            }
            if loaded_custom.is_some() {
                break;
            }
        }

        let (got_uuid, got_hash, got_jpeg) =
            loaded_custom.expect("Peer B must download custom avatar over P2P");
        assert_eq!(got_uuid, uuid_a);
        assert_eq!(got_hash, avatar_hash);
        assert_eq!(got_jpeg, jpeg_bytes);
        assert_eq!(
            crate::avatar_core::load_cached_avatar_jpeg(&avatar_hash).as_deref(),
            Some(jpeg_bytes.as_slice())
        );

        // Reset to Identicon and verify [0u8; 32] broadcast
        {
            let mut ctx_a = TransferCtx {
                local_peer_id: 1,
                local_uuid: uuid_a,
                local_nickname: "Alpha",
                socket: &sock_a,
                cipher: &cipher,
                room_prefix: prefix,
                control_seq: &mut seq_a,
                seal_buf: &mut seal_a,
                evt_tx: &tx_a,
            };
            mgr_a.set_local_avatar(None, &ab_list, &mut ctx_a).await;
        }
        {
            let mut ctx_b = TransferCtx {
                local_peer_id: 2,
                local_uuid: uuid_b,
                local_nickname: "Bravo",
                socket: &sock_b,
                cipher: &cipher,
                room_prefix: prefix,
                control_seq: &mut seq_b,
                seal_buf: &mut seal_b,
                evt_tx: &tx_b,
            };
            pump_socket_with_uuids(&mut mgr_b, &nicks, &peer_uuids, &ba_list, &mut ctx_b).await;
        }

        let mut reset_ok = false;
        while let Ok(ev) = rx_b.try_recv() {
            if let TransferEvent::PeerAvatarLoaded {
                client_uuid,
                avatar_hash: h,
                jpeg_bytes: None,
            } = ev
            {
                if client_uuid == uuid_a && h == [0u8; 32] {
                    reset_ok = true;
                }
            }
        }
        assert!(reset_ok, "Peer B must receive Identicon reset notification");

        let _ = fs::remove_file(crate::avatar_core::avatar_disk_path(&avatar_hash));
    }

    #[tokio::test]
    async fn test_persistent_room_history_rejoin_author_preservation_and_seeding() {
        let invite = InviteLink::generate();
        let prefix = invite.room_prefix();
        let cipher = RoomCipher::from_secret(&invite.secret);

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp_root = std::env::temp_dir().join(format!("squad_hist_test_{nonce}"));
        fs::create_dir_all(&tmp_root).unwrap();

        let src_file = tmp_root.join("reseed_payload.bin");
        let file_bytes = vec![0x5Au8; 4096];
        fs::write(&src_file, &file_bytes).unwrap();

        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let uuid_a = [0xA1; 16];
        let uuid_b = [0xB2; 16];

        let mut mgr_a = SwarmChatManager::new();
        let mut mgr_b = SwarmChatManager::new();
        mgr_b.set_room_persistence(invite.room_id, tmp_root.clone());

        let (tx_a, mut _rx_a) = mpsc::channel(128);
        let (tx_b, mut rx_b) = mpsc::channel(128);
        let mut seq_a = 1u64;
        let mut seq_b = 1u64;
        let mut seal_a = Vec::with_capacity(MAX_UDP_PAYLOAD);
        let mut seal_b = Vec::with_capacity(MAX_UDP_PAYLOAD);

        let ab_list = vec![(2, PeerEndpoint::Direct(addr_b))];
        let ba_list = vec![(1, PeerEndpoint::Direct(addr_a))];
        let mut ab_map = HashMap::new();
        ab_map.insert(2, PeerEndpoint::Direct(addr_b));
        let mut ba_map = HashMap::new();
        ba_map.insert(1, PeerEndpoint::Direct(addr_a));

        let mut nicks = HashMap::new();
        nicks.insert(1, "Alpha".to_string());
        nicks.insert(2, "Bravo".to_string());
        nicks.insert(9, "Alpha".to_string());

        let mut peer_uuids = HashMap::new();
        peer_uuids.insert(1, uuid_a);
        peer_uuids.insert(2, uuid_b);
        peer_uuids.insert(9, uuid_a);

        {
            let mut ctx_a = TransferCtx {
                local_peer_id: 1,
                local_uuid: uuid_a,
                local_nickname: "Alpha",
                socket: &sock_a,
                cipher: &cipher,
                room_prefix: prefix,
                control_seq: &mut seq_a,
                seal_buf: &mut seal_a,
                evt_tx: &tx_a,
            };
            for i in 0..34u32 {
                mgr_a
                    .send_chat_text(format!("Alpha msg #{i}"), &ab_list, &mut ctx_a)
                    .await;
            }
            mgr_a
                .offer_file(src_file.clone(), &ab_list, &mut ctx_a)
                .await;
        }

        let mut offered_blob_id = None;
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(12)).await;
            {
                let mut ctx_b = TransferCtx {
                    local_peer_id: 2,
                    local_uuid: uuid_b,
                    local_nickname: "Bravo",
                    socket: &sock_b,
                    cipher: &cipher,
                    room_prefix: prefix,
                    control_seq: &mut seq_b,
                    seal_buf: &mut seal_b,
                    evt_tx: &tx_b,
                };
                pump_socket_with_uuids(&mut mgr_b, &nicks, &peer_uuids, &ba_list, &mut ctx_b).await;
            }
            while let Ok(ev) = rx_b.try_recv() {
                if let TransferEvent::ChatItemReceived(item) = ev {
                    if let ChatContentUi::File { blob_id, .. } = item.content {
                        offered_blob_id = Some(blob_id);
                    }
                }
            }
            if offered_blob_id.is_some() {
                break;
            }
        }
        let bid = offered_blob_id.expect("Peer B must receive file offer");
        {
            let mut ctx_b = TransferCtx {
                local_peer_id: 2,
                local_uuid: uuid_b,
                local_nickname: "Bravo",
                socket: &sock_b,
                cipher: &cipher,
                room_prefix: prefix,
                control_seq: &mut seq_b,
                seal_buf: &mut seal_b,
                evt_tx: &tx_b,
            };
            mgr_b
                .send_chat_text("Reply from Bravo".to_string(), &ba_list, &mut ctx_b)
                .await;
        }
        mgr_b.start_download(bid, &tx_b).await;

        let mut saved_download: Option<PathBuf> = None;
        for _ in 0..45 {
            tokio::time::sleep(Duration::from_millis(12)).await;
            {
                let mut ctx_b = TransferCtx {
                    local_peer_id: 2,
                    local_uuid: uuid_b,
                    local_nickname: "Bravo",
                    socket: &sock_b,
                    cipher: &cipher,
                    room_prefix: prefix,
                    control_seq: &mut seq_b,
                    seal_buf: &mut seal_b,
                    evt_tx: &tx_b,
                };
                pump_socket_with_uuids(&mut mgr_b, &nicks, &peer_uuids, &ba_list, &mut ctx_b).await;
                mgr_b
                    .tick_10ms(&ba_map, &peer_uuids, &[uuid_a], &mut ctx_b)
                    .await;
            }
            {
                let mut ctx_a = TransferCtx {
                    local_peer_id: 1,
                    local_uuid: uuid_a,
                    local_nickname: "Alpha",
                    socket: &sock_a,
                    cipher: &cipher,
                    room_prefix: prefix,
                    control_seq: &mut seq_a,
                    seal_buf: &mut seal_a,
                    evt_tx: &tx_a,
                };
                pump_socket_with_uuids(&mut mgr_a, &nicks, &peer_uuids, &ab_list, &mut ctx_a).await;
                mgr_a
                    .tick_10ms(&ab_map, &peer_uuids, &[uuid_b], &mut ctx_a)
                    .await;
            }
            while let Ok(ev) = rx_b.try_recv() {
                if let TransferEvent::BlobStateChanged {
                    blob_id,
                    state: BlobTransferState::Completed { saved_path },
                    ..
                } = ev
                {
                    if blob_id == bid {
                        saved_download = saved_path;
                    }
                }
            }
            if saved_download.is_some() {
                break;
            }
        }
        let dl_path = saved_download.expect("File download should complete");

        // Restart Bravo's SwarmChatManager from disk and verify 30-message cap, UUID identity, and auto-seeding
        drop(mgr_b);
        let mut mgr_b_reloaded = SwarmChatManager::new();
        mgr_b_reloaded.set_room_persistence(invite.room_id, tmp_root.clone());
        let (tx_b2, mut rx_b2) = mpsc::channel(64);
        mgr_b_reloaded.load_room_from_disk(uuid_b, &tx_b2).await;

        let mut restored_items = Vec::new();
        while let Ok(ev) = rx_b2.try_recv() {
            if let TransferEvent::ChatItemReceived(item) = ev {
                restored_items.push(item);
            }
        }
        assert_eq!(
            restored_items.len(),
            30,
            "History must cap cleanly at 30 messages"
        );
        assert!(
            mgr_b_reloaded.local_blobs.contains_key(&bid),
            "Reloaded manager must auto-seed previously downloaded blobs"
        );
        for item in &restored_items {
            if item.is_self {
                assert_eq!(item.sender_uuid, uuid_b);
                assert_eq!(item.sender_nickname, "Bravo");
            } else {
                assert_eq!(item.sender_uuid, uuid_a);
                assert_eq!(item.sender_nickname, "Alpha");
            }
        }
        assert!(
            restored_items
                .iter()
                .any(|i| i.is_self && i.sender_nickname == "Bravo"),
            "Bravo reply must persist inside latest 30 history items"
        );

        // Alpha rejoins under a brand-new temporary PeerId = 9 and syncs room history
        let sock_a2 = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a2 = sock_a2.local_addr().unwrap();
        let mut mgr_a2 = SwarmChatManager::new();
        let (tx_a2, mut rx_a2) = mpsc::channel(64);
        let a2_b_list = vec![(2, PeerEndpoint::Direct(addr_b))];
        let b_a2_list = vec![(9, PeerEndpoint::Direct(addr_a2))];
        let mut a2_b_map = HashMap::new();
        a2_b_map.insert(2, PeerEndpoint::Direct(addr_b));
        let mut b_a2_map = HashMap::new();
        b_a2_map.insert(9, PeerEndpoint::Direct(addr_a2));

        {
            let mut ctx_a2 = TransferCtx {
                local_peer_id: 9,
                local_uuid: uuid_a,
                local_nickname: "Alpha",
                socket: &sock_a2,
                cipher: &cipher,
                room_prefix: prefix,
                control_seq: &mut seq_a,
                seal_buf: &mut seal_a,
                evt_tx: &tx_a2,
            };
            mgr_a2
                .send_history_sync_req(PeerEndpoint::Direct(addr_b), &mut ctx_a2)
                .await;
            mgr_a2
                .send_chat_text("Rejoined under slot 9".to_string(), &a2_b_list, &mut ctx_a2)
                .await;
        }
        let mut new_msg_from_slot9 = None;
        let mut synced_for_alpha = Vec::new();
        for _ in 0..45 {
            tokio::time::sleep(Duration::from_millis(14)).await;
            {
                let mut ctx_b2 = TransferCtx {
                    local_peer_id: 2,
                    local_uuid: uuid_b,
                    local_nickname: "Bravo",
                    socket: &sock_b,
                    cipher: &cipher,
                    room_prefix: prefix,
                    control_seq: &mut seq_b,
                    seal_buf: &mut seal_b,
                    evt_tx: &tx_b2,
                };
                pump_socket_with_uuids(
                    &mut mgr_b_reloaded,
                    &nicks,
                    &peer_uuids,
                    &b_a2_list,
                    &mut ctx_b2,
                )
                .await;
                mgr_b_reloaded
                    .tick_10ms(&b_a2_map, &peer_uuids, &[uuid_a], &mut ctx_b2)
                    .await;
            }
            {
                let mut ctx_a2 = TransferCtx {
                    local_peer_id: 9,
                    local_uuid: uuid_a,
                    local_nickname: "Alpha",
                    socket: &sock_a2,
                    cipher: &cipher,
                    room_prefix: prefix,
                    control_seq: &mut seq_a,
                    seal_buf: &mut seal_a,
                    evt_tx: &tx_a2,
                };
                pump_socket_with_uuids(&mut mgr_a2, &nicks, &peer_uuids, &a2_b_list, &mut ctx_a2)
                    .await;
                mgr_a2
                    .tick_10ms(&a2_b_map, &peer_uuids, &[uuid_b], &mut ctx_a2)
                    .await;
            }
            while let Ok(ev) = rx_b2.try_recv() {
                if let TransferEvent::ChatItemReceived(item) = ev {
                    new_msg_from_slot9 = Some(item);
                }
            }
            while let Ok(ev) = rx_a2.try_recv() {
                if let TransferEvent::ChatItemReceived(item) = ev {
                    synced_for_alpha.push(item);
                }
            }
            if new_msg_from_slot9.is_some()
                && synced_for_alpha
                    .iter()
                    .any(|i| !i.is_self && i.sender_uuid == uuid_b)
                && synced_for_alpha
                    .iter()
                    .any(|i| i.is_self && i.sender_uuid == uuid_a)
            {
                break;
            }
        }

        let slot9_item = new_msg_from_slot9.expect("Bravo must receive Alpha msg under PeerId 9");
        assert_eq!(slot9_item.sender_uuid, uuid_a);
        assert_eq!(slot9_item.sender_nickname, "Alpha");
        assert!(!slot9_item.is_self);

        assert!(
            synced_for_alpha
                .iter()
                .any(|i| !i.is_self && i.sender_uuid == uuid_b && i.sender_nickname == "Bravo"),
            "Synced history on rejoin must identify Bravo accurately"
        );
        assert!(
            synced_for_alpha
                .iter()
                .any(|i| i.is_self && i.sender_uuid == uuid_a && i.sender_nickname == "Alpha"),
            "Synced history on rejoin must restore Alpha's own historical authorship"
        );

        let _ = fs::remove_file(&dl_path);
        let _ = fs::remove_dir_all(&tmp_root);
    }

    #[test]
    fn test_parse_json_string_malformed_utf8_escapes() {
        let mut idx = 0;
        assert_eq!(
            parse_json_string("\"hello\\éworld\"", &mut idx).as_deref(),
            Some("helloéworld")
        );
        assert_eq!(idx, "\"hello\\éworld\"".len());

        let mut idx = 0;
        assert_eq!(
            parse_json_string("\"\\🦀\"", &mut idx).as_deref(),
            Some("🦀")
        );
        assert_eq!(idx, "\"\\🦀\"".len());

        let mut idx = 0;
        assert_eq!(parse_json_string("\"\\u00é1\"", &mut idx), None);

        let mut idx = 0;
        assert_eq!(parse_json_string("\"\\u12\"", &mut idx), None);

        let mut idx = 0;
        assert_eq!(parse_json_string("\"unterminated\\", &mut idx), None);

        let mut idx = 0;
        assert_eq!(
            parse_json_string("\"\\u0041\\n\\t\\\"\\\\\"", &mut idx).as_deref(),
            Some("A\n\t\"\\")
        );

        let parsed = parse_json_object_array("[{\"text\":\"ok\\é\"}]");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].get("text").and_then(JsonVal::as_str), Some("oké"));
    }
}
