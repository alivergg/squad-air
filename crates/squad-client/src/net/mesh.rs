use crate::net::nat::discover_lan_ipv4_addrs;
use crate::net::transfer::{
    app_data_root_dir, BlobTransferState, ChatItemUi, SwarmChatManager, TransferCtx,
    TransferEvent,
};
use crate::tray::release_os_memory;
use squad_dsp::{AudioEngineHandle, OutboundAudioFrame, SoundCue};
use squad_proto::{
    ClientUuid, InviteLink, P2pHeader, P2pMsgType, P2pPayload, PeerId, PeerRecord, RoomCipher,
    SignalingPacket, MAX_UDP_PAYLOAD,
};
use squad_video::{
    detect_hardware_encoder_backend, fragment_frame, DecoderError, FrameReassembler, H264Decoder,
    H264Encoder, ScreenCapturer, StreamConfig, VideoCodec, VideoPacketOut,
};
use std::collections::{HashMap, HashSet};
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

const REGISTER_INTERVAL: Duration = Duration::from_secs(2);
const PUNCH_INTERVAL: Duration = Duration::from_millis(60);
const PUNCH_TIMEOUT: Duration = Duration::from_secs(12);
const PING_INTERVAL: Duration = Duration::from_secs(1);
const PEER_STALE_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerConnStatus {
    Punching,
    Connected,
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

#[derive(Debug, Clone)]
pub struct DecodedVideoFrame {
    pub streamer_peer_id: PeerId,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct PeerUiSnapshot {
    pub peer_id: PeerId,
    pub client_uuid: ClientUuid,
    pub nickname: String,
    pub status: PeerConnStatus,
    pub rtt_ms: u32,
    pub loss_pct: f32,
    pub is_speaking: bool,
    pub stream_info: Option<RemoteStreamInfo>,
}

#[derive(Debug, Clone)]
pub enum MeshCommand {
    JoinRoom {
        rendezvous_server: String,
        invite: InviteLink,
        nickname: String,
        client_uuid: ClientUuid,
        initial_volumes: HashMap<ClientUuid, f32>,
    },
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
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum MeshEvent {
    JoinedRoom {
        local_peer_id: PeerId,
        invite_uri: String,
    },
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
    Error(String),
}

struct RemotePeerState {
    record: PeerRecord,
    status: PeerConnStatus,
    active_addr: Option<SocketAddr>,
    punch_started_at: Instant,
    last_rx: Instant,
    rtt_ms: u32,
    stream_info: Option<RemoteStreamInfo>,
}

struct CaptureWorkerHandle {
    stop: Arc<AtomicBool>,
    force_idr: Arc<AtomicBool>,
    preview_active: Arc<AtomicBool>,
    subs_active: Arc<AtomicBool>,
}

struct ActiveSession {
    rendezvous_addr: SocketAddr,
    invite: InviteLink,
    cipher: RoomCipher,
    room_prefix: [u8; 4],
    local_uuid: ClientUuid,
    nickname: String,
    local_peer_id: PeerId,
    lan_addrs: Vec<std::net::SocketAddrV4>,
    peers: HashMap<PeerId, RemotePeerState>,
    control_seq: u64,
    transfer: SwarmChatManager,
    local_encoder_backend: String,
    local_stream_config: Option<StreamConfig>,
    local_preview_enabled: bool,
    stream_subscribers: HashSet<PeerId>,
    next_video_frame_id: Arc<AtomicU32>,
    watched_peer_id: Option<PeerId>,
    sent_decoder_error_to: Option<PeerId>,
    video_reassembler: FrameReassembler,
    video_decoder: H264Decoder,
    last_pli_sent: Instant,
}

pub fn spawn_mesh_worker(
    audio_engine: AudioEngineHandle,
    mut audio_out_rx: mpsc::Receiver<OutboundAudioFrame>,
    video_frame_slot: Arc<Mutex<Option<DecodedVideoFrame>>>,
) -> (mpsc::Sender<MeshCommand>, mpsc::Receiver<MeshEvent>) {
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<MeshCommand>(64);
    let (evt_tx, evt_rx) = mpsc::channel::<MeshEvent>(512);
    let (tr_tx, mut tr_rx) = mpsc::channel::<TransferEvent>(256);
    let (video_out_tx, mut video_out_rx) = mpsc::channel::<Vec<VideoPacketOut>>(3);
    let local_encoder_backend = detect_hardware_encoder_backend();

    tokio::spawn(async move {
        let socket = match UdpSocket::bind("0.0.0.0:0").await {
            Ok(s) => s,
            Err(e) => {
                let _ = evt_tx
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
        let mut pending_local_avatar: Option<([u8; 32], Vec<u8>)> = None;
        let mut capture_worker: Option<CaptureWorkerHandle> = None;
        let mut recv_buf = vec![0u8; MAX_UDP_PAYLOAD + 64];
        let mut seal_buf = Vec::with_capacity(MAX_UDP_PAYLOAD);

        let mut register_tick = tokio::time::interval(REGISTER_INTERVAL);
        let mut punch_tick = tokio::time::interval(PUNCH_INTERVAL);
        let mut ping_tick = tokio::time::interval(PING_INTERVAL);
        let mut qos_tick = tokio::time::interval(Duration::from_millis(10));
        qos_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut ui_sync_tick = tokio::time::interval(Duration::from_millis(120));

        loop {
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
                        } => {
                            let rv_addr = match resolve_host(&rendezvous_server) {
                                Ok(a) => a,
                                Err(e) => {
                                    let _ = evt_tx.send(MeshEvent::Error(e)).await;
                                    continue;
                                }
                            };

                            audio_engine.clear_peers();
                            audio_engine.set_share_stream_audio(false);
                            let lan_addrs = discover_lan_ipv4_addrs(bound_port);
                            let cipher = RoomCipher::from_secret(&invite.secret);
                            let room_prefix = invite.room_prefix();

                            let mut transfer = SwarmChatManager::new();
                            transfer.set_initial_local_avatar(pending_local_avatar.clone());
                            transfer.set_room_persistence(invite.room_id, app_data_root_dir());
                            transfer.load_room_from_disk(client_uuid, &tr_tx).await;
                            forward_transfer_events(&mut tr_rx, &evt_tx).await;

                            let new_sess = ActiveSession {
                                rendezvous_addr: rv_addr,
                                invite: invite.clone(),
                                cipher,
                                room_prefix,
                                local_uuid: client_uuid,
                                nickname,
                                local_peer_id: 0,
                                lan_addrs,
                                peers: HashMap::new(),
                                control_seq: 1,
                                transfer,
                                local_encoder_backend: local_encoder_backend.clone(),
                                local_stream_config: None,
                                local_preview_enabled: false,
                                stream_subscribers: HashSet::new(),
                                next_video_frame_id: Arc::new(AtomicU32::new(1)),
                                watched_peer_id: None,
                                sent_decoder_error_to: None,
                                video_reassembler: FrameReassembler::new(),
                                video_decoder: H264Decoder::new(),
                                last_pli_sent: Instant::now() - Duration::from_secs(5),
                            };

                            send_register(&socket, &new_sess).await;
                            stop_capture_worker(&mut capture_worker);
                            *video_frame_slot.lock().unwrap() = None;
                            session = Some(new_sess);
                            audio_engine.play_cue(SoundCue::Join);

                            let _ = initial_volumes;
                        }
                        MeshCommand::LeaveRoom => {
                            stop_capture_worker(&mut capture_worker);
                            drain_video_out_rx(&mut video_out_rx);
                            audio_engine.set_share_stream_audio(false);
                            audio_engine.clear_stream_audio();
                            *video_frame_slot.lock().unwrap() = None;
                            if let Some(mut sess) = session.take() {
                                sess.local_preview_enabled = false;
                                if sess.local_stream_config.take().is_some() {
                                    broadcast_stream_announce(
                                        &socket,
                                        &mut sess,
                                        &mut seal_buf,
                                        false,
                                        None,
                                    )
                                    .await;
                                }
                                send_leave(&socket, &sess).await;
                                audio_engine.clear_peers();
                                audio_engine.play_cue(SoundCue::Leave);
                                let _ = evt_tx.send(MeshEvent::LeftRoom).await;
                            }
                        }
                        MeshCommand::SendChatText { text } => {
                            if let Some(sess) = session.as_mut() {
                                if sess.local_peer_id != 0 {
                                    let peers = connected_peer_list(&sess.peers);
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
                                if sess.local_peer_id != 0 {
                                    let peers = connected_peer_list(&sess.peers);
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
                                if sess.local_peer_id != 0 {
                                    let peers = connected_peer_list(&sess.peers);
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
                                    let peers = connected_peer_list(&sess.peers);
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
                                sess.stream_subscribers.clear();
                                sess.local_preview_enabled = false;
                                audio_engine.set_share_stream_audio(config.share_audio);
                                sess.local_stream_config = Some(config.clone());
                                if sess.local_peer_id != 0 {
                                    broadcast_stream_announce(
                                        &socket,
                                        sess,
                                        &mut seal_buf,
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
                                if sess.local_preview_enabled && sess.watched_peer_id.is_none() {
                                    *video_frame_slot.lock().unwrap() = None;
                                }
                                sess.local_preview_enabled = false;
                                sess.local_stream_config = None;
                                sess.stream_subscribers.clear();
                                if sess.local_peer_id != 0 {
                                    broadcast_stream_announce(
                                        &socket,
                                        sess,
                                        &mut seal_buf,
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
                                        true,
                                    );
                                } else {
                                    sess.local_preview_enabled = false;
                                    reconcile_capture_worker(
                                        sess,
                                        &mut capture_worker,
                                        &video_out_tx,
                                        &video_frame_slot,
                                        false,
                                    );
                                    if sess.watched_peer_id.is_none() {
                                        *video_frame_slot.lock().unwrap() = None;
                                    }
                                }
                            } else {
                                *video_frame_slot.lock().unwrap() = None;
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
                                        false,
                                    );
                                }
                                if let Some(prev_pid) = sess.watched_peer_id {
                                    if Some(prev_pid) != peer_id {
                                        send_stream_subscribe(
                                            &socket,
                                            sess,
                                            &mut seal_buf,
                                            prev_pid,
                                            false,
                                        )
                                        .await;
                                    }
                                }
                                sess.watched_peer_id = peer_id;
                                sess.sent_decoder_error_to = None;
                                sess.video_reassembler.reset();
                                sess.video_decoder.reset();
                                audio_engine.clear_stream_audio();
                                *video_frame_slot.lock().unwrap() = None;
                                if let Some(target_pid) = peer_id {
                                    let peer_uses_hevc = sess
                                        .peers
                                        .get(&target_pid)
                                        .and_then(|p| p.stream_info.as_ref())
                                        .is_some_and(|s| s.codec == VideoCodec::H265);
                                    if peer_uses_hevc && !sess.video_decoder.is_hevc_decode_supported()
                                    {
                                        sess.sent_decoder_error_to = Some(target_pid);
                                        send_stream_decoder_error(
                                            &socket,
                                            sess,
                                            &mut seal_buf,
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
                                        target_pid,
                                        true,
                                    )
                                    .await;
                                    send_stream_pli(&socket, sess, &mut seal_buf, target_pid).await;
                                    sess.last_pli_sent = Instant::now();
                                }
                            } else {
                                audio_engine.clear_stream_audio();
                                *video_frame_slot.lock().unwrap() = None;
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
                                        false,
                                    );
                                    sess.video_reassembler.reset_and_shrink();
                                    sess.video_decoder.reset_and_shrink();
                                    sess.transfer.trim_memory();
                                }
                                audio_engine.clear_stream_audio();
                                *video_frame_slot.lock().unwrap() = None;
                                drain_video_out_rx(&mut video_out_rx);
                                seal_buf.clear();
                                seal_buf.shrink_to(2048);
                                recv_buf = vec![0u8; MAX_UDP_PAYLOAD + 64];
                                release_os_memory();
                            }
                        }
                        MeshCommand::Shutdown => {
                            stop_capture_worker(&mut capture_worker);
                            audio_engine.set_share_stream_audio(false);
                            audio_engine.clear_stream_audio();
                            if let Some(sess) = session.take() {
                                send_leave(&socket, &sess).await;
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
                                for peer in sess.peers.values() {
                                    if let Some(dst) = peer.active_addr {
                                        let _ = socket.send_to(&seal_buf, dst).await;
                                    }
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
                                for peer in sess.peers.values() {
                                    if let Some(dst) = peer.active_addr {
                                        let _ = socket.send_to(&seal_buf, dst).await;
                                    }
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
                                        if let Some(dst) =
                                            sess.peers.get(&sub_id).and_then(|p| p.active_addr)
                                        {
                                            let _ = socket.send_to(&seal_buf, dst).await;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Прием входящих UDP-датаграмм (от Rendezvous-сервера и от P2P-пиров)
                Ok((len, src_addr)) = socket.recv_from(&mut recv_buf) => {
                    let Some(sess) = session.as_mut() else { continue; };
                    let datagram = &mut recv_buf[..len];

                    if src_addr == sess.rendezvous_addr || is_signaling_opcode(datagram) {
                        handle_signaling_rx(
                            datagram,
                            sess,
                            &audio_engine,
                            &evt_tx,
                            &mut capture_worker,
                            &video_out_tx,
                            &video_frame_slot,
                        )
                        .await;
                    } else {
                        let mut rx_ctx = MeshRxCtx {
                            audio_engine: &audio_engine,
                            seal_buf: &mut seal_buf,
                            tr_tx: &tr_tx,
                            evt_tx: &evt_tx,
                            capture_worker: &mut capture_worker,
                            video_out_tx: &video_out_tx,
                            video_frame_slot: &video_frame_slot,
                        };
                        handle_p2p_rx(&socket, datagram, src_addr, sess, &mut rx_ctx).await;
                        forward_transfer_events(&mut tr_rx, &evt_tx).await;
                    }
                }

                // Priority 2: video fragments only while subscribers exist (paced to prevent kernel drops).
                Some(frags) = video_out_rx.recv() => {
                    let Some(sess) = session.as_mut() else { continue; };
                    if sess.local_peer_id == 0 || sess.stream_subscribers.is_empty() {
                        continue;
                    }
                    let subs: Vec<PeerId> = sess.stream_subscribers.iter().copied().collect();
                    for (idx, frag) in frags.into_iter().enumerate() {
                        sess.control_seq = sess.control_seq.wrapping_add(1);
                        let hdr = P2pHeader {
                            msg_type: P2pMsgType::VideoFrameFrag,
                            room_prefix: sess.room_prefix,
                            sender_peer_id: sess.local_peer_id,
                            seq_num: sess.control_seq,
                        };
                        let payload = P2pPayload::VideoFrameFrag {
                            frame_id: frag.frame_id,
                            frag_idx: frag.frag_idx,
                            frag_count: frag.frag_count,
                            flags: frag.flags,
                            data: frag.data,
                        };
                        if sess.cipher.seal_packet(&hdr, &payload, &mut seal_buf).is_ok() {
                            for &sub_id in &subs {
                                if let Some(dst) = sess.peers.get(&sub_id).and_then(|p| p.active_addr) {
                                    let _ = socket.send_to(&seal_buf, dst).await;
                                }
                            }
                        }
                        if idx > 0 && idx % 12 == 0 {
                            tokio::time::sleep(Duration::from_micros(250)).await;
                        }
                    }
                }

                _ = qos_tick.tick() => {
                    let Some(sess) = session.as_mut() else { continue; };
                    if sess.local_peer_id == 0 {
                        continue;
                    }
                    let peer_addrs = connected_peer_map(&sess.peers);
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
                    if let Some(sess) = session.as_ref() {
                        send_register(&socket, sess).await;
                    }
                }

                // Активный UDP Hole Punching (каждые 60 мс по внешнему и LAN адресам)
                _ = punch_tick.tick() => {
                    let Some(sess) = session.as_mut() else { continue; };
                    if sess.local_peer_id == 0 {
                        continue;
                    }
                    let now = Instant::now();
                    let now_us = now_micros();

                    for peer in sess.peers.values_mut() {
                        if peer.status == PeerConnStatus::Punching {
                            if now.duration_since(peer.punch_started_at) > PUNCH_TIMEOUT {
                                peer.status = PeerConnStatus::NatFailed;
                                continue;
                            }
                            sess.control_seq = sess.control_seq.wrapping_add(1);
                            let hdr = P2pHeader {
                                msg_type: P2pMsgType::PunchSyn,
                                room_prefix: sess.room_prefix,
                                sender_peer_id: sess.local_peer_id,
                                seq_num: sess.control_seq,
                            };
                            let payload = P2pPayload::PunchSyn {
                                client_uuid: sess.local_uuid,
                                timestamp_us: now_us,
                            };
                            if sess.cipher.seal_packet(&hdr, &payload, &mut seal_buf).is_ok() {
                                let _ = socket.send_to(&seal_buf, peer.record.public_addr).await;
                                for lan in &peer.record.lan_addrs {
                                    let _ = socket.send_to(&seal_buf, SocketAddr::V4(*lan)).await;
                                }
                            }
                        }
                    }
                }

                // Замер RTT (PING раз в 1 сек) и проверка таймаутов пиров
                _ = ping_tick.tick() => {
                    let Some(sess) = session.as_mut() else { continue; };
                    if sess.local_peer_id == 0 {
                        continue;
                    }
                    let now = Instant::now();
                    let now_us = now_micros();

                    let mut went_stale = Vec::new();
                    for (&pid, peer) in &mut sess.peers {
                        if peer.status == PeerConnStatus::Connected {
                            if now.duration_since(peer.last_rx) > PEER_STALE_TIMEOUT {
                                peer.status = PeerConnStatus::Punching;
                                peer.active_addr = None;
                                peer.punch_started_at = now;
                                went_stale.push(pid);
                                continue;
                            }
                            if let Some(dst) = peer.active_addr {
                                sess.control_seq = sess.control_seq.wrapping_add(1);
                                let hdr = P2pHeader {
                                    msg_type: P2pMsgType::Ping,
                                    room_prefix: sess.room_prefix,
                                    sender_peer_id: sess.local_peer_id,
                                    seq_num: sess.control_seq,
                                };
                                let payload = P2pPayload::Ping { timestamp_us: now_us };
                                if sess.cipher.seal_packet(&hdr, &payload, &mut seal_buf).is_ok() {
                                    let _ = socket.send_to(&seal_buf, dst).await;
                                }
                            }
                        }
                    }
                    if !went_stale.is_empty() {
                        let mut sub_changed = false;
                        for pid in went_stale {
                            if sess.stream_subscribers.remove(&pid) {
                                sub_changed = true;
                            }
                        }
                        if sub_changed {
                            reconcile_capture_worker(
                                sess,
                                &mut capture_worker,
                                &video_out_tx,
                                &video_frame_slot,
                                false,
                            );
                            let _ = evt_tx
                                .send(MeshEvent::LocalStreamStatus {
                                    is_sharing: sess.local_stream_config.is_some(),
                                    subscriber_count: sess.stream_subscribers.len(),
                                })
                                .await;
                        }
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
                                nickname: p.record.nickname.clone(),
                                status: p.status,
                                rtt_ms: p.rtt_ms,
                                loss_pct: dsp_stats.loss_pct,
                                is_speaking: dsp_stats.is_speaking,
                                stream_info: p.stream_info.clone(),
                            });
                        }
                        snapshots.sort_by_key(|s| s.peer_id);
                        let _ = evt_tx.try_send(MeshEvent::PeersUpdated(snapshots));
                    }
                }
            }
        }
    });

    (cmd_tx, evt_rx)
}

#[inline]
fn is_signaling_opcode(datagram: &[u8]) -> bool {
    datagram.len() >= 4 && matches!(datagram[3], 0x01..=0x04)
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

async fn handle_signaling_rx(
    datagram: &[u8],
    sess: &mut ActiveSession,
    audio_engine: &AudioEngineHandle,
    evt_tx: &mpsc::Sender<MeshEvent>,
    capture_worker: &mut Option<CaptureWorkerHandle>,
    video_out_tx: &mpsc::Sender<Vec<VideoPacketOut>>,
    video_frame_slot: &Arc<Mutex<Option<DecodedVideoFrame>>>,
) {
    let Ok(pkt) = SignalingPacket::decode(datagram) else {
        return;
    };

    match pkt {
        SignalingPacket::S2cRoomState {
            assigned_peer_id,
            peers,
        } => {
            if sess.local_peer_id == 0 {
                sess.local_peer_id = assigned_peer_id;
                info!("Joined room as peer #{}", assigned_peer_id);
                let _ = evt_tx
                    .send(MeshEvent::JoinedRoom {
                        local_peer_id: assigned_peer_id,
                        invite_uri: sess.invite.to_uri(),
                    })
                    .await;
            }

            let now = Instant::now();
            let mut active_ids = Vec::with_capacity(peers.len());

            for rec in peers {
                if rec.client_uuid == sess.local_uuid || rec.peer_id == sess.local_peer_id {
                    continue;
                }
                active_ids.push(rec.peer_id);

                if let Some(existing) = sess.peers.get_mut(&rec.peer_id) {
                    existing.record = rec;
                } else {
                    info!("Discovered peer #{} ({})", rec.peer_id, rec.nickname);
                    sess.peers.insert(
                        rec.peer_id,
                        RemotePeerState {
                            record: rec,
                            status: PeerConnStatus::Punching,
                            active_addr: None,
                            punch_started_at: now,
                            last_rx: now,
                            rtt_ms: 0,
                            stream_info: None,
                        },
                    );
                    audio_engine.play_cue(SoundCue::PeerJoin);
                }
            }

            // Удаляем вышедших пиров
            let stale: Vec<PeerId> = sess
                .peers
                .keys()
                .copied()
                .filter(|id| !active_ids.contains(id))
                .collect();

            let mut sub_changed = false;
            for id in stale {
                sess.peers.remove(&id);
                sess.transfer.remove_peer(id);
                if sess.stream_subscribers.remove(&id) {
                    sub_changed = true;
                }
                if sess.watched_peer_id == Some(id) {
                    sess.watched_peer_id = None;
                    sess.sent_decoder_error_to = None;
                    sess.video_reassembler.reset();
                    sess.video_decoder.reset();
                    audio_engine.clear_stream_audio();
                    *video_frame_slot.lock().unwrap() = None;
                    let _ = evt_tx
                        .send(MeshEvent::RemoteStreamUpdated {
                            peer_id: id,
                            stream: None,
                        })
                        .await;
                }
                audio_engine.remove_peer(id);
                audio_engine.play_cue(SoundCue::PeerLeave);
            }
            if sub_changed {
                reconcile_capture_worker(
                    sess,
                    capture_worker,
                    video_out_tx,
                    video_frame_slot,
                    false,
                );
                let _ = evt_tx
                    .send(MeshEvent::LocalStreamStatus {
                        is_sharing: sess.local_stream_config.is_some(),
                        subscriber_count: sess.stream_subscribers.len(),
                    })
                    .await;
            }
        }
        SignalingPacket::S2cError { code } => {
            warn!("Rendezvous signaling error: {code:?}");
            let _ = evt_tx
                .send(MeshEvent::Error(format!("Server error: {code:?}")))
                .await;
        }
        _ => {}
    }
}

struct MeshRxCtx<'a> {
    audio_engine: &'a AudioEngineHandle,
    seal_buf: &'a mut Vec<u8>,
    tr_tx: &'a mpsc::Sender<TransferEvent>,
    evt_tx: &'a mpsc::Sender<MeshEvent>,
    capture_worker: &'a mut Option<CaptureWorkerHandle>,
    video_out_tx: &'a mpsc::Sender<Vec<VideoPacketOut>>,
    video_frame_slot: &'a Arc<Mutex<Option<DecodedVideoFrame>>>,
}

async fn handle_p2p_rx(
    socket: &UdpSocket,
    datagram: &mut [u8],
    src_addr: SocketAddr,
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

    let now = Instant::now();
    let (sender_uuid, sender_nickname, newly_connected) = {
        let Some(peer) = sess.peers.get_mut(&header.sender_peer_id) else {
            return;
        };

        peer.last_rx = now;
        let newly_connected = peer.status != PeerConnStatus::Connected;
        if newly_connected {
            peer.status = PeerConnStatus::Connected;
            peer.active_addr = Some(src_addr);
            info!(
                "Hole punch succeeded with peer #{} ({}) at {}",
                peer.record.peer_id, peer.record.nickname, src_addr
            );
        } else if peer.active_addr.is_none() {
            peer.active_addr = Some(src_addr);
        }
        (peer.record.client_uuid, peer.record.nickname.clone(), newly_connected)
    };

    if newly_connected {
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
        sess.transfer.announce_haves_to_peer(src_addr, &mut ctx).await;
        sess.transfer.announce_avatar_to_peer(src_addr, &mut ctx).await;
        sess.transfer.send_history_sync_req(src_addr, &mut ctx).await;
        if let Some(cfg) = sess.local_stream_config.clone() {
            send_stream_announce_to_addr(socket, src_addr, sess, io.seal_buf, true, Some(&cfg))
                .await;
        }
    }

    match payload {
        P2pPayload::PunchSyn { timestamp_us, .. } => {
            if let Some(peer) = sess.peers.get_mut(&header.sender_peer_id) {
                peer.active_addr = Some(src_addr);
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
                let _ = socket.send_to(io.seal_buf, src_addr).await;
            }
        }
        P2pPayload::PunchAck {
            echo_timestamp_us, ..
        } => {
            if let Some(peer) = sess.peers.get_mut(&header.sender_peer_id) {
                peer.active_addr = Some(src_addr);
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
                let _ = socket.send_to(io.seal_buf, src_addr).await;
            }
        }
        P2pPayload::Pong { echo_timestamp_us } => {
            if let Some(peer) = sess.peers.get_mut(&header.sender_peer_id) {
                update_rtt(&mut peer.rtt_ms, echo_timestamp_us);
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
                && !sess.video_decoder.is_hevc_decode_supported()
                && sess.sent_decoder_error_to != Some(header.sender_peer_id)
            {
                sess.sent_decoder_error_to = Some(header.sender_peer_id);
                send_stream_decoder_error(
                    socket,
                    sess,
                    io.seal_buf,
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
                sess.video_decoder.reset();
                io.audio_engine.clear_stream_audio();
                *io.video_frame_slot.lock().unwrap() = None;
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
                if need_pli && now.duration_since(sess.last_pli_sent) > Duration::from_millis(450) {
                    send_stream_pli(socket, sess, io.seal_buf, header.sender_peer_id).await;
                    sess.last_pli_sent = now;
                }
                if let Some(reassembled) = frame_opt {
                    match sess.video_decoder.decode_checked(&reassembled.annex_b_bytes) {
                        Ok(Some(raw_rgba)) => {
                            *io.video_frame_slot.lock().unwrap() = Some(DecodedVideoFrame {
                                streamer_peer_id: header.sender_peer_id,
                                width: raw_rgba.width,
                                height: raw_rgba.height,
                                rgba: raw_rgba.rgba,
                            });
                        }
                        Ok(None) => {}
                        Err(DecoderError::UnsupportedCodec(reason)) => {
                            sess.video_decoder.reset();
                            sess.video_reassembler.reset();
                            if sess.sent_decoder_error_to != Some(header.sender_peer_id) {
                                sess.sent_decoder_error_to = Some(header.sender_peer_id);
                                send_stream_decoder_error(
                                    socket,
                                    sess,
                                    io.seal_buf,
                                    header.sender_peer_id,
                                    VideoCodec::H265.as_u8(),
                                    &reason,
                                )
                                .await;
                            }
                        }
                        Err(DecoderError::CorruptBitstream) => {
                            sess.video_reassembler.reset();
                            if now.duration_since(sess.last_pli_sent) > Duration::from_millis(450) {
                                send_stream_pli(socket, sess, io.seal_buf, header.sender_peer_id)
                                    .await;
                                sess.last_pli_sent = now;
                            }
                        }
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
                            true,
                        );
                        broadcast_stream_announce(
                            socket,
                            sess,
                            io.seal_buf,
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
            let peers = connected_peer_list(&sess.peers);
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
                    src_addr,
                    other,
                    &peers,
                    &mut ctx,
                )
                .await;
        }
    }
}

fn connected_peer_list(peers: &HashMap<PeerId, RemotePeerState>) -> Vec<(PeerId, SocketAddr)> {
    peers
        .iter()
        .filter_map(|(&pid, st)| {
            if st.status == PeerConnStatus::Connected {
                st.active_addr.map(|addr| (pid, addr))
            } else {
                None
            }
        })
        .collect()
}

fn connected_peer_map(peers: &HashMap<PeerId, RemotePeerState>) -> HashMap<PeerId, SocketAddr> {
    peers
        .iter()
        .filter_map(|(&pid, st)| {
            if st.status == PeerConnStatus::Connected {
                st.active_addr.map(|addr| (pid, addr))
            } else {
                None
            }
        })
        .collect()
}

async fn forward_transfer_events(
    tr_rx: &mut mpsc::Receiver<TransferEvent>,
    evt_tx: &mpsc::Sender<MeshEvent>,
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
        let _ = evt_tx.send(mapped).await;
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
    host_port
        .to_socket_addrs()
        .map_err(|e| format!("Failed to resolve '{host_port}': {e}"))?
        .next()
        .ok_or_else(|| format!("No IP addresses found for '{host_port}'"))
}

fn stop_capture_worker(handle: &mut Option<CaptureWorkerHandle>) {
    if let Some(worker) = handle.take() {
        worker.stop.store(true, Ordering::Release);
    }
}

fn drain_video_out_rx(rx: &mut mpsc::Receiver<Vec<VideoPacketOut>>) {
    while rx.try_recv().is_ok() {}
}

fn reconcile_capture_worker(
    sess: &ActiveSession,
    capture_handle: &mut Option<CaptureWorkerHandle>,
    video_out_tx: &mpsc::Sender<Vec<VideoPacketOut>>,
    video_frame_slot: &Arc<Mutex<Option<DecodedVideoFrame>>>,
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
    let stop_clone = Arc::clone(&stop);
    let force_clone = Arc::clone(&force_idr_flag);
    let preview_clone = Arc::clone(&preview_flag);
    let subs_clone = Arc::clone(&subs_flag);
    let frame_counter = Arc::clone(&sess.next_video_frame_id);
    let worker_cfg = cfg.clone();
    let tx = video_out_tx.clone();
    let slot_clone = Arc::clone(video_frame_slot);
    let local_pid = sess.local_peer_id;

    std::thread::Builder::new()
        .name("squad-video-capture".into())
        .spawn(move || {
            let mut capturer = ScreenCapturer::new(worker_cfg.clone());
            let mut encoder = H264Encoder::new_with_codec(
                worker_cfg.fps.as_u8(),
                worker_cfg.bitrate_bps,
                worker_cfg.codec,
            );
            let frame_interval = Duration::from_secs_f64(1.0 / f64::from(worker_cfg.fps.as_u8()));

            while !stop_clone.load(Ordering::Acquire) {
                let tick_start = Instant::now();
                let raw = capturer.capture_frame();

                if preview_clone.load(Ordering::Acquire) {
                    if let Ok(mut guard) = slot_clone.try_lock() {
                        *guard = Some(DecodedVideoFrame {
                            streamer_peer_id: local_pid,
                            width: raw.width,
                            height: raw.height,
                            rgba: if subs_clone.load(Ordering::Acquire) {
                                raw.rgba.clone()
                            } else {
                                raw.rgba.clone()
                            },
                        });
                    }
                }

                if subs_clone.load(Ordering::Acquire) {
                    if force_clone.swap(false, Ordering::AcqRel) {
                        encoder.force_keyframe();
                    }
                    let encoded = encoder.encode(&raw);
                    let frame_id = frame_counter.fetch_add(1, Ordering::Relaxed);
                    let frags = fragment_frame(frame_id, encoded.is_keyframe, &encoded.annex_b);
                    let _ = tx.try_send(frags);
                }

                let elapsed = tick_start.elapsed();
                if elapsed < frame_interval {
                    std::thread::sleep(frame_interval - elapsed);
                }
            }
        })
        .ok();

    *capture_handle = Some(CaptureWorkerHandle {
        stop,
        force_idr: force_idr_flag,
        preview_active: preview_flag,
        subs_active: subs_flag,
    });
}

async fn broadcast_stream_announce(
    socket: &UdpSocket,
    sess: &mut ActiveSession,
    seal_buf: &mut Vec<u8>,
    is_streaming: bool,
    config: Option<&StreamConfig>,
) {
    let addrs: Vec<SocketAddr> = sess
        .peers
        .values()
        .filter(|p| p.status == PeerConnStatus::Connected)
        .filter_map(|p| p.active_addr)
        .collect();
    for addr in addrs {
        send_stream_announce_to_addr(socket, addr, sess, seal_buf, is_streaming, config).await;
    }
}

async fn send_stream_announce_to_addr(
    socket: &UdpSocket,
    dst: SocketAddr,
    sess: &mut ActiveSession,
    seal_buf: &mut Vec<u8>,
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
        let _ = socket.send_to(seal_buf, dst).await;
    }
}

async fn send_stream_subscribe(
    socket: &UdpSocket,
    sess: &mut ActiveSession,
    seal_buf: &mut Vec<u8>,
    target_peer_id: PeerId,
    subscribe: bool,
) {
    let Some(dst) = sess
        .peers
        .get(&target_peer_id)
        .and_then(|p| p.active_addr)
    else {
        return;
    };
    sess.control_seq = sess.control_seq.wrapping_add(1);
    let hdr = P2pHeader {
        msg_type: P2pMsgType::StreamSubscribe,
        room_prefix: sess.room_prefix,
        sender_peer_id: sess.local_peer_id,
        seq_num: sess.control_seq,
    };
    let payload = P2pPayload::StreamSubscribe { subscribe };
    if sess.cipher.seal_packet(&hdr, &payload, seal_buf).is_ok() {
        let _ = socket.send_to(seal_buf, dst).await;
    }
}

async fn send_stream_pli(
    socket: &UdpSocket,
    sess: &mut ActiveSession,
    seal_buf: &mut Vec<u8>,
    target_peer_id: PeerId,
) {
    let Some(dst) = sess
        .peers
        .get(&target_peer_id)
        .and_then(|p| p.active_addr)
    else {
        return;
    };
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
        let _ = socket.send_to(seal_buf, dst).await;
    }
}

async fn send_stream_decoder_error(
    socket: &UdpSocket,
    sess: &mut ActiveSession,
    seal_buf: &mut Vec<u8>,
    target_peer_id: PeerId,
    failed_codec: u8,
    reason: &str,
) {
    let Some(dst) = sess
        .peers
        .get(&target_peer_id)
        .and_then(|p| p.active_addr)
    else {
        return;
    };
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
        let _ = socket.send_to(seal_buf, dst).await;
    }
}