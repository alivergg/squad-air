use crate::crypto_identity::{
    derive_lan_announce_id, open_friend_invite, seal_friend_invite, spake2_host_seal_invite,
    spake2_joiner_open_invite, spake2_lan_host_seal_invite, spake2_lan_joiner_open_invite,
    spake2_lan_start_joiner, spake2_start_joiner,
};
use crate::mem::release_os_memory;
use crate::net::lan::{
    bind_lan_broadcast_sender, bind_lan_discovery_listener, broadcast_lan_packet,
    start_embedded_rendezvous, DiscoveredLanRoom, EmbeddedRendezvousHandle,
};
use crate::net::nat::discover_lan_ipv4_addrs;
use crate::net::transfer::{
    app_data_root_dir, BlobTransferState, ChatItemUi, SwarmChatManager, TransferCtx, TransferEvent,
};
use crate::net::PeerEndpoint;
use rand::{rngs::OsRng, Rng};
use spake2::{Ed25519Group, Spake2};
use squad_dsp::{AudioEngineHandle, OutboundAudioFrame, SoundCue};
use squad_proto::{
    encode_peer_relay_datagram_into, encode_relay_datagram_into, is_peer_relay_datagram,
    is_relay_datagram, is_spake2_datagram, parse_peer_relay_header, parse_relay_header,
    rewrite_relay_peer_id, sanitize_display_str, ClientUuid, InviteLink, LanDiscoveryPacket,
    P2pHeader, P2pMsgType, P2pPayload, PeerId, PeerRecord, RoomCipher, RoomId, SignalingErrorCode,
    SignalingPacket, Spake2Packet, LAN_AUTH_STATUS_NOT_FOUND, LAN_AUTH_STATUS_OK,
    LAN_AUTH_STATUS_RATE_LIMITED, LAN_DISCOVERY_PORT, MAX_FRIENDS_STATUS_PER_PACKET,
    MAX_LAN_PASSWORD_LEN, MAX_QUERY_FRIENDS_PER_PACKET, MAX_RELAY_UDP_PAYLOAD, MAX_ROOM_PEERS,
    MAX_UDP_PAYLOAD, RELAY_HEADER_LEN,
};
use squad_video::{
    detect_hardware_encoder_backend, fragment_frame, is_hevc_hardware_supported, DecoderError,
    FrameReassembler, H264Decoder, H264Encoder, RawRgbaFrame, ScreenCapturer, StreamConfig,
    VideoCodec, VideoPacketOut,
};
use std::collections::{HashMap, HashSet};
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Notify};
use tracing::{debug, info, warn};
use x25519_dalek::{PublicKey, StaticSecret};

const REGISTER_INTERVAL: Duration = Duration::from_secs(2);
const PUNCH_INTERVAL: Duration = Duration::from_millis(60);
const RELAY_FALLBACK_AFTER: Duration = Duration::from_secs(3);
const RELAY_DIRECT_PROBE_INTERVAL: Duration = Duration::from_secs(2);
const PING_INTERVAL: Duration = Duration::from_secs(1);
const PEER_STALE_TIMEOUT: Duration = Duration::from_secs(6);
const RELAY_STALE_TIMEOUT: Duration = Duration::from_secs(12);
const MESH_REACHABILITY_INTERVAL: Duration = Duration::from_secs(2);
const MESH_REACHABILITY_TTL: Duration = Duration::from_secs(7);
const NAT_FAILED_PROBE_INTERVAL: Duration = Duration::from_secs(5);
const LAN_BEACON_INTERVAL: Duration = Duration::from_millis(1500);
const LAN_ROOM_TTL: Duration = Duration::from_secs(5);
const LAN_RV_LOST_TIMEOUT: Duration = Duration::from_secs(7);
const LAN_MIGRATION_GRACE: Duration = Duration::from_secs(3);
const PRESENCE_PING_INTERVAL: Duration = Duration::from_secs(4);
const PRESENCE_UI_TTL: Duration = Duration::from_secs(15);
const SHORT_CODE_JOIN_TIMEOUT: Duration = Duration::from_secs(10);
const SHORT_CODE_RETRY_INTERVAL: Duration = Duration::from_millis(250);
const SPAKE2_HOST_MAX_REPLIES_PER_SEC: usize = 8;
const LAN_AUTH_TIMEOUT: Duration = Duration::from_secs(5);
const LAN_AUTH_RETRY_INTERVAL: Duration = Duration::from_millis(250);
const LAN_AUTH_MAX_ATTEMPTS_PER_MIN: usize = 5;
const LAN_AUTH_BLOCK_DURATION: Duration = Duration::from_secs(60);
const LAN_AUTH_MAX_BUCKETS: usize = 256;
const LAN_AUTH_GLOBAL_MAX_PER_SEC: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerConnStatus {
    Punching,
    Connected,
    MeshRelayed,
    Relayed,
    NatFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerRouteKind {
    Connecting,
    LanDirect,
    P2pDirect,
    MeshRelayed,
    Relayed,
    NatFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteStreamInfo {
    pub codec: VideoCodec,
    pub has_audio: bool,
    pub width: u16,
    pub height: u16,
    pub fps: u8,
    pub source_title: String,
    pub encoder_backend: String,
}

static NEXT_VIDEO_FRAME_SEQ: AtomicU64 = AtomicU64::new(1);
const VIDEO_TX_BURST_PACKETS: usize = 16;
const MAX_UDP_RX_BATCH: usize = 32;

#[derive(Debug, Clone)]
pub struct DecodedVideoFrame {
    pub streamer_peer_id: PeerId,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
    pub frame_seq: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PeerUiSnapshot {
    pub peer_id: PeerId,
    pub client_uuid: ClientUuid,
    pub pub_key: Option<[u8; 32]>,
    pub nickname: String,
    pub status: PeerConnStatus,
    pub route: PeerRouteKind,
    pub rtt_ms: u32,
    pub jitter_ms: u32,
    pub loss_pct: f32,
    pub is_speaking: bool,
    pub stream_info: Option<RemoteStreamInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FriendPresenceUi {
    pub pub_key: [u8; 32],
    pub nickname: String,
    pub online: bool,
    pub in_room: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingFriendCallUi {
    pub sender_pub_key: [u8; 32],
    pub sender_nickname: String,
    pub invite: InviteLink,
    pub is_party_invite: bool,
    pub is_known_friend: bool,
}

#[derive(Debug, Clone)]
pub enum MeshCommand {
    JoinRoom {
        rendezvous_server: String,
        invite: InviteLink,
        nickname: String,
        client_uuid: ClientUuid,
        initial_volumes: HashMap<ClientUuid, f32>,
        is_lan_room: bool,
        room_name: Option<String>,
    },
    HostLanRoom {
        room_name: String,
        password: Option<String>,
        nickname: String,
        client_uuid: ClientUuid,
    },
    JoinLockedLanRoom {
        room: DiscoveredLanRoom,
        password: String,
        nickname: String,
        client_uuid: ClientUuid,
    },
    CancelLanAuth,
    LeaveRoom,
    SendChatText {
        text: String,
    },
    SendInlineImage {
        filename: String,
        bytes: Vec<u8>,
    },
    OfferFile {
        path: PathBuf,
    },
    DownloadBlob {
        blob_id: [u8; 16],
    },
    SetLocalAvatar {
        avatar: Option<([u8; 32], Vec<u8>)>,
    },
    StartScreenShare {
        config: StreamConfig,
    },
    StopScreenShare,
    SetLocalStreamPreview {
        enabled: bool,
    },
    WatchPeerStream {
        peer_id: Option<PeerId>,
    },
    SetTrayMode {
        in_tray: bool,
    },
    UpdatePresenceConfig {
        rendezvous_server: String,
        nickname: String,
        client_uuid: ClientUuid,
        identity_secret: [u8; 32],
        friend_pub_keys: Vec<[u8; 32]>,
    },
    CallOrInviteFriend {
        target_pub_key: [u8; 32],
    },
    GenerateShortCode,
    JoinByShortCode {
        rendezvous_server: String,
        code: u32,
        nickname: String,
        client_uuid: ClientUuid,
    },
    #[cfg(test)]
    TestGetSessionInfo(mpsc::Sender<Option<(SocketAddr, SocketAddr, RoomId, bool)>>),
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum MeshEvent {
    JoinedRoom {
        local_peer_id: PeerId,
        invite_uri: String,
        is_lan_locked: bool,
    },
    LanAuthFailed(String),
    LeftRoom,
    PeersUpdated(Vec<PeerUiSnapshot>),
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
    RemoteStreamUpdated {
        peer_id: PeerId,
        stream: Option<RemoteStreamInfo>,
    },
    LocalStreamStatus {
        is_sharing: bool,
        subscriber_count: usize,
    },
    StreamCodecFallbackToH264 {
        reason: String,
    },
    LanRoomsUpdated(Vec<DiscoveredLanRoom>),
    FriendsStatusUpdated(Vec<FriendPresenceUi>),
    IncomingFriendCall(IncomingFriendCallUi),
    ShortCodeGenerated {
        code: u32,
        expires_in_secs: u16,
    },
    ShortCodeCleared,
    FriendInviteSent {
        target_pub_key: [u8; 32],
    },
    Error(String),
}

struct RemotePeerState {
    record: PeerRecord,
    pub_key: Option<[u8; 32]>,
    status: PeerConnStatus,
    active_addr: Option<SocketAddr>,
    mesh_relay_peer: Option<PeerId>,
    reported_direct_peers: HashSet<PeerId>,
    reachability_updated_at: Option<Instant>,
    punch_started_at: Instant,
    last_direct_probe: Instant,
    last_rx: Instant,
    rtt_ms: u32,
    has_announced_once: bool,
    stream_info: Option<RemoteStreamInfo>,
    replay_control: squad_proto::ReplayWindow,
    replay_voice: squad_proto::ReplayWindow,
    replay_dtx: squad_proto::ReplayWindow,
    replay_stream_audio: squad_proto::ReplayWindow,
    replay_video: squad_proto::LargeReplayWindow,
}

type VideoEncodeWake = (Mutex<Option<RawRgbaFrame>>, Condvar);

struct CaptureWorkerHandle {
    stop: Arc<AtomicBool>,
    force_idr: Arc<AtomicBool>,
    preview_active: Arc<AtomicBool>,
    subs_active: Arc<AtomicBool>,
    encode_wake: Arc<VideoEncodeWake>,
}

#[derive(Default)]
struct VideoTxQueue {
    frags: Vec<VideoPacketOut>,
    cursor: usize,
}

impl VideoTxQueue {
    fn is_empty(&self) -> bool {
        self.cursor >= self.frags.len()
    }

    fn clear(&mut self) {
        self.frags.clear();
        self.cursor = 0;
    }

    fn load(&mut self, frags: Vec<VideoPacketOut>) {
        self.frags = frags;
        self.cursor = 0;
    }
}

enum DecodeJob {
    Frame {
        streamer_peer_id: PeerId,
        generation: u64,
        annex_b_bytes: Vec<u8>,
    },
    Reset,
    ResetAndShrink,
    Shutdown,
}

enum DecodeThreadEvent {
    UnsupportedCodec {
        streamer_peer_id: PeerId,
        generation: u64,
        reason: String,
    },
    CorruptBitstream {
        streamer_peer_id: PeerId,
        generation: u64,
    },
}

struct VideoDecodeHandle {
    tx: std::sync::mpsc::SyncSender<DecodeJob>,
    generation: Arc<AtomicU64>,
    shrink_pending: Arc<AtomicBool>,
}

impl VideoDecodeHandle {
    fn reset(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let _ = self.tx.try_send(DecodeJob::Reset);
    }

    fn reset_and_shrink(&self) {
        self.shrink_pending.store(true, Ordering::Release);
        self.generation.fetch_add(1, Ordering::AcqRel);
        let _ = self.tx.try_send(DecodeJob::ResetAndShrink);
    }
}

struct ActiveSession {
    rendezvous_addr: SocketAddr,
    invite: InviteLink,
    cipher: RoomCipher,
    room_prefix: [u8; 4],
    local_uuid: ClientUuid,
    local_pub_key: [u8; 32],
    nickname: String,
    local_peer_id: PeerId,
    lan_addrs: Vec<std::net::SocketAddrV4>,
    peers: HashMap<PeerId, RemotePeerState>,
    control_seq: u64,
    video_seq: u64,
    video_tx_queue: VideoTxQueue,
    transfer: SwarmChatManager,
    local_encoder_backend: String,
    local_stream_config: Option<StreamConfig>,
    local_preview_enabled: bool,
    stream_subscribers: HashSet<PeerId>,
    next_video_frame_id: Arc<AtomicU32>,
    watched_peer_id: Option<PeerId>,
    sent_decoder_error_to: Option<PeerId>,
    video_reassembler: FrameReassembler,
    hevc_decode_supported: bool,
    last_pli_sent: Instant,
    is_lan_room: bool,
    lan_room_name: String,
    embedded_rv: Option<EmbeddedRendezvousHandle>,
    last_rv_rx: Instant,
    migrating_until: Option<Instant>,
    migration_claimed_at: Option<Instant>,
    last_mesh_reachability_sent: Instant,
    last_advertised_direct_peers: Vec<PeerId>,
    active_short_code: Option<(u32, Instant)>,
    last_spake2_reply_times: Vec<Instant>,
    local_udp_port: u16,
    lan_room_password: Option<String>,
    lan_auth_buckets: HashMap<std::net::IpAddr, LanAuthRateBucket>,
    lan_auth_global_times: Vec<Instant>,
}

struct LanAuthRateBucket {
    attempts: Vec<Instant>,
    blocked_until: Option<Instant>,
    last_reply: Option<(u64, Vec<u8>, Instant)>,
}

struct PresenceWorkerState {
    rendezvous_addr: Option<SocketAddr>,
    nickname: String,
    client_uuid: ClientUuid,
    identity_secret: Option<StaticSecret>,
    my_pub_key: [u8; 32],
    friend_pub_keys: Vec<[u8; 32]>,
    online_friends: HashMap<[u8; 32], (String, bool, Instant)>,
}

struct PendingShortCodeJoin {
    code: u32,
    rendezvous_addr: SocketAddr,
    nickname: String,
    client_uuid: ClientUuid,
    spake2_state: Option<Spake2<Ed25519Group>>,
    msg_a: Vec<u8>,
    host_addrs: Vec<SocketAddr>,
    started_at: Instant,
    last_retry_sent: Instant,
}

struct PendingLanAuth {
    room: DiscoveredLanRoom,
    auth_addr: SocketAddr,
    attempt_id: u64,
    password: String,
    nickname: String,
    client_uuid: ClientUuid,
    spake2_state: Option<Spake2<Ed25519Group>>,
    msg_a: Vec<u8>,
    started_at: Instant,
    last_retry_sent: Instant,
}

#[derive(Clone)]
struct NotifyingEventTx {
    tx: mpsc::Sender<MeshEvent>,
    notify: Arc<Notify>,
}

impl NotifyingEventTx {
    async fn send(&self, ev: MeshEvent) {
        if self.tx.send(ev).await.is_ok() {
            self.notify.notify_one();
        }
    }

    fn try_send(&self, ev: MeshEvent) {
        if self.tx.try_send(ev).is_ok() {
            self.notify.notify_one();
        }
    }
}

pub fn spawn_mesh_worker(
    audio_engine: AudioEngineHandle,
    mut audio_out_rx: mpsc::Receiver<OutboundAudioFrame>,
    video_frame_slot: Arc<Mutex<Option<DecodedVideoFrame>>>,
    mesh_notify: Arc<Notify>,
    video_notify: Arc<Notify>,
) -> (mpsc::Sender<MeshCommand>, mpsc::Receiver<MeshEvent>) {
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<MeshCommand>(64);
    let (raw_evt_tx, evt_rx) = mpsc::channel::<MeshEvent>(512);
    let evt_tx = NotifyingEventTx {
        tx: raw_evt_tx,
        notify: mesh_notify,
    };
    let (tr_tx, mut tr_rx) = mpsc::channel::<TransferEvent>(256);
    let (video_out_tx, mut video_out_rx) = mpsc::channel::<Vec<VideoPacketOut>>(3);
    let local_encoder_backend = detect_hardware_encoder_backend();
    let hevc_decode_supported = is_hevc_hardware_supported();

    let (decode_tx, decode_rx) = std::sync::mpsc::sync_channel::<DecodeJob>(4);
    let decode_gen = Arc::new(AtomicU64::new(0));
    let decode_shrink = Arc::new(AtomicBool::new(false));
    let (decode_err_tx, mut decode_err_rx) = mpsc::channel::<DecodeThreadEvent>(16);
    let decode_handle = VideoDecodeHandle {
        tx: decode_tx,
        generation: Arc::clone(&decode_gen),
        shrink_pending: Arc::clone(&decode_shrink),
    };
    let slot_decode = Arc::clone(&video_frame_slot);
    let video_notify_decode = Arc::clone(&video_notify);
    let _ = std::thread::Builder::new()
        .name("squad-video-decode".into())
        .spawn(move || {
            let mut decoder = H264Decoder::new();
            let mut last_gen = 0u64;
            while let Ok(job) = decode_rx.recv() {
                match job {
                    DecodeJob::Reset => {
                        if decode_shrink.swap(false, Ordering::AcqRel) {
                            decoder.reset_and_shrink();
                        } else {
                            decoder.reset();
                        }
                        last_gen = decode_gen.load(Ordering::Acquire);
                    }
                    DecodeJob::ResetAndShrink => {
                        decode_shrink.store(false, Ordering::Release);
                        decoder.reset_and_shrink();
                        last_gen = decode_gen.load(Ordering::Acquire);
                    }
                    DecodeJob::Shutdown => break,
                    DecodeJob::Frame {
                        streamer_peer_id,
                        generation,
                        annex_b_bytes,
                    } => {
                        let current_gen = decode_gen.load(Ordering::Acquire);
                        if generation != current_gen {
                            if current_gen != last_gen {
                                if decode_shrink.swap(false, Ordering::AcqRel) {
                                    decoder.reset_and_shrink();
                                } else {
                                    decoder.reset();
                                }
                                last_gen = current_gen;
                            }
                            continue;
                        }
                        match decoder.decode_checked_shared(&annex_b_bytes) {
                            Ok(Some((width, height, rgba))) => {
                                if decode_gen.load(Ordering::Acquire) == generation {
                                    let frame_seq =
                                        NEXT_VIDEO_FRAME_SEQ.fetch_add(1, Ordering::Relaxed);
                                    *slot_decode.lock().unwrap_or_else(|e| e.into_inner()) =
                                        Some(DecodedVideoFrame {
                                            streamer_peer_id,
                                            width,
                                            height,
                                            rgba,
                                            frame_seq,
                                        });
                                    video_notify_decode.notify_one();
                                }
                            }
                            Ok(None) => {}
                            Err(DecoderError::UnsupportedCodec(reason)) => {
                                decoder.reset();
                                let _ =
                                    decode_err_tx.try_send(DecodeThreadEvent::UnsupportedCodec {
                                        streamer_peer_id,
                                        generation,
                                        reason,
                                    });
                            }
                            Err(DecoderError::CorruptBitstream) => {
                                decoder.reset();
                                let _ =
                                    decode_err_tx.try_send(DecodeThreadEvent::CorruptBitstream {
                                        streamer_peer_id,
                                        generation,
                                    });
                            }
                        }
                    }
                }
            }
        });

    tokio::spawn(async move {
        let socket = match UdpSocket::bind("0.0.0.0:0").await {
            Ok(s) => s,
            Err(e) => {
                evt_tx
                    .send(MeshEvent::Error(format!("UDP socket bind failed: {e}")))
                    .await;
                return;
            }
        };
        let sock_ref = socket2::SockRef::from(&socket);
        let _ = sock_ref.set_recv_buffer_size(2 * 1024 * 1024);
        let _ = sock_ref.set_send_buffer_size(2 * 1024 * 1024);
        let bound_port = socket.local_addr().map(|a| a.port()).unwrap_or(0);

        let mut session: Option<ActiveSession> = None;
        let mut last_snapshots: Vec<PeerUiSnapshot> = Vec::new();
        let mut pending_local_avatar: Option<([u8; 32], Vec<u8>)> = None;
        let mut capture_worker: Option<CaptureWorkerHandle> = None;
        let mut recv_buf = vec![0u8; MAX_RELAY_UDP_PAYLOAD + 64];
        let mut seal_buf = Vec::with_capacity(MAX_UDP_PAYLOAD);
        let mut relay_buf = Vec::with_capacity(MAX_RELAY_UDP_PAYLOAD);

        let fallback_sk = StaticSecret::random_from_rng(OsRng);
        let fallback_pk = PublicKey::from(&fallback_sk).to_bytes();
        let mut presence = PresenceWorkerState {
            rendezvous_addr: None,
            nickname: String::new(),
            client_uuid: [0u8; 16],
            identity_secret: Some(fallback_sk),
            my_pub_key: fallback_pk,
            friend_pub_keys: Vec::new(),
            online_friends: HashMap::new(),
        };
        let mut pending_short_code_join: Option<PendingShortCodeJoin> = None;
        let mut pending_lan_auth: Option<PendingLanAuth> = None;

        let lan_rx_sock = bind_lan_discovery_listener(LAN_DISCOVERY_PORT)
            .map_err(|e| {
                warn!("LAN discovery listener unavailable on UDP {LAN_DISCOVERY_PORT}: {e}")
            })
            .ok();
        let lan_tx_sock = bind_lan_broadcast_sender().ok();
        let mut lan_rx_buf = [0u8; MAX_UDP_PAYLOAD];
        let mut discovered_lan: HashMap<RoomId, (DiscoveredLanRoom, Instant)> = HashMap::new();
        let mut last_published_lan: Vec<DiscoveredLanRoom> = Vec::new();

        let mut register_tick = tokio::time::interval(REGISTER_INTERVAL);
        let mut punch_tick = tokio::time::interval(PUNCH_INTERVAL);
        let mut ping_tick = tokio::time::interval(PING_INTERVAL);
        let mut qos_tick = tokio::time::interval(Duration::from_millis(10));
        qos_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut ui_sync_tick = tokio::time::interval(Duration::from_millis(120));
        let mut lan_tick = tokio::time::interval(LAN_BEACON_INTERVAL);
        let mut presence_tick = tokio::time::interval(PRESENCE_PING_INTERVAL);
        let mut video_pace_tick = tokio::time::interval(Duration::from_millis(2));
        video_pace_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            let lan_work_active = session.as_ref().is_some_and(|s| s.embedded_rv.is_some())
                || !discovered_lan.is_empty();
            let has_paced_video = session
                .as_ref()
                .is_some_and(|s| !s.video_tx_queue.is_empty());

            tokio::select! {
                biased;

                Some(cmd) = cmd_rx.recv() => {
                    match cmd {
                        MeshCommand::JoinRoom {
                            rendezvous_server,
                            invite,
                            nickname,
                            client_uuid,
                            initial_volumes,
                            is_lan_room,
                            room_name,
                        } => {
                            let rv_addr = match resolve_host(&rendezvous_server) {
                                Ok(a) => a,
                                Err(e) => {
                                    let _ = evt_tx.send(MeshEvent::Error(e)).await;
                                    continue;
                                }
                            };
                            pending_short_code_join = None;
                            pending_lan_auth = None;
                            let mut act_ctx = SessionActivationCtx {
                                socket: &socket,
                                lan_tx_sock: lan_tx_sock.as_ref(),
                                bound_port,
                                pending_local_avatar: &pending_local_avatar,
                                local_encoder_backend: &local_encoder_backend,
                                hevc_decode_supported,
                                audio_engine: &audio_engine,
                                tr_tx: &tr_tx,
                                tr_rx: &mut tr_rx,
                                evt_tx: &evt_tx,
                                capture_worker: &mut capture_worker,
                                video_out_rx: &mut video_out_rx,
                                decode_handle: &decode_handle,
                                video_frame_slot: &video_frame_slot,
                                discovered_lan: &mut discovered_lan,
                                last_published_lan: &mut last_published_lan,
                                last_snapshots: &mut last_snapshots,
                                seal_buf: &mut seal_buf,
                                relay_buf: &mut relay_buf,
                                presence: &presence,
                            };
                            activate_joined_session(
                                &mut session,
                                &mut act_ctx,
                                rv_addr,
                                invite,
                                nickname,
                                client_uuid,
                                is_lan_room,
                                room_name.unwrap_or_default(),
                                None,
                                None,
                            )
                            .await;

                            let _ = initial_volumes;
                        }
                        MeshCommand::HostLanRoom {
                            room_name,
                            password,
                            nickname,
                            client_uuid,
                        } => {
                            let clean_password = password
                                .map(|p| p.trim().to_string())
                                .filter(|p| !p.is_empty());
                            if clean_password
                                .as_ref()
                                .is_some_and(|p| p.len() > MAX_LAN_PASSWORD_LEN)
                            {
                                let _ = evt_tx
                                    .send(MeshEvent::Error(format!(
                                        "LAN room password exceeds maximum length of {MAX_LAN_PASSWORD_LEN} bytes"
                                    )))
                                    .await;
                                continue;
                            }
                            pending_short_code_join = None;
                            pending_lan_auth = None;
                            if let Some(old_sess) = session.take() {
                                if old_sess.active_short_code.is_some() {
                                    evt_tx.send(MeshEvent::ShortCodeCleared).await;
                                }
                                teardown_session(
                                    &socket,
                                    lan_tx_sock.as_ref(),
                                    old_sess,
                                    &mut seal_buf,
                                    &mut relay_buf,
                                    true,
                                )
                                .await;
                            }

                            let embedded_rv = match start_embedded_rendezvous().await {
                                Ok(rv) => rv,
                                Err(e) => {
                                    let _ = evt_tx
                                        .send(MeshEvent::Error(format!(
                                            "Failed to start embedded LAN host: {e}"
                                        )))
                                        .await;
                                    continue;
                                }
                            };
                            let rv_addr = SocketAddr::from(([127, 0, 0, 1], embedded_rv.port));
                            let invite = InviteLink::generate();
                            let clean_name = if room_name.trim().is_empty() {
                                format!("{nickname}'s LAN")
                            } else {
                                room_name.trim().to_string()
                            };

                            let mut act_ctx = SessionActivationCtx {
                                socket: &socket,
                                lan_tx_sock: lan_tx_sock.as_ref(),
                                bound_port,
                                pending_local_avatar: &pending_local_avatar,
                                local_encoder_backend: &local_encoder_backend,
                                hevc_decode_supported,
                                audio_engine: &audio_engine,
                                tr_tx: &tr_tx,
                                tr_rx: &mut tr_rx,
                                evt_tx: &evt_tx,
                                capture_worker: &mut capture_worker,
                                video_out_rx: &mut video_out_rx,
                                decode_handle: &decode_handle,
                                video_frame_slot: &video_frame_slot,
                                discovered_lan: &mut discovered_lan,
                                last_published_lan: &mut last_published_lan,
                                last_snapshots: &mut last_snapshots,
                                seal_buf: &mut seal_buf,
                                relay_buf: &mut relay_buf,
                                presence: &presence,
                            };
                            activate_joined_session(
                                &mut session,
                                &mut act_ctx,
                                rv_addr,
                                invite,
                                nickname,
                                client_uuid,
                                true,
                                clean_name,
                                clean_password,
                                Some(embedded_rv),
                            )
                            .await;
                        }
                        MeshCommand::JoinLockedLanRoom {
                            room,
                            password,
                            nickname,
                            client_uuid,
                        } => {
                            let clean_pw = password.trim().to_string();
                            if clean_pw.is_empty() || clean_pw.len() > MAX_LAN_PASSWORD_LEN {
                                evt_tx
                                    .send(MeshEvent::LanAuthFailed(format!(
                                        "Password must be 1..={MAX_LAN_PASSWORD_LEN} characters"
                                    )))
                                    .await;
                                continue;
                            }
                            let Some(auth_addr) = room.auth_addr else {
                                evt_tx
                                    .send(MeshEvent::LanAuthFailed(
                                        "Host did not advertise a LAN authentication port"
                                            .to_string(),
                                    ))
                                    .await;
                                continue;
                            };
                            let (spake2_state, msg_a) = spake2_lan_start_joiner(&clean_pw);
                            let attempt_id: u64 = rand::thread_rng().gen();
                            let announce_id = room.room_id;
                            let now = Instant::now();
                            pending_lan_auth = Some(PendingLanAuth {
                                room,
                                auth_addr,
                                attempt_id,
                                password: clean_pw,
                                nickname,
                                client_uuid,
                                spake2_state: Some(spake2_state),
                                msg_a: msg_a.clone(),
                                started_at: now,
                                last_retry_sent: now,
                            });
                            let req = Spake2Packet::LanAuthReq {
                                announce_id,
                                attempt_id,
                                spake2_msg_a: msg_a,
                            };
                            if let Ok(bytes) = req.encode() {
                                let _ = socket.send_to(&bytes, auth_addr).await;
                            }
                        }
                        MeshCommand::CancelLanAuth => {
                            pending_lan_auth = None;
                        }
                        MeshCommand::LeaveRoom => {
                            pending_short_code_join = None;
                            pending_lan_auth = None;
                            stop_capture_worker(&mut capture_worker);
                            drain_video_out_rx(&mut video_out_rx);
                            audio_engine.set_share_stream_audio(false);
                            audio_engine.clear_stream_audio();
                            decode_handle.reset();
                            clear_video_slot(&video_frame_slot);
                            last_snapshots.clear();
                            if let Some(mut sess) = session.take() {
                                let had_short_code = sess.active_short_code.take().is_some();
                                sess.local_preview_enabled = false;
                                if sess.local_stream_config.take().is_some() {
                                    broadcast_stream_announce(
                                        &socket,
                                        &mut sess,
                                        &mut seal_buf,
                                        &mut relay_buf,
                                        false,
                                        None,
                                    )
                                    .await;
                                }
                                teardown_session(
                                    &socket,
                                    lan_tx_sock.as_ref(),
                                    sess,
                                    &mut seal_buf,
                                    &mut relay_buf,
                                    true,
                                )
                                .await;
                                audio_engine.clear_peers();
                                audio_engine.play_cue(SoundCue::Leave);
                                if had_short_code {
                                    evt_tx.send(MeshEvent::ShortCodeCleared).await;
                                }
                                evt_tx.send(MeshEvent::LeftRoom).await;
                                send_presence_ping(&socket, &presence, false).await;
                            }
                        }
                        MeshCommand::SendChatText { text } => {
                            if let Some(sess) = session.as_mut() {
                                if sess.local_peer_id != 0 {
                                    let peers = connected_peer_list(sess);
                                    let mut ctx = TransferCtx {
                                        local_peer_id: sess.local_peer_id,
                                        local_uuid: sess.local_uuid,
                                        local_nickname: &sess.nickname,
                                        socket: &socket,
                                        cipher: &sess.cipher,
                                        room_prefix: sess.room_prefix,
                                        control_seq: &mut sess.control_seq,
                                        seal_buf: &mut seal_buf,
                                        evt_tx: &tr_tx,
                                    };
                                    sess.transfer.send_chat_text(text, &peers, &mut ctx).await;
                                    forward_transfer_events(&mut tr_rx, &evt_tx).await;
                                }
                            }
                        }
                        MeshCommand::SendInlineImage { filename, bytes } => {
                            if let Some(sess) = session.as_mut() {
                                let has_relayed = sess
                                    .peers
                                    .values()
                                    .any(|p| p.status == PeerConnStatus::Relayed);
                                if sess.local_peer_id != 0 && !has_relayed {
                                    let peers = connected_peer_list(sess);
                                    let mut ctx = TransferCtx {
                                        local_peer_id: sess.local_peer_id,
                                        local_uuid: sess.local_uuid,
                                        local_nickname: &sess.nickname,
                                        socket: &socket,
                                        cipher: &sess.cipher,
                                        room_prefix: sess.room_prefix,
                                        control_seq: &mut sess.control_seq,
                                        seal_buf: &mut seal_buf,
                                        evt_tx: &tr_tx,
                                    };
                                    sess.transfer
                                        .send_inline_image(filename, bytes, &peers, &mut ctx)
                                        .await;
                                    forward_transfer_events(&mut tr_rx, &evt_tx).await;
                                }
                            }
                        }
                        MeshCommand::OfferFile { path } => {
                            if let Some(sess) = session.as_mut() {
                                let has_relayed = sess
                                    .peers
                                    .values()
                                    .any(|p| p.status == PeerConnStatus::Relayed);
                                if sess.local_peer_id != 0 && !has_relayed {
                                    let peers = connected_peer_list(sess);
                                    let mut ctx = TransferCtx {
                                        local_peer_id: sess.local_peer_id,
                                        local_uuid: sess.local_uuid,
                                        local_nickname: &sess.nickname,
                                        socket: &socket,
                                        cipher: &sess.cipher,
                                        room_prefix: sess.room_prefix,
                                        control_seq: &mut sess.control_seq,
                                        seal_buf: &mut seal_buf,
                                        evt_tx: &tr_tx,
                                    };
                                    sess.transfer.offer_file(path, &peers, &mut ctx).await;
                                    forward_transfer_events(&mut tr_rx, &evt_tx).await;
                                }
                            }
                        }
                        MeshCommand::DownloadBlob { blob_id } => {
                            if let Some(sess) = session.as_mut() {
                                sess.transfer.start_download(blob_id, &tr_tx).await;
                                forward_transfer_events(&mut tr_rx, &evt_tx).await;
                            }
                        }
                        MeshCommand::SetLocalAvatar { avatar } => {
                            pending_local_avatar = avatar.clone();
                            if let Some(sess) = session.as_mut() {
                                if sess.local_peer_id != 0 {
                                    let peers = connected_peer_list(sess);
                                    let mut ctx = TransferCtx {
                                        local_peer_id: sess.local_peer_id,
                                        local_uuid: sess.local_uuid,
                                        local_nickname: &sess.nickname,
                                        socket: &socket,
                                        cipher: &sess.cipher,
                                        room_prefix: sess.room_prefix,
                                        control_seq: &mut sess.control_seq,
                                        seal_buf: &mut seal_buf,
                                        evt_tx: &tr_tx,
                                    };
                                    sess.transfer.set_local_avatar(avatar, &peers, &mut ctx).await;
                                    forward_transfer_events(&mut tr_rx, &evt_tx).await;
                                } else {
                                    sess.transfer.set_initial_local_avatar(avatar);
                                }
                            }
                        }
                        MeshCommand::StartScreenShare { config } => {
                            if let Some(sess) = session.as_mut() {
                                stop_capture_worker(&mut capture_worker);
                                drain_video_out_rx(&mut video_out_rx);
                                sess.video_tx_queue.clear();
                                sess.stream_subscribers.clear();
                                sess.local_preview_enabled = false;
                                audio_engine.set_share_stream_audio(config.share_audio);
                                sess.local_stream_config = Some(config.clone());
                                if sess.local_peer_id != 0 {
                                    broadcast_stream_announce(
                                        &socket,
                                        sess,
                                        &mut seal_buf,
                                        &mut relay_buf,
                                        true,
                                        Some(&config),
                                    )
                                    .await;
                                }
                                let _ = evt_tx
                                    .send(MeshEvent::LocalStreamStatus {
                                        is_sharing: true,
                                        subscriber_count: 0,
                                    })
                                    .await;
                            }
                        }
                        MeshCommand::StopScreenShare => {
                            stop_capture_worker(&mut capture_worker);
                            drain_video_out_rx(&mut video_out_rx);
                            audio_engine.set_share_stream_audio(false);
                            if let Some(sess) = session.as_mut() {
                                sess.video_tx_queue.clear();
                                if sess.local_preview_enabled && sess.watched_peer_id.is_none() {
                                    clear_video_slot(&video_frame_slot);
                                }
                                sess.local_preview_enabled = false;
                                sess.local_stream_config = None;
                                sess.stream_subscribers.clear();
                                if sess.local_peer_id != 0 {
                                    broadcast_stream_announce(
                                        &socket,
                                        sess,
                                        &mut seal_buf,
                                        &mut relay_buf,
                                        false,
                                        None,
                                    )
                                    .await;
                                }
                                let _ = evt_tx
                                    .send(MeshEvent::LocalStreamStatus {
                                        is_sharing: false,
                                        subscriber_count: 0,
                                    })
                                    .await;
                            }
                        }
                        MeshCommand::SetLocalStreamPreview { enabled } => {
                            if let Some(sess) = session.as_mut() {
                                if enabled && sess.local_stream_config.is_some() {
                                    if let Some(prev_pid) = sess.watched_peer_id.take() {
                                        send_stream_subscribe(
                                            &socket,
                                            sess,
                                            &mut seal_buf,
                                            &mut relay_buf,
                                            prev_pid,
                                            false,
                                        )
                                        .await;
                                        audio_engine.clear_stream_audio();
                                    }
                                    sess.local_preview_enabled = true;
                                    reconcile_capture_worker(
                                        sess,
                                        &mut capture_worker,
                                        &video_out_tx,
                                        &video_frame_slot,
                                        &video_notify,
                                        true,
                                    );
                                } else {
                                    sess.local_preview_enabled = false;
                                    reconcile_capture_worker(
                                        sess,
                                        &mut capture_worker,
                                        &video_out_tx,
                                        &video_frame_slot,
                                        &video_notify,
                                        false,
                                    );
                                    if sess.watched_peer_id.is_none() {
                                        clear_video_slot(&video_frame_slot);
                                    }
                                }
                            } else {
                                clear_video_slot(&video_frame_slot);
                            }
                        }
                        MeshCommand::WatchPeerStream { peer_id } => {
                            if let Some(sess) = session.as_mut() {
                                if peer_id.is_some() && sess.local_preview_enabled {
                                    sess.local_preview_enabled = false;
                                    reconcile_capture_worker(
                                        sess,
                                        &mut capture_worker,
                                        &video_out_tx,
                                        &video_frame_slot,
                                        &video_notify,
                                        false,
                                    );
                                }
                                if let Some(prev_pid) = sess.watched_peer_id {
                                    if Some(prev_pid) != peer_id {
                                        send_stream_subscribe(
                                            &socket,
                                            sess,
                                            &mut seal_buf,
                                            &mut relay_buf,
                                            prev_pid,
                                            false,
                                        )
                                        .await;
                                    }
                                }
                                sess.watched_peer_id = peer_id;
                                sess.sent_decoder_error_to = None;
                                sess.video_reassembler.reset();
                                decode_handle.reset();
                                audio_engine.clear_stream_audio();
                                clear_video_slot(&video_frame_slot);
                                if let Some(target_pid) = peer_id {
                                    let relay_only = sess
                                        .peers
                                        .get(&target_pid)
                                        .is_some_and(|p| p.status == PeerConnStatus::Relayed);
                                    if relay_only {
                                        sess.watched_peer_id = None;
                                        let _ = evt_tx
                                            .send(MeshEvent::Error(
                                                "Screen share requires Direct P2P or Peer-Mesh Relay (disabled over server relay)".to_string(),
                                            ))
                                            .await;
                                        continue;
                                    }
                                    let peer_uses_hevc = sess
                                        .peers
                                        .get(&target_pid)
                                        .and_then(|p| p.stream_info.as_ref())
                                        .is_some_and(|s| s.codec == VideoCodec::H265);
                                    if peer_uses_hevc && !sess.hevc_decode_supported {
                                        sess.sent_decoder_error_to = Some(target_pid);
                                        send_stream_decoder_error(
                                            &socket,
                                            sess,
                                            &mut seal_buf,
                                            &mut relay_buf,
                                            target_pid,
                                            VideoCodec::H265.as_u8(),
                                            "HEVC decoder unavailable on viewer",
                                        )
                                        .await;
                                    }
                                    send_stream_subscribe(
                                        &socket,
                                        sess,
                                        &mut seal_buf,
                                        &mut relay_buf,
                                        target_pid,
                                        true,
                                    )
                                    .await;
                                    send_stream_pli(
                                        &socket,
                                        sess,
                                        &mut seal_buf,
                                        &mut relay_buf,
                                        target_pid,
                                    )
                                    .await;
                                    sess.last_pli_sent = Instant::now();
                                }
                            } else {
                                audio_engine.clear_stream_audio();
                                clear_video_slot(&video_frame_slot);
                            }
                        }
                        MeshCommand::SetTrayMode { in_tray } => {
                            if in_tray {
                                if let Some(sess) = session.as_mut() {
                                    if let Some(prev_pid) = sess.watched_peer_id.take() {
                                        send_stream_subscribe(
                                            &socket,
                                            sess,
                                            &mut seal_buf,
                                            &mut relay_buf,
                                            prev_pid,
                                            false,
                                        )
                                        .await;
                                    }
                                    sess.local_preview_enabled = false;
                                    sess.sent_decoder_error_to = None;
                                    reconcile_capture_worker(
                                        sess,
                                        &mut capture_worker,
                                        &video_out_tx,
                                        &video_frame_slot,
                                        &video_notify,
                                        false,
                                    );
                                    sess.video_reassembler.reset_and_shrink();
                                    decode_handle.reset_and_shrink();
                                    sess.transfer.trim_memory();
                                }
                                audio_engine.clear_stream_audio();
                                clear_video_slot(&video_frame_slot);
                                drain_video_out_rx(&mut video_out_rx);
                                seal_buf.clear();
                                seal_buf.shrink_to(2048);
                                relay_buf.clear();
                                relay_buf.shrink_to(2048);
                                recv_buf = vec![0u8; MAX_RELAY_UDP_PAYLOAD + 64];
                                release_os_memory();
                            }
                        }
                        MeshCommand::UpdatePresenceConfig {
                            rendezvous_server,
                            nickname,
                            client_uuid,
                            identity_secret,
                            friend_pub_keys,
                        } => {
                            let old_rv = presence.rendezvous_addr;
                            presence.rendezvous_addr = resolve_host(&rendezvous_server).ok();
                            presence.nickname = sanitize_display_str(
                                nickname.trim(),
                                squad_proto::MAX_NICKNAME_LEN,
                            );
                            presence.client_uuid = client_uuid;
                            let sk = StaticSecret::from(identity_secret);
                            presence.my_pub_key = PublicKey::from(&sk).to_bytes();
                            presence.identity_secret = Some(sk);
                            presence.friend_pub_keys = friend_pub_keys;
                            let before = snapshot_friends(&presence.online_friends);
                            if old_rv != presence.rendezvous_addr {
                                presence.online_friends.clear();
                            } else {
                                presence
                                    .online_friends
                                    .retain(|k, _| presence.friend_pub_keys.contains(k));
                            }
                            let after = snapshot_friends(&presence.online_friends);
                            if before != after {
                                evt_tx.send(MeshEvent::FriendsStatusUpdated(after)).await;
                            }
                            if let Some(sess) = session.as_mut() {
                                sess.nickname = nickname;
                                sess.local_pub_key = presence.my_pub_key;
                            }
                            send_presence_ping_and_query(&socket, &presence, session.is_some())
                                .await;
                        }
                        MeshCommand::CallOrInviteFriend { target_pub_key } => {
                            let Some(identity_sk) = presence.identity_secret.as_ref() else {
                                evt_tx
                                    .send(MeshEvent::Error(
                                        "Identity key is not initialized".to_string(),
                                    ))
                                    .await;
                                continue;
                            };
                            if target_pub_key == presence.my_pub_key {
                                continue;
                            }
                            let (invite, is_party_invite, relay_rv_addr) =
                                if let Some(sess) = session.as_ref() {
                                    if sess
                                        .peers
                                        .values()
                                        .any(|p| p.pub_key == Some(target_pub_key))
                                    {
                                        evt_tx
                                            .send(MeshEvent::Error(
                                                "This friend is already in the room".to_string(),
                                            ))
                                            .await;
                                        continue;
                                    }
                                    if sess.embedded_rv.is_some() {
                                        evt_tx
                                            .send(MeshEvent::Error(
                                                "Friend invites require a Rendezvous room rather than a LAN-only hosted room"
                                                    .to_string(),
                                            ))
                                            .await;
                                        continue;
                                    }
                                    (
                                        sess.invite.clone(),
                                        true,
                                        presence.rendezvous_addr.unwrap_or(sess.rendezvous_addr),
                                    )
                                } else {
                                    let Some(rv_addr) = presence.rendezvous_addr else {
                                        evt_tx
                                            .send(MeshEvent::Error(
                                                "Rendezvous server address is not configured"
                                                    .to_string(),
                                            ))
                                            .await;
                                        continue;
                                    };
                                    (InviteLink::generate(), false, rv_addr)
                                };

                            let (ephemeral_pub_key, nonce, ciphertext) = match seal_friend_invite(
                                identity_sk,
                                &target_pub_key,
                                &invite,
                                &presence.nickname,
                                is_party_invite,
                            ) {
                                Ok(sealed) => sealed,
                                Err(e) => {
                                    evt_tx.send(MeshEvent::Error(e)).await;
                                    continue;
                                }
                            };

                            let pkt = SignalingPacket::C2sRelayFriendInvite {
                                sender_pub_key: presence.my_pub_key,
                                target_pub_key,
                                ephemeral_pub_key,
                                nonce,
                                ciphertext,
                            };
                            if let Ok(bytes) = pkt.encode() {
                                let _ = socket.send_to(&bytes, relay_rv_addr).await;
                            }

                            if !is_party_invite {
                                pending_short_code_join = None;
                                pending_lan_auth = None;
                                let mut act_ctx = SessionActivationCtx {
                                    socket: &socket,
                                    lan_tx_sock: lan_tx_sock.as_ref(),
                                    bound_port,
                                    pending_local_avatar: &pending_local_avatar,
                                    local_encoder_backend: &local_encoder_backend,
                                    hevc_decode_supported,
                                    audio_engine: &audio_engine,
                                    tr_tx: &tr_tx,
                                    tr_rx: &mut tr_rx,
                                    evt_tx: &evt_tx,
                                    capture_worker: &mut capture_worker,
                                    video_out_rx: &mut video_out_rx,
                                    decode_handle: &decode_handle,
                                    video_frame_slot: &video_frame_slot,
                                    discovered_lan: &mut discovered_lan,
                                    last_published_lan: &mut last_published_lan,
                                    last_snapshots: &mut last_snapshots,
                                    seal_buf: &mut seal_buf,
                                    relay_buf: &mut relay_buf,
                                    presence: &presence,
                                };
                                activate_joined_session(
                                    &mut session,
                                    &mut act_ctx,
                                    relay_rv_addr,
                                    invite,
                                    presence.nickname.clone(),
                                    presence.client_uuid,
                                    false,
                                    String::new(),
                                    None,
                                    None,
                                )
                                .await;
                            }
                            evt_tx
                                .send(MeshEvent::FriendInviteSent { target_pub_key })
                                .await;
                        }
                        MeshCommand::GenerateShortCode => {
                            let Some(sess) = session.as_mut() else {
                                evt_tx
                                    .send(MeshEvent::Error(
                                        "Join a room before generating a 6-digit code".to_string(),
                                    ))
                                    .await;
                                continue;
                            };
                            if sess.embedded_rv.is_some() {
                                evt_tx
                                    .send(MeshEvent::Error(
                                        "6-digit voice codes require a Rendezvous room rather than a LAN-only hosted room"
                                            .to_string(),
                                    ))
                                    .await;
                                continue;
                            }
                            let code: u32 = rand::thread_rng().gen_range(100_000..=999_999);
                            let pkt = SignalingPacket::C2sRegisterShortCode {
                                room_id: sess.invite.room_id,
                                host_pub_key: presence.my_pub_key,
                                code,
                            };
                            if let Ok(bytes) = pkt.encode() {
                                let _ = socket.send_to(&bytes, sess.rendezvous_addr).await;
                            }
                        }
                        MeshCommand::JoinByShortCode {
                            rendezvous_server,
                            code,
                            nickname,
                            client_uuid,
                        } => {
                            let rv_addr = match resolve_host(&rendezvous_server) {
                                Ok(a) => a,
                                Err(e) => {
                                    evt_tx.send(MeshEvent::Error(e)).await;
                                    continue;
                                }
                            };
                            pending_lan_auth = None;
                            let (spake2_state, msg_a) = spake2_start_joiner(code);
                            let lan_addrs = discover_lan_ipv4_addrs(bound_port);
                            let now = Instant::now();
                            pending_short_code_join = Some(PendingShortCodeJoin {
                                code,
                                rendezvous_addr: rv_addr,
                                nickname,
                                client_uuid,
                                spake2_state: Some(spake2_state),
                                msg_a: msg_a.clone(),
                                host_addrs: Vec::new(),
                                started_at: now,
                                last_retry_sent: now,
                            });
                            let pkt = SignalingPacket::C2sResolveShortCode {
                                code,
                                joiner_pub_key: presence.my_pub_key,
                                joiner_lan_addrs: lan_addrs,
                                spake2_msg_a: msg_a,
                            };
                            if let Ok(bytes) = pkt.encode() {
                                let _ = socket.send_to(&bytes, rv_addr).await;
                            }
                        }
                        #[cfg(test)]
                        MeshCommand::TestGetSessionInfo(reply_tx) => {
                            let info = session.as_ref().map(|s| {
                                (
                                    s.rendezvous_addr,
                                    SocketAddr::from(([127, 0, 0, 1], s.local_udp_port)),
                                    derive_lan_announce_id(&s.invite),
                                    s.embedded_rv.is_some(),
                                )
                            });
                            let _ = reply_tx.try_send(info);
                        }
                        MeshCommand::Shutdown => {
                            stop_capture_worker(&mut capture_worker);
                            decode_handle.reset();
                            let _ = decode_handle.tx.try_send(DecodeJob::Shutdown);
                            audio_engine.set_share_stream_audio(false);
                            audio_engine.clear_stream_audio();
                            if let Some(sess) = session.take() {
                                teardown_session(
                                    &socket,
                                    lan_tx_sock.as_ref(),
                                    sess,
                                    &mut seal_buf,
                                    &mut relay_buf,
                                    true,
                                )
                                .await;
                            }
                            break;
                        }
                    }
                }

                // Voice traffic always has strict priority over file transfers.
                Some(audio_frame) = audio_out_rx.recv() => {
                    let Some(sess) = session.as_mut() else { continue; };
                    if sess.local_peer_id == 0 || sess.peers.is_empty() {
                        continue;
                    }

                    match audio_frame {
                        OutboundAudioFrame::Opus { seq_num, opus_bytes } => {
                            let header = P2pHeader {
                                msg_type: P2pMsgType::AudioOpus,
                                room_prefix: sess.room_prefix,
                                sender_peer_id: sess.local_peer_id,
                                seq_num,
                            };
                            let payload = P2pPayload::AudioOpus {
                                opus_data: opus_bytes.to_vec(),
                            };
                            if sess.cipher.seal_packet(&header, &payload, &mut seal_buf).is_ok() {
                                for &peer_id in sess.peers.keys() {
                                    send_sealed_to_peer(
                                        &socket,
                                        sess,
                                        peer_id,
                                        P2pMsgType::AudioOpus,
                                        &seal_buf,
                                        &mut relay_buf,
                                    )
                                    .await;
                                }
                            }
                        }
                        OutboundAudioFrame::DtxKeepalive { seq_num } => {
                            let header = P2pHeader {
                                msg_type: P2pMsgType::AudioDtxKeepalive,
                                room_prefix: sess.room_prefix,
                                sender_peer_id: sess.local_peer_id,
                                seq_num,
                            };
                            if sess
                                .cipher
                                .seal_packet(&header, &P2pPayload::AudioDtxKeepalive, &mut seal_buf)
                                .is_ok()
                            {
                                for &peer_id in sess.peers.keys() {
                                    send_sealed_to_peer(
                                        &socket,
                                        sess,
                                        peer_id,
                                        P2pMsgType::AudioDtxKeepalive,
                                        &seal_buf,
                                        &mut relay_buf,
                                    )
                                    .await;
                                }
                            }
                        }
                        OutboundAudioFrame::StreamOpus { seq_num, opus_bytes } => {
                            let sharing_audio = sess
                                .local_stream_config
                                .as_ref()
                                .is_some_and(|c| c.share_audio);
                            if sharing_audio && !sess.stream_subscribers.is_empty() {
                                let header = P2pHeader {
                                    msg_type: P2pMsgType::StreamAudioOpus,
                                    room_prefix: sess.room_prefix,
                                    sender_peer_id: sess.local_peer_id,
                                    seq_num,
                                };
                                let payload = P2pPayload::StreamAudioOpus {
                                    opus_data: opus_bytes.to_vec(),
                                };
                                if sess.cipher.seal_packet(&header, &payload, &mut seal_buf).is_ok() {
                                    for &sub_id in &sess.stream_subscribers {
                                        send_sealed_to_peer(
                                            &socket,
                                            sess,
                                            sub_id,
                                            P2pMsgType::StreamAudioOpus,
                                            &seal_buf,
                                            &mut relay_buf,
                                        )
                                        .await;
                                    }
                                }
                            }
                        }
                    }
                }

                Ok((first_len, first_src)) = socket.recv_from(&mut recv_buf) => {
                    let mut cur_len = first_len;
                    let mut cur_src = first_src;
                    for batch_idx in 0..MAX_UDP_RX_BATCH {
                        if batch_idx > 0 {
                            match socket.try_recv_from(&mut recv_buf) {
                                Ok((n, addr)) => {
                                    cur_len = n;
                                    cur_src = addr;
                                }
                                Err(_) => break,
                            }
                        }
                        let len = cur_len;
                        let src_addr = cur_src;
                        let datagram = &mut recv_buf[..len];
                        if let Some(sess) = session.as_mut() {
                            if src_addr == sess.rendezvous_addr {
                                sess.last_rv_rx = Instant::now();
                            }
                        }

                    if is_spake2_datagram(datagram) {
                        if let Ok(spkt) = Spake2Packet::decode(datagram) {
                            match spkt {
                                Spake2Packet::HostReply {
                                    code,
                                    spake2_msg_b,
                                    nonce,
                                    encrypted_invite,
                                } => {
                                    if pending_short_code_join
                                        .as_ref()
                                        .is_some_and(|p| p.code == code)
                                    {
                                        if let Some(mut pending) = pending_short_code_join.take() {
                                            if let Some(st) = pending.spake2_state.take() {
                                                match spake2_joiner_open_invite(
                                                    st,
                                                    code,
                                                    &spake2_msg_b,
                                                    &nonce,
                                                    &encrypted_invite,
                                                ) {
                                                    Ok(invite) => {
                                                        let mut act_ctx = SessionActivationCtx {
                                                            socket: &socket,
                                                            lan_tx_sock: lan_tx_sock.as_ref(),
                                                            bound_port,
                                                            pending_local_avatar: &pending_local_avatar,
                                                            local_encoder_backend: &local_encoder_backend,
                                                            hevc_decode_supported,
                                                            audio_engine: &audio_engine,
                                                            tr_tx: &tr_tx,
                                                            tr_rx: &mut tr_rx,
                                                            evt_tx: &evt_tx,
                                                            capture_worker: &mut capture_worker,
                                                            video_out_rx: &mut video_out_rx,
                                                            decode_handle: &decode_handle,
                                                            video_frame_slot: &video_frame_slot,
                                                            discovered_lan: &mut discovered_lan,
                                                            last_published_lan: &mut last_published_lan,
                                                            last_snapshots: &mut last_snapshots,
                                                            seal_buf: &mut seal_buf,
                                                            relay_buf: &mut relay_buf,
                                                            presence: &presence,
                                                        };
                                                        activate_joined_session(
                                                            &mut session,
                                                            &mut act_ctx,
                                                            pending.rendezvous_addr,
                                                            invite,
                                                            pending.nickname,
                                                            pending.client_uuid,
                                                            false,
                                                            String::new(),
                                                            None,
                                                            None,
                                                        )
                                                        .await;
                                                    }
                                                    Err(err) => {
                                                        evt_tx
                                                            .send(MeshEvent::Error(format!(
                                                                "6-digit code verification failed: {err}"
                                                            )))
                                                            .await;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                Spake2Packet::JoinerRetry {
                                    code,
                                    spake2_msg_a,
                                } => {
                                    if let Some(sess) = session.as_mut() {
                                        try_host_spake2_reply(
                                            &socket,
                                            sess,
                                            code,
                                            &spake2_msg_a,
                                            &[src_addr],
                                        )
                                        .await;
                                    }
                                }
                                Spake2Packet::LanAuthReq {
                                    announce_id,
                                    attempt_id,
                                    spake2_msg_a,
                                } => {
                                    handle_lan_auth_req(
                                        &socket,
                                        session.as_mut(),
                                        announce_id,
                                        attempt_id,
                                        &spake2_msg_a,
                                        src_addr,
                                    )
                                    .await;
                                }
                                Spake2Packet::LanAuthReply {
                                    announce_id,
                                    attempt_id,
                                    status,
                                    spake2_msg_b,
                                    nonce,
                                    encrypted_invite,
                                } => {
                                    let matches_pending =
                                        pending_lan_auth.as_ref().is_some_and(|p| {
                                            p.room.room_id == announce_id
                                                && p.attempt_id == attempt_id
                                                && src_addr.port() == p.auth_addr.port()
                                                && (src_addr.ip() == p.auth_addr.ip()
                                                    || (src_addr.ip().is_loopback()
                                                        && p.auth_addr.ip().is_loopback()))
                                        });
                                    if matches_pending {
                                        if let Some(mut pending) = pending_lan_auth.take() {
                                            if status == LAN_AUTH_STATUS_RATE_LIMITED {
                                                evt_tx
                                                    .send(MeshEvent::LanAuthFailed(
                                                        "Too many wrong password attempts. Please wait 60 seconds."
                                                            .to_string(),
                                                    ))
                                                    .await;
                                            } else if status == LAN_AUTH_STATUS_NOT_FOUND {
                                                evt_tx
                                                    .send(MeshEvent::LanAuthFailed(
                                                        "This LAN room is no longer available."
                                                            .to_string(),
                                                    ))
                                                    .await;
                                            } else if status != LAN_AUTH_STATUS_OK {
                                                evt_tx
                                                    .send(MeshEvent::LanAuthFailed(
                                                        "Rejected by LAN room host.".to_string(),
                                                    ))
                                                    .await;
                                            } else if let Some(st) = pending.spake2_state.take() {
                                                match spake2_lan_joiner_open_invite(
                                                    st,
                                                    &announce_id,
                                                    attempt_id,
                                                    &spake2_msg_b,
                                                    &nonce,
                                                    &encrypted_invite,
                                                ) {
                                                    Ok(invite) => {
                                                        let mut act_ctx = SessionActivationCtx {
                                                            socket: &socket,
                                                            lan_tx_sock: lan_tx_sock.as_ref(),
                                                            bound_port,
                                                            pending_local_avatar:
                                                                &pending_local_avatar,
                                                            local_encoder_backend:
                                                                &local_encoder_backend,
                                                            hevc_decode_supported,
                                                            audio_engine: &audio_engine,
                                                            tr_tx: &tr_tx,
                                                            tr_rx: &mut tr_rx,
                                                            evt_tx: &evt_tx,
                                                            capture_worker: &mut capture_worker,
                                                            video_out_rx: &mut video_out_rx,
                                                            decode_handle: &decode_handle,
                                                            video_frame_slot: &video_frame_slot,
                                                            discovered_lan: &mut discovered_lan,
                                                            last_published_lan:
                                                                &mut last_published_lan,
                                                            last_snapshots: &mut last_snapshots,
                                                            seal_buf: &mut seal_buf,
                                                            relay_buf: &mut relay_buf,
                                                            presence: &presence,
                                                        };
                                                        activate_joined_session(
                                                            &mut session,
                                                            &mut act_ctx,
                                                            pending.room.host_addr,
                                                            invite,
                                                            pending.nickname,
                                                            pending.client_uuid,
                                                            true,
                                                            pending.room.room_name,
                                                            Some(pending.password),
                                                            None,
                                                        )
                                                        .await;
                                                    }
                                                    Err(err) => {
                                                        evt_tx
                                                            .send(MeshEvent::LanAuthFailed(err))
                                                            .await;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    } else if is_relay_datagram(datagram) {
                        let Some(sess) = session.as_mut() else { continue; };
                        if src_addr == sess.rendezvous_addr {
                            if let Some((room_id, relay_sender_pid)) =
                                parse_relay_header(datagram).ok().map(|(r, p, _)| (r, p))
                            {
                                if room_id == sess.invite.room_id {
                                    let mut rx_ctx = MeshRxCtx {
                                        socket: &socket,
                                        audio_engine: &audio_engine,
                                        seal_buf: &mut seal_buf,
                                        relay_buf: &mut relay_buf,
                                        tr_tx: &tr_tx,
                                        evt_tx: &evt_tx,
                                        capture_worker: &mut capture_worker,
                                        video_out_tx: &video_out_tx,
                                        video_frame_slot: &video_frame_slot,
                                        video_notify: &video_notify,
                                        decode_handle: &decode_handle,
                                        lan_tx_sock: lan_tx_sock.as_ref(),
                                    };
                                    handle_p2p_rx(
                                        &socket,
                                        &mut datagram[RELAY_HEADER_LEN..],
                                        src_addr,
                                        Some(RelaySource::Server),
                                        Some(relay_sender_pid),
                                        sess,
                                        &mut rx_ctx,
                                    )
                                    .await;
                                    forward_transfer_events(&mut tr_rx, &evt_tx).await;
                                }
                            }
                        }
                    } else if is_peer_relay_datagram(datagram) {
                        let Some(sess) = session.as_mut() else { continue; };
                        let mut rx_ctx = MeshRxCtx {
                            socket: &socket,
                            audio_engine: &audio_engine,
                            seal_buf: &mut seal_buf,
                            relay_buf: &mut relay_buf,
                            tr_tx: &tr_tx,
                            evt_tx: &evt_tx,
                            capture_worker: &mut capture_worker,
                            video_out_tx: &video_out_tx,
                            video_frame_slot: &video_frame_slot,
                            video_notify: &video_notify,
                            decode_handle: &decode_handle,
                            lan_tx_sock: lan_tx_sock.as_ref(),
                        };
                        handle_peer_relay_datagram(&socket, datagram, src_addr, sess, &mut rx_ctx)
                            .await;
                        forward_transfer_events(&mut tr_rx, &evt_tx).await;
                    } else if Some(src_addr) == presence.rendezvous_addr
                        || pending_short_code_join
                            .as_ref()
                            .is_some_and(|p| p.rendezvous_addr == src_addr)
                        || session.as_ref().is_some_and(|s| src_addr == s.rendezvous_addr)
                    {
                        if let Ok(pkt) = SignalingPacket::decode(datagram) {
                            let handled = handle_global_signaling_packet(
                                &pkt,
                                &socket,
                                &audio_engine,
                                &evt_tx,
                                &mut presence,
                                &mut session,
                                &mut pending_short_code_join,
                            )
                            .await;
                            if !handled {
                                if let Some(sess) = session.as_mut() {
                                    if src_addr == sess.rendezvous_addr {
                                        let mut rx_ctx = MeshRxCtx {
                                            socket: &socket,
                                            audio_engine: &audio_engine,
                                            seal_buf: &mut seal_buf,
                                            relay_buf: &mut relay_buf,
                                            tr_tx: &tr_tx,
                                            evt_tx: &evt_tx,
                                            capture_worker: &mut capture_worker,
                                            video_out_tx: &video_out_tx,
                                            video_frame_slot: &video_frame_slot,
                                            video_notify: &video_notify,
                                            decode_handle: &decode_handle,
                                            lan_tx_sock: lan_tx_sock.as_ref(),
                                        };
                                        handle_signaling_rx(datagram, sess, &mut rx_ctx).await;
                                    }
                                }
                            }
                        }
                    } else {
                        let Some(sess) = session.as_mut() else { continue; };
                        let mut rx_ctx = MeshRxCtx {
                            socket: &socket,
                            audio_engine: &audio_engine,
                            seal_buf: &mut seal_buf,
                            relay_buf: &mut relay_buf,
                            tr_tx: &tr_tx,
                            evt_tx: &evt_tx,
                            capture_worker: &mut capture_worker,
                            video_out_tx: &video_out_tx,
                            video_frame_slot: &video_frame_slot,
                            video_notify: &video_notify,
                            decode_handle: &decode_handle,
                            lan_tx_sock: lan_tx_sock.as_ref(),
                        };
                        handle_p2p_rx(
                            &socket,
                            datagram,
                            src_addr,
                            None,
                            None,
                            sess,
                            &mut rx_ctx,
                        )
                        .await;
                        forward_transfer_events(&mut tr_rx, &evt_tx).await;
                    }
                    }
                }

                // Priority 2: paced video fragments while subscribers exist.
                Some(frags) = video_out_rx.recv(), if !has_paced_video => {
                    let Some(sess) = session.as_mut() else { continue; };
                    if sess.local_peer_id == 0 || sess.stream_subscribers.is_empty() {
                        sess.video_tx_queue.clear();
                        continue;
                    }
                    sess.video_tx_queue.load(frags);
                    flush_video_tx_burst(&socket, sess, &mut seal_buf, &mut relay_buf).await;
                }

                _ = video_pace_tick.tick(), if has_paced_video => {
                    if let Some(sess) = session.as_mut() {
                        flush_video_tx_burst(&socket, sess, &mut seal_buf, &mut relay_buf).await;
                    }
                }

                Some(dec_ev) = decode_err_rx.recv() => {
                    let Some(sess) = session.as_mut() else { continue; };
                    let now = Instant::now();
                    match dec_ev {
                        DecodeThreadEvent::UnsupportedCodec {
                            streamer_peer_id,
                            generation,
                            reason,
                        } => {
                            if sess.watched_peer_id == Some(streamer_peer_id)
                                && generation == decode_handle.generation.load(Ordering::Acquire)
                            {
                                sess.video_reassembler.reset();
                                decode_handle.reset();
                                if sess.sent_decoder_error_to != Some(streamer_peer_id) {
                                    sess.sent_decoder_error_to = Some(streamer_peer_id);
                                    send_stream_decoder_error(
                                        &socket,
                                        sess,
                                        &mut seal_buf,
                                        &mut relay_buf,
                                        streamer_peer_id,
                                        VideoCodec::H265.as_u8(),
                                        &reason,
                                    )
                                    .await;
                                }
                            }
                        }
                        DecodeThreadEvent::CorruptBitstream {
                            streamer_peer_id,
                            generation,
                        } => {
                            if sess.watched_peer_id == Some(streamer_peer_id)
                                && generation == decode_handle.generation.load(Ordering::Acquire)
                            {
                                sess.video_reassembler.reset();
                                if now.duration_since(sess.last_pli_sent) > Duration::from_millis(120) {
                                    send_stream_pli(
                                        &socket,
                                        sess,
                                        &mut seal_buf,
                                        &mut relay_buf,
                                        streamer_peer_id,
                                    )
                                    .await;
                                    sess.last_pli_sent = now;
                                }
                            }
                        }
                    }
                }

                _ = qos_tick.tick() => {
                    let Some(sess) = session.as_mut() else { continue; };
                    if sess.local_peer_id == 0
                        || sess.peers.is_empty()
                        || !sess.transfer.has_pending_work()
                    {
                        continue;
                    }
                    let peer_addrs = connected_peer_map(sess);
                    let peer_uuids: HashMap<PeerId, ClientUuid> = sess
                        .peers
                        .iter()
                        .map(|(&pid, st)| (pid, st.record.client_uuid))
                        .collect();
                    let expected_uuids: Vec<ClientUuid> = peer_uuids.values().copied().collect();
                    let mut ctx = TransferCtx {
                        local_peer_id: sess.local_peer_id,
                        local_uuid: sess.local_uuid,
                        local_nickname: &sess.nickname,
                        socket: &socket,
                        cipher: &sess.cipher,
                        room_prefix: sess.room_prefix,
                        control_seq: &mut sess.control_seq,
                        seal_buf: &mut seal_buf,
                        evt_tx: &tr_tx,
                    };
                    sess.transfer
                        .tick_10ms(&peer_addrs, &peer_uuids, &expected_uuids, &mut ctx)
                        .await;
                    forward_transfer_events(&mut tr_rx, &evt_tx).await;
                }

                // Периодический Heartbeat на Rendezvous-сервер (раз в 2 сек)
                _ = register_tick.tick() => {
                    if let Some(sess) = session.as_mut() {
                        if sess.is_lan_room
                            && sess.embedded_rv.is_none()
                            && sess.local_peer_id != 0
                            && sess.last_rv_rx.elapsed() > LAN_RV_LOST_TIMEOUT
                            && sess
                                .migration_claimed_at
                                .is_none_or(|t| t.elapsed() > Duration::from_secs(5))
                        {
                            let min_active_other = sess
                                .peers
                                .iter()
                                .filter(|(_, p)| {
                                    matches!(
                                        p.status,
                                        PeerConnStatus::Connected
                                            | PeerConnStatus::MeshRelayed
                                            | PeerConnStatus::Relayed
                                    )
                                })
                                .map(|(&pid, _)| pid)
                                .min();
                            if min_active_other.is_some_and(|min_id| sess.local_peer_id < min_id) {
                                promote_to_lan_host(
                                    &socket,
                                    lan_tx_sock.as_ref(),
                                    sess,
                                    &mut seal_buf,
                                    &mut relay_buf,
                                )
                                .await;
                            }
                        }
                        send_register(&socket, sess).await;
                    }
                }

                Some((len, src_addr)) = recv_lan_opt(lan_rx_sock.as_ref(), &mut lan_rx_buf) => {
                    let now = Instant::now();
                    if let Ok(pkt) = LanDiscoveryPacket::decode(&lan_rx_buf[..len]) {
                        match pkt {
                            LanDiscoveryPacket::Announce {
                                room_id,
                                secret,
                                rv_port,
                                peer_count,
                                max_peers,
                                room_name,
                                host_nickname,
                            } => {
                                let is_own_active = session
                                    .as_ref()
                                    .is_some_and(|s| s.invite.room_id == room_id);
                                if !is_own_active && rv_port != 0 {
                                    if !discovered_lan.contains_key(&room_id)
                                        && discovered_lan.len() >= 64
                                    {
                                        continue;
                                    }
                                    let room = DiscoveredLanRoom {
                                        room_id,
                                        secret: Some(secret),
                                        host_addr: SocketAddr::new(src_addr.ip(), rv_port),
                                        auth_addr: None,
                                        room_name,
                                        host_nickname,
                                        peer_count,
                                        max_peers,
                                    };
                                    discovered_lan.insert(room_id, (room, now));
                                    publish_lan_rooms(&discovered_lan, &mut last_published_lan, &evt_tx)
                                        .await;
                                }
                            }
                            LanDiscoveryPacket::AnnounceLocked {
                                announce_id,
                                rv_port,
                                auth_port,
                                peer_count,
                                max_peers,
                                room_name,
                                host_nickname,
                            } => {
                                let is_own_active = session.as_ref().is_some_and(|s| {
                                    s.invite.room_id == announce_id
                                        || derive_lan_announce_id(&s.invite) == announce_id
                                });
                                if !is_own_active && rv_port != 0 && auth_port != 0 {
                                    if !discovered_lan.contains_key(&announce_id)
                                        && discovered_lan.len() >= 64
                                    {
                                        continue;
                                    }
                                    let room = DiscoveredLanRoom {
                                        room_id: announce_id,
                                        secret: None,
                                        host_addr: SocketAddr::new(src_addr.ip(), rv_port),
                                        auth_addr: Some(SocketAddr::new(src_addr.ip(), auth_port)),
                                        room_name,
                                        host_nickname,
                                        peer_count,
                                        max_peers,
                                    };
                                    discovered_lan.insert(announce_id, (room, now));
                                    publish_lan_rooms(
                                        &discovered_lan,
                                        &mut last_published_lan,
                                        &evt_tx,
                                    )
                                    .await;
                                }
                            }
                            LanDiscoveryPacket::Goodbye { room_id, rv_port } => {
                                let should_remove = discovered_lan.get(&room_id).is_some_and(|(r, _)| {
                                    r.host_addr.ip() == src_addr.ip()
                                        && (rv_port == 0 || r.host_addr.port() == rv_port)
                                });
                                if should_remove {
                                    discovered_lan.remove(&room_id);
                                    publish_lan_rooms(&discovered_lan, &mut last_published_lan, &evt_tx)
                                        .await;
                                }
                            }
                        }
                    }
                }

                _ = lan_tick.tick(), if lan_work_active => {
                    let now = Instant::now();
                    let before = discovered_lan.len();
                    discovered_lan.retain(|_, (_, seen_at)| now.duration_since(*seen_at) <= LAN_ROOM_TTL);
                    if discovered_lan.len() != before {
                        publish_lan_rooms(&discovered_lan, &mut last_published_lan, &evt_tx).await;
                    }
                    if let Some(sess) = session.as_ref() {
                        if sess.embedded_rv.is_some() {
                            broadcast_lan_announce(lan_tx_sock.as_ref(), sess).await;
                        }
                    }
                }

                _ = punch_tick.tick() => {
                    let now = Instant::now();
                    if let Some(pending) = pending_short_code_join.as_mut() {
                        if now.duration_since(pending.started_at) >= SHORT_CODE_JOIN_TIMEOUT {
                            pending_short_code_join = None;
                            evt_tx
                                .send(MeshEvent::Error(
                                    "Timed out connecting with 6-digit voice code".to_string(),
                                ))
                                .await;
                        } else if !pending.host_addrs.is_empty()
                            && now.duration_since(pending.last_retry_sent) >= SHORT_CODE_RETRY_INTERVAL
                        {
                            pending.last_retry_sent = now;
                            let pkt = Spake2Packet::JoinerRetry {
                                code: pending.code,
                                spake2_msg_a: pending.msg_a.clone(),
                            };
                            if let Ok(bytes) = pkt.encode() {
                                for &addr in &pending.host_addrs {
                                    let _ = socket.send_to(&bytes, addr).await;
                                }
                            }
                        }
                    }

                    if let Some(pending) = pending_lan_auth.as_mut() {
                        if now.duration_since(pending.started_at) >= LAN_AUTH_TIMEOUT {
                            pending_lan_auth = None;
                            evt_tx
                                .send(MeshEvent::LanAuthFailed(
                                    "Timed out waiting for LAN host password verification"
                                        .to_string(),
                                ))
                                .await;
                        } else if now.duration_since(pending.last_retry_sent)
                            >= LAN_AUTH_RETRY_INTERVAL
                        {
                            pending.last_retry_sent = now;
                            let req = Spake2Packet::LanAuthReq {
                                announce_id: pending.room.room_id,
                                attempt_id: pending.attempt_id,
                                spake2_msg_a: pending.msg_a.clone(),
                            };
                            if let Ok(bytes) = req.encode() {
                                let _ = socket.send_to(&bytes, pending.auth_addr).await;
                            }
                        }
                    }

                    let Some(sess) = session.as_mut() else { continue; };
                    if sess.local_peer_id == 0 {
                        continue;
                    }
                    let now_us = now_micros();
                    let mut relay_fallback_peers = Vec::new();
                    let mut degraded_to_server_relay = Vec::new();
                    let peer_ids: Vec<PeerId> = sess.peers.keys().copied().collect();

                    for peer_id in peer_ids {
                        let Some(status) = sess.peers.get(&peer_id).map(|p| p.status) else {
                            continue;
                        };
                        match status {
                            PeerConnStatus::Punching => {
                                let timed_out = sess
                                    .peers
                                    .get(&peer_id)
                                    .is_some_and(|p| now.duration_since(p.punch_started_at) >= RELAY_FALLBACK_AFTER);
                                if timed_out {
                                    let mesh_candidate = find_best_mesh_relay(sess, peer_id, None, now);
                                    if let Some(peer) = sess.peers.get_mut(&peer_id) {
                                        peer.last_direct_probe = now;
                                        if let Some(b_pid) = mesh_candidate {
                                            peer.status = PeerConnStatus::MeshRelayed;
                                            peer.mesh_relay_peer = Some(b_pid);
                                            info!(
                                                "Hole punch timed out for peer #{} ({}); activating Peer-Mesh relay via #{}",
                                                peer.record.peer_id, peer.record.nickname, b_pid
                                            );
                                        } else {
                                            peer.status = PeerConnStatus::Relayed;
                                            peer.mesh_relay_peer = None;
                                            degraded_to_server_relay.push(peer_id);
                                            info!(
                                                "Hole punch timed out for peer #{} ({}); activating server UDP relay fallback (voice/chat only)",
                                                peer.record.peer_id, peer.record.nickname
                                            );
                                        }
                                    }
                                    relay_fallback_peers.push(peer_id);
                                    continue;
                                }
                                if seal_punch_syn_parts(
                                    &sess.cipher,
                                    sess.room_prefix,
                                    sess.local_peer_id,
                                    sess.local_uuid,
                                    &mut sess.control_seq,
                                    now_us,
                                    &mut seal_buf,
                                ) {
                                    if let Some(peer) = sess.peers.get(&peer_id) {
                                        let _ = socket.send_to(&seal_buf, peer.record.public_addr).await;
                                        for lan in &peer.record.lan_addrs {
                                            let _ = socket.send_to(&seal_buf, SocketAddr::V4(*lan)).await;
                                        }
                                    }
                                }
                            }
                            PeerConnStatus::MeshRelayed | PeerConnStatus::Relayed => {
                                let should_probe = sess
                                    .peers
                                    .get(&peer_id)
                                    .is_some_and(|p| now.duration_since(p.last_direct_probe) >= RELAY_DIRECT_PROBE_INTERVAL);
                                if should_probe {
                                    if let Some(peer) = sess.peers.get_mut(&peer_id) {
                                        peer.last_direct_probe = now;
                                    }
                                    if seal_punch_syn_parts(
                                        &sess.cipher,
                                        sess.room_prefix,
                                        sess.local_peer_id,
                                        sess.local_uuid,
                                        &mut sess.control_seq,
                                        now_us,
                                        &mut seal_buf,
                                    ) {
                                        if let Some(peer) = sess.peers.get(&peer_id) {
                                            let _ = socket.send_to(&seal_buf, peer.record.public_addr).await;
                                            for lan in &peer.record.lan_addrs {
                                                let _ = socket.send_to(&seal_buf, SocketAddr::V4(*lan)).await;
                                            }
                                        }
                                    }
                                }
                            }
                            PeerConnStatus::NatFailed => {
                                let should_probe = sess
                                    .peers
                                    .get(&peer_id)
                                    .is_some_and(|p| now.duration_since(p.last_direct_probe) >= NAT_FAILED_PROBE_INTERVAL);
                                if should_probe {
                                    if let Some(peer) = sess.peers.get_mut(&peer_id) {
                                        peer.last_direct_probe = now;
                                    }
                                    if seal_punch_syn_parts(
                                        &sess.cipher,
                                        sess.room_prefix,
                                        sess.local_peer_id,
                                        sess.local_uuid,
                                        &mut sess.control_seq,
                                        now_us,
                                        &mut seal_buf,
                                    ) {
                                        if let Some(peer) = sess.peers.get(&peer_id) {
                                            let _ = socket.send_to(&seal_buf, peer.record.public_addr).await;
                                            for lan in &peer.record.lan_addrs {
                                                let _ = socket.send_to(&seal_buf, SocketAddr::V4(*lan)).await;
                                            }
                                        }
                                    }
                                }
                            }
                            PeerConnStatus::Connected => {}
                        }
                    }

                    if !degraded_to_server_relay.is_empty() {
                        enforce_video_route_policy(
                            sess,
                            &degraded_to_server_relay,
                            &mut capture_worker,
                            &video_out_tx,
                            &video_frame_slot,
                            &video_notify,
                            &decode_handle,
                            &audio_engine,
                            &evt_tx,
                        )
                        .await;
                    }

                    for peer_id in relay_fallback_peers {
                        if seal_punch_syn_parts(
                            &sess.cipher,
                            sess.room_prefix,
                            sess.local_peer_id,
                            sess.local_uuid,
                            &mut sess.control_seq,
                            now_us,
                            &mut seal_buf,
                        ) {
                            send_sealed_to_peer(
                                &socket,
                                sess,
                                peer_id,
                                P2pMsgType::PunchSyn,
                                &seal_buf,
                                &mut relay_buf,
                            )
                            .await;
                        }
                    }
                }

                _ = ping_tick.tick() => {
                    let Some(sess) = session.as_mut() else { continue; };
                    if sess.local_peer_id == 0 {
                        continue;
                    }
                    let now = Instant::now();
                    let now_us = now_micros();

                    let mut went_stale = Vec::new();
                    let mut ping_peers = Vec::new();
                    for (&pid, peer) in &mut sess.peers {
                        match peer.status {
                            PeerConnStatus::Connected => {
                                if now.duration_since(peer.last_rx) > PEER_STALE_TIMEOUT {
                                    peer.status = PeerConnStatus::Punching;
                                    peer.active_addr = None;
                                    peer.mesh_relay_peer = None;
                                    peer.reported_direct_peers.clear();
                                    peer.reachability_updated_at = None;
                                    peer.punch_started_at = now;
                                    went_stale.push(pid);
                                    continue;
                                }
                                if peer.active_addr.is_some() {
                                    ping_peers.push(pid);
                                }
                            }
                            PeerConnStatus::MeshRelayed => {
                                if now.duration_since(peer.last_rx) > RELAY_STALE_TIMEOUT {
                                    peer.status = PeerConnStatus::Relayed;
                                    peer.mesh_relay_peer = None;
                                    peer.last_rx = now;
                                    went_stale.push(pid);
                                    continue;
                                }
                                ping_peers.push(pid);
                            }
                            PeerConnStatus::Relayed => {
                                if now.duration_since(peer.last_rx) > RELAY_STALE_TIMEOUT {
                                    peer.status = PeerConnStatus::NatFailed;
                                    went_stale.push(pid);
                                    continue;
                                }
                                ping_peers.push(pid);
                            }
                            PeerConnStatus::Punching | PeerConnStatus::NatFailed => {}
                        }
                    }

                    let route_changes = reconcile_mesh_routes(&socket, sess, &mut seal_buf, &mut relay_buf, now).await;
                    went_stale.extend(route_changes);

                    for pid in ping_peers {
                        sess.control_seq = sess.control_seq.wrapping_add(1);
                        let hdr = P2pHeader {
                            msg_type: P2pMsgType::Ping,
                            room_prefix: sess.room_prefix,
                            sender_peer_id: sess.local_peer_id,
                            seq_num: sess.control_seq,
                        };
                        let payload = P2pPayload::Ping { timestamp_us: now_us };
                        if sess.cipher.seal_packet(&hdr, &payload, &mut seal_buf).is_ok() {
                            send_sealed_to_peer(&socket, sess, pid, P2pMsgType::Ping, &seal_buf, &mut relay_buf).await;
                        }
                        if sess.peers.get(&pid).is_some_and(|p| p.pub_key.is_none()) {
                            send_identity_announce_to_peer(&socket, sess, pid, &mut seal_buf, &mut relay_buf).await;
                        }
                    }

                    broadcast_mesh_reachability_if_needed(&socket, sess, &mut seal_buf, &mut relay_buf, true).await;

                    if !went_stale.is_empty() {
                        enforce_video_route_policy(
                            sess,
                            &went_stale,
                            &mut capture_worker,
                            &video_out_tx,
                            &video_frame_slot,
                            &video_notify,
                            &decode_handle,
                            &audio_engine,
                            &evt_tx,
                        )
                        .await;
                    }
                }

                // Синхронизация телеметрии в UI (раз в 120 мс)
                _ = ui_sync_tick.tick() => {
                    if let Some(sess) = session.as_ref() {
                        let mut snapshots = Vec::with_capacity(sess.peers.len());
                        for (&peer_id, p) in &sess.peers {
                            let dsp_stats = audio_engine.peer_stats(peer_id);
                            snapshots.push(PeerUiSnapshot {
                                peer_id,
                                client_uuid: p.record.client_uuid,
                                pub_key: p.pub_key,
                                nickname: p.record.nickname.clone(),
                                status: p.status,
                                route: classify_peer_route(p.status, p.active_addr),
                                rtt_ms: p.rtt_ms,
                                jitter_ms: audio_engine.jitter_target_ms(),
                                loss_pct: dsp_stats.loss_pct,
                                is_speaking: dsp_stats.is_speaking,
                                stream_info: p.stream_info.clone(),
                            });
                        }
                        snapshots.sort_by_key(|s| s.peer_id);
                        if snapshots != last_snapshots {
                            last_snapshots = snapshots.clone();
                            evt_tx.try_send(MeshEvent::PeersUpdated(snapshots));
                        }
                    } else if !last_snapshots.is_empty() {
                        last_snapshots.clear();
                    }
                }

                _ = presence_tick.tick() => {
                    let now = Instant::now();
                    if let Some(sess) = session.as_mut() {
                        if sess
                            .active_short_code
                            .is_some_and(|(_, expires_at)| now >= expires_at)
                        {
                            sess.active_short_code = None;
                            evt_tx.send(MeshEvent::ShortCodeCleared).await;
                        }
                    }
                    let before = snapshot_friends(&presence.online_friends);
                    presence
                        .online_friends
                        .retain(|_, (_, _, seen_at)| now.duration_since(*seen_at) <= PRESENCE_UI_TTL);
                    let after = snapshot_friends(&presence.online_friends);
                    if before != after {
                        evt_tx.send(MeshEvent::FriendsStatusUpdated(after)).await;
                    }
                    send_presence_ping_and_query(&socket, &presence, session.is_some()).await;
                }
            }
        }
    });

    (cmd_tx, evt_rx)
}

fn classify_peer_route(status: PeerConnStatus, active_addr: Option<SocketAddr>) -> PeerRouteKind {
    match status {
        PeerConnStatus::Punching => PeerRouteKind::Connecting,
        PeerConnStatus::MeshRelayed => PeerRouteKind::MeshRelayed,
        PeerConnStatus::Relayed => PeerRouteKind::Relayed,
        PeerConnStatus::NatFailed => PeerRouteKind::NatFailed,
        PeerConnStatus::Connected => match active_addr {
            Some(SocketAddr::V4(v4))
                if v4.ip().is_private() || v4.ip().is_loopback() || v4.ip().is_link_local() =>
            {
                PeerRouteKind::LanDirect
            }
            Some(SocketAddr::V6(v6))
                if v6.ip().is_loopback()
                    || ((v6.ip().segments()[0] & 0xfe00) == 0xfc00)
                    || ((v6.ip().segments()[0] & 0xffc0) == 0xfe80) =>
            {
                PeerRouteKind::LanDirect
            }
            _ => PeerRouteKind::P2pDirect,
        },
    }
}

fn seal_punch_syn_parts(
    cipher: &RoomCipher,
    room_prefix: [u8; 4],
    local_peer_id: PeerId,
    local_uuid: ClientUuid,
    control_seq: &mut u64,
    now_us: u64,
    seal_buf: &mut Vec<u8>,
) -> bool {
    *control_seq = control_seq.wrapping_add(1);
    let hdr = P2pHeader {
        msg_type: P2pMsgType::PunchSyn,
        room_prefix,
        sender_peer_id: local_peer_id,
        seq_num: *control_seq,
    };
    let payload = P2pPayload::PunchSyn {
        client_uuid: local_uuid,
        timestamp_us: now_us,
    };
    cipher.seal_packet(&hdr, &payload, seal_buf).is_ok()
}

async fn send_register(socket: &UdpSocket, sess: &ActiveSession) {
    let pkt = SignalingPacket::C2sRegister {
        room_id: sess.invite.room_id,
        client_uuid: sess.local_uuid,
        nickname: sess.nickname.clone(),
        lan_addrs: sess.lan_addrs.clone(),
    };
    if let Ok(bytes) = pkt.encode() {
        let _ = socket.send_to(&bytes, sess.rendezvous_addr).await;
    }
}

async fn send_leave(socket: &UdpSocket, sess: &ActiveSession) {
    let pkt = SignalingPacket::C2sLeave {
        room_id: sess.invite.room_id,
        client_uuid: sess.local_uuid,
    };
    if let Ok(bytes) = pkt.encode() {
        let _ = socket.send_to(&bytes, sess.rendezvous_addr).await;
    }
}

async fn handle_signaling_rx(datagram: &[u8], sess: &mut ActiveSession, io: &mut MeshRxCtx<'_>) {
    let Ok(pkt) = SignalingPacket::decode(datagram) else {
        return;
    };

    match pkt {
        SignalingPacket::S2cRoomState {
            assigned_peer_id,
            peers,
        } => {
            let prev_local_id = sess.local_peer_id;
            if sess.local_peer_id != assigned_peer_id {
                sess.local_peer_id = assigned_peer_id;
                if prev_local_id == 0 {
                    info!("Joined room as peer #{}", assigned_peer_id);
                    let _ = io
                        .evt_tx
                        .send(MeshEvent::JoinedRoom {
                            local_peer_id: assigned_peer_id,
                            invite_uri: sess.invite.to_uri(),
                            is_lan_locked: sess.is_lan_room && sess.lan_room_password.is_some(),
                        })
                        .await;
                } else {
                    info!(
                        "Reassigned local peer slot #{} -> #{} after coordinator handoff",
                        prev_local_id, assigned_peer_id
                    );
                }
            }

            let now = Instant::now();
            let mut active_ids = Vec::with_capacity(peers.len());

            for rec in peers {
                if rec.client_uuid == sess.local_uuid || rec.peer_id == sess.local_peer_id {
                    continue;
                }
                active_ids.push(rec.peer_id);

                if let Some(existing) = sess.peers.get_mut(&rec.peer_id) {
                    if existing.record.client_uuid == rec.client_uuid {
                        existing.record = rec;
                        continue;
                    }
                }

                let existing_old_id = sess
                    .peers
                    .iter()
                    .find(|(_, st)| st.record.client_uuid == rec.client_uuid)
                    .map(|(&pid, _)| pid);

                if let Some(old_id) = existing_old_id {
                    if let Some(mut moved) = sess.peers.remove(&old_id) {
                        moved.record = rec.clone();
                        if sess.stream_subscribers.remove(&old_id) {
                            sess.stream_subscribers.insert(rec.peer_id);
                        }
                        if sess.watched_peer_id == Some(old_id) {
                            sess.watched_peer_id = Some(rec.peer_id);
                        }
                        io.audio_engine.remove_peer(old_id);
                        for other in sess.peers.values_mut() {
                            if other.mesh_relay_peer == Some(old_id) {
                                other.mesh_relay_peer = Some(rec.peer_id);
                            }
                            if other.reported_direct_peers.remove(&old_id) {
                                other.reported_direct_peers.insert(rec.peer_id);
                            }
                        }
                        sess.peers.insert(rec.peer_id, moved);
                    }
                } else {
                    info!("Discovered peer #{} ({})", rec.peer_id, rec.nickname);
                    sess.peers.insert(
                        rec.peer_id,
                        RemotePeerState {
                            record: rec,
                            pub_key: None,
                            status: PeerConnStatus::Punching,
                            active_addr: None,
                            mesh_relay_peer: None,
                            reported_direct_peers: HashSet::new(),
                            reachability_updated_at: None,
                            punch_started_at: now,
                            last_direct_probe: now,
                            last_rx: now,
                            rtt_ms: 0,
                            has_announced_once: false,
                            stream_info: None,
                            replay_control: squad_proto::ReplayWindow::new(),
                            replay_voice: squad_proto::ReplayWindow::new(),
                            replay_dtx: squad_proto::ReplayWindow::new(),
                            replay_stream_audio: squad_proto::ReplayWindow::new(),
                            replay_video: squad_proto::LargeReplayWindow::new(),
                        },
                    );
                    io.audio_engine.play_cue(SoundCue::PeerJoin);
                }
            }

            let in_migration_grace = sess.migrating_until.is_some_and(|until| now < until);
            let stale: Vec<PeerId> = if in_migration_grace {
                Vec::new()
            } else {
                sess.migrating_until = None;
                sess.peers
                    .keys()
                    .copied()
                    .filter(|id| !active_ids.contains(id))
                    .collect()
            };

            let mut sub_changed = false;
            let mut removed_any = false;
            for id in stale {
                removed_any = true;
                sess.peers.remove(&id);
                sess.transfer.remove_peer(id);
                for other in sess.peers.values_mut() {
                    if other.mesh_relay_peer == Some(id) {
                        other.mesh_relay_peer = None;
                    }
                    other.reported_direct_peers.remove(&id);
                }
                if sess.stream_subscribers.remove(&id) {
                    sub_changed = true;
                }
                if sess.watched_peer_id == Some(id) {
                    sess.watched_peer_id = None;
                    sess.sent_decoder_error_to = None;
                    sess.video_reassembler.reset();
                    io.decode_handle.reset();
                    io.audio_engine.clear_stream_audio();
                    clear_video_slot(io.video_frame_slot);
                    let _ = io
                        .evt_tx
                        .send(MeshEvent::RemoteStreamUpdated {
                            peer_id: id,
                            stream: None,
                        })
                        .await;
                }
                io.audio_engine.remove_peer(id);
                io.audio_engine.play_cue(SoundCue::PeerLeave);
            }
            if removed_any {
                let route_changes =
                    reconcile_mesh_routes(io.socket, sess, io.seal_buf, io.relay_buf, now).await;
                if !route_changes.is_empty() {
                    enforce_video_route_policy(
                        sess,
                        &route_changes,
                        io.capture_worker,
                        io.video_out_tx,
                        io.video_frame_slot,
                        io.video_notify,
                        io.decode_handle,
                        io.audio_engine,
                        io.evt_tx,
                    )
                    .await;
                }
                broadcast_mesh_reachability_if_needed(
                    io.socket,
                    sess,
                    io.seal_buf,
                    io.relay_buf,
                    false,
                )
                .await;
            }
            if sub_changed {
                reconcile_capture_worker(
                    sess,
                    io.capture_worker,
                    io.video_out_tx,
                    io.video_frame_slot,
                    io.video_notify,
                    false,
                );
                io.evt_tx
                    .send(MeshEvent::LocalStreamStatus {
                        is_sharing: sess.local_stream_config.is_some(),
                        subscriber_count: sess.stream_subscribers.len(),
                    })
                    .await;
            }
        }
        SignalingPacket::S2cError { code } => {
            warn!("Rendezvous signaling error: {code:?}");
            io.evt_tx
                .send(MeshEvent::Error(format!("Server error: {code:?}")))
                .await;
        }
        _ => {}
    }
}

struct MeshRxCtx<'a> {
    socket: &'a UdpSocket,
    audio_engine: &'a AudioEngineHandle,
    seal_buf: &'a mut Vec<u8>,
    relay_buf: &'a mut Vec<u8>,
    tr_tx: &'a mpsc::Sender<TransferEvent>,
    evt_tx: &'a NotifyingEventTx,
    capture_worker: &'a mut Option<CaptureWorkerHandle>,
    video_out_tx: &'a mpsc::Sender<Vec<VideoPacketOut>>,
    video_frame_slot: &'a Arc<Mutex<Option<DecodedVideoFrame>>>,
    video_notify: &'a Arc<Notify>,
    decode_handle: &'a VideoDecodeHandle,
    lan_tx_sock: Option<&'a UdpSocket>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RelaySource {
    Server,
    PeerMesh(PeerId),
}

async fn handle_peer_relay_datagram(
    socket: &UdpSocket,
    datagram: &mut [u8],
    src_addr: SocketAddr,
    sess: &mut ActiveSession,
    io: &mut MeshRxCtx<'_>,
) {
    let Some(relay_peer_id) = sess
        .peers
        .iter()
        .find(|(_, p)| p.status == PeerConnStatus::Connected && p.active_addr == Some(src_addr))
        .map(|(&pid, _)| pid)
    else {
        return;
    };

    let Ok((room_id, outer_peer_id, inner)) = parse_peer_relay_header(datagram) else {
        return;
    };
    if room_id != sess.invite.room_id {
        return;
    }
    let Ok(inner_hdr) = P2pHeader::decode(inner) else {
        return;
    };
    if inner_hdr.room_prefix != sess.room_prefix {
        return;
    }

    // Forwarding role (A -> B -> C, where local node is B).
    if outer_peer_id != sess.local_peer_id
        && outer_peer_id != relay_peer_id
        && inner_hdr.sender_peer_id == relay_peer_id
    {
        if sess.peers.len() < 2 {
            return;
        }
        let Some(target_addr) = sess
            .peers
            .get(&outer_peer_id)
            .filter(|p| p.status == PeerConnStatus::Connected)
            .and_then(|p| p.active_addr)
        else {
            return;
        };
        rewrite_relay_peer_id(datagram, relay_peer_id);
        let _ = socket.send_to(datagram, target_addr).await;
        return;
    }

    // Receiving role (B -> C, where local node is C and outer_peer_id is A).
    if outer_peer_id == inner_hdr.sender_peer_id
        && outer_peer_id != relay_peer_id
        && outer_peer_id != sess.local_peer_id
    {
        handle_p2p_rx(
            socket,
            &mut datagram[RELAY_HEADER_LEN..],
            src_addr,
            Some(RelaySource::PeerMesh(relay_peer_id)),
            Some(outer_peer_id),
            sess,
            io,
        )
        .await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_p2p_rx(
    socket: &UdpSocket,
    datagram: &mut [u8],
    src_addr: SocketAddr,
    relay_source: Option<RelaySource>,
    expected_sender_id: Option<PeerId>,
    sess: &mut ActiveSession,
    io: &mut MeshRxCtx<'_>,
) {
    let Ok((header, payload)) = sess.cipher.open_packet(datagram) else {
        debug!("Dropped unauthenticated P2P packet from {src_addr}");
        return;
    };

    if header.room_prefix != sess.room_prefix {
        return;
    }

    let via_relay = relay_source.is_some();
    if via_relay && expected_sender_id != Some(header.sender_peer_id) {
        debug!(
            "Dropped relay packet with mismatched sender id: header={}, relay={:?}",
            header.sender_peer_id, expected_sender_id
        );
        return;
    }

    if matches!(relay_source, Some(RelaySource::Server))
        && (!header.msg_type.is_server_relay_allowed()
            || matches!(header.msg_type, P2pMsgType::StreamAnnounce))
    {
        return;
    }

    let now = Instant::now();
    let mut reachability_dirty = false;
    let (sender_uuid, sender_nickname, first_connect) = {
        let Some(peer) = sess.peers.get_mut(&header.sender_peer_id) else {
            return;
        };

        if matches!(header.msg_type, P2pMsgType::PunchSyn)
            && now.saturating_duration_since(peer.last_rx) > Duration::from_secs(2)
        {
            peer.replay_control.reset();
            peer.replay_voice.reset();
            peer.replay_dtx.reset();
            peer.replay_stream_audio.reset();
            peer.replay_video.reset();
        }

        let replay_ok = match header.msg_type {
            P2pMsgType::AudioOpus => peer.replay_voice.check_and_accept(header.seq_num),
            P2pMsgType::AudioDtxKeepalive => peer.replay_dtx.check_and_accept(header.seq_num),
            P2pMsgType::StreamAudioOpus => {
                peer.replay_stream_audio.check_and_accept(header.seq_num)
            }
            P2pMsgType::VideoFrameFrag => peer.replay_video.check_and_accept(header.seq_num),
            _ => peer.replay_control.check_and_accept(header.seq_num),
        };
        if !replay_ok {
            return;
        }

        peer.last_rx = now;
        match relay_source {
            None => {
                if peer.status != PeerConnStatus::Connected {
                    info!(
                        "Direct P2P connection active with peer #{} ({}) at {}",
                        peer.record.peer_id, peer.record.nickname, src_addr
                    );
                    reachability_dirty = true;
                }
                peer.status = PeerConnStatus::Connected;
                peer.active_addr = Some(src_addr);
                peer.mesh_relay_peer = None;
            }
            Some(RelaySource::PeerMesh(b_pid)) => {
                if peer.status != PeerConnStatus::Connected {
                    if peer.status != PeerConnStatus::MeshRelayed
                        || peer.mesh_relay_peer != Some(b_pid)
                    {
                        info!(
                            "Peer-Mesh relay path active with peer #{} ({}) via teammate #{}",
                            peer.record.peer_id, peer.record.nickname, b_pid
                        );
                    }
                    peer.status = PeerConnStatus::MeshRelayed;
                    peer.mesh_relay_peer = Some(b_pid);
                }
            }
            Some(RelaySource::Server) => {
                if !matches!(
                    peer.status,
                    PeerConnStatus::Connected | PeerConnStatus::MeshRelayed
                ) {
                    if peer.status != PeerConnStatus::Relayed {
                        info!(
                            "Server UDP relay path active with peer #{} ({}) via {} (voice/chat only)",
                            peer.record.peer_id, peer.record.nickname, sess.rendezvous_addr
                        );
                    }
                    peer.status = PeerConnStatus::Relayed;
                    peer.mesh_relay_peer = None;
                }
            }
        }

        let first_connect = !peer.has_announced_once
            && matches!(
                peer.status,
                PeerConnStatus::Connected | PeerConnStatus::MeshRelayed | PeerConnStatus::Relayed
            );
        if first_connect {
            peer.has_announced_once = true;
        }
        (
            peer.record.client_uuid,
            peer.record.nickname.clone(),
            first_connect,
        )
    };

    if reachability_dirty {
        broadcast_mesh_reachability_if_needed(socket, sess, io.seal_buf, io.relay_buf, false).await;
    }

    if first_connect {
        if let Some(endpoint) = peer_endpoint(sess, header.sender_peer_id) {
            let mut ctx = TransferCtx {
                local_peer_id: sess.local_peer_id,
                local_uuid: sess.local_uuid,
                local_nickname: &sess.nickname,
                socket,
                cipher: &sess.cipher,
                room_prefix: sess.room_prefix,
                control_seq: &mut sess.control_seq,
                seal_buf: io.seal_buf,
                evt_tx: io.tr_tx,
            };
            sess.transfer
                .announce_haves_to_peer(endpoint, &mut ctx)
                .await;
            sess.transfer
                .announce_avatar_to_peer(endpoint, &mut ctx)
                .await;
            sess.transfer
                .send_history_sync_req(endpoint, &mut ctx)
                .await;
            if let Some(cfg) = sess.local_stream_config.clone() {
                send_stream_announce_to_endpoint(
                    socket,
                    endpoint,
                    sess,
                    io.seal_buf,
                    io.relay_buf,
                    true,
                    Some(&cfg),
                )
                .await;
            }
            send_identity_announce_to_peer(
                socket,
                sess,
                header.sender_peer_id,
                io.seal_buf,
                io.relay_buf,
            )
            .await;
        }
    }

    match payload {
        P2pPayload::PunchSyn { timestamp_us, .. } => {
            if !via_relay {
                if let Some(peer) = sess.peers.get_mut(&header.sender_peer_id) {
                    peer.active_addr = Some(src_addr);
                }
            }
            sess.control_seq = sess.control_seq.wrapping_add(1);
            let ack_hdr = P2pHeader {
                msg_type: P2pMsgType::PunchAck,
                room_prefix: sess.room_prefix,
                sender_peer_id: sess.local_peer_id,
                seq_num: sess.control_seq,
            };
            let ack_body = P2pPayload::PunchAck {
                client_uuid: sess.local_uuid,
                echo_timestamp_us: timestamp_us,
            };
            if sess
                .cipher
                .seal_packet(&ack_hdr, &ack_body, io.seal_buf)
                .is_ok()
            {
                if !via_relay {
                    let _ = socket.send_to(io.seal_buf, src_addr).await;
                } else {
                    send_sealed_to_peer(
                        socket,
                        sess,
                        header.sender_peer_id,
                        P2pMsgType::PunchAck,
                        io.seal_buf,
                        io.relay_buf,
                    )
                    .await;
                }
            }
        }
        P2pPayload::PunchAck {
            echo_timestamp_us, ..
        } => {
            if let Some(peer) = sess.peers.get_mut(&header.sender_peer_id) {
                if !via_relay {
                    peer.active_addr = Some(src_addr);
                }
                update_rtt(&mut peer.rtt_ms, echo_timestamp_us);
            }
        }
        P2pPayload::Ping { timestamp_us } => {
            sess.control_seq = sess.control_seq.wrapping_add(1);
            let pong_hdr = P2pHeader {
                msg_type: P2pMsgType::Pong,
                room_prefix: sess.room_prefix,
                sender_peer_id: sess.local_peer_id,
                seq_num: sess.control_seq,
            };
            let pong_body = P2pPayload::Pong {
                echo_timestamp_us: timestamp_us,
            };
            if sess
                .cipher
                .seal_packet(&pong_hdr, &pong_body, io.seal_buf)
                .is_ok()
            {
                send_sealed_to_peer(
                    socket,
                    sess,
                    header.sender_peer_id,
                    P2pMsgType::Pong,
                    io.seal_buf,
                    io.relay_buf,
                )
                .await;
            }
        }
        P2pPayload::Pong { echo_timestamp_us } => {
            if let Some(peer) = sess.peers.get_mut(&header.sender_peer_id) {
                update_rtt(&mut peer.rtt_ms, echo_timestamp_us);
            }
        }
        P2pPayload::LanHostMigrate { new_host_peer_id } => {
            if sess.is_lan_room
                && sess.embedded_rv.is_none()
                && sess.local_peer_id != 0
                && new_host_peer_id == sess.local_peer_id
                && sess
                    .migration_claimed_at
                    .is_none_or(|t| t.elapsed() > Duration::from_secs(2))
            {
                promote_to_lan_host(socket, io.lan_tx_sock, sess, io.seal_buf, io.relay_buf).await;
            }
        }
        P2pPayload::LanHostClaim { rv_port, room_name } => {
            if sess.is_lan_room && sess.embedded_rv.is_none() && rv_port != 0 {
                let candidate_ip = if !via_relay {
                    Some(src_addr.ip())
                } else {
                    sess.peers
                        .get(&header.sender_peer_id)
                        .and_then(|p| p.active_addr.map(|a| a.ip()))
                };
                if let Some(host_ip) = candidate_ip {
                    let next_rv = SocketAddr::new(host_ip, rv_port);
                    sess.rendezvous_addr = next_rv;
                    if !room_name.is_empty() {
                        sess.lan_room_name = room_name;
                    }
                    sess.last_rv_rx = Instant::now();
                    sess.migrating_until = Some(Instant::now() + LAN_MIGRATION_GRACE);
                    info!(
                        "LAN room coordinator migrated to peer #{} ({}) at {}",
                        header.sender_peer_id, sender_nickname, next_rv
                    );
                    send_register(socket, sess).await;
                }
            }
        }
        P2pPayload::MeshReachability { direct_peers } => {
            if let Some(peer) = sess.peers.get_mut(&header.sender_peer_id) {
                peer.reported_direct_peers = direct_peers
                    .into_iter()
                    .filter(|&pid| pid != header.sender_peer_id && pid != sess.local_peer_id)
                    .collect();
                peer.reachability_updated_at = Some(now);
            }
            let route_changes =
                reconcile_mesh_routes(socket, sess, io.seal_buf, io.relay_buf, now).await;
            if !route_changes.is_empty() {
                enforce_video_route_policy(
                    sess,
                    &route_changes,
                    io.capture_worker,
                    io.video_out_tx,
                    io.video_frame_slot,
                    io.video_notify,
                    io.decode_handle,
                    io.audio_engine,
                    io.evt_tx,
                )
                .await;
            }
        }
        P2pPayload::IdentityAnnounce { pub_key } => {
            if pub_key != [0u8; 32] {
                if let Some(peer) = sess.peers.get_mut(&header.sender_peer_id) {
                    peer.pub_key = Some(pub_key);
                }
            }
        }
        P2pPayload::AudioOpus { opus_data } => {
            io.audio_engine
                .push_remote_opus(header.sender_peer_id, header.seq_num, &opus_data);
        }
        P2pPayload::AudioDtxKeepalive => {}
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
            let parsed_codec = VideoCodec::from_u8(codec);
            let stream_opt = if is_streaming {
                Some(RemoteStreamInfo {
                    codec: parsed_codec,
                    has_audio,
                    width,
                    height,
                    fps,
                    source_title,
                    encoder_backend,
                })
            } else {
                None
            };
            if let Some(peer) = sess.peers.get_mut(&header.sender_peer_id) {
                peer.stream_info = stream_opt.clone();
            }
            if is_streaming
                && sess.watched_peer_id == Some(header.sender_peer_id)
                && parsed_codec == VideoCodec::H265
                && !sess.hevc_decode_supported
                && sess.sent_decoder_error_to != Some(header.sender_peer_id)
            {
                sess.sent_decoder_error_to = Some(header.sender_peer_id);
                send_stream_decoder_error(
                    socket,
                    sess,
                    io.seal_buf,
                    io.relay_buf,
                    header.sender_peer_id,
                    VideoCodec::H265.as_u8(),
                    "HEVC decoder unavailable on viewer",
                )
                .await;
            }
            if !is_streaming && sess.watched_peer_id == Some(header.sender_peer_id) {
                sess.watched_peer_id = None;
                sess.sent_decoder_error_to = None;
                sess.video_reassembler.reset();
                io.decode_handle.reset();
                io.audio_engine.clear_stream_audio();
                clear_video_slot(io.video_frame_slot);
            }
            let _ = io
                .evt_tx
                .send(MeshEvent::RemoteStreamUpdated {
                    peer_id: header.sender_peer_id,
                    stream: stream_opt,
                })
                .await;
        }
        P2pPayload::StreamSubscribe { subscribe } => {
            if sess.local_stream_config.is_some() {
                let mut force_idr = false;
                if subscribe {
                    sess.stream_subscribers.insert(header.sender_peer_id);
                    force_idr = true;
                } else {
                    sess.stream_subscribers.remove(&header.sender_peer_id);
                }
                reconcile_capture_worker(
                    sess,
                    io.capture_worker,
                    io.video_out_tx,
                    io.video_frame_slot,
                    io.video_notify,
                    force_idr,
                );
                let _ = io
                    .evt_tx
                    .send(MeshEvent::LocalStreamStatus {
                        is_sharing: true,
                        subscriber_count: sess.stream_subscribers.len(),
                    })
                    .await;
            }
        }
        P2pPayload::StreamPli => {
            if let Some(worker) = io.capture_worker.as_ref() {
                worker.force_idr.store(true, Ordering::Release);
            }
        }
        P2pPayload::VideoFrameFrag {
            frame_id,
            frag_idx,
            frag_count,
            flags,
            data,
        } => {
            if sess.watched_peer_id == Some(header.sender_peer_id) {
                let (frame_opt, need_pli) = sess
                    .video_reassembler
                    .push_fragment(frame_id, frag_idx, frag_count, flags, data);
                if need_pli && now.duration_since(sess.last_pli_sent) > Duration::from_millis(120) {
                    send_stream_pli(
                        socket,
                        sess,
                        io.seal_buf,
                        io.relay_buf,
                        header.sender_peer_id,
                    )
                    .await;
                    sess.last_pli_sent = now;
                }
                if let Some(reassembled) = frame_opt {
                    let generation = io.decode_handle.generation.load(Ordering::Acquire);
                    let job = DecodeJob::Frame {
                        streamer_peer_id: header.sender_peer_id,
                        generation,
                        annex_b_bytes: reassembled.annex_b_bytes,
                    };
                    match io.decode_handle.tx.try_send(job) {
                        Ok(()) => {}
                        Err(std::sync::mpsc::TrySendError::Full(_)) => {
                            sess.video_reassembler.reset();
                            if now.duration_since(sess.last_pli_sent) > Duration::from_millis(120) {
                                send_stream_pli(
                                    socket,
                                    sess,
                                    io.seal_buf,
                                    io.relay_buf,
                                    header.sender_peer_id,
                                )
                                .await;
                                sess.last_pli_sent = now;
                            }
                        }
                        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {}
                    }
                }
            }
        }
        P2pPayload::StreamDecoderError {
            failed_codec,
            reason,
        } => {
            if failed_codec == VideoCodec::H265.as_u8() {
                if let Some(cfg) = sess.local_stream_config.as_mut() {
                    if cfg.codec == VideoCodec::H265 {
                        cfg.codec = VideoCodec::H264;
                        let cfg_clone = cfg.clone();
                        stop_capture_worker(io.capture_worker);
                        reconcile_capture_worker(
                            sess,
                            io.capture_worker,
                            io.video_out_tx,
                            io.video_frame_slot,
                            io.video_notify,
                            true,
                        );
                        broadcast_stream_announce(
                            socket,
                            sess,
                            io.seal_buf,
                            io.relay_buf,
                            true,
                            Some(&cfg_clone),
                        )
                        .await;
                        let _ = io
                            .evt_tx
                            .send(MeshEvent::StreamCodecFallbackToH264 { reason })
                            .await;
                    }
                }
            }
        }
        P2pPayload::StreamAudioOpus { opus_data } => {
            if sess.watched_peer_id == Some(header.sender_peer_id) {
                io.audio_engine.push_remote_stream_opus(
                    header.sender_peer_id,
                    header.seq_num,
                    &opus_data,
                );
            }
        }
        other => {
            let peers = connected_peer_list(sess);
            let sender_endpoint = peer_endpoint(sess, header.sender_peer_id)
                .unwrap_or(PeerEndpoint::Direct(src_addr));
            let mut ctx = TransferCtx {
                local_peer_id: sess.local_peer_id,
                local_uuid: sess.local_uuid,
                local_nickname: &sess.nickname,
                socket,
                cipher: &sess.cipher,
                room_prefix: sess.room_prefix,
                control_seq: &mut sess.control_seq,
                seal_buf: io.seal_buf,
                evt_tx: io.tr_tx,
            };
            sess.transfer
                .handle_payload(
                    header.sender_peer_id,
                    sender_uuid,
                    &sender_nickname,
                    sender_endpoint,
                    other,
                    &peers,
                    &mut ctx,
                )
                .await;
        }
    }
}

fn is_mesh_relay_candidate_valid(
    sess: &ActiveSession,
    relay_pid: PeerId,
    target_pid: PeerId,
    now: Instant,
) -> bool {
    if relay_pid == target_pid || relay_pid == sess.local_peer_id {
        return false;
    }
    sess.peers.get(&relay_pid).is_some_and(|relay_peer| {
        relay_peer.status == PeerConnStatus::Connected
            && relay_peer.active_addr.is_some()
            && relay_peer
                .reachability_updated_at
                .is_some_and(|t| now.duration_since(t) <= MESH_REACHABILITY_TTL)
            && relay_peer.reported_direct_peers.contains(&target_pid)
    })
}

fn find_best_mesh_relay(
    sess: &ActiveSession,
    target_pid: PeerId,
    current_relay: Option<PeerId>,
    now: Instant,
) -> Option<PeerId> {
    if sess.peers.len() < 2 {
        return None;
    }
    let current_score = current_relay
        .filter(|&pid| is_mesh_relay_candidate_valid(sess, pid, target_pid, now))
        .and_then(|pid| sess.peers.get(&pid).map(|p| (pid, effective_rtt(p.rtt_ms))));

    let best = sess
        .peers
        .iter()
        .filter(|(&pid, _)| is_mesh_relay_candidate_valid(sess, pid, target_pid, now))
        .map(|(&pid, p)| (pid, effective_rtt(p.rtt_ms)))
        .min_by_key(|&(pid, rtt)| (rtt, pid));

    match (current_score, best) {
        (Some((cur_pid, cur_rtt)), Some((best_pid, best_rtt))) => {
            if best_pid != cur_pid && best_rtt + 20 < cur_rtt {
                Some(best_pid)
            } else {
                Some(cur_pid)
            }
        }
        (Some((cur_pid, _)), None) => Some(cur_pid),
        (None, Some((best_pid, _))) => Some(best_pid),
        (None, None) => None,
    }
}

#[inline]
fn effective_rtt(rtt_ms: u32) -> u32 {
    if rtt_ms == 0 {
        500
    } else {
        rtt_ms
    }
}

async fn broadcast_mesh_reachability_if_needed(
    socket: &UdpSocket,
    sess: &mut ActiveSession,
    seal_buf: &mut Vec<u8>,
    relay_buf: &mut Vec<u8>,
    periodic: bool,
) {
    if sess.local_peer_id == 0 {
        return;
    }
    let mut direct_peers: Vec<PeerId> = sess
        .peers
        .iter()
        .filter(|(_, p)| p.status == PeerConnStatus::Connected && p.active_addr.is_some())
        .map(|(&pid, _)| pid)
        .collect();
    direct_peers.sort_unstable();

    let changed = direct_peers != sess.last_advertised_direct_peers;
    let periodic_due =
        periodic && sess.last_mesh_reachability_sent.elapsed() >= MESH_REACHABILITY_INTERVAL;
    if !changed && !periodic_due {
        return;
    }
    if direct_peers.is_empty() && !changed {
        return;
    }

    sess.last_advertised_direct_peers = direct_peers.clone();
    sess.last_mesh_reachability_sent = Instant::now();

    let targets: Vec<PeerId> = direct_peers.clone();
    if targets.is_empty() {
        return;
    }

    sess.control_seq = sess.control_seq.wrapping_add(1);
    let hdr = P2pHeader {
        msg_type: P2pMsgType::MeshReachability,
        room_prefix: sess.room_prefix,
        sender_peer_id: sess.local_peer_id,
        seq_num: sess.control_seq,
    };
    let payload = P2pPayload::MeshReachability { direct_peers };
    if sess.cipher.seal_packet(&hdr, &payload, seal_buf).is_ok() {
        for pid in targets {
            send_sealed_to_peer(
                socket,
                sess,
                pid,
                P2pMsgType::MeshReachability,
                seal_buf,
                relay_buf,
            )
            .await;
        }
    }
}

async fn reconcile_mesh_routes(
    socket: &UdpSocket,
    sess: &mut ActiveSession,
    seal_buf: &mut Vec<u8>,
    relay_buf: &mut Vec<u8>,
    now: Instant,
) -> Vec<PeerId> {
    let mut degraded = Vec::new();
    let mut promoted = Vec::new();
    let peer_ids: Vec<PeerId> = sess.peers.keys().copied().collect();

    for pid in peer_ids {
        let Some((status, current_relay)) =
            sess.peers.get(&pid).map(|p| (p.status, p.mesh_relay_peer))
        else {
            continue;
        };

        match status {
            PeerConnStatus::MeshRelayed => {
                let next_relay = find_best_mesh_relay(sess, pid, current_relay, now);
                if let Some(peer) = sess.peers.get_mut(&pid) {
                    if let Some(b_pid) = next_relay {
                        if peer.mesh_relay_peer != Some(b_pid) {
                            info!(
                                "Switched Peer-Mesh relay for peer #{} ({}) to teammate #{}",
                                peer.record.peer_id, peer.record.nickname, b_pid
                            );
                            peer.mesh_relay_peer = Some(b_pid);
                            promoted.push(pid);
                        }
                    } else {
                        info!(
                            "Peer-Mesh relay lost for peer #{} ({}); falling back to server relay (voice/chat only)",
                            peer.record.peer_id, peer.record.nickname
                        );
                        peer.status = PeerConnStatus::Relayed;
                        peer.mesh_relay_peer = None;
                        peer.last_rx = now;
                        degraded.push(pid);
                    }
                }
            }
            PeerConnStatus::Relayed | PeerConnStatus::NatFailed => {
                if let Some(b_pid) = find_best_mesh_relay(sess, pid, None, now) {
                    if let Some(peer) = sess.peers.get_mut(&pid) {
                        info!(
                            "Upgraded peer #{} ({}) to Peer-Mesh relay via teammate #{}",
                            peer.record.peer_id, peer.record.nickname, b_pid
                        );
                        peer.status = PeerConnStatus::MeshRelayed;
                        peer.mesh_relay_peer = Some(b_pid);
                        peer.last_rx = now;
                        promoted.push(pid);
                    }
                }
            }
            PeerConnStatus::Punching | PeerConnStatus::Connected => {}
        }
    }

    let now_us = now_micros();
    for pid in promoted {
        if seal_punch_syn_parts(
            &sess.cipher,
            sess.room_prefix,
            sess.local_peer_id,
            sess.local_uuid,
            &mut sess.control_seq,
            now_us,
            seal_buf,
        ) {
            send_sealed_to_peer(socket, sess, pid, P2pMsgType::PunchSyn, seal_buf, relay_buf).await;
        }
    }

    degraded
}

#[allow(clippy::too_many_arguments)]
async fn enforce_video_route_policy(
    sess: &mut ActiveSession,
    affected_peers: &[PeerId],
    capture_worker: &mut Option<CaptureWorkerHandle>,
    video_out_tx: &mpsc::Sender<Vec<VideoPacketOut>>,
    video_frame_slot: &Arc<Mutex<Option<DecodedVideoFrame>>>,
    video_notify: &Arc<Notify>,
    decode_handle: &VideoDecodeHandle,
    audio_engine: &AudioEngineHandle,
    evt_tx: &NotifyingEventTx,
) {
    let mut sub_changed = false;
    let mut stopped_watch = false;

    for &pid in affected_peers {
        let allows_video = sess.peers.get(&pid).is_some_and(|p| {
            matches!(
                p.status,
                PeerConnStatus::Connected | PeerConnStatus::MeshRelayed
            )
        });
        if !allows_video {
            if sess.stream_subscribers.remove(&pid) {
                sub_changed = true;
            }
            if sess.watched_peer_id == Some(pid) {
                sess.watched_peer_id = None;
                sess.sent_decoder_error_to = None;
                sess.video_reassembler.reset();
                decode_handle.reset();
                audio_engine.clear_stream_audio();
                clear_video_slot(video_frame_slot);
                stopped_watch = true;
            }
        }
    }

    if sub_changed {
        reconcile_capture_worker(
            sess,
            capture_worker,
            video_out_tx,
            video_frame_slot,
            video_notify,
            false,
        );
        let _ = evt_tx
            .send(MeshEvent::LocalStreamStatus {
                is_sharing: sess.local_stream_config.is_some(),
                subscriber_count: sess.stream_subscribers.len(),
            })
            .await;
    }

    if stopped_watch {
        let _ = evt_tx
            .send(MeshEvent::Error(
                "Screen share stopped: peer switched to server relay (video requires Direct P2P or Peer-Mesh Relay)".to_string(),
            ))
            .await;
    }
}

fn peer_endpoint(sess: &ActiveSession, peer_id: PeerId) -> Option<PeerEndpoint> {
    let peer = sess.peers.get(&peer_id)?;
    match peer.status {
        PeerConnStatus::Connected => peer.active_addr.map(PeerEndpoint::Direct),
        PeerConnStatus::MeshRelayed => {
            let relay_pid = peer.mesh_relay_peer?;
            let relay_addr = sess
                .peers
                .get(&relay_pid)
                .filter(|r| r.status == PeerConnStatus::Connected)
                .and_then(|r| r.active_addr)?;
            Some(PeerEndpoint::PeerRelay {
                relay_addr,
                room_id: sess.invite.room_id,
                target_peer_id: peer_id,
            })
        }
        PeerConnStatus::Relayed => Some(PeerEndpoint::Relay {
            server: sess.rendezvous_addr,
            room_id: sess.invite.room_id,
            peer_id,
        }),
        PeerConnStatus::Punching | PeerConnStatus::NatFailed => None,
    }
}

async fn send_identity_announce_to_peer(
    socket: &UdpSocket,
    sess: &mut ActiveSession,
    peer_id: PeerId,
    seal_buf: &mut Vec<u8>,
    relay_buf: &mut Vec<u8>,
) {
    if sess.local_peer_id == 0 || sess.local_pub_key == [0u8; 32] {
        return;
    }
    sess.control_seq = sess.control_seq.wrapping_add(1);
    let hdr = P2pHeader {
        msg_type: P2pMsgType::IdentityAnnounce,
        room_prefix: sess.room_prefix,
        sender_peer_id: sess.local_peer_id,
        seq_num: sess.control_seq,
    };
    let body = P2pPayload::IdentityAnnounce {
        pub_key: sess.local_pub_key,
    };
    if sess.cipher.seal_packet(&hdr, &body, seal_buf).is_ok() {
        send_sealed_to_peer(
            socket,
            sess,
            peer_id,
            P2pMsgType::IdentityAnnounce,
            seal_buf,
            relay_buf,
        )
        .await;
    }
}

async fn send_sealed_to_peer(
    socket: &UdpSocket,
    sess: &ActiveSession,
    peer_id: PeerId,
    msg_type: P2pMsgType,
    sealed_bytes: &[u8],
    relay_buf: &mut Vec<u8>,
) {
    let Some(endpoint) = peer_endpoint(sess, peer_id) else {
        return;
    };
    send_sealed_to_endpoint(socket, endpoint, msg_type, sealed_bytes, relay_buf).await;
}

async fn send_sealed_to_endpoint(
    socket: &UdpSocket,
    endpoint: PeerEndpoint,
    msg_type: P2pMsgType,
    sealed_bytes: &[u8],
    relay_buf: &mut Vec<u8>,
) {
    match endpoint {
        PeerEndpoint::Direct(addr) => {
            let _ = socket.send_to(sealed_bytes, addr).await;
        }
        PeerEndpoint::PeerRelay {
            relay_addr,
            room_id,
            target_peer_id,
        } => {
            if encode_peer_relay_datagram_into(&room_id, target_peer_id, sealed_bytes, relay_buf)
                .is_ok()
            {
                let _ = socket.send_to(relay_buf, relay_addr).await;
            }
        }
        PeerEndpoint::Relay {
            server,
            room_id,
            peer_id,
        } => {
            if !msg_type.is_server_relay_allowed() {
                return;
            }
            if encode_relay_datagram_into(&room_id, peer_id, sealed_bytes, relay_buf).is_ok() {
                let _ = socket.send_to(relay_buf, server).await;
            }
        }
    }
}

fn connected_peer_list(sess: &ActiveSession) -> Vec<(PeerId, PeerEndpoint)> {
    sess.peers
        .keys()
        .copied()
        .filter_map(|pid| peer_endpoint(sess, pid).map(|ep| (pid, ep)))
        .collect()
}

fn connected_peer_map(sess: &ActiveSession) -> HashMap<PeerId, PeerEndpoint> {
    sess.peers
        .keys()
        .copied()
        .filter_map(|pid| peer_endpoint(sess, pid).map(|ep| (pid, ep)))
        .collect()
}

async fn forward_transfer_events(
    tr_rx: &mut mpsc::Receiver<TransferEvent>,
    evt_tx: &NotifyingEventTx,
) {
    while let Ok(ev) = tr_rx.try_recv() {
        let mapped = match ev {
            TransferEvent::ChatItemReceived(item) => MeshEvent::ChatItemReceived(item),
            TransferEvent::BlobStateChanged {
                blob_id,
                state,
                inline_bytes,
            } => MeshEvent::BlobStateChanged {
                blob_id,
                state,
                inline_bytes,
            },
            TransferEvent::PeerAvatarLoaded {
                client_uuid,
                avatar_hash,
                jpeg_bytes,
            } => MeshEvent::PeerAvatarLoaded {
                client_uuid,
                avatar_hash,
                jpeg_bytes,
            },
            TransferEvent::Error(e) => MeshEvent::Error(e),
        };
        evt_tx.send(mapped).await;
    }
}

#[inline]
fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

#[inline]
fn update_rtt(rtt_slot: &mut u32, echo_timestamp_us: u64) {
    let now_us = now_micros();
    if now_us >= echo_timestamp_us {
        let sample_ms = ((now_us - echo_timestamp_us) / 1000) as u32;
        if *rtt_slot == 0 {
            *rtt_slot = sample_ms.max(1);
        } else {
            *rtt_slot = ((*rtt_slot as f32 * 0.75) + (sample_ms as f32 * 0.25)) as u32;
        }
    }
}

fn resolve_host(host_port: &str) -> Result<SocketAddr, String> {
    let trimmed = host_port.trim();
    let target = if trimmed.is_empty() {
        crate::config::DEFAULT_RENDEZVOUS_SERVER
    } else {
        trimmed
    };
    let has_port = if target.starts_with('[') {
        target.contains("]:")
    } else {
        target.contains(':')
    };
    let resolved_str: std::borrow::Cow<'_, str> = if has_port {
        std::borrow::Cow::Borrowed(target)
    } else {
        std::borrow::Cow::Owned(format!("{target}:3478"))
    };

    let addrs = resolved_str
        .to_socket_addrs()
        .map_err(|e| format!("Failed to resolve '{resolved_str}': {e}"))?;
    let mut first_v6 = None;
    for addr in addrs {
        if addr.is_ipv4() {
            return Ok(addr);
        }
        if first_v6.is_none() {
            first_v6 = Some(addr);
        }
    }
    first_v6.ok_or_else(|| format!("No IP addresses found for '{resolved_str}'"))
}

fn stop_capture_worker(handle: &mut Option<CaptureWorkerHandle>) {
    if let Some(worker) = handle.take() {
        worker.stop.store(true, Ordering::Release);
        let (lock, cvar) = &*worker.encode_wake;
        if let Ok(mut guard) = lock.lock() {
            *guard = None;
        }
        cvar.notify_all();
    }
}

fn drain_video_out_rx(rx: &mut mpsc::Receiver<Vec<VideoPacketOut>>) {
    while rx.try_recv().is_ok() {}
}

fn clear_video_slot(slot: &Mutex<Option<DecodedVideoFrame>>) {
    *slot.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

async fn flush_video_tx_burst(
    socket: &UdpSocket,
    sess: &mut ActiveSession,
    seal_buf: &mut Vec<u8>,
    relay_buf: &mut Vec<u8>,
) {
    if sess.local_peer_id == 0
        || sess.stream_subscribers.is_empty()
        || sess.video_tx_queue.is_empty()
    {
        sess.video_tx_queue.clear();
        return;
    }

    let subs: Vec<PeerId> = sess
        .stream_subscribers
        .iter()
        .copied()
        .filter(|&pid| {
            sess.peers.get(&pid).is_some_and(|p| {
                matches!(
                    p.status,
                    PeerConnStatus::Connected | PeerConnStatus::MeshRelayed
                )
            })
        })
        .collect();
    if subs.is_empty() {
        sess.video_tx_queue.clear();
        return;
    }

    let start = sess.video_tx_queue.cursor;
    let end = (start + VIDEO_TX_BURST_PACKETS).min(sess.video_tx_queue.frags.len());
    sess.video_tx_queue.cursor = end;

    for idx in start..end {
        let frag = std::mem::replace(
            &mut sess.video_tx_queue.frags[idx],
            VideoPacketOut {
                frame_id: 0,
                frag_idx: 0,
                frag_count: 0,
                flags: 0,
                data: Vec::new(),
            },
        );
        sess.video_seq = sess.video_seq.wrapping_add(1);
        let hdr = P2pHeader {
            msg_type: P2pMsgType::VideoFrameFrag,
            room_prefix: sess.room_prefix,
            sender_peer_id: sess.local_peer_id,
            seq_num: sess.video_seq,
        };
        let payload = P2pPayload::VideoFrameFrag {
            frame_id: frag.frame_id,
            frag_idx: frag.frag_idx,
            frag_count: frag.frag_count,
            flags: frag.flags,
            data: frag.data,
        };
        if sess.cipher.seal_packet(&hdr, &payload, seal_buf).is_ok() {
            for &sub_id in &subs {
                send_sealed_to_peer(
                    socket,
                    sess,
                    sub_id,
                    P2pMsgType::VideoFrameFrag,
                    seal_buf,
                    relay_buf,
                )
                .await;
            }
        }
    }

    if sess.video_tx_queue.is_empty() {
        sess.video_tx_queue.clear();
    }
}

fn reconcile_capture_worker(
    sess: &ActiveSession,
    capture_handle: &mut Option<CaptureWorkerHandle>,
    video_out_tx: &mpsc::Sender<Vec<VideoPacketOut>>,
    video_frame_slot: &Arc<Mutex<Option<DecodedVideoFrame>>>,
    video_notify: &Arc<Notify>,
    force_idr: bool,
) {
    let Some(cfg) = sess.local_stream_config.as_ref() else {
        stop_capture_worker(capture_handle);
        return;
    };
    let has_subs = !sess.stream_subscribers.is_empty();
    let preview_on = sess.local_preview_enabled;
    if !has_subs && !preview_on {
        stop_capture_worker(capture_handle);
        return;
    }

    if let Some(existing) = capture_handle.as_ref() {
        existing.preview_active.store(preview_on, Ordering::Release);
        let prev_subs = existing.subs_active.swap(has_subs, Ordering::AcqRel);
        if force_idr || (!prev_subs && has_subs) {
            existing.force_idr.store(true, Ordering::Release);
        }
        return;
    }

    let stop = Arc::new(AtomicBool::new(false));
    let force_idr_flag = Arc::new(AtomicBool::new(true));
    let preview_flag = Arc::new(AtomicBool::new(preview_on));
    let subs_flag = Arc::new(AtomicBool::new(has_subs));
    let encode_wake: Arc<VideoEncodeWake> = Arc::new((Mutex::new(None), Condvar::new()));

    let stop_cap = Arc::clone(&stop);
    let preview_cap = Arc::clone(&preview_flag);
    let subs_cap = Arc::clone(&subs_flag);
    let wake_cap = Arc::clone(&encode_wake);
    let cap_cfg = cfg.clone();
    let slot_clone = Arc::clone(video_frame_slot);
    let notify_clone = Arc::clone(video_notify);
    let local_pid = sess.local_peer_id;

    let stop_enc = Arc::clone(&stop);
    let force_enc = Arc::clone(&force_idr_flag);
    let subs_enc = Arc::clone(&subs_flag);
    let wake_enc = Arc::clone(&encode_wake);
    let frame_counter = Arc::clone(&sess.next_video_frame_id);
    let enc_cfg = cfg.clone();
    let tx = video_out_tx.clone();

    std::thread::Builder::new()
        .name("squad-video-encode".into())
        .spawn(move || {
            let mut encoder = H264Encoder::new_with_codec(
                enc_cfg.fps.as_u8(),
                enc_cfg.bitrate_bps,
                enc_cfg.codec,
            );
            while !stop_enc.load(Ordering::Acquire) {
                let raw = {
                    let (lock, cvar) = &*wake_enc;
                    let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                    while !stop_enc.load(Ordering::Acquire) && guard.is_none() {
                        guard = cvar.wait(guard).unwrap_or_else(|e| e.into_inner());
                    }
                    if stop_enc.load(Ordering::Acquire) {
                        break;
                    }
                    match guard.take() {
                        Some(frame) => frame,
                        None => continue,
                    }
                };

                if subs_enc.load(Ordering::Acquire) && tx.capacity() > 0 {
                    if force_enc.swap(false, Ordering::AcqRel) {
                        encoder.force_keyframe();
                    }
                    let encoded = encoder.encode(&raw);
                    let frame_id = frame_counter.fetch_add(1, Ordering::Relaxed);
                    let frags = fragment_frame(frame_id, encoded.is_keyframe, &encoded.annex_b);
                    let _ = tx.try_send(frags);
                }
            }
        })
        .ok();

    std::thread::Builder::new()
        .name("squad-video-capture".into())
        .spawn(move || {
            let mut capturer = ScreenCapturer::new(cap_cfg.clone());
            let frame_interval = Duration::from_secs_f64(1.0 / f64::from(cap_cfg.fps.as_u8()));
            let mut next_tick = Instant::now();

            while !stop_cap.load(Ordering::Acquire) {
                let mut raw = capturer.capture_frame();
                let has_subscribers = subs_cap.load(Ordering::Acquire);

                if preview_cap.load(Ordering::Acquire) {
                    let preview_rgba = if has_subscribers {
                        Arc::new(raw.rgba.clone())
                    } else {
                        Arc::new(std::mem::take(&mut raw.rgba))
                    };
                    if let Ok(mut guard) = slot_clone.try_lock() {
                        *guard = Some(DecodedVideoFrame {
                            streamer_peer_id: local_pid,
                            width: raw.width,
                            height: raw.height,
                            rgba: preview_rgba,
                            frame_seq: NEXT_VIDEO_FRAME_SEQ.fetch_add(1, Ordering::Relaxed),
                        });
                        notify_clone.notify_one();
                    }
                }

                if has_subscribers {
                    let (lock, cvar) = &*wake_cap;
                    let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                    *guard = Some(raw);
                    drop(guard);
                    cvar.notify_one();
                }

                next_tick += frame_interval;
                let now = Instant::now();
                if next_tick > now {
                    std::thread::sleep(next_tick - now);
                } else if now.duration_since(next_tick) > frame_interval * 2 {
                    next_tick = now;
                }
            }

            let (lock, cvar) = &*wake_cap;
            if let Ok(mut guard) = lock.lock() {
                *guard = None;
            }
            cvar.notify_all();
        })
        .ok();

    *capture_handle = Some(CaptureWorkerHandle {
        stop,
        force_idr: force_idr_flag,
        preview_active: preview_flag,
        subs_active: subs_flag,
        encode_wake,
    });
}

async fn broadcast_stream_announce(
    socket: &UdpSocket,
    sess: &mut ActiveSession,
    seal_buf: &mut Vec<u8>,
    relay_buf: &mut Vec<u8>,
    is_streaming: bool,
    config: Option<&StreamConfig>,
) {
    let endpoints: Vec<PeerEndpoint> = sess
        .peers
        .keys()
        .copied()
        .filter_map(|pid| peer_endpoint(sess, pid))
        .collect();
    for endpoint in endpoints {
        send_stream_announce_to_endpoint(
            socket,
            endpoint,
            sess,
            seal_buf,
            relay_buf,
            is_streaming,
            config,
        )
        .await;
    }
}

async fn send_stream_announce_to_endpoint(
    socket: &UdpSocket,
    dst: PeerEndpoint,
    sess: &mut ActiveSession,
    seal_buf: &mut Vec<u8>,
    relay_buf: &mut Vec<u8>,
    is_streaming: bool,
    config: Option<&StreamConfig>,
) {
    let (codec, has_audio, width, height, fps, source_title, encoder_backend) = if is_streaming {
        if let Some(cfg) = config {
            let (w, h) = cfg.output_dimensions();
            (
                cfg.codec.as_u8(),
                cfg.share_audio,
                w as u16,
                h as u16,
                cfg.fps.as_u8(),
                cfg.target.title().to_string(),
                sess.local_encoder_backend.clone(),
            )
        } else {
            (0, false, 0, 0, 0, String::new(), String::new())
        }
    } else {
        (0, false, 0, 0, 0, String::new(), String::new())
    };

    sess.control_seq = sess.control_seq.wrapping_add(1);
    let hdr = P2pHeader {
        msg_type: P2pMsgType::StreamAnnounce,
        room_prefix: sess.room_prefix,
        sender_peer_id: sess.local_peer_id,
        seq_num: sess.control_seq,
    };
    let payload = P2pPayload::StreamAnnounce {
        is_streaming,
        codec,
        has_audio,
        width,
        height,
        fps,
        source_title,
        encoder_backend,
    };
    if sess.cipher.seal_packet(&hdr, &payload, seal_buf).is_ok() {
        send_sealed_to_endpoint(socket, dst, P2pMsgType::StreamAnnounce, seal_buf, relay_buf).await;
    }
}

async fn send_stream_subscribe(
    socket: &UdpSocket,
    sess: &mut ActiveSession,
    seal_buf: &mut Vec<u8>,
    relay_buf: &mut Vec<u8>,
    target_peer_id: PeerId,
    subscribe: bool,
) {
    if peer_endpoint(sess, target_peer_id).is_none() {
        return;
    }
    sess.control_seq = sess.control_seq.wrapping_add(1);
    let hdr = P2pHeader {
        msg_type: P2pMsgType::StreamSubscribe,
        room_prefix: sess.room_prefix,
        sender_peer_id: sess.local_peer_id,
        seq_num: sess.control_seq,
    };
    let payload = P2pPayload::StreamSubscribe { subscribe };
    if sess.cipher.seal_packet(&hdr, &payload, seal_buf).is_ok() {
        send_sealed_to_peer(
            socket,
            sess,
            target_peer_id,
            P2pMsgType::StreamSubscribe,
            seal_buf,
            relay_buf,
        )
        .await;
    }
}

async fn send_stream_pli(
    socket: &UdpSocket,
    sess: &mut ActiveSession,
    seal_buf: &mut Vec<u8>,
    relay_buf: &mut Vec<u8>,
    target_peer_id: PeerId,
) {
    if peer_endpoint(sess, target_peer_id).is_none() {
        return;
    }
    sess.control_seq = sess.control_seq.wrapping_add(1);
    let hdr = P2pHeader {
        msg_type: P2pMsgType::StreamPli,
        room_prefix: sess.room_prefix,
        sender_peer_id: sess.local_peer_id,
        seq_num: sess.control_seq,
    };
    if sess
        .cipher
        .seal_packet(&hdr, &P2pPayload::StreamPli, seal_buf)
        .is_ok()
    {
        send_sealed_to_peer(
            socket,
            sess,
            target_peer_id,
            P2pMsgType::StreamPli,
            seal_buf,
            relay_buf,
        )
        .await;
    }
}

async fn send_stream_decoder_error(
    socket: &UdpSocket,
    sess: &mut ActiveSession,
    seal_buf: &mut Vec<u8>,
    relay_buf: &mut Vec<u8>,
    target_peer_id: PeerId,
    failed_codec: u8,
    reason: &str,
) {
    if peer_endpoint(sess, target_peer_id).is_none() {
        return;
    }
    sess.control_seq = sess.control_seq.wrapping_add(1);
    let hdr = P2pHeader {
        msg_type: P2pMsgType::StreamDecoderError,
        room_prefix: sess.room_prefix,
        sender_peer_id: sess.local_peer_id,
        seq_num: sess.control_seq,
    };
    let payload = P2pPayload::StreamDecoderError {
        failed_codec,
        reason: reason.to_string(),
    };
    if sess.cipher.seal_packet(&hdr, &payload, seal_buf).is_ok() {
        send_sealed_to_peer(
            socket,
            sess,
            target_peer_id,
            P2pMsgType::StreamDecoderError,
            seal_buf,
            relay_buf,
        )
        .await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn init_session(
    rv_addr: SocketAddr,
    invite: InviteLink,
    nickname: String,
    client_uuid: ClientUuid,
    local_pub_key: [u8; 32],
    bound_port: u16,
    pending_local_avatar: Option<([u8; 32], Vec<u8>)>,
    local_encoder_backend: String,
    hevc_decode_supported: bool,
    is_lan_room: bool,
    lan_room_name: String,
    lan_room_password: Option<String>,
    embedded_rv: Option<EmbeddedRendezvousHandle>,
    audio_engine: &AudioEngineHandle,
    tr_tx: &mpsc::Sender<TransferEvent>,
    tr_rx: &mut mpsc::Receiver<TransferEvent>,
    evt_tx: &NotifyingEventTx,
) -> ActiveSession {
    audio_engine.clear_peers();
    audio_engine.set_share_stream_audio(false);
    let lan_addrs = discover_lan_ipv4_addrs(bound_port);
    let cipher = RoomCipher::from_secret(&invite.secret);
    let room_prefix = invite.room_prefix();

    let mut transfer = SwarmChatManager::new();
    transfer.set_initial_local_avatar(pending_local_avatar);
    transfer.set_room_persistence(invite.room_id, app_data_root_dir());
    transfer.load_room_from_disk(client_uuid, tr_tx).await;
    forward_transfer_events(tr_rx, evt_tx).await;

    ActiveSession {
        rendezvous_addr: rv_addr,
        invite,
        cipher,
        room_prefix,
        local_uuid: client_uuid,
        local_pub_key,
        nickname,
        local_peer_id: 0,
        lan_addrs,
        peers: HashMap::new(),
        control_seq: 1,
        video_seq: 1,
        video_tx_queue: VideoTxQueue::default(),
        transfer,
        local_encoder_backend,
        local_stream_config: None,
        local_preview_enabled: false,
        stream_subscribers: HashSet::new(),
        next_video_frame_id: Arc::new(AtomicU32::new(1)),
        watched_peer_id: None,
        sent_decoder_error_to: None,
        video_reassembler: FrameReassembler::new(),
        hevc_decode_supported,
        last_pli_sent: Instant::now() - Duration::from_secs(5),
        is_lan_room,
        lan_room_name,
        embedded_rv,
        last_rv_rx: Instant::now(),
        migrating_until: None,
        migration_claimed_at: None,
        last_mesh_reachability_sent: Instant::now() - MESH_REACHABILITY_INTERVAL,
        last_advertised_direct_peers: Vec::new(),
        active_short_code: None,
        last_spake2_reply_times: Vec::new(),
        local_udp_port: bound_port,
        lan_room_password,
        lan_auth_buckets: HashMap::new(),
        lan_auth_global_times: Vec::new(),
    }
}

async fn teardown_session(
    socket: &UdpSocket,
    lan_tx_sock: Option<&UdpSocket>,
    mut sess: ActiveSession,
    seal_buf: &mut Vec<u8>,
    relay_buf: &mut Vec<u8>,
    migrate_if_active: bool,
) {
    let embedded = sess.embedded_rv.take();
    if let Some(rv) = embedded {
        if migrate_if_active && sess.local_peer_id != 0 {
            let successor = sess
                .peers
                .iter()
                .filter(|(_, p)| {
                    matches!(
                        p.status,
                        PeerConnStatus::Connected
                            | PeerConnStatus::MeshRelayed
                            | PeerConnStatus::Relayed
                    )
                })
                .map(|(&pid, _)| pid)
                .min();
            if let Some(new_host_peer_id) = successor {
                sess.control_seq = sess.control_seq.wrapping_add(1);
                let hdr = P2pHeader {
                    msg_type: P2pMsgType::LanHostMigrate,
                    room_prefix: sess.room_prefix,
                    sender_peer_id: sess.local_peer_id,
                    seq_num: sess.control_seq,
                };
                let payload = P2pPayload::LanHostMigrate { new_host_peer_id };
                if sess.cipher.seal_packet(&hdr, &payload, seal_buf).is_ok() {
                    let targets: Vec<PeerId> = sess.peers.keys().copied().collect();
                    for pid in targets {
                        send_sealed_to_peer(
                            socket,
                            &sess,
                            pid,
                            P2pMsgType::LanHostMigrate,
                            seal_buf,
                            relay_buf,
                        )
                        .await;
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            }
        }
        if let Some(tx) = lan_tx_sock {
            let bye_id = if sess.lan_room_password.is_some() {
                derive_lan_announce_id(&sess.invite)
            } else {
                sess.invite.room_id
            };
            let bye = LanDiscoveryPacket::Goodbye {
                room_id: bye_id,
                rv_port: rv.port,
            };
            broadcast_lan_packet(tx, &bye).await;
        }
        send_leave(socket, &sess).await;
        rv.stop().await;
    } else {
        send_leave(socket, &sess).await;
    }
    if let Some(mut pw) = sess.lan_room_password.take() {
        pw.clear();
    }
}

async fn promote_to_lan_host(
    socket: &UdpSocket,
    lan_tx_sock: Option<&UdpSocket>,
    sess: &mut ActiveSession,
    seal_buf: &mut Vec<u8>,
    relay_buf: &mut Vec<u8>,
) {
    let rv = match start_embedded_rendezvous().await {
        Ok(handle) => handle,
        Err(err) => {
            warn!("Failed to start embedded LAN host during migration: {err}");
            return;
        }
    };
    let new_port = rv.port;
    sess.embedded_rv = Some(rv);
    sess.rendezvous_addr = SocketAddr::from(([127, 0, 0, 1], new_port));
    sess.last_rv_rx = Instant::now();
    sess.migrating_until = Some(Instant::now() + LAN_MIGRATION_GRACE);
    sess.migration_claimed_at = Some(Instant::now());

    sess.control_seq = sess.control_seq.wrapping_add(1);
    let hdr = P2pHeader {
        msg_type: P2pMsgType::LanHostClaim,
        room_prefix: sess.room_prefix,
        sender_peer_id: sess.local_peer_id,
        seq_num: sess.control_seq,
    };
    let payload = P2pPayload::LanHostClaim {
        rv_port: new_port,
        room_name: sess.lan_room_name.clone(),
    };
    if sess.cipher.seal_packet(&hdr, &payload, seal_buf).is_ok() {
        let peer_ids: Vec<PeerId> = sess.peers.keys().copied().collect();
        for pid in peer_ids {
            send_sealed_to_peer(
                socket,
                sess,
                pid,
                P2pMsgType::LanHostClaim,
                seal_buf,
                relay_buf,
            )
            .await;
        }
    }

    send_register(socket, sess).await;
    broadcast_lan_announce(lan_tx_sock, sess).await;
    info!(
        "Promoted peer #{} to LAN coordinator on port {}",
        sess.local_peer_id, new_port
    );
}

async fn broadcast_lan_announce(lan_tx_sock: Option<&UdpSocket>, sess: &ActiveSession) {
    let Some(tx) = lan_tx_sock else {
        return;
    };
    let Some(rv) = sess.embedded_rv.as_ref() else {
        return;
    };
    let peer_count = (sess.peers.len() + 1).min(MAX_ROOM_PEERS) as u8;
    let pkt = if sess.lan_room_password.is_some() {
        LanDiscoveryPacket::AnnounceLocked {
            announce_id: derive_lan_announce_id(&sess.invite),
            rv_port: rv.port,
            auth_port: sess.local_udp_port,
            peer_count,
            max_peers: MAX_ROOM_PEERS as u8,
            room_name: sess.lan_room_name.clone(),
            host_nickname: sess.nickname.clone(),
        }
    } else {
        LanDiscoveryPacket::Announce {
            room_id: sess.invite.room_id,
            secret: sess.invite.secret,
            rv_port: rv.port,
            peer_count,
            max_peers: MAX_ROOM_PEERS as u8,
            room_name: sess.lan_room_name.clone(),
            host_nickname: sess.nickname.clone(),
        }
    };
    broadcast_lan_packet(tx, &pkt).await;
}

async fn publish_lan_rooms(
    map: &HashMap<RoomId, (DiscoveredLanRoom, Instant)>,
    last_published: &mut Vec<DiscoveredLanRoom>,
    evt_tx: &NotifyingEventTx,
) {
    let mut rooms: Vec<DiscoveredLanRoom> = map.values().map(|(r, _)| r.clone()).collect();
    rooms.sort_by(|a, b| {
        a.room_name
            .cmp(&b.room_name)
            .then_with(|| a.room_id.cmp(&b.room_id))
    });
    if rooms != *last_published {
        *last_published = rooms.clone();
        evt_tx.send(MeshEvent::LanRoomsUpdated(rooms)).await;
    }
}

async fn recv_lan_opt(sock: Option<&UdpSocket>, buf: &mut [u8]) -> Option<(usize, SocketAddr)> {
    match sock {
        Some(s) => s.recv_from(buf).await.ok(),
        None => std::future::pending().await,
    }
}

struct SessionActivationCtx<'a> {
    socket: &'a UdpSocket,
    lan_tx_sock: Option<&'a UdpSocket>,
    bound_port: u16,
    pending_local_avatar: &'a Option<([u8; 32], Vec<u8>)>,
    local_encoder_backend: &'a str,
    hevc_decode_supported: bool,
    audio_engine: &'a AudioEngineHandle,
    tr_tx: &'a mpsc::Sender<TransferEvent>,
    tr_rx: &'a mut mpsc::Receiver<TransferEvent>,
    evt_tx: &'a NotifyingEventTx,
    capture_worker: &'a mut Option<CaptureWorkerHandle>,
    video_out_rx: &'a mut mpsc::Receiver<Vec<VideoPacketOut>>,
    decode_handle: &'a VideoDecodeHandle,
    video_frame_slot: &'a Arc<Mutex<Option<DecodedVideoFrame>>>,
    discovered_lan: &'a mut HashMap<RoomId, (DiscoveredLanRoom, Instant)>,
    last_published_lan: &'a mut Vec<DiscoveredLanRoom>,
    last_snapshots: &'a mut Vec<PeerUiSnapshot>,
    seal_buf: &'a mut Vec<u8>,
    relay_buf: &'a mut Vec<u8>,
    presence: &'a PresenceWorkerState,
}

#[allow(clippy::too_many_arguments)]
async fn activate_joined_session(
    session: &mut Option<ActiveSession>,
    ctx: &mut SessionActivationCtx<'_>,
    rv_addr: SocketAddr,
    invite: InviteLink,
    nickname: String,
    client_uuid: ClientUuid,
    is_lan_room: bool,
    room_name: String,
    lan_room_password: Option<String>,
    embedded_rv: Option<EmbeddedRendezvousHandle>,
) {
    if let Some(old_sess) = session.take() {
        if old_sess.active_short_code.is_some() {
            ctx.evt_tx.send(MeshEvent::ShortCodeCleared).await;
        }
        teardown_session(
            ctx.socket,
            ctx.lan_tx_sock,
            old_sess,
            ctx.seal_buf,
            ctx.relay_buf,
            true,
        )
        .await;
    }

    let new_sess = init_session(
        rv_addr,
        invite,
        nickname,
        client_uuid,
        ctx.presence.my_pub_key,
        ctx.bound_port,
        ctx.pending_local_avatar.clone(),
        ctx.local_encoder_backend.to_string(),
        ctx.hevc_decode_supported,
        is_lan_room,
        room_name,
        lan_room_password,
        embedded_rv,
        ctx.audio_engine,
        ctx.tr_tx,
        ctx.tr_rx,
        ctx.evt_tx,
    )
    .await;

    ctx.discovered_lan.remove(&new_sess.invite.room_id);
    ctx.discovered_lan
        .remove(&derive_lan_announce_id(&new_sess.invite));
    publish_lan_rooms(ctx.discovered_lan, ctx.last_published_lan, ctx.evt_tx).await;

    send_register(ctx.socket, &new_sess).await;
    if new_sess.embedded_rv.is_some() {
        broadcast_lan_announce(ctx.lan_tx_sock, &new_sess).await;
    }
    stop_capture_worker(ctx.capture_worker);
    drain_video_out_rx(ctx.video_out_rx);
    ctx.audio_engine.clear_stream_audio();
    ctx.decode_handle.reset();
    clear_video_slot(ctx.video_frame_slot);
    ctx.last_snapshots.clear();
    *session = Some(new_sess);
    ctx.audio_engine.play_cue(SoundCue::Join);
    send_presence_ping(ctx.socket, ctx.presence, true).await;
}

fn snapshot_friends(map: &HashMap<[u8; 32], (String, bool, Instant)>) -> Vec<FriendPresenceUi> {
    let mut out: Vec<FriendPresenceUi> = map
        .iter()
        .map(|(&pub_key, (nickname, in_room, _))| FriendPresenceUi {
            pub_key,
            nickname: nickname.clone(),
            online: true,
            in_room: *in_room,
        })
        .collect();
    out.sort_by_key(|f| f.pub_key);
    out
}

async fn send_presence_ping(socket: &UdpSocket, presence: &PresenceWorkerState, in_room: bool) {
    let Some(rv_addr) = presence.rendezvous_addr else {
        return;
    };
    if presence.nickname.is_empty() && presence.identity_secret.is_none() {
        return;
    }
    let ping = SignalingPacket::C2sPresencePing {
        pub_key: presence.my_pub_key,
        nickname: presence.nickname.clone(),
        in_room,
    };
    if let Ok(bytes) = ping.encode() {
        let _ = socket.send_to(&bytes, rv_addr).await;
    }
}

async fn send_presence_ping_and_query(
    socket: &UdpSocket,
    presence: &PresenceWorkerState,
    in_room: bool,
) {
    let Some(rv_addr) = presence.rendezvous_addr else {
        return;
    };
    send_presence_ping(socket, presence, in_room).await;
    for chunk in presence
        .friend_pub_keys
        .chunks(MAX_QUERY_FRIENDS_PER_PACKET)
    {
        let query = SignalingPacket::C2sQueryFriends {
            friend_pub_keys: chunk.to_vec(),
        };
        if let Ok(bytes) = query.encode() {
            let _ = socket.send_to(&bytes, rv_addr).await;
        }
    }
}

async fn try_host_spake2_reply(
    socket: &UdpSocket,
    sess: &mut ActiveSession,
    code: u32,
    spake2_msg_a: &[u8],
    dest_addrs: &[SocketAddr],
) {
    let now = Instant::now();
    let Some((active_code, expires_at)) = sess.active_short_code else {
        return;
    };
    if active_code != code || now >= expires_at {
        return;
    }
    sess.last_spake2_reply_times
        .retain(|t| now.duration_since(*t) < Duration::from_secs(1));
    if sess.last_spake2_reply_times.len() >= SPAKE2_HOST_MAX_REPLIES_PER_SEC {
        return;
    }
    sess.last_spake2_reply_times.push(now);

    match spake2_host_seal_invite(code, spake2_msg_a, &sess.invite) {
        Ok((spake2_msg_b, nonce, encrypted_invite)) => {
            let reply = Spake2Packet::HostReply {
                code,
                spake2_msg_b,
                nonce,
                encrypted_invite,
            };
            if let Ok(bytes) = reply.encode() {
                for &addr in dest_addrs {
                    let _ = socket.send_to(&bytes, addr).await;
                }
            }
        }
        Err(e) => {
            debug!("SPAKE2 host seal failed for short code challenge: {e}");
        }
    }
}

async fn handle_global_signaling_packet(
    pkt: &SignalingPacket,
    socket: &UdpSocket,
    audio_engine: &AudioEngineHandle,
    evt_tx: &NotifyingEventTx,
    presence: &mut PresenceWorkerState,
    session: &mut Option<ActiveSession>,
    pending_short_code_join: &mut Option<PendingShortCodeJoin>,
) -> bool {
    match pkt {
        SignalingPacket::S2cFriendsStatus { online_friends } => {
            let before = snapshot_friends(&presence.online_friends);
            let now = Instant::now();
            if presence.friend_pub_keys.len() <= MAX_QUERY_FRIENDS_PER_PACKET
                && online_friends.len() < MAX_FRIENDS_STATUS_PER_PACKET
            {
                presence.online_friends.clear();
            }
            for info in online_friends {
                if presence.friend_pub_keys.contains(&info.pub_key) {
                    presence
                        .online_friends
                        .insert(info.pub_key, (info.nickname.clone(), info.in_room, now));
                }
            }
            presence
                .online_friends
                .retain(|_, (_, _, seen_at)| now.duration_since(*seen_at) <= PRESENCE_UI_TTL);
            let after = snapshot_friends(&presence.online_friends);
            if before != after {
                evt_tx.send(MeshEvent::FriendsStatusUpdated(after)).await;
            }
            true
        }
        SignalingPacket::S2cIncomingFriendInvite {
            sender_pub_key,
            ephemeral_pub_key,
            nonce,
            ciphertext,
        } => {
            if *sender_pub_key == presence.my_pub_key {
                return true;
            }
            let Some(sk) = presence.identity_secret.as_ref() else {
                return true;
            };
            match open_friend_invite(sk, sender_pub_key, ephemeral_pub_key, nonce, ciphertext) {
                Ok(dec) => {
                    if session
                        .as_ref()
                        .is_some_and(|s| s.invite.room_id == dec.invite.room_id)
                    {
                        return true;
                    }
                    let is_known_friend = presence.friend_pub_keys.contains(sender_pub_key);
                    audio_engine.play_cue(SoundCue::IncomingCall);
                    evt_tx
                        .send(MeshEvent::IncomingFriendCall(IncomingFriendCallUi {
                            sender_pub_key: *sender_pub_key,
                            sender_nickname: dec.sender_nickname,
                            invite: dec.invite,
                            is_party_invite: dec.is_party_invite,
                            is_known_friend,
                        }))
                        .await;
                }
                Err(e) => {
                    warn!(
                        "Ignored incoming friend invite that failed X25519 AEAD verification: {e}"
                    );
                }
            }
            true
        }
        SignalingPacket::S2cShortCodeRegistered {
            code,
            expires_in_secs,
        } => {
            if let Some(sess) = session.as_mut() {
                sess.active_short_code = Some((
                    *code,
                    Instant::now() + Duration::from_secs(*expires_in_secs as u64),
                ));
            }
            evt_tx
                .send(MeshEvent::ShortCodeGenerated {
                    code: *code,
                    expires_in_secs: *expires_in_secs,
                })
                .await;
            true
        }
        SignalingPacket::S2cShortCodeChallenge {
            code,
            joiner_pub_key: _,
            joiner_public_addr,
            joiner_lan_addrs,
            spake2_msg_a,
        } => {
            if let Some(sess) = session.as_mut() {
                let mut dest_addrs = Vec::with_capacity(1 + joiner_lan_addrs.len());
                dest_addrs.push(*joiner_public_addr);
                for lan in joiner_lan_addrs {
                    let sa = SocketAddr::V4(*lan);
                    if !dest_addrs.contains(&sa) {
                        dest_addrs.push(sa);
                    }
                }
                try_host_spake2_reply(socket, sess, *code, spake2_msg_a, &dest_addrs).await;
            }
            true
        }
        SignalingPacket::S2cShortCodeJoinerInfo {
            code,
            host_pub_key: _,
            host_public_addr,
            host_lan_addrs,
        } => {
            if let Some(pending) = pending_short_code_join.as_mut() {
                if pending.code == *code {
                    let mut addrs = Vec::with_capacity(1 + host_lan_addrs.len());
                    addrs.push(*host_public_addr);
                    for lan in host_lan_addrs {
                        let sa = SocketAddr::V4(*lan);
                        if !addrs.contains(&sa) {
                            addrs.push(sa);
                        }
                    }
                    pending.host_addrs = addrs;
                    pending.last_retry_sent = Instant::now();
                    let retry = Spake2Packet::JoinerRetry {
                        code: pending.code,
                        spake2_msg_a: pending.msg_a.clone(),
                    };
                    if let Ok(bytes) = retry.encode() {
                        for &addr in &pending.host_addrs {
                            let _ = socket.send_to(&bytes, addr).await;
                        }
                    }
                }
            }
            true
        }
        SignalingPacket::S2cError { code } => match code {
            SignalingErrorCode::FriendOffline => {
                evt_tx
                    .send(MeshEvent::Error(
                        "Friend is offline or unreachable".to_string(),
                    ))
                    .await;
                true
            }
            SignalingErrorCode::InvalidShortCode => {
                if pending_short_code_join.take().is_some() {
                    evt_tx
                        .send(MeshEvent::Error(
                            "Invalid or expired 6-digit voice code".to_string(),
                        ))
                        .await;
                } else {
                    if let Some(sess) = session.as_mut() {
                        if sess.active_short_code.take().is_some() {
                            evt_tx.send(MeshEvent::ShortCodeCleared).await;
                        }
                    }
                    evt_tx
                        .send(MeshEvent::Error(
                            "Short code collision or room not registered yet; try again."
                                .to_string(),
                        ))
                        .await;
                }
                true
            }
            SignalingErrorCode::RateLimited if pending_short_code_join.take().is_some() => {
                evt_tx
                    .send(MeshEvent::Error(
                        "Too many wrong code attempts. Please wait 60 seconds.".to_string(),
                    ))
                    .await;
                true
            }
            _ => false,
        },
        _ => false,
    }
}

async fn send_lan_auth_error(
    socket: &UdpSocket,
    dst: SocketAddr,
    announce_id: RoomId,
    attempt_id: u64,
    status: u8,
) {
    let reply = Spake2Packet::LanAuthReply {
        announce_id,
        attempt_id,
        status,
        spake2_msg_b: Vec::new(),
        nonce: [0u8; 12],
        encrypted_invite: Vec::new(),
    };
    if let Ok(bytes) = reply.encode() {
        let _ = socket.send_to(&bytes, dst).await;
    }
}

async fn handle_lan_auth_req(
    socket: &UdpSocket,
    session: Option<&mut ActiveSession>,
    announce_id: RoomId,
    attempt_id: u64,
    spake2_msg_a: &[u8],
    src_addr: SocketAddr,
) {
    let Some(sess) = session else {
        return;
    };
    if !sess.is_lan_room || sess.embedded_rv.is_none() {
        return;
    }

    let now = Instant::now();
    sess.lan_auth_global_times
        .retain(|t| now.duration_since(*t) < Duration::from_secs(1));
    if sess.lan_auth_global_times.len() >= LAN_AUTH_GLOBAL_MAX_PER_SEC {
        return;
    }
    sess.lan_auth_global_times.push(now);

    let expected_announce_id = derive_lan_announce_id(&sess.invite);
    let Some(ref password) = sess.lan_room_password else {
        send_lan_auth_error(
            socket,
            src_addr,
            announce_id,
            attempt_id,
            LAN_AUTH_STATUS_NOT_FOUND,
        )
        .await;
        return;
    };
    if announce_id != expected_announce_id {
        send_lan_auth_error(
            socket,
            src_addr,
            announce_id,
            attempt_id,
            LAN_AUTH_STATUS_NOT_FOUND,
        )
        .await;
        return;
    }

    let src_ip = src_addr.ip();
    if let Some(bucket) = sess.lan_auth_buckets.get(&src_ip) {
        if let Some((cached_attempt, ref cached_bytes, ts)) = bucket.last_reply {
            if cached_attempt == attempt_id && now.duration_since(ts) <= Duration::from_secs(6) {
                let _ = socket.send_to(cached_bytes, src_addr).await;
                return;
            }
        }
    }

    if !sess.lan_auth_buckets.contains_key(&src_ip)
        && sess.lan_auth_buckets.len() >= LAN_AUTH_MAX_BUCKETS
    {
        sess.lan_auth_buckets.retain(|_, b| {
            b.blocked_until.is_some_and(|u| now < u)
                || b.attempts
                    .iter()
                    .any(|t| now.duration_since(*t) <= Duration::from_secs(60))
        });
        if sess.lan_auth_buckets.len() >= LAN_AUTH_MAX_BUCKETS {
            send_lan_auth_error(
                socket,
                src_addr,
                announce_id,
                attempt_id,
                LAN_AUTH_STATUS_RATE_LIMITED,
            )
            .await;
            return;
        }
    }

    let bucket = sess
        .lan_auth_buckets
        .entry(src_ip)
        .or_insert_with(|| LanAuthRateBucket {
            attempts: Vec::new(),
            blocked_until: None,
            last_reply: None,
        });

    if bucket.blocked_until.is_some_and(|until| now < until) {
        send_lan_auth_error(
            socket,
            src_addr,
            announce_id,
            attempt_id,
            LAN_AUTH_STATUS_RATE_LIMITED,
        )
        .await;
        return;
    }
    if bucket.blocked_until.is_some_and(|until| now >= until) {
        bucket.blocked_until = None;
        bucket.attempts.clear();
    }

    bucket
        .attempts
        .retain(|t| now.duration_since(*t) <= Duration::from_secs(60));
    bucket.attempts.push(now);

    if bucket.attempts.len() > LAN_AUTH_MAX_ATTEMPTS_PER_MIN {
        bucket.blocked_until = Some(now + LAN_AUTH_BLOCK_DURATION);
        send_lan_auth_error(
            socket,
            src_addr,
            announce_id,
            attempt_id,
            LAN_AUTH_STATUS_RATE_LIMITED,
        )
        .await;
        return;
    }

    match spake2_lan_host_seal_invite(
        password,
        &announce_id,
        attempt_id,
        spake2_msg_a,
        &sess.invite,
    ) {
        Ok((spake2_msg_b, nonce, encrypted_invite)) => {
            let reply = Spake2Packet::LanAuthReply {
                announce_id,
                attempt_id,
                status: LAN_AUTH_STATUS_OK,
                spake2_msg_b,
                nonce,
                encrypted_invite,
            };
            if let Ok(bytes) = reply.encode() {
                bucket.last_reply = Some((attempt_id, bytes.clone(), now));
                let _ = socket.send_to(&bytes, src_addr).await;
            }
        }
        Err(e) => {
            debug!("SPAKE2 LAN host seal failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::transfer::ChatContentUi;
    use squad_dsp::AudioEngineConfig;

    #[tokio::test]
    async fn test_lan_host_migration_preserves_room_and_chat() {
        let rv = start_embedded_rendezvous().await.unwrap();
        let rv_addr = format!("127.0.0.1:{}", rv.port);
        let invite = InviteLink::generate();

        let mk_node = |uuid_byte: u8| {
            let (out_tx, out_rx) = mpsc::channel(64);
            let audio = AudioEngineHandle::start(AudioEngineConfig::default(), out_tx).unwrap();
            let slot = Arc::new(Mutex::new(None));
            let mesh_notify = Arc::new(Notify::new());
            let video_notify = Arc::new(Notify::new());
            let (cmd_tx, evt_rx) =
                spawn_mesh_worker(audio.clone(), out_rx, slot, mesh_notify, video_notify);
            (audio, cmd_tx, evt_rx, [uuid_byte; 16])
        };

        let (aud_a, tx_a, mut rx_a, uuid_a) = mk_node(0xA1);
        let (aud_b, tx_b, mut rx_b, uuid_b) = mk_node(0xB2);
        let (aud_c, tx_c, mut rx_c, uuid_c) = mk_node(0xC3);

        // Give Peer A embedded server directly via JoinRoom? Even better, let's test BOTH:
        // 1) A, B, C join LAN server; then drop A's server to trigger deterministic LAN host failover!
        for (tx, nick, uuid) in [
            (&tx_a, "AlphaHost", uuid_a),
            (&tx_b, "BravoSurv", uuid_b),
            (&tx_c, "CharlieSurv", uuid_c),
        ] {
            tx.send(MeshCommand::JoinRoom {
                rendezvous_server: rv_addr.clone(),
                invite: invite.clone(),
                nickname: nick.to_string(),
                client_uuid: uuid,
                initial_volumes: HashMap::new(),
                is_lan_room: true,
                room_name: Some("Migration Test LAN".to_string()),
            })
            .await
            .unwrap();
        }

        // Wait until B & C see A + each other Connected via LanDirect
        let deadline = Instant::now() + Duration::from_secs(6);
        let mut b_ready = false;
        let mut c_ready = false;
        while Instant::now() < deadline && (!b_ready || !c_ready) {
            while let Ok(ev) = rx_a.try_recv() {
                let _ = ev;
            }
            while let Ok(ev) = rx_b.try_recv() {
                if let MeshEvent::PeersUpdated(peers) = ev {
                    if peers
                        .iter()
                        .filter(|p| p.status == PeerConnStatus::Connected)
                        .count()
                        >= 2
                    {
                        b_ready = true;
                    }
                }
            }
            while let Ok(ev) = rx_c.try_recv() {
                if let MeshEvent::PeersUpdated(peers) = ev {
                    if peers
                        .iter()
                        .filter(|p| p.status == PeerConnStatus::Connected)
                        .count()
                        >= 2
                    {
                        c_ready = true;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        assert!(
            b_ready && c_ready,
            "All three peers must reach direct Connected state before failover"
        );

        // Simulate Host A leaving & shutting down original embedded rendezvous server
        tx_a.send(MeshCommand::LeaveRoom).await.unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
        rv.stop().await;

        // Wait up to 11s for deterministic LAN_RV_LOST_TIMEOUT (7s) promotion between B and C
        // and verify surviving chat works after host migration!
        tokio::time::sleep(Duration::from_millis(8200)).await;
        tx_b.send(MeshCommand::SendChatText {
            text: "Survived LAN host migration!".to_string(),
        })
        .await
        .unwrap();

        let chat_deadline = Instant::now() + Duration::from_secs(3);
        let mut c_received_chat = false;
        while Instant::now() < chat_deadline && !c_received_chat {
            while let Ok(ev) = rx_c.try_recv() {
                if let MeshEvent::ChatItemReceived(item) = ev {
                    if matches!(item.content, ChatContentUi::Text(ref t) if t == "Survived LAN host migration!")
                    {
                        c_received_chat = true;
                    }
                }
            }
            while rx_b.try_recv().is_ok() {}
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        assert!(
            c_received_chat,
            "Surviving peers must stay connected and exchange chat after LAN host failover"
        );

        let _ = tx_a.send(MeshCommand::Shutdown).await;
        let _ = tx_b.send(MeshCommand::Shutdown).await;
        let _ = tx_c.send(MeshCommand::Shutdown).await;
        aud_a.shutdown();
        aud_b.shutdown();
        aud_c.shutdown();
    }

    #[tokio::test]
    async fn test_hybrid_three_tier_routing_peer_mesh_and_server_relay_video_block() {
        let rv_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let rv_addr = rv_sock.local_addr().unwrap();

        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sock_c = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let addr_b = sock_b.local_addr().unwrap();
        let addr_c = sock_c.local_addr().unwrap();

        let invite = InviteLink::generate();
        let (out_tx_a, _out_rx_a) = mpsc::channel(16);
        let (out_tx_b, _out_rx_b) = mpsc::channel(16);
        let (out_tx_c, _out_rx_c) = mpsc::channel(16);
        let aud_a = AudioEngineHandle::start(AudioEngineConfig::default(), out_tx_a).unwrap();
        let aud_b = AudioEngineHandle::start(AudioEngineConfig::default(), out_tx_b).unwrap();
        let aud_c = AudioEngineHandle::start(AudioEngineConfig::default(), out_tx_c).unwrap();

        let mk_sess = |uuid_b: u8, local_pid: PeerId, nick: &str| {
            let cipher = RoomCipher::from_secret(&invite.secret);
            let now = Instant::now();
            ActiveSession {
                rendezvous_addr: rv_addr,
                invite: invite.clone(),
                cipher,
                room_prefix: invite.room_prefix(),
                local_uuid: [uuid_b; 16],
                local_pub_key: [0u8; 32],
                nickname: nick.to_string(),
                local_peer_id: local_pid,
                lan_addrs: Vec::new(),
                peers: HashMap::new(),
                control_seq: 1,
                video_seq: 1,
                video_tx_queue: VideoTxQueue::default(),
                transfer: SwarmChatManager::new(),
                local_encoder_backend: "test".to_string(),
                local_stream_config: None,
                local_preview_enabled: false,
                stream_subscribers: HashSet::new(),
                next_video_frame_id: Arc::new(AtomicU32::new(1)),
                watched_peer_id: None,
                sent_decoder_error_to: None,
                video_reassembler: FrameReassembler::new(),
                hevc_decode_supported: false,
                last_pli_sent: now,
                is_lan_room: false,
                lan_room_name: String::new(),
                embedded_rv: None,
                last_rv_rx: now,
                migrating_until: None,
                migration_claimed_at: None,
                last_mesh_reachability_sent: now - MESH_REACHABILITY_INTERVAL,
                last_advertised_direct_peers: Vec::new(),
                active_short_code: None,
                last_spake2_reply_times: Vec::new(),
                local_udp_port: 0,
                lan_room_password: None,
                lan_auth_buckets: HashMap::new(),
                lan_auth_global_times: Vec::new(),
            }
        };

        let mk_peer = |pid: PeerId,
                       uuid_b: u8,
                       nick: &str,
                       status: PeerConnStatus,
                       addr: Option<SocketAddr>| {
            let now = Instant::now();
            RemotePeerState {
                record: PeerRecord {
                    peer_id: pid,
                    client_uuid: [uuid_b; 16],
                    public_addr: addr.unwrap_or_else(|| "127.0.0.1:1".parse().unwrap()),
                    lan_addrs: Vec::new(),
                    nickname: nick.to_string(),
                },
                pub_key: None,
                status,
                active_addr: addr,
                mesh_relay_peer: None,
                reported_direct_peers: HashSet::new(),
                reachability_updated_at: None,
                punch_started_at: now,
                last_direct_probe: now,
                last_rx: now,
                rtt_ms: 10,
                has_announced_once: true,
                stream_info: None,
                replay_control: squad_proto::ReplayWindow::new(),
                replay_voice: squad_proto::ReplayWindow::new(),
                replay_dtx: squad_proto::ReplayWindow::new(),
                replay_stream_audio: squad_proto::ReplayWindow::new(),
                replay_video: squad_proto::LargeReplayWindow::new(),
            }
        };

        let mut sess_a = mk_sess(0xA1, 1, "Alice");
        let mut sess_b = mk_sess(0xB2, 2, "Bravo");
        let mut sess_c = mk_sess(0xC3, 3, "Charlie");

        // Priority 1: B sees both A and C directly; A and C cannot see each other directly (stuck on Relayed).
        sess_a.peers.insert(
            2,
            mk_peer(2, 0xB2, "Bravo", PeerConnStatus::Connected, Some(addr_b)),
        );
        sess_a.peers.insert(
            3,
            mk_peer(3, 0xC3, "Charlie", PeerConnStatus::Relayed, None),
        );

        sess_b.peers.insert(
            1,
            mk_peer(1, 0xA1, "Alice", PeerConnStatus::Connected, Some(addr_a)),
        );
        sess_b.peers.insert(
            3,
            mk_peer(3, 0xC3, "Charlie", PeerConnStatus::Connected, Some(addr_c)),
        );

        sess_c
            .peers
            .insert(1, mk_peer(1, 0xA1, "Alice", PeerConnStatus::Relayed, None));
        sess_c.peers.insert(
            2,
            mk_peer(2, 0xB2, "Bravo", PeerConnStatus::Connected, Some(addr_b)),
        );

        let mut seal_buf = Vec::new();
        let mut relay_buf = Vec::new();

        // 1. B advertises its direct peers [1, 3] to A and C.
        broadcast_mesh_reachability_if_needed(
            &sock_b,
            &mut sess_b,
            &mut seal_buf,
            &mut relay_buf,
            false,
        )
        .await;

        let (tr_tx_a, _tr_rx_a) = mpsc::channel(16);
        let (tr_tx_b, _tr_rx_b) = mpsc::channel(16);
        let (tr_tx_c, _tr_rx_c) = mpsc::channel(16);
        let (raw_evt_a, _evt_rx_a) = mpsc::channel(32);
        let (raw_evt_b, _evt_rx_b) = mpsc::channel(32);
        let (raw_evt_c, mut evt_rx_c) = mpsc::channel(32);
        let evt_a = NotifyingEventTx {
            tx: raw_evt_a,
            notify: Arc::new(Notify::new()),
        };
        let evt_b = NotifyingEventTx {
            tx: raw_evt_b,
            notify: Arc::new(Notify::new()),
        };
        let evt_c = NotifyingEventTx {
            tx: raw_evt_c,
            notify: Arc::new(Notify::new()),
        };
        let (vtx_a, _vrx_a) = mpsc::channel(4);
        let (vtx_b, _vrx_b) = mpsc::channel(4);
        let (vtx_c, _vrx_c) = mpsc::channel(4);
        let (dtx_a, _drx_a) = std::sync::mpsc::sync_channel(4);
        let (dtx_b, _drx_b) = std::sync::mpsc::sync_channel(4);
        let (dtx_c, _drx_c) = std::sync::mpsc::sync_channel(4);
        let dec_a = VideoDecodeHandle {
            tx: dtx_a,
            generation: Arc::new(AtomicU64::new(0)),
            shrink_pending: Arc::new(AtomicBool::new(false)),
        };
        let dec_b = VideoDecodeHandle {
            tx: dtx_b,
            generation: Arc::new(AtomicU64::new(0)),
            shrink_pending: Arc::new(AtomicBool::new(false)),
        };
        let dec_c = VideoDecodeHandle {
            tx: dtx_c,
            generation: Arc::new(AtomicU64::new(0)),
            shrink_pending: Arc::new(AtomicBool::new(false)),
        };
        let slot_a = Arc::new(Mutex::new(None));
        let slot_b = Arc::new(Mutex::new(None));
        let slot_c = Arc::new(Mutex::new(None));
        let vnot_a = Arc::new(Notify::new());
        let vnot_b = Arc::new(Notify::new());
        let vnot_c = Arc::new(Notify::new());
        let mut cap_a = None;
        let mut cap_b = None;
        let mut cap_c = None;

        // A and C receive B's MeshReachability -> both promote A<->C to Priority 2 (MeshRelayed via B).
        let mut buf = vec![0u8; MAX_RELAY_UDP_PAYLOAD + 64];
        for (sock, sess, aud, tr_tx, evt_tx, cap, vtx, slot, vnot, dec) in [
            (
                &sock_a,
                &mut sess_a,
                &aud_a,
                &tr_tx_a,
                &evt_a,
                &mut cap_a,
                &vtx_a,
                &slot_a,
                &vnot_a,
                &dec_a,
            ),
            (
                &sock_c,
                &mut sess_c,
                &aud_c,
                &tr_tx_c,
                &evt_c,
                &mut cap_c,
                &vtx_c,
                &slot_c,
                &vnot_c,
                &dec_c,
            ),
        ] {
            let (len, from) =
                tokio::time::timeout(Duration::from_secs(1), sock.recv_from(&mut buf))
                    .await
                    .unwrap()
                    .unwrap();
            let mut sb = Vec::new();
            let mut rb = Vec::new();
            let mut ctx = MeshRxCtx {
                socket: sock,
                audio_engine: aud,
                seal_buf: &mut sb,
                relay_buf: &mut rb,
                tr_tx,
                evt_tx,
                capture_worker: cap,
                video_out_tx: vtx,
                video_frame_slot: slot,
                video_notify: vnot,
                decode_handle: dec,
                lan_tx_sock: None,
            };
            handle_p2p_rx(sock, &mut buf[..len], from, None, None, sess, &mut ctx).await;
        }

        assert_eq!(
            sess_a.peers.get(&3).unwrap().status,
            PeerConnStatus::MeshRelayed
        );
        assert_eq!(sess_a.peers.get(&3).unwrap().mesh_relay_peer, Some(2));
        assert_eq!(
            sess_c.peers.get(&1).unwrap().status,
            PeerConnStatus::MeshRelayed
        );
        assert_eq!(sess_c.peers.get(&1).unwrap().mesh_relay_peer, Some(2));

        // 2. A sends a VideoFrameFrag to C -> must traverse B via OPCODE_PEER_RELAY (0 bytes to rv_sock!).
        sess_c.watched_peer_id = Some(1);
        sess_a.stream_subscribers.insert(3);
        let vid_hdr = P2pHeader {
            msg_type: P2pMsgType::VideoFrameFrag,
            room_prefix: sess_a.room_prefix,
            sender_peer_id: 1,
            seq_num: 99,
        };
        let vid_payload = P2pPayload::VideoFrameFrag {
            frame_id: 1,
            frag_idx: 0,
            frag_count: 1,
            flags: squad_proto::VIDEO_FLAG_KEYFRAME,
            data: vec![0x7E; 64],
        };
        sess_a
            .cipher
            .seal_packet(&vid_hdr, &vid_payload, &mut seal_buf)
            .unwrap();
        send_sealed_to_peer(
            &sock_a,
            &sess_a,
            3,
            P2pMsgType::VideoFrameFrag,
            &seal_buf,
            &mut relay_buf,
        )
        .await;

        // B forwards all queued OPCODE_PEER_RELAY datagrams (PunchSyn + VideoFrameFrag) without decrypting.
        while let Ok(Ok((len_b, from_b))) =
            tokio::time::timeout(Duration::from_millis(50), sock_b.recv_from(&mut buf)).await
        {
            assert!(is_peer_relay_datagram(&buf[..len_b]));
            let mut sb = Vec::new();
            let mut rb = Vec::new();
            let mut ctx_b = MeshRxCtx {
                socket: &sock_b,
                audio_engine: &aud_b,
                seal_buf: &mut sb,
                relay_buf: &mut rb,
                tr_tx: &tr_tx_b,
                evt_tx: &evt_b,
                capture_worker: &mut cap_b,
                video_out_tx: &vtx_b,
                video_frame_slot: &slot_b,
                video_notify: &vnot_b,
                decode_handle: &dec_b,
                lan_tx_sock: None,
            };
            handle_peer_relay_datagram(&sock_b, &mut buf[..len_b], from_b, &mut sess_b, &mut ctx_b)
                .await;
        }

        // C receives the forwarded OPCODE_PEER_RELAY datagrams from B and decodes the keyframe.
        let mut got_video_frame = false;
        while let Ok(Ok((len_c, from_c))) =
            tokio::time::timeout(Duration::from_millis(50), sock_c.recv_from(&mut buf)).await
        {
            assert_eq!(from_c, addr_b);
            assert!(is_peer_relay_datagram(&buf[..len_c]));
            let mut sb = Vec::new();
            let mut rb = Vec::new();
            let mut ctx_c = MeshRxCtx {
                socket: &sock_c,
                audio_engine: &aud_c,
                seal_buf: &mut sb,
                relay_buf: &mut rb,
                tr_tx: &tr_tx_c,
                evt_tx: &evt_c,
                capture_worker: &mut cap_c,
                video_out_tx: &vtx_c,
                video_frame_slot: &slot_c,
                video_notify: &vnot_c,
                decode_handle: &dec_c,
                lan_tx_sock: None,
            };
            handle_peer_relay_datagram(&sock_c, &mut buf[..len_c], from_c, &mut sess_c, &mut ctx_c)
                .await;
            if matches!(
                _drx_c.try_recv(),
                Ok(DecodeJob::Frame {
                    streamer_peer_id: 1,
                    ..
                })
            ) {
                got_video_frame = true;
            }
        }
        assert!(
            got_video_frame,
            "C must receive and reassemble video frame relayed through teammate B"
        );
        // Server received 0 packets during Peer-Mesh Relay!
        assert!(rv_sock.try_recv_from(&mut buf).is_err());

        // 3. B disconnects -> C degrades to Priority 3 Server Relay; video watch stops and video sends are blocked.
        sess_c.peers.remove(&2);
        let degraded = reconcile_mesh_routes(
            &sock_c,
            &mut sess_c,
            &mut seal_buf,
            &mut relay_buf,
            Instant::now(),
        )
        .await;
        assert_eq!(degraded, vec![1]);
        assert_eq!(
            sess_c.peers.get(&1).unwrap().status,
            PeerConnStatus::Relayed
        );
        enforce_video_route_policy(
            &mut sess_c,
            &degraded,
            &mut cap_c,
            &vtx_c,
            &slot_c,
            &vnot_c,
            &dec_c,
            &aud_c,
            &evt_c,
        )
        .await;
        assert_eq!(sess_c.watched_peer_id, None);
        assert!(matches!(evt_rx_c.try_recv(), Ok(MeshEvent::Error(_))));

        // Attempting to send VideoFrameFrag while in Server Relay must send nothing to rv_sock,
        // whereas AudioOpus must be sent via Server Relay.
        send_sealed_to_peer(
            &sock_c,
            &sess_c,
            1,
            P2pMsgType::VideoFrameFrag,
            &seal_buf,
            &mut relay_buf,
        )
        .await;
        assert!(rv_sock.try_recv_from(&mut buf).is_err());

        let aud_hdr = P2pHeader {
            msg_type: P2pMsgType::AudioOpus,
            room_prefix: sess_c.room_prefix,
            sender_peer_id: 3,
            seq_num: 100,
        };
        sess_c
            .cipher
            .seal_packet(
                &aud_hdr,
                &P2pPayload::AudioOpus {
                    opus_data: vec![1, 2, 3],
                },
                &mut seal_buf,
            )
            .unwrap();
        send_sealed_to_peer(
            &sock_c,
            &sess_c,
            1,
            P2pMsgType::AudioOpus,
            &seal_buf,
            &mut relay_buf,
        )
        .await;
        let (srv_len, _) =
            tokio::time::timeout(Duration::from_secs(1), rv_sock.recv_from(&mut buf))
                .await
                .unwrap()
                .unwrap();
        assert!(is_relay_datagram(&buf[..srv_len]));

        aud_a.shutdown();
        aud_b.shutdown();
        aud_c.shutdown();
    }

    fn spawn_test_node(
        uuid_byte: u8,
    ) -> (
        AudioEngineHandle,
        mpsc::Sender<MeshCommand>,
        mpsc::Receiver<MeshEvent>,
        ClientUuid,
    ) {
        let (out_tx, out_rx) = mpsc::channel(64);
        let audio = AudioEngineHandle::start(AudioEngineConfig::default(), out_tx).unwrap();
        let slot = Arc::new(Mutex::new(None));
        let mesh_notify = Arc::new(Notify::new());
        let video_notify = Arc::new(Notify::new());
        let (cmd_tx, evt_rx) =
            spawn_mesh_worker(audio.clone(), out_rx, slot, mesh_notify, video_notify);
        (audio, cmd_tx, evt_rx, [uuid_byte; 16])
    }

    #[tokio::test]
    async fn test_mesh_worker_spake2_short_code_join_and_wrong_code_rejection() {
        let rv = start_embedded_rendezvous().await.unwrap();
        let rv_addr = format!("127.0.0.1:{}", rv.port);

        let (aud_a, tx_a, mut rx_a, uuid_a) = spawn_test_node(0xD1);
        let (aud_b, tx_b, mut rx_b, uuid_b) = spawn_test_node(0xD2);

        let sk_a = StaticSecret::random_from_rng(OsRng);
        let sk_b = StaticSecret::random_from_rng(OsRng);
        tx_a.send(MeshCommand::UpdatePresenceConfig {
            rendezvous_server: rv_addr.clone(),
            nickname: "HostAlpha".to_string(),
            client_uuid: uuid_a,
            identity_secret: sk_a.to_bytes(),
            friend_pub_keys: Vec::new(),
        })
        .await
        .unwrap();
        tx_b.send(MeshCommand::UpdatePresenceConfig {
            rendezvous_server: rv_addr.clone(),
            nickname: "JoinerBravo".to_string(),
            client_uuid: uuid_b,
            identity_secret: sk_b.to_bytes(),
            friend_pub_keys: Vec::new(),
        })
        .await
        .unwrap();

        let invite = InviteLink::generate();
        let expected_uri = invite.to_uri();
        tx_a.send(MeshCommand::JoinRoom {
            rendezvous_server: rv_addr.clone(),
            invite,
            nickname: "HostAlpha".to_string(),
            client_uuid: uuid_a,
            initial_volumes: HashMap::new(),
            is_lan_room: false,
            room_name: None,
        })
        .await
        .unwrap();

        let deadline = Instant::now() + Duration::from_secs(4);
        let mut host_joined = false;
        while Instant::now() < deadline && !host_joined {
            while let Ok(ev) = rx_a.try_recv() {
                if let MeshEvent::JoinedRoom { invite_uri, .. } = ev {
                    assert_eq!(invite_uri, expected_uri);
                    host_joined = true;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            host_joined,
            "Host must join room before generating short code"
        );

        tx_a.send(MeshCommand::GenerateShortCode).await.unwrap();
        let code_deadline = Instant::now() + Duration::from_secs(3);
        let mut active_code: Option<u32> = None;
        while Instant::now() < code_deadline && active_code.is_none() {
            while let Ok(ev) = rx_a.try_recv() {
                if let MeshEvent::ShortCodeGenerated {
                    code,
                    expires_in_secs,
                } = ev
                {
                    assert_eq!(expires_in_secs, 600);
                    active_code = Some(code);
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let code = active_code.expect("Host must receive ShortCodeGenerated");

        // 1. Wrong code attempt must fail cleanly without joining
        let wrong_code = if code == 999_999 { 100_000 } else { code + 1 };
        tx_b.send(MeshCommand::JoinByShortCode {
            rendezvous_server: rv_addr.clone(),
            code: wrong_code,
            nickname: "JoinerBravo".to_string(),
            client_uuid: uuid_b,
        })
        .await
        .unwrap();

        let err_deadline = Instant::now() + Duration::from_secs(3);
        let mut got_wrong_err = false;
        while Instant::now() < err_deadline && !got_wrong_err {
            while let Ok(ev) = rx_b.try_recv() {
                match ev {
                    MeshEvent::Error(_) => got_wrong_err = true,
                    MeshEvent::JoinedRoom { .. } => {
                        panic!("Wrong 6-digit code must never allow joining the room");
                    }
                    _ => {}
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            got_wrong_err,
            "Wrong 6-digit code must return MeshEvent::Error"
        );

        // 2. Valid code must authenticate via SPAKE2, deliver RoomCipher invite, and connect P2P
        tx_b.send(MeshCommand::JoinByShortCode {
            rendezvous_server: rv_addr,
            code,
            nickname: "JoinerBravo".to_string(),
            client_uuid: uuid_b,
        })
        .await
        .unwrap();

        let join_deadline = Instant::now() + Duration::from_secs(6);
        let mut b_joined = false;
        let mut a_connected = false;
        let mut b_connected = false;
        while Instant::now() < join_deadline && !(b_joined && a_connected && b_connected) {
            while let Ok(ev) = rx_a.try_recv() {
                if let MeshEvent::PeersUpdated(peers) = ev {
                    if peers.iter().any(|p| p.status == PeerConnStatus::Connected) {
                        a_connected = true;
                    }
                }
            }
            while let Ok(ev) = rx_b.try_recv() {
                match ev {
                    MeshEvent::JoinedRoom { invite_uri, .. } => {
                        assert_eq!(invite_uri, expected_uri);
                        b_joined = true;
                    }
                    MeshEvent::PeersUpdated(peers)
                        if peers.iter().any(|p| p.status == PeerConnStatus::Connected) =>
                    {
                        b_connected = true;
                    }
                    _ => {}
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(
            b_joined,
            "Joiner must join the exact room via SPAKE2 short code"
        );
        assert!(
            a_connected && b_connected,
            "Host and SPAKE2 joiner must reach Connected state"
        );

        tx_a.send(MeshCommand::SendChatText {
            text: "SPAKE2 code verified!".to_string(),
        })
        .await
        .unwrap();
        let chat_deadline = Instant::now() + Duration::from_secs(3);
        let mut got_chat = false;
        while Instant::now() < chat_deadline && !got_chat {
            while let Ok(ev) = rx_b.try_recv() {
                if let MeshEvent::ChatItemReceived(item) = ev {
                    if matches!(item.content, ChatContentUi::Text(ref t) if t == "SPAKE2 code verified!")
                    {
                        got_chat = true;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            got_chat,
            "Joiner must decrypt chat encrypted with shared RoomCipher"
        );

        let _ = tx_a.send(MeshCommand::Shutdown).await;
        let _ = tx_b.send(MeshCommand::Shutdown).await;
        aud_a.shutdown();
        aud_b.shutdown();
        rv.stop().await;
    }

    #[tokio::test]
    async fn test_mesh_worker_x25519_friend_call_and_third_peer_party_invite() {
        let rv = start_embedded_rendezvous().await.unwrap();
        let rv_addr = format!("127.0.0.1:{}", rv.port);

        let (aud_a, tx_a, mut rx_a, uuid_a) = spawn_test_node(0xE1);
        let (aud_b, tx_b, mut rx_b, uuid_b) = spawn_test_node(0xE2);
        let (aud_c, tx_c, mut rx_c, uuid_c) = spawn_test_node(0xE3);

        let sk_a = StaticSecret::random_from_rng(OsRng);
        let sk_b = StaticSecret::random_from_rng(OsRng);
        let sk_c = StaticSecret::random_from_rng(OsRng);
        let pk_a = PublicKey::from(&sk_a).to_bytes();
        let pk_b = PublicKey::from(&sk_b).to_bytes();
        let pk_c = PublicKey::from(&sk_c).to_bytes();

        for (tx, nick, uuid, sk, friends) in [
            (&tx_a, "Alice", uuid_a, &sk_a, vec![pk_b, pk_c]),
            (&tx_b, "Bob", uuid_b, &sk_b, vec![pk_a, pk_c]),
            (&tx_c, "Charlie", uuid_c, &sk_c, vec![pk_a, pk_b]),
        ] {
            tx.send(MeshCommand::UpdatePresenceConfig {
                rendezvous_server: rv_addr.clone(),
                nickname: nick.to_string(),
                client_uuid: uuid,
                identity_secret: sk.to_bytes(),
                friend_pub_keys: friends,
            })
            .await
            .unwrap();
        }

        // 1. Wait until Alice observes Bob & Charlie online in RAM presence table
        let pres_deadline = Instant::now() + Duration::from_secs(5);
        let mut a_sees_both = false;
        while Instant::now() < pres_deadline && !a_sees_both {
            while let Ok(ev) = rx_a.try_recv() {
                if let MeshEvent::FriendsStatusUpdated(list) = ev {
                    let has_b = list
                        .iter()
                        .any(|f| f.pub_key == pk_b && f.online && !f.in_room);
                    let has_c = list
                        .iter()
                        .any(|f| f.pub_key == pk_c && f.online && !f.in_room);
                    if has_b && has_c {
                        a_sees_both = true;
                    }
                }
            }
            while rx_b.try_recv().is_ok() {}
            while rx_c.try_recv().is_ok() {}
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        assert!(
            a_sees_both,
            "Alice must see Bob and Charlie online via presence table"
        );

        // 2. Alice calls Bob in 1 click from lobby
        tx_a.send(MeshCommand::CallOrInviteFriend {
            target_pub_key: pk_b,
        })
        .await
        .unwrap();

        let call_deadline = Instant::now() + Duration::from_secs(4);
        let mut a_uri: Option<String> = None;
        let mut b_call: Option<IncomingFriendCallUi> = None;
        while Instant::now() < call_deadline && (a_uri.is_none() || b_call.is_none()) {
            while let Ok(ev) = rx_a.try_recv() {
                if let MeshEvent::JoinedRoom { invite_uri, .. } = ev {
                    a_uri = Some(invite_uri);
                }
            }
            while let Ok(ev) = rx_b.try_recv() {
                if let MeshEvent::IncomingFriendCall(call) = ev {
                    b_call = Some(call);
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let room_uri = a_uri.expect("Caller Alice must join created room");
        let b_call = b_call.expect("Callee Bob must receive decrypted IncomingFriendCall");
        assert_eq!(b_call.sender_pub_key, pk_a);
        assert_eq!(b_call.sender_nickname, "Alice");
        assert!(!b_call.is_party_invite);
        assert!(b_call.is_known_friend);
        assert_eq!(b_call.invite.to_uri(), room_uri);

        // Bob accepts call
        tx_b.send(MeshCommand::JoinRoom {
            rendezvous_server: rv_addr.clone(),
            invite: b_call.invite,
            nickname: "Bob".to_string(),
            client_uuid: uuid_b,
            initial_volumes: HashMap::new(),
            is_lan_room: false,
            room_name: None,
        })
        .await
        .unwrap();

        // 3. Alice invites Charlie into the already-active party
        tx_a.send(MeshCommand::CallOrInviteFriend {
            target_pub_key: pk_c,
        })
        .await
        .unwrap();

        let party_deadline = Instant::now() + Duration::from_secs(4);
        let mut a_sent_party = false;
        let mut c_call: Option<IncomingFriendCallUi> = None;
        while Instant::now() < party_deadline && (!a_sent_party || c_call.is_none()) {
            while let Ok(ev) = rx_a.try_recv() {
                if let MeshEvent::FriendInviteSent { target_pub_key } = ev {
                    if target_pub_key == pk_c {
                        a_sent_party = true;
                    }
                }
            }
            while let Ok(ev) = rx_c.try_recv() {
                if let MeshEvent::IncomingFriendCall(call) = ev {
                    c_call = Some(call);
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            a_sent_party,
            "Alice must receive FriendInviteSent ack for Charlie"
        );
        let c_call = c_call.expect("Charlie must receive E2EE party invite");
        assert!(
            c_call.is_party_invite,
            "Active-room invite must set is_party_invite"
        );
        assert_eq!(c_call.invite.to_uri(), room_uri);

        tx_c.send(MeshCommand::JoinRoom {
            rendezvous_server: rv_addr,
            invite: c_call.invite,
            nickname: "Charlie".to_string(),
            client_uuid: uuid_c,
            initial_volumes: HashMap::new(),
            is_lan_room: false,
            room_name: None,
        })
        .await
        .unwrap();

        let mesh_deadline = Instant::now() + Duration::from_secs(6);
        let (mut a_ok, mut b_ok, mut c_ok) = (false, false, false);
        let mut a_knows_bob = false;
        while Instant::now() < mesh_deadline && !(a_ok && b_ok && c_ok) {
            while let Ok(ev) = rx_a.try_recv() {
                if let MeshEvent::PeersUpdated(peers) = ev {
                    if peers.iter().any(|p| p.pub_key == Some(pk_b)) {
                        a_knows_bob = true;
                    }
                    if peers
                        .iter()
                        .filter(|p| p.status == PeerConnStatus::Connected)
                        .count()
                        >= 2
                    {
                        a_ok = true;
                    }
                }
            }
            while let Ok(ev) = rx_b.try_recv() {
                if let MeshEvent::PeersUpdated(peers) = ev {
                    if peers
                        .iter()
                        .filter(|p| p.status == PeerConnStatus::Connected)
                        .count()
                        >= 2
                    {
                        b_ok = true;
                    }
                }
            }
            while let Ok(ev) = rx_c.try_recv() {
                if let MeshEvent::PeersUpdated(peers) = ev {
                    if peers
                        .iter()
                        .filter(|p| p.status == PeerConnStatus::Connected)
                        .count()
                        >= 2
                    {
                        c_ok = true;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        assert!(
            a_ok && b_ok && c_ok,
            "All three peers must reach full Connected mesh after friend call + party invite"
        );

        tx_a.send(MeshCommand::SendChatText {
            text: "Three-peer party mesh connected!".to_string(),
        })
        .await
        .unwrap();
        let chat_deadline = Instant::now() + Duration::from_secs(3);
        let (mut b_got, mut c_got) = (false, false);
        while Instant::now() < chat_deadline && !(b_got && c_got) {
            while let Ok(ev) = rx_b.try_recv() {
                if let MeshEvent::ChatItemReceived(item) = ev {
                    if matches!(item.content, ChatContentUi::Text(ref t) if t == "Three-peer party mesh connected!")
                    {
                        b_got = true;
                    }
                }
            }
            while let Ok(ev) = rx_c.try_recv() {
                if let MeshEvent::ChatItemReceived(item) = ev {
                    if matches!(item.content, ChatContentUi::Text(ref t) if t == "Three-peer party mesh connected!")
                    {
                        c_got = true;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            b_got && c_got,
            "Both Bob and Charlie must decrypt Alice's party chat"
        );

        // 4. Alice must now know Bob & Charlie's pub_keys in the room and reject re-inviting Bob.
        let id_deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < id_deadline && !a_knows_bob {
            while let Ok(ev) = rx_a.try_recv() {
                if let MeshEvent::PeersUpdated(peers) = ev {
                    if peers.iter().any(|p| p.pub_key == Some(pk_b)) {
                        a_knows_bob = true;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            a_knows_bob,
            "Alice must receive Bob's E2EE IdentityAnnounce inside the room"
        );

        tx_a.send(MeshCommand::CallOrInviteFriend {
            target_pub_key: pk_b,
        })
        .await
        .unwrap();

        let dup_deadline = Instant::now() + Duration::from_millis(250);
        let mut a_got_error = false;
        let mut bob_got_dup_call = false;
        while Instant::now() < dup_deadline {
            while let Ok(ev) = rx_a.try_recv() {
                if matches!(ev, MeshEvent::Error(ref msg) if msg.contains("already in the room")) {
                    a_got_error = true;
                }
                assert!(
                    !matches!(ev, MeshEvent::FriendInviteSent { target_pub_key } if target_pub_key == pk_b),
                    "Alice must not emit FriendInviteSent for peer already in room"
                );
            }
            while let Ok(ev) = rx_b.try_recv() {
                if matches!(ev, MeshEvent::IncomingFriendCall(_)) {
                    bob_got_dup_call = true;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            a_got_error,
            "Alice must receive error when attempting to re-invite Bob who is already in room"
        );
        assert!(
            !bob_got_dup_call,
            "Bob must never receive duplicate call invite while already in the same room"
        );

        let _ = tx_a.send(MeshCommand::Shutdown).await;
        let _ = tx_b.send(MeshCommand::Shutdown).await;
        let _ = tx_c.send(MeshCommand::Shutdown).await;
        aud_a.shutdown();
        aud_b.shutdown();
        aud_c.shutdown();
        rv.stop().await;
    }

    async fn query_session_info(
        tx: &mpsc::Sender<MeshCommand>,
    ) -> Option<(SocketAddr, SocketAddr, RoomId, bool)> {
        let (reply_tx, mut reply_rx) = mpsc::channel(1);
        tx.send(MeshCommand::TestGetSessionInfo(reply_tx))
            .await
            .unwrap();
        reply_rx.recv().await.flatten()
    }

    #[tokio::test]
    async fn test_mesh_worker_locked_lan_room_spake2_auth_rate_limit_and_failover() {
        let (aud_a, tx_a, mut rx_a, uuid_a) = spawn_test_node(0xF1);
        let (aud_b, tx_b, mut rx_b, uuid_b) = spawn_test_node(0xF2);
        let (aud_c, tx_c, mut rx_c, uuid_c) = spawn_test_node(0xF3);
        let (aud_d, tx_d, mut rx_d, uuid_d) = spawn_test_node(0xF4);

        // 1. Host A creates a password-protected LAN room.
        tx_a.send(MeshCommand::HostLanRoom {
            room_name: "Private LAN".to_string(),
            password: Some("squad-lan-42".to_string()),
            nickname: "LanHostA".to_string(),
            client_uuid: uuid_a,
        })
        .await
        .unwrap();

        let join_deadline = Instant::now() + Duration::from_secs(4);
        let mut uri_a: Option<String> = None;
        while Instant::now() < join_deadline && uri_a.is_none() {
            while let Ok(ev) = rx_a.try_recv() {
                if let MeshEvent::JoinedRoom {
                    invite_uri,
                    is_lan_locked,
                    ..
                } = ev
                {
                    assert!(is_lan_locked, "Locked LAN host must set is_lan_locked=true");
                    uri_a = Some(invite_uri);
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let uri_a = uri_a.expect("Host A must emit JoinedRoom");
        let invite_a = InviteLink::parse(&uri_a).unwrap();

        let (rv_addr_a, auth_addr_a, announce_id_a, has_rv_a) = query_session_info(&tx_a)
            .await
            .expect("Host A active session");
        assert!(has_rv_a);
        assert_eq!(announce_id_a, derive_lan_announce_id(&invite_a));
        assert_ne!(announce_id_a, invite_a.room_id);

        // 2. Wire check: AnnounceLocked never leaks room_id or secret on broadcast.
        let announce_pkt = LanDiscoveryPacket::AnnounceLocked {
            announce_id: announce_id_a,
            rv_port: rv_addr_a.port(),
            auth_port: auth_addr_a.port(),
            peer_count: 1,
            max_peers: 10,
            room_name: "Private LAN".to_string(),
            host_nickname: "LanHostA".to_string(),
        };
        let announce_bytes = announce_pkt.encode().unwrap();
        assert!(!announce_bytes.windows(32).any(|w| w == invite_a.secret));
        assert!(!announce_bytes.windows(16).any(|w| w == invite_a.room_id));

        let locked_room = DiscoveredLanRoom {
            room_id: announce_id_a,
            secret: None,
            host_addr: rv_addr_a,
            auth_addr: Some(auth_addr_a),
            room_name: "Private LAN".to_string(),
            host_nickname: "LanHostA".to_string(),
            peer_count: 1,
            max_peers: 10,
        };
        assert!(locked_room.requires_password());

        // 3. Joiner B attempts wrong password (#1 from 127.0.0.1) -> must reject without joining.
        tx_b.send(MeshCommand::JoinLockedLanRoom {
            room: locked_room.clone(),
            password: "wrong-pw".to_string(),
            nickname: "JoinerB".to_string(),
            client_uuid: uuid_b,
        })
        .await
        .unwrap();

        let err_deadline = Instant::now() + Duration::from_secs(3);
        let mut wrong_err = None;
        while Instant::now() < err_deadline && wrong_err.is_none() {
            while let Ok(ev) = rx_b.try_recv() {
                match ev {
                    MeshEvent::LanAuthFailed(msg) => wrong_err = Some(msg),
                    MeshEvent::JoinedRoom { .. } => {
                        panic!("Wrong LAN password must never emit JoinedRoom");
                    }
                    _ => {}
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            wrong_err
                .as_deref()
                .is_some_and(|m| m.contains("Incorrect room password")),
            "Expected Incorrect room password, got {wrong_err:?}"
        );

        // 4. Joiner B unlocks with right password (#2 from 127.0.0.1) -> joins & exchanges encrypted chat.
        tx_b.send(MeshCommand::JoinLockedLanRoom {
            room: locked_room.clone(),
            password: "squad-lan-42".to_string(),
            nickname: "JoinerB".to_string(),
            client_uuid: uuid_b,
        })
        .await
        .unwrap();

        let connect_deadline = Instant::now() + Duration::from_secs(6);
        let mut b_joined = false;
        let mut a_connected = false;
        let mut b_connected = false;
        while Instant::now() < connect_deadline && !(b_joined && a_connected && b_connected) {
            while let Ok(ev) = rx_a.try_recv() {
                if let MeshEvent::PeersUpdated(peers) = ev {
                    if peers.iter().any(|p| p.status == PeerConnStatus::Connected) {
                        a_connected = true;
                    }
                }
            }
            while let Ok(ev) = rx_b.try_recv() {
                match ev {
                    MeshEvent::JoinedRoom {
                        invite_uri,
                        is_lan_locked,
                        ..
                    } => {
                        assert_eq!(invite_uri, uri_a);
                        assert!(is_lan_locked);
                        b_joined = true;
                    }
                    MeshEvent::PeersUpdated(peers)
                        if peers.iter().any(|p| p.status == PeerConnStatus::Connected) =>
                    {
                        b_connected = true;
                    }
                    _ => {}
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(b_joined && a_connected && b_connected);

        tx_a.send(MeshCommand::SendChatText {
            text: "Locked LAN chat ok!".to_string(),
        })
        .await
        .unwrap();
        let chat_deadline = Instant::now() + Duration::from_secs(3);
        let mut b_got_chat = false;
        while Instant::now() < chat_deadline && !b_got_chat {
            while let Ok(ev) = rx_b.try_recv() {
                if let MeshEvent::ChatItemReceived(item) = ev {
                    if matches!(item.content, ChatContentUi::Text(ref t) if t == "Locked LAN chat ok!")
                    {
                        b_got_chat = true;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(b_got_chat, "Joiner B must decrypt Host A chat");

        // 5. Rate limiting on Host A: 3 more wrong attempts (#3..#5) succeed in SPAKE2 fail, #6 is blocked.
        for attempt_idx in 3..=6 {
            tx_c.send(MeshCommand::JoinLockedLanRoom {
                room: locked_room.clone(),
                password: format!("bad-{attempt_idx}"),
                nickname: "AttackerC".to_string(),
                client_uuid: uuid_c,
            })
            .await
            .unwrap();

            let rl_deadline = Instant::now() + Duration::from_secs(3);
            let mut c_err = None;
            while Instant::now() < rl_deadline && c_err.is_none() {
                while let Ok(ev) = rx_c.try_recv() {
                    if let MeshEvent::LanAuthFailed(msg) = ev {
                        c_err = Some(msg);
                    }
                }
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
            let msg = c_err.expect("Expected LanAuthFailed for attempt");
            if attempt_idx <= 5 {
                assert!(
                    msg.contains("Incorrect room password"),
                    "Attempt #{attempt_idx} expected wrong password error, got: {msg}"
                );
            } else {
                assert!(
                    msg.contains("Too many wrong password attempts"),
                    "6th attempt must trigger rate limit, got: {msg}"
                );
            }
        }

        // 6. LAN Host Failover preserves password & announce_id on promoted Host B.
        tx_a.send(MeshCommand::LeaveRoom).await.unwrap();
        let promo_deadline = Instant::now() + Duration::from_secs(6);
        let mut promoted_info = None;
        while Instant::now() < promo_deadline && promoted_info.is_none() {
            if let Some((rv_b, auth_b, ann_b, true)) = query_session_info(&tx_b).await {
                promoted_info = Some((rv_b, auth_b, ann_b));
                break;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        let (rv_addr_b, auth_addr_b, announce_id_b) =
            promoted_info.expect("Peer B must promote to embedded LAN coordinator");
        assert_eq!(announce_id_b, announce_id_a);

        let migrated_room = DiscoveredLanRoom {
            room_id: announce_id_b,
            secret: None,
            host_addr: rv_addr_b,
            auth_addr: Some(auth_addr_b),
            room_name: "Private LAN".to_string(),
            host_nickname: "JoinerB".to_string(),
            peer_count: 1,
            max_peers: 10,
        };

        // Joiner D fails with wrong password against promoted Host B
        tx_d.send(MeshCommand::JoinLockedLanRoom {
            room: migrated_room.clone(),
            password: "wrong-after-failover".to_string(),
            nickname: "JoinerD".to_string(),
            client_uuid: uuid_d,
        })
        .await
        .unwrap();
        let d_err_deadline = Instant::now() + Duration::from_secs(3);
        let mut d_err = None;
        while Instant::now() < d_err_deadline && d_err.is_none() {
            while let Ok(ev) = rx_d.try_recv() {
                if let MeshEvent::LanAuthFailed(msg) = ev {
                    d_err = Some(msg);
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            d_err
                .as_deref()
                .is_some_and(|m| m.contains("Incorrect room password")),
            "Promoted Host B must still reject wrong password, got {d_err:?}"
        );

        // Joiner D succeeds with original password against promoted Host B
        tx_d.send(MeshCommand::JoinLockedLanRoom {
            room: migrated_room,
            password: "squad-lan-42".to_string(),
            nickname: "JoinerD".to_string(),
            client_uuid: uuid_d,
        })
        .await
        .unwrap();

        let d_join_deadline = Instant::now() + Duration::from_secs(6);
        let mut d_joined = false;
        let mut d_connected = false;
        while Instant::now() < d_join_deadline && !(d_joined && d_connected) {
            while let Ok(ev) = rx_d.try_recv() {
                match ev {
                    MeshEvent::JoinedRoom {
                        invite_uri,
                        is_lan_locked,
                        ..
                    } => {
                        assert_eq!(invite_uri, uri_a);
                        assert!(is_lan_locked);
                        d_joined = true;
                    }
                    MeshEvent::PeersUpdated(peers)
                        if peers.iter().any(|p| p.status == PeerConnStatus::Connected) =>
                    {
                        d_connected = true;
                    }
                    _ => {}
                }
            }
            while rx_b.try_recv().is_ok() {}
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(
            d_joined && d_connected,
            "Joiner D must unlock and connect to promoted LAN Host B"
        );

        tx_b.send(MeshCommand::SendChatText {
            text: "Survived locked LAN failover!".to_string(),
        })
        .await
        .unwrap();
        let d_chat_deadline = Instant::now() + Duration::from_secs(3);
        let mut d_got_chat = false;
        while Instant::now() < d_chat_deadline && !d_got_chat {
            while let Ok(ev) = rx_d.try_recv() {
                if let MeshEvent::ChatItemReceived(item) = ev {
                    if matches!(item.content, ChatContentUi::Text(ref t) if t == "Survived locked LAN failover!")
                    {
                        d_got_chat = true;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(d_got_chat, "Joiner D must decrypt promoted Host B chat");

        let _ = tx_a.send(MeshCommand::Shutdown).await;
        let _ = tx_b.send(MeshCommand::Shutdown).await;
        let _ = tx_c.send(MeshCommand::Shutdown).await;
        let _ = tx_d.send(MeshCommand::Shutdown).await;
        aud_a.shutdown();
        aud_b.shutdown();
        aud_c.shutdown();
        aud_d.shutdown();
    }

    #[test]
    fn test_resolve_host_default_port_and_explicit_port() {
        let bare_ip = resolve_host("127.0.0.1").expect("bare IP resolves");
        assert_eq!(bare_ip, "127.0.0.1:3478".parse().unwrap());

        let explicit = resolve_host("127.0.0.1:4567").expect("explicit port resolves");
        assert_eq!(explicit, "127.0.0.1:4567".parse().unwrap());

        let empty = resolve_host("   ");
        if let Ok(addr) = empty {
            assert_eq!(addr.port(), 3478);
        }
    }
}
