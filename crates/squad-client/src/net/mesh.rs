use crate::net::nat::discover_lan_ipv4_addrs;
use crate::net::transfer::{
    BlobTransferState, ChatItemUi, SwarmChatManager, TransferCtx, TransferEvent,
};
use squad_dsp::{AudioEngineHandle, OutboundAudioFrame, SoundCue};
use squad_proto::{
    ClientUuid, InviteLink, P2pHeader, P2pMsgType, P2pPayload, PeerId, PeerRecord, RoomCipher,
    SignalingPacket, MAX_UDP_PAYLOAD,
};
use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
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

#[derive(Debug, Clone)]
pub struct PeerUiSnapshot {
    pub peer_id: PeerId,
    pub client_uuid: ClientUuid,
    pub nickname: String,
    pub status: PeerConnStatus,
    pub rtt_ms: u32,
    pub loss_pct: f32,
    pub is_speaking: bool,
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
    Error(String),
}

struct RemotePeerState {
    record: PeerRecord,
    status: PeerConnStatus,
    active_addr: Option<SocketAddr>,
    punch_started_at: Instant,
    last_rx: Instant,
    rtt_ms: u32,
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
    history_synced: bool,
    transfer: SwarmChatManager,
}

pub fn spawn_mesh_worker(
    audio_engine: AudioEngineHandle,
    mut audio_out_rx: mpsc::Receiver<OutboundAudioFrame>,
) -> (mpsc::Sender<MeshCommand>, mpsc::Receiver<MeshEvent>) {
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<MeshCommand>(64);
    let (evt_tx, evt_rx) = mpsc::channel::<MeshEvent>(512);
    let (tr_tx, mut tr_rx) = mpsc::channel::<TransferEvent>(256);

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
        let bound_port = socket.local_addr().map(|a| a.port()).unwrap_or(0);

        let mut session: Option<ActiveSession> = None;
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
                            let lan_addrs = discover_lan_ipv4_addrs(bound_port);
                            let cipher = RoomCipher::from_secret(&invite.secret);
                            let room_prefix = invite.room_prefix();

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
                                history_synced: false,
                                transfer: SwarmChatManager::new(),
                            };

                            send_register(&socket, &new_sess).await;
                            session = Some(new_sess);
                            audio_engine.play_cue(SoundCue::Join);

                            let _ = initial_volumes;
                        }
                        MeshCommand::LeaveRoom => {
                            if let Some(sess) = session.take() {
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
                        MeshCommand::Shutdown => {
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

                    let (header, payload) = match audio_frame {
                        OutboundAudioFrame::Opus { seq_num, opus_bytes } => (
                            P2pHeader {
                                msg_type: P2pMsgType::AudioOpus,
                                room_prefix: sess.room_prefix,
                                sender_peer_id: sess.local_peer_id,
                                seq_num,
                            },
                            P2pPayload::AudioOpus {
                                opus_data: opus_bytes.to_vec(),
                            },
                        ),
                        OutboundAudioFrame::DtxKeepalive { seq_num } => (
                            P2pHeader {
                                msg_type: P2pMsgType::AudioDtxKeepalive,
                                room_prefix: sess.room_prefix,
                                sender_peer_id: sess.local_peer_id,
                                seq_num,
                            },
                            P2pPayload::AudioDtxKeepalive,
                        ),
                    };

                    if sess.cipher.seal_packet(&header, &payload, &mut seal_buf).is_ok() {
                        for peer in sess.peers.values() {
                            if let Some(dst) = peer.active_addr {
                                let _ = socket.send_to(&seal_buf, dst).await;
                            }
                        }
                    }
                }

                // Прием входящих UDP-датаграмм (от Rendezvous-сервера и от P2P-пиров)
                Ok((len, src_addr)) = socket.recv_from(&mut recv_buf) => {
                    let Some(sess) = session.as_mut() else { continue; };
                    let datagram = &mut recv_buf[..len];

                    if src_addr == sess.rendezvous_addr || is_signaling_opcode(datagram) {
                        handle_signaling_rx(datagram, sess, &audio_engine, &evt_tx).await;
                    } else {
                        handle_p2p_rx(
                            &socket,
                            datagram,
                            src_addr,
                            sess,
                            &audio_engine,
                            &mut seal_buf,
                            &tr_tx,
                        )
                        .await;
                        forward_transfer_events(&mut tr_rx, &evt_tx).await;
                    }
                }

                _ = qos_tick.tick() => {
                    let Some(sess) = session.as_mut() else { continue; };
                    if sess.local_peer_id == 0 {
                        continue;
                    }
                    let peer_addrs = connected_peer_map(&sess.peers);
                    let mut ctx = TransferCtx {
                        local_peer_id: sess.local_peer_id,
                        local_nickname: &sess.nickname,
                        socket: &socket,
                        cipher: &sess.cipher,
                        room_prefix: sess.room_prefix,
                        control_seq: &mut sess.control_seq,
                        seal_buf: &mut seal_buf,
                        evt_tx: &tr_tx,
                    };
                    sess.transfer.tick_10ms(&peer_addrs, &mut ctx).await;
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

                    for peer in sess.peers.values_mut() {
                        if peer.status == PeerConnStatus::Connected {
                            if now.duration_since(peer.last_rx) > PEER_STALE_TIMEOUT {
                                peer.status = PeerConnStatus::Punching;
                                peer.active_addr = None;
                                peer.punch_started_at = now;
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

            for id in stale {
                sess.peers.remove(&id);
                sess.transfer.remove_peer(id);
                audio_engine.remove_peer(id);
                audio_engine.play_cue(SoundCue::PeerLeave);
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

async fn handle_p2p_rx(
    socket: &UdpSocket,
    datagram: &mut [u8],
    src_addr: SocketAddr,
    sess: &mut ActiveSession,
    audio_engine: &AudioEngineHandle,
    seal_buf: &mut Vec<u8>,
    tr_tx: &mpsc::Sender<TransferEvent>,
) {
    let Ok((header, payload)) = sess.cipher.open_packet(datagram) else {
        debug!("Dropped unauthenticated P2P packet from {src_addr}");
        return;
    };

    if header.room_prefix != sess.room_prefix {
        return;
    }

    let now = Instant::now();
    let (sender_nickname, newly_connected) = {
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
        (peer.record.nickname.clone(), newly_connected)
    };

    if newly_connected {
        let mut ctx = TransferCtx {
            local_peer_id: sess.local_peer_id,
            local_nickname: &sess.nickname,
            socket,
            cipher: &sess.cipher,
            room_prefix: sess.room_prefix,
            control_seq: &mut sess.control_seq,
            seal_buf,
            evt_tx: tr_tx,
        };
        sess.transfer.announce_haves_to_peer(src_addr, &mut ctx).await;

        if !sess.history_synced {
            sess.history_synced = true;
            sess.transfer.send_history_sync_req(src_addr, &mut ctx).await;
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
                .seal_packet(&ack_hdr, &ack_body, seal_buf)
                .is_ok()
            {
                let _ = socket.send_to(seal_buf, src_addr).await;
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
                .seal_packet(&pong_hdr, &pong_body, seal_buf)
                .is_ok()
            {
                let _ = socket.send_to(seal_buf, src_addr).await;
            }
        }
        P2pPayload::Pong { echo_timestamp_us } => {
            if let Some(peer) = sess.peers.get_mut(&header.sender_peer_id) {
                update_rtt(&mut peer.rtt_ms, echo_timestamp_us);
            }
        }
        P2pPayload::AudioOpus { opus_data } => {
            audio_engine.push_remote_opus(header.sender_peer_id, header.seq_num, &opus_data);
        }
        P2pPayload::AudioDtxKeepalive => {}
        other => {
            let peers = connected_peer_list(&sess.peers);
            let mut ctx = TransferCtx {
                local_peer_id: sess.local_peer_id,
                local_nickname: &sess.nickname,
                socket,
                cipher: &sess.cipher,
                room_prefix: sess.room_prefix,
                control_seq: &mut sess.control_seq,
                seal_buf,
                evt_tx: tr_tx,
            };
            sess.transfer
                .handle_payload(
                    header.sender_peer_id,
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