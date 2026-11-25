use squad_core::crypto_identity::{open_friend_invite, seal_friend_invite};
use squad_core::net::{
    spawn_mesh_worker, DecodedVideoFrame, MeshCommand, MeshEvent, PeerConnStatus, RemoteStreamInfo,
};
use squad_dsp::{
    AudioEngineConfig, AudioEngineHandle, AudioMixer, EchoCanceller, JitterProfile,
    OpusEncoder10ms, OutboundAudioFrame, SoundCueBank, FRAME_SAMPLES, SAMPLE_RATE,
};
use squad_proto::{
    encode_relay_datagram_into, is_relay_datagram, parse_relay_header, InviteLink, P2pHeader,
    P2pMsgType, P2pPayload, PeerId, ReplayWindow, RoomCipher, SignalingPacket, MAX_UDP_PAYLOAD,
    RELAY_HEADER_LEN,
};
use squad_rendezvous::{
    run_server, run_server_with_config, RendezvousConfig, MAX_SERVER_RELAY_DATAGRAM_BYTES,
};
use squad_video::{CaptureTarget, StreamConfig, StreamFps, StreamResolution, VideoCodec};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, watch, Notify};
use x25519_dalek::{PublicKey, StaticSecret};

fn make_sine_frame(freq_hz: f32, amplitude: f32) -> [f32; FRAME_SAMPLES] {
    let mut frame = [0.0f32; FRAME_SAMPLES];
    for (i, sample) in frame.iter_mut().enumerate() {
        let t = i as f32 / SAMPLE_RATE as f32;
        *sample = (2.0 * std::f32::consts::PI * freq_hz * t).sin() * amplitude;
    }
    frame
}

fn frame_rms(frame: &[f32; FRAME_SAMPLES]) -> f32 {
    let sum_sq: f32 = frame.iter().map(|&s| s * s).sum();
    (sum_sq / FRAME_SAMPLES as f32).sqrt()
}

fn assert_synthetic_stream_info(info: &RemoteStreamInfo, expected_audio: bool) {
    assert_eq!(
        info.has_audio, expected_audio,
        "RemoteStreamInfo.has_audio mismatch"
    );
    assert_eq!(
        info.codec,
        VideoCodec::H264,
        "RemoteStreamInfo.codec mismatch"
    );
    assert_eq!(info.width, 640, "RemoteStreamInfo.width mismatch");
    assert_eq!(info.height, 360, "RemoteStreamInfo.height mismatch");
    assert_eq!(info.fps, 30, "RemoteStreamInfo.fps mismatch");
}

fn synthetic_stream_config(share_audio: bool) -> StreamConfig {
    StreamConfig {
        target: CaptureTarget::Monitor {
            id: u64::from(u32::MAX),
            name: "TestDisplay".to_string(),
            width: 640,
            height: 360,
            is_primary: true,
        },
        resolution: StreamResolution::R720p,
        fps: StreamFps::Fps30,
        bitrate_bps: 1_500_000,
        codec: VideoCodec::H264,
        share_audio,
    }
}

struct TestMeshNode {
    cmd_tx: mpsc::Sender<MeshCommand>,
    evt_rx: mpsc::Receiver<MeshEvent>,
    audio: AudioEngineHandle,
    video_slot: Arc<Mutex<Option<DecodedVideoFrame>>>,
    video_notify: Arc<Notify>,
}

impl TestMeshNode {
    fn spawn() -> Self {
        let (out_tx, out_rx) = mpsc::channel::<OutboundAudioFrame>(64);
        let audio = AudioEngineHandle::start(AudioEngineConfig::default(), out_tx)
            .expect("AudioEngineHandle::start should succeed in test");
        let video_slot = Arc::new(Mutex::new(None));
        let mesh_notify = Arc::new(Notify::new());
        let video_notify = Arc::new(Notify::new());
        let (cmd_tx, evt_rx) = spawn_mesh_worker(
            audio.clone(),
            out_rx,
            Arc::clone(&video_slot),
            mesh_notify,
            Arc::clone(&video_notify),
        );
        Self {
            cmd_tx,
            evt_rx,
            audio,
            video_slot,
            video_notify,
        }
    }

    async fn shutdown(self) {
        let _ = self.cmd_tx.send(MeshCommand::Shutdown).await;
        self.audio.shutdown();
    }
}

async fn wait_joined_room(node: &mut TestMeshNode, label: &str) -> PeerId {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        while let Ok(ev) = node.evt_rx.try_recv() {
            if let MeshEvent::JoinedRoom { local_peer_id, .. } = ev {
                return local_peer_id;
            }
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    panic!("{label} timed out waiting for JoinedRoom");
}

async fn wait_for_video_frame(
    node: &mut TestMeshNode,
    expected_streamer: PeerId,
    timeout: Duration,
) -> Option<DecodedVideoFrame> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        while let Ok(_ev) = node.evt_rx.try_recv() {}
        if let Ok(mut guard) = node.video_slot.lock() {
            if let Some(frame) = guard.take() {
                if frame.streamer_peer_id == expected_streamer
                    && frame.width > 0
                    && frame.height > 0
                    && !frame.rgba.is_empty()
                {
                    return Some(frame);
                }
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let _ = tokio::time::timeout(
            remaining.min(Duration::from_millis(50)),
            node.video_notify.notified(),
        )
        .await;
    }
    None
}

async fn spawn_relay_only_proxy(
    rv_addr: SocketAddr,
    block_direct: Arc<AtomicBool>,
) -> (SocketAddr, watch::Sender<bool>, tokio::task::JoinHandle<()>) {
    let sock = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind relay-only proxy socket");
    let proxy_addr = sock.local_addr().unwrap();
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    let handle = tokio::spawn(async move {
        let mut buf = vec![0u8; MAX_UDP_PAYLOAD + 256];
        let mut client_addr: Option<SocketAddr> = None;

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => break,
                res = sock.recv_from(&mut buf) => {
                    let Ok((len, from)) = res else { continue; };
                    let pkt = &buf[..len];

                    if from == rv_addr {
                        let Some(c_addr) = client_addr else { continue; };
                        if block_direct.load(Ordering::Acquire) {
                            if let Ok(SignalingPacket::S2cRoomState { assigned_peer_id, mut peers }) =
                                SignalingPacket::decode(pkt)
                            {
                                for p in &mut peers {
                                    p.public_addr = SocketAddr::from(([127, 0, 0, 1], 1));
                                    p.lan_addrs.clear();
                                }
                                if let Ok(rewritten) = (SignalingPacket::S2cRoomState {
                                    assigned_peer_id,
                                    peers,
                                })
                                .encode()
                                {
                                    let _ = sock.send_to(&rewritten, c_addr).await;
                                    continue;
                                }
                            }
                        }
                        let _ = sock.send_to(pkt, c_addr).await;
                    } else {
                        if let Ok(SignalingPacket::C2sRegister {
                            room_id,
                            client_uuid,
                            nickname,
                            lan_addrs: _,
                        }) = SignalingPacket::decode(pkt)
                        {
                            client_addr = Some(from);
                            if let Ok(rewritten) = (SignalingPacket::C2sRegister {
                                room_id,
                                client_uuid,
                                nickname,
                                lan_addrs: Vec::new(),
                            })
                            .encode()
                            {
                                let _ = sock.send_to(&rewritten, rv_addr).await;
                                continue;
                            }
                        }

                        if Some(from) == client_addr || is_relay_datagram(pkt) {
                            if client_addr.is_none() {
                                client_addr = Some(from);
                            }
                            if Some(from) == client_addr {
                                let _ = sock.send_to(pkt, rv_addr).await;
                            }
                        }
                        // Packets from other peers arriving at proxy_addr are dropped while
                        // block_direct is true (and C upgrades via S2cRoomState once lifted).
                    }
                }
            }
        }
    });

    (proxy_addr, shutdown_tx, handle)
}

async fn spawn_drop_first_subscribe_proxy(
    rv_addr: SocketAddr,
    dropped_subscribe_count: Arc<std::sync::atomic::AtomicUsize>,
) -> (
    SocketAddr,
    SocketAddr,
    watch::Sender<bool>,
    tokio::task::JoinHandle<()>,
) {
    let rv_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let rv_c = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let bridge_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let bridge_c = UdpSocket::bind("127.0.0.1:0").await.unwrap();

    let rv_a_addr = rv_a.local_addr().unwrap();
    let rv_c_addr = rv_c.local_addr().unwrap();
    let bridge_a_addr = bridge_a.local_addr().unwrap();
    let bridge_c_addr = bridge_c.local_addr().unwrap();

    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    let handle = tokio::spawn(async move {
        let mut buf_ra = vec![0u8; MAX_UDP_PAYLOAD + 256];
        let mut buf_rc = vec![0u8; MAX_UDP_PAYLOAD + 256];
        let mut buf_ba = vec![0u8; MAX_UDP_PAYLOAD + 256];
        let mut buf_bc = vec![0u8; MAX_UDP_PAYLOAD + 256];

        let mut addr_a: Option<SocketAddr> = None;
        let mut addr_c: Option<SocketAddr> = None;
        let mut dropped_once = false;

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => break,
                res = rv_a.recv_from(&mut buf_ra) => {
                    let Ok((len, from)) = res else { continue; };
                    let pkt = &buf_ra[..len];
                    if from == rv_addr {
                        let Some(dst) = addr_a else { continue; };
                        if let Ok(SignalingPacket::S2cRoomState { assigned_peer_id, mut peers }) =
                            SignalingPacket::decode(pkt)
                        {
                            for p in &mut peers {
                                p.public_addr = bridge_a_addr;
                                p.lan_addrs.clear();
                            }
                            if let Ok(rewritten) = (SignalingPacket::S2cRoomState {
                                assigned_peer_id,
                                peers,
                            })
                            .encode()
                            {
                                let _ = rv_a.send_to(&rewritten, dst).await;
                                continue;
                            }
                        }
                        let _ = rv_a.send_to(pkt, dst).await;
                    } else {
                        addr_a = Some(from);
                        if let Ok(SignalingPacket::C2sRegister {
                            room_id,
                            client_uuid,
                            nickname,
                            lan_addrs: _,
                        }) = SignalingPacket::decode(pkt)
                        {
                            if let Ok(rewritten) = (SignalingPacket::C2sRegister {
                                room_id,
                                client_uuid,
                                nickname,
                                lan_addrs: Vec::new(),
                            })
                            .encode()
                            {
                                let _ = rv_a.send_to(&rewritten, rv_addr).await;
                                continue;
                            }
                        }
                        let _ = rv_a.send_to(pkt, rv_addr).await;
                    }
                }
                res = rv_c.recv_from(&mut buf_rc) => {
                    let Ok((len, from)) = res else { continue; };
                    let pkt = &buf_rc[..len];
                    if from == rv_addr {
                        let Some(dst) = addr_c else { continue; };
                        if let Ok(SignalingPacket::S2cRoomState { assigned_peer_id, mut peers }) =
                            SignalingPacket::decode(pkt)
                        {
                            for p in &mut peers {
                                p.public_addr = bridge_c_addr;
                                p.lan_addrs.clear();
                            }
                            if let Ok(rewritten) = (SignalingPacket::S2cRoomState {
                                assigned_peer_id,
                                peers,
                            })
                            .encode()
                            {
                                let _ = rv_c.send_to(&rewritten, dst).await;
                                continue;
                            }
                        }
                        let _ = rv_c.send_to(pkt, dst).await;
                    } else {
                        addr_c = Some(from);
                        if let Ok(SignalingPacket::C2sRegister {
                            room_id,
                            client_uuid,
                            nickname,
                            lan_addrs: _,
                        }) = SignalingPacket::decode(pkt)
                        {
                            if let Ok(rewritten) = (SignalingPacket::C2sRegister {
                                room_id,
                                client_uuid,
                                nickname,
                                lan_addrs: Vec::new(),
                            })
                            .encode()
                            {
                                let _ = rv_c.send_to(&rewritten, rv_addr).await;
                                continue;
                            }
                        }
                        let _ = rv_c.send_to(pkt, rv_addr).await;
                    }
                }
                res = bridge_a.recv_from(&mut buf_ba) => {
                    let Ok((len, _)) = res else { continue; };
                    if let Some(dst_c) = addr_c {
                        let _ = bridge_c.send_to(&buf_ba[..len], dst_c).await;
                    }
                }
                res = bridge_c.recv_from(&mut buf_bc) => {
                    let Ok((len, _)) = res else { continue; };
                    let pkt = &buf_bc[..len];
                    if let Ok(hdr) = P2pHeader::decode(pkt) {
                        if hdr.msg_type == P2pMsgType::StreamSubscribe && !dropped_once {
                            dropped_once = true;
                            dropped_subscribe_count.fetch_add(1, Ordering::AcqRel);
                            continue;
                        }
                    }
                    if let Some(dst_a) = addr_a {
                        let _ = bridge_a.send_to(pkt, dst_a).await;
                    }
                }
            }
        }
    });

    (rv_a_addr, rv_c_addr, shutdown_tx, handle)
}

#[test]
fn t1_proto_stream_announce_server_relay_policy_and_size() {
    assert!(
        P2pMsgType::StreamAnnounce.is_server_relay_allowed(),
        "StreamAnnounce must be allowed over server UDP relay for late-joiner sync"
    );
    for blocked in [
        P2pMsgType::StreamSubscribe,
        P2pMsgType::StreamPli,
        P2pMsgType::VideoFrameFrag,
        P2pMsgType::StreamDecoderError,
        P2pMsgType::StreamAudioOpus,
    ] {
        assert!(
            !blocked.is_server_relay_allowed(),
            "{blocked:?} must remain blocked over server UDP relay"
        );
    }

    let invite = InviteLink::generate();
    let cipher = RoomCipher::from_secret(&invite.secret);
    let mut wire = Vec::new();

    let announce = P2pPayload::StreamAnnounce {
        is_streaming: true,
        codec: VideoCodec::H264.as_u8(),
        has_audio: true,
        width: 1920,
        height: 1080,
        fps: 60,
        source_title: "Primary Monitor (1920x1080)".to_string(),
        encoder_backend: "VideoToolbox".to_string(),
    };
    cipher
        .seal_packet(
            &P2pHeader {
                msg_type: announce.msg_type(),
                room_prefix: invite.room_prefix(),
                sender_peer_id: 1,
                seq_num: 1,
            },
            &announce,
            &mut wire,
        )
        .expect("seal StreamAnnounce");

    assert!(
        RELAY_HEADER_LEN + wire.len() <= MAX_SERVER_RELAY_DATAGRAM_BYTES,
        "Relayed StreamAnnounce ({}) must fit inside MAX_SERVER_RELAY_DATAGRAM_BYTES ({})",
        RELAY_HEADER_LEN + wire.len(),
        MAX_SERVER_RELAY_DATAGRAM_BYTES
    );

    let payloads = vec![
        announce,
        P2pPayload::StreamSubscribe { subscribe: true },
        P2pPayload::StreamPli,
        P2pPayload::VideoFrameFrag {
            frame_id: 7,
            frag_idx: 0,
            frag_count: 1,
            flags: squad_proto::VIDEO_FLAG_KEYFRAME,
            data: vec![0xAA, 0xBB, 0xCC],
        },
        P2pPayload::StreamDecoderError {
            failed_codec: VideoCodec::H265.as_u8(),
            reason: "HEVC unavailable".to_string(),
        },
        P2pPayload::StreamAudioOpus {
            opus_data: vec![0x01, 0x02, 0x03, 0x04],
        },
    ];

    for (idx, payload) in payloads.into_iter().enumerate() {
        let hdr = P2pHeader {
            msg_type: payload.msg_type(),
            room_prefix: invite.room_prefix(),
            sender_peer_id: 2,
            seq_num: idx as u64 + 10,
        };
        cipher.seal_packet(&hdr, &payload, &mut wire).unwrap();
        let (dec_hdr, dec_payload) = cipher.open_packet(&mut wire).unwrap();
        assert_eq!(dec_hdr, hdr);
        assert_eq!(dec_payload, payload);
    }
}

#[test]
fn t1_proto_invite_link_roundtrip_and_invalid_inputs() {
    let invite = InviteLink::generate();
    let uri = invite.to_uri();
    assert!(uri.starts_with("squad://"));

    let parsed = InviteLink::parse(&uri).expect("valid squad:// invite should parse");
    assert_eq!(parsed, invite);

    let legacy_uri = uri.replacen("squad://", "voice://", 1);
    let parsed_legacy =
        InviteLink::parse(&legacy_uri).expect("legacy voice:// invite should parse");
    assert_eq!(parsed_legacy, invite);

    let padded = format!("  {uri}  ");
    assert_eq!(InviteLink::parse(&padded).unwrap(), invite);

    let too_long = format!("squad://{}#{}", "1".repeat(130), "1".repeat(130));
    for invalid in [
        "",
        "   ",
        "http://room#secret",
        "squad://",
        "squad://missinghash",
        "squad://#onlysecret",
        "squad://onlyroom#",
        "squad://0OIl#1111",
        "squad://11#11",
        too_long.as_str(),
    ] {
        assert!(
            InviteLink::parse(invalid).is_err(),
            "expected invalid invite URI to fail: {invalid:?}"
        );
    }
}

#[test]
fn t1_dsp_aec_toggle_and_capture_passthrough() {
    let mut aec = EchoCanceller::new(false);
    assert!(!aec.is_enabled());

    let reference = make_sine_frame(440.0, 0.45);
    let mut capture = reference;
    aec.feed_render_frame(&reference);
    aec.process_capture_frame(&mut capture);

    for (i, (actual, expected)) in capture.iter().zip(reference.iter()).enumerate() {
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "disabled AEC modified sample {i}"
        );
    }

    aec.set_enabled(true);
    assert!(aec.is_enabled());
    let mut active_capture = reference;
    for _ in 0..5 {
        aec.feed_render_frame(&reference);
        aec.process_capture_frame(&mut active_capture);
    }
    assert!(active_capture.iter().all(|s| s.is_finite()));

    aec.set_enabled(false);
    assert!(!aec.is_enabled());
    let mut restored = reference;
    aec.process_capture_frame(&mut restored);
    for (i, (actual, expected)) in restored.iter().zip(reference.iter()).enumerate() {
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "re-disabled AEC modified sample {i}"
        );
    }
}

#[test]
fn t1_dsp_stream_audio_opus_encode_mix_and_deafen_gating() {
    let mut enc = OpusEncoder10ms::new_audio(96_000).expect("OpusEncoder10ms::new_audio");
    let tone = make_sine_frame(440.0, 0.5);

    let mut encoded_packets = Vec::new();
    for _ in 0..8 {
        if let Some(pkt) = enc.encode_frame(&tone).expect("encode_frame") {
            encoded_packets.push(pkt);
        }
    }
    assert!(
        !encoded_packets.is_empty(),
        "440Hz active tone should encode into non-DTX Opus packets"
    );

    let mut mixer = AudioMixer::new(SoundCueBank::load_optional(), JitterProfile::LanUltraLow);
    mixer.set_stream_gain(1.0);
    for (idx, pkt) in encoded_packets.into_iter().enumerate() {
        mixer.push_stream_opus(1, idx as u64 + 1, pkt);
    }

    let mut out = [0.0f32; FRAME_SAMPLES];
    let mut mixed_audible = false;
    for _ in 0..8 {
        if mixer.mix_frame(false, &mut out) && frame_rms(&out) > 0.01 {
            mixed_audible = true;
            break;
        }
    }
    assert!(
        mixed_audible,
        "stream Opus packets must mix into audible PCM when not deafened"
    );

    let mut deaf_enc = OpusEncoder10ms::new_audio(96_000).unwrap();
    let mut deaf_mixer = AudioMixer::new(SoundCueBank::load_optional(), JitterProfile::LanUltraLow);
    for seq in 1..=6u64 {
        if let Some(pkt) = deaf_enc.encode_frame(&tone).unwrap() {
            deaf_mixer.push_stream_opus(2, seq, pkt);
        }
    }
    let mut deaf_out = [1.0f32; FRAME_SAMPLES];
    let had_audio_when_deafened = deaf_mixer.mix_frame(true, &mut deaf_out);
    assert!(
        !had_audio_when_deafened,
        "deafened AudioMixer must suppress stream audio output"
    );
    assert!(deaf_out.iter().all(|&s| s == 0.0));
}

#[tokio::test]
async fn t1_dsp_aec_speaker_earpiece_headset_restore_sequence() {
    // Sequence 1: pre-speaker baseline OFF -> Speaker forces ON -> switch to Earpiece restores OFF
    let mut aec = EchoCanceller::new(false);
    let user_baseline_off = aec.is_enabled();
    assert!(!user_baseline_off);

    // Switch to speaker -> force AEC ON
    aec.set_enabled(true);
    assert!(aec.is_enabled());

    // Switch back to earpiece -> restore saved baseline (OFF)
    aec.set_enabled(user_baseline_off);
    assert!(!aec.is_enabled());

    // Sequence 2: pre-speaker baseline ON -> Speaker keeps ON -> switch to Wired Headset keeps ON
    aec.set_enabled(true);
    let user_baseline_on = aec.is_enabled();
    assert!(user_baseline_on);
    aec.set_enabled(true);
    assert!(aec.is_enabled());
    aec.set_enabled(user_baseline_on);
    assert!(aec.is_enabled());

    // Also exercise AudioEngineHandle AEC + route switch + None restore path with active streams
    let (tx, mut rx) = mpsc::channel(16);
    let handle = AudioEngineHandle::start(AudioEngineConfig::default(), tx)
        .expect("AudioEngineHandle::start");
    handle.set_streams_active(true);
    tokio::time::sleep(Duration::from_millis(70)).await;

    handle.set_aec_enabled(false);
    handle.set_audio_devices(Some("@id:17".to_string()), Some("@id:2".to_string()));
    tokio::time::sleep(Duration::from_millis(40)).await;
    handle.set_aec_enabled(true);
    handle.set_audio_devices(Some("@id:17".to_string()), None);
    tokio::time::sleep(Duration::from_millis(40)).await;
    handle.set_aec_enabled(false);
    handle.set_audio_devices(None, None);
    handle.set_deafened(true);
    assert!(handle.is_deafened());
    handle.set_deafened(false);
    assert!(!handle.is_deafened());
    handle.set_mic_muted(true);
    assert!(handle.is_mic_muted());
    handle.set_mic_muted(false);
    assert!(!handle.is_mic_muted());
    assert!(handle.device_list_gen() >= 1);
    assert!(handle.local_mic_level_db().is_finite());

    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(
        !matches!(rx.try_recv(), Err(mpsc::error::TryRecvError::Disconnected)),
        "AudioEngineHandle worker must remain alive across active AEC/route transitions"
    );
    handle.set_streams_active(false);
    tokio::time::sleep(Duration::from_millis(30)).await;
    handle.shutdown();
}

#[tokio::test]
async fn t2_rendezvous_relay_forwards_stream_announce_and_blocks_media() {
    let rv_sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let rv_addr = rv_sock.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let rv_task = tokio::spawn(run_server_with_config(
        Arc::clone(&rv_sock),
        RendezvousConfig { enable_relay: true },
        shutdown_rx,
    ));

    let invite = InviteLink::generate();
    let cipher = RoomCipher::from_secret(&invite.secret);
    let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();

    let reg_a = SignalingPacket::C2sRegister {
        room_id: invite.room_id,
        client_uuid: [0xA1; 16],
        nickname: "Alice".to_string(),
        lan_addrs: Vec::new(),
    };
    sock_a
        .send_to(&reg_a.encode().unwrap(), rv_addr)
        .await
        .unwrap();
    let mut rx_buf = [0u8; 1500];
    let (n, _) = tokio::time::timeout(Duration::from_secs(1), sock_a.recv_from(&mut rx_buf))
        .await
        .unwrap()
        .unwrap();
    let peer_a_id = match SignalingPacket::decode(&rx_buf[..n]).unwrap() {
        SignalingPacket::S2cRoomState {
            assigned_peer_id, ..
        } => assigned_peer_id,
        other => panic!("expected S2cRoomState for A, got {other:?}"),
    };

    let reg_b = SignalingPacket::C2sRegister {
        room_id: invite.room_id,
        client_uuid: [0xB2; 16],
        nickname: "Bob".to_string(),
        lan_addrs: Vec::new(),
    };
    sock_b
        .send_to(&reg_b.encode().unwrap(), rv_addr)
        .await
        .unwrap();
    let (n, _) = tokio::time::timeout(Duration::from_secs(1), sock_b.recv_from(&mut rx_buf))
        .await
        .unwrap()
        .unwrap();
    let peer_b_id = match SignalingPacket::decode(&rx_buf[..n]).unwrap() {
        SignalingPacket::S2cRoomState {
            assigned_peer_id, ..
        } => assigned_peer_id,
        other => panic!("expected S2cRoomState for B, got {other:?}"),
    };

    // Drain any initial room state pushed to A when B joined
    while tokio::time::timeout(Duration::from_millis(40), sock_a.recv_from(&mut rx_buf))
        .await
        .is_ok()
    {}

    let mut inner = Vec::new();
    let mut relay = Vec::new();

    // Blocked stream media/control payloads must be dropped by server relay
    let blocked_payloads = [
        P2pPayload::StreamAudioOpus {
            opus_data: vec![1, 2, 3, 4],
        },
        P2pPayload::StreamSubscribe { subscribe: true },
        P2pPayload::StreamPli,
        P2pPayload::VideoFrameFrag {
            frame_id: 1,
            frag_idx: 0,
            frag_count: 1,
            flags: 0,
            data: vec![9, 8, 7],
        },
    ];
    for (idx, payload) in blocked_payloads.into_iter().enumerate() {
        cipher
            .seal_packet(
                &P2pHeader {
                    msg_type: payload.msg_type(),
                    room_prefix: invite.room_prefix(),
                    sender_peer_id: peer_a_id,
                    seq_num: idx as u64 + 1,
                },
                &payload,
                &mut inner,
            )
            .unwrap();
        encode_relay_datagram_into(&invite.room_id, peer_b_id, &inner, &mut relay).unwrap();
        sock_a.send_to(&relay, rv_addr).await.unwrap();
    }

    assert!(
        tokio::time::timeout(Duration::from_millis(120), sock_b.recv_from(&mut rx_buf))
            .await
            .is_err(),
        "server relay must drop StreamAudioOpus, StreamSubscribe, StreamPli, and VideoFrameFrag"
    );

    // StreamAnnounce must be forwarded by server relay with sender peer_id rewritten to peer_a_id
    let announce = P2pPayload::StreamAnnounce {
        is_streaming: true,
        codec: VideoCodec::H264.as_u8(),
        has_audio: true,
        width: 1280,
        height: 720,
        fps: 30,
        source_title: "Desktop".to_string(),
        encoder_backend: "Software".to_string(),
    };
    cipher
        .seal_packet(
            &P2pHeader {
                msg_type: announce.msg_type(),
                room_prefix: invite.room_prefix(),
                sender_peer_id: peer_a_id,
                seq_num: 100,
            },
            &announce,
            &mut inner,
        )
        .unwrap();
    encode_relay_datagram_into(&invite.room_id, peer_b_id, &inner, &mut relay).unwrap();
    sock_a.send_to(&relay, rv_addr).await.unwrap();

    let (n, _) = tokio::time::timeout(Duration::from_secs(1), sock_b.recv_from(&mut rx_buf))
        .await
        .unwrap()
        .unwrap();
    let (relay_room, relay_sender, inner_slice) = parse_relay_header(&rx_buf[..n]).unwrap();
    assert_eq!(relay_room, invite.room_id);
    assert_eq!(relay_sender, peer_a_id);
    let mut inner_mut = inner_slice.to_vec();
    let (dec_hdr, dec_payload) = cipher.open_packet(&mut inner_mut).unwrap();
    assert_eq!(dec_hdr.msg_type, P2pMsgType::StreamAnnounce);
    assert_eq!(dec_payload, announce);

    let _ = shutdown_tx.send(true);
    let _ = rv_task.await;
}

#[tokio::test]
async fn t2_p2p_stream_audio_opus_loopback_and_replay_window() {
    let invite = InviteLink::generate();
    let cipher_tx = RoomCipher::from_secret(&invite.secret);
    let cipher_rx = RoomCipher::from_secret(&invite.secret);

    let sock_tx = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let sock_rx = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr_rx = sock_rx.local_addr().unwrap();

    let mut enc = OpusEncoder10ms::new_audio(96_000).unwrap();
    let tone = make_sine_frame(523.25, 0.55);

    let mut sent_seqs = Vec::new();
    let mut wire = Vec::new();
    let mut dup_wire = Vec::new();
    for seq in 1..=5u64 {
        let Some(opus_data) = enc.encode_frame(&tone).unwrap() else {
            continue;
        };
        let hdr = P2pHeader {
            msg_type: P2pMsgType::StreamAudioOpus,
            room_prefix: invite.room_prefix(),
            sender_peer_id: 1,
            seq_num: seq,
        };
        cipher_tx
            .seal_packet(&hdr, &P2pPayload::StreamAudioOpus { opus_data }, &mut wire)
            .unwrap();
        sock_tx.send_to(&wire, addr_rx).await.unwrap();
        if dup_wire.is_empty() {
            dup_wire = wire.clone();
        }
        sent_seqs.push(seq);
    }
    assert!(!sent_seqs.is_empty());
    // Send a duplicate replay packet
    sock_tx.send_to(&dup_wire, addr_rx).await.unwrap();

    let mut replay = ReplayWindow::new();
    let mut mixer = AudioMixer::new(SoundCueBank::load_optional(), JitterProfile::LanUltraLow);
    let mut buf = [0u8; 1500];
    let mut accepted = 0usize;
    let mut rejected = 0usize;

    for _ in 0..=sent_seqs.len() {
        let (n, _) = tokio::time::timeout(Duration::from_secs(1), sock_rx.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
        let (hdr, payload) = cipher_rx.open_packet(&mut buf[..n]).unwrap();
        if replay.check_and_accept(hdr.seq_num) {
            accepted += 1;
            if let P2pPayload::StreamAudioOpus { opus_data } = payload {
                mixer.push_stream_opus(hdr.sender_peer_id, hdr.seq_num, opus_data);
            }
        } else {
            rejected += 1;
        }
    }

    assert_eq!(accepted, sent_seqs.len());
    assert_eq!(
        rejected, 1,
        "duplicate seq_num must be rejected by ReplayWindow"
    );

    let mut out = [0.0f32; FRAME_SAMPLES];
    let mut audible = false;
    for _ in 0..8 {
        if mixer.mix_frame(false, &mut out) && frame_rms(&out) > 0.01 {
            audible = true;
            break;
        }
    }
    assert!(audible, "stream audio loopback must produce audible output");
}

#[tokio::test]
async fn t2_presence_and_friend_invite_x25519_delivery() {
    let rv_sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let rv_addr = rv_sock.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let rv_task = tokio::spawn(run_server(Arc::clone(&rv_sock), shutdown_rx));

    let sk_a = StaticSecret::from([0x11u8; 32]);
    let pk_a = PublicKey::from(&sk_a).to_bytes();
    let sk_b = StaticSecret::from([0x22u8; 32]);
    let pk_b = PublicKey::from(&sk_b).to_bytes();
    let sk_c = StaticSecret::from([0x33u8; 32]);

    let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();

    for (sock, pk, nick) in [(&sock_a, pk_a, "Alice"), (&sock_b, pk_b, "Bob")] {
        let ping = SignalingPacket::C2sPresencePing {
            pub_key: pk,
            nickname: nick.to_string(),
            in_room: false,
        };
        sock.send_to(&ping.encode().unwrap(), rv_addr)
            .await
            .unwrap();
    }

    let query = SignalingPacket::C2sQueryFriends {
        friend_pub_keys: vec![pk_b],
    };
    sock_a
        .send_to(&query.encode().unwrap(), rv_addr)
        .await
        .unwrap();
    let mut rx_buf = [0u8; 1500];
    let (n, _) = tokio::time::timeout(Duration::from_secs(1), sock_a.recv_from(&mut rx_buf))
        .await
        .unwrap()
        .unwrap();
    match SignalingPacket::decode(&rx_buf[..n]).unwrap() {
        SignalingPacket::S2cFriendsStatus { online_friends } => {
            assert_eq!(online_friends.len(), 1);
            assert_eq!(online_friends[0].pub_key, pk_b);
            assert_eq!(online_friends[0].nickname, "Bob");
            assert!(!online_friends[0].in_room);
        }
        other => panic!("expected S2cFriendsStatus, got {other:?}"),
    }

    let invite = InviteLink::generate();
    let (eph_pub, nonce, ciphertext) =
        seal_friend_invite(&sk_a, &pk_b, &invite, "Alice", true).unwrap();
    let relay_pkt = SignalingPacket::C2sRelayFriendInvite {
        sender_pub_key: pk_a,
        target_pub_key: pk_b,
        ephemeral_pub_key: eph_pub,
        nonce,
        ciphertext,
    };
    sock_a
        .send_to(&relay_pkt.encode().unwrap(), rv_addr)
        .await
        .unwrap();

    let (n, _) = tokio::time::timeout(Duration::from_secs(1), sock_b.recv_from(&mut rx_buf))
        .await
        .unwrap()
        .unwrap();
    match SignalingPacket::decode(&rx_buf[..n]).unwrap() {
        SignalingPacket::S2cIncomingFriendInvite {
            sender_pub_key,
            ephemeral_pub_key,
            nonce,
            ciphertext,
        } => {
            assert_eq!(sender_pub_key, pk_a);
            let dec = open_friend_invite(
                &sk_b,
                &sender_pub_key,
                &ephemeral_pub_key,
                &nonce,
                &ciphertext,
            )
            .expect("Bob should decrypt friend invite");
            assert_eq!(dec.invite, invite);
            assert_eq!(dec.sender_nickname, "Alice");
            assert!(dec.is_party_invite);

            assert!(
                open_friend_invite(
                    &sk_c,
                    &sender_pub_key,
                    &ephemeral_pub_key,
                    &nonce,
                    &ciphertext
                )
                .is_err(),
                "wrong StaticSecret must fail X25519 invite decryption"
            );
        }
        other => panic!("expected S2cIncomingFriendInvite, got {other:?}"),
    }

    let _ = shutdown_tx.send(true);
    let _ = rv_task.await;
}

#[tokio::test]
async fn t2_dsp_invalid_and_empty_device_ids_graceful_fallback() {
    let (tx, mut rx) = mpsc::channel(16);
    let handle = AudioEngineHandle::start(AudioEngineConfig::default(), tx)
        .expect("AudioEngineHandle::start");

    handle.set_streams_active(true);
    tokio::time::sleep(Duration::from_millis(70)).await;

    handle.set_audio_devices(Some(String::new()), Some(String::new()));
    tokio::time::sleep(Duration::from_millis(30)).await;
    handle.set_audio_devices(
        Some("@id:999999".to_string()),
        Some("@id:999999".to_string()),
    );
    tokio::time::sleep(Duration::from_millis(30)).await;
    handle.set_audio_devices(
        Some("nonexistent-device-id-xyz".to_string()),
        Some("nonexistent-device-id-xyz".to_string()),
    );
    tokio::time::sleep(Duration::from_millis(30)).await;
    handle.set_audio_devices(None, None);
    handle.set_aec_enabled(true);
    handle.set_aec_enabled(false);
    handle.set_mic_muted(true);
    assert!(handle.is_mic_muted());
    handle.set_mic_muted(false);
    assert!(!handle.is_mic_muted());
    assert!(handle.device_list_gen() >= 1);

    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(
        !matches!(rx.try_recv(), Err(mpsc::error::TryRecvError::Disconnected)),
        "AudioEngineHandle worker must survive invalid/empty device fallback while streams active"
    );
    handle.set_streams_active(false);
    tokio::time::sleep(Duration::from_millis(30)).await;
    handle.shutdown();
}

#[tokio::test]
async fn t3_late_joiner_receives_active_stream_and_idr_direct() {
    let rv_sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let rv_addr = rv_sock.local_addr().unwrap();
    let rv_str = rv_addr.to_string();
    let (rv_shutdown_tx, rv_shutdown_rx) = watch::channel(false);
    let rv_task = tokio::spawn(run_server_with_config(
        Arc::clone(&rv_sock),
        RendezvousConfig { enable_relay: true },
        rv_shutdown_rx,
    ));

    let invite = InviteLink::generate();
    let mut node_a = TestMeshNode::spawn();
    let mut node_b = TestMeshNode::spawn();

    for (node, uuid, nick) in [
        (&mut node_a, [0x01u8; 16], "PeerA"),
        (&mut node_b, [0x02u8; 16], "PeerB"),
    ] {
        node.cmd_tx
            .send(MeshCommand::JoinRoom {
                rendezvous_server: rv_str.clone(),
                invite: invite.clone(),
                nickname: nick.to_string(),
                client_uuid: uuid,
                initial_volumes: HashMap::new(),
                is_lan_room: false,
                room_name: None,
            })
            .await
            .unwrap();
    }

    let peer_a_id = wait_joined_room(&mut node_a, "PeerA").await;
    let _peer_b_id = wait_joined_room(&mut node_b, "PeerB").await;

    // Wait until B sees A connected before starting stream
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut b_connected = false;
    while Instant::now() < deadline && !b_connected {
        while let Ok(ev) = node_b.evt_rx.try_recv() {
            if let MeshEvent::PeersUpdated(peers) = ev {
                if peers
                    .iter()
                    .any(|p| p.peer_id == peer_a_id && p.status == PeerConnStatus::Connected)
                {
                    b_connected = true;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(b_connected, "PeerB must reach Connected with PeerA");

    node_a
        .cmd_tx
        .send(MeshCommand::StartScreenShare {
            config: synthetic_stream_config(true),
        })
        .await
        .unwrap();

    // B observes PeerA's stream announce
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut b_saw_stream = false;
    while Instant::now() < deadline && !b_saw_stream {
        while let Ok(ev) = node_b.evt_rx.try_recv() {
            match ev {
                MeshEvent::RemoteStreamUpdated {
                    peer_id,
                    stream: Some(ref info),
                } if peer_id == peer_a_id => {
                    assert_synthetic_stream_info(info, true);
                    b_saw_stream = true;
                }
                MeshEvent::PeersUpdated(peers) => {
                    if let Some(info) = peers
                        .iter()
                        .find(|p| p.peer_id == peer_a_id)
                        .and_then(|p| p.stream_info.as_ref())
                    {
                        assert_synthetic_stream_info(info, true);
                        b_saw_stream = true;
                    }
                }
                _ => {}
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(b_saw_stream, "PeerB must see PeerA's StreamAnnounce");

    node_b
        .cmd_tx
        .send(MeshCommand::WatchPeerStream {
            peer_id: Some(peer_a_id),
        })
        .await
        .unwrap();
    let frame_b = wait_for_video_frame(&mut node_b, peer_a_id, Duration::from_secs(5)).await;
    assert!(frame_b.is_some(), "PeerB must receive DecodedVideoFrame");

    // Now C joins late WITHOUT A restarting screen share
    let mut node_c = TestMeshNode::spawn();
    node_c
        .cmd_tx
        .send(MeshCommand::JoinRoom {
            rendezvous_server: rv_str,
            invite,
            nickname: "PeerC".to_string(),
            client_uuid: [0x03u8; 16],
            initial_volumes: HashMap::new(),
            is_lan_room: false,
            room_name: None,
        })
        .await
        .unwrap();
    let _peer_c_id = wait_joined_room(&mut node_c, "PeerC").await;

    let deadline = Instant::now() + Duration::from_secs(6);
    let mut c_saw_stream = false;
    while Instant::now() < deadline && !c_saw_stream {
        while let Ok(ev) = node_c.evt_rx.try_recv() {
            match ev {
                MeshEvent::RemoteStreamUpdated {
                    peer_id,
                    stream: Some(ref info),
                } if peer_id == peer_a_id => {
                    assert_synthetic_stream_info(info, true);
                    c_saw_stream = true;
                }
                MeshEvent::PeersUpdated(peers) => {
                    if let Some(info) = peers
                        .iter()
                        .find(|p| p.peer_id == peer_a_id)
                        .and_then(|p| p.stream_info.as_ref())
                    {
                        assert_synthetic_stream_info(info, true);
                        c_saw_stream = true;
                    }
                }
                _ => {}
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        c_saw_stream,
        "Late joiner PeerC must receive active stream state without restarting screen share"
    );

    node_c
        .cmd_tx
        .send(MeshCommand::WatchPeerStream {
            peer_id: Some(peer_a_id),
        })
        .await
        .unwrap();
    let frame_c = wait_for_video_frame(&mut node_c, peer_a_id, Duration::from_secs(5)).await;
    assert!(
        frame_c.is_some(),
        "Late joiner PeerC must receive DecodedVideoFrame after watching PeerA"
    );

    node_a.shutdown().await;
    node_b.shutdown().await;
    node_c.shutdown().await;
    let _ = rv_shutdown_tx.send(true);
    let _ = rv_task.await;
}

#[tokio::test]
async fn t3_late_joiner_stream_announce_over_server_relay_and_upgrade() {
    let rv_sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let rv_addr = rv_sock.local_addr().unwrap();
    let (rv_shutdown_tx, rv_shutdown_rx) = watch::channel(false);
    let rv_task = tokio::spawn(run_server_with_config(
        Arc::clone(&rv_sock),
        RendezvousConfig { enable_relay: true },
        rv_shutdown_rx,
    ));

    let block_direct = Arc::new(AtomicBool::new(true));
    let (proxy_addr, proxy_shutdown_tx, proxy_task) =
        spawn_relay_only_proxy(rv_addr, Arc::clone(&block_direct)).await;

    let invite = InviteLink::generate();
    let mut node_a = TestMeshNode::spawn();
    node_a
        .cmd_tx
        .send(MeshCommand::JoinRoom {
            rendezvous_server: rv_addr.to_string(),
            invite: invite.clone(),
            nickname: "StreamerA".to_string(),
            client_uuid: [0x11u8; 16],
            initial_volumes: HashMap::new(),
            is_lan_room: false,
            room_name: None,
        })
        .await
        .unwrap();
    let peer_a_id = wait_joined_room(&mut node_a, "StreamerA").await;

    node_a
        .cmd_tx
        .send(MeshCommand::StartScreenShare {
            config: synthetic_stream_config(true),
        })
        .await
        .unwrap();

    // Let capture worker warm up before late joiner C connects over Server Relay
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut node_c = TestMeshNode::spawn();
    node_c
        .cmd_tx
        .send(MeshCommand::JoinRoom {
            rendezvous_server: proxy_addr.to_string(),
            invite,
            nickname: "RelayedC".to_string(),
            client_uuid: [0x33u8; 16],
            initial_volumes: HashMap::new(),
            is_lan_room: false,
            room_name: None,
        })
        .await
        .unwrap();
    let _peer_c_id = wait_joined_room(&mut node_c, "RelayedC").await;

    // Wait for RELAY_FALLBACK_AFTER (3s) + Server Relay StreamAnnounce arrival
    let deadline = Instant::now() + Duration::from_secs(7);
    let mut c_is_relayed = false;
    let mut c_saw_stream_while_relayed = false;
    while Instant::now() < deadline && !(c_is_relayed && c_saw_stream_while_relayed) {
        while let Ok(ev) = node_c.evt_rx.try_recv() {
            match ev {
                MeshEvent::PeersUpdated(peers) => {
                    if let Some(p) = peers.iter().find(|p| p.peer_id == peer_a_id) {
                        if p.status == PeerConnStatus::Relayed {
                            c_is_relayed = true;
                        }
                        if let Some(ref info) = p.stream_info {
                            assert_synthetic_stream_info(info, true);
                            c_saw_stream_while_relayed = true;
                        }
                    }
                }
                MeshEvent::RemoteStreamUpdated {
                    peer_id,
                    stream: Some(ref info),
                } if peer_id == peer_a_id => {
                    assert_synthetic_stream_info(info, true);
                    c_saw_stream_while_relayed = true;
                }
                _ => {}
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    assert!(
        c_is_relayed,
        "PeerC and PeerA must fall back to PeerConnStatus::Relayed via Server Relay when direct UDP is blackholed"
    );
    assert!(
        c_saw_stream_while_relayed,
        "PeerC must receive StreamAnnounce over RelaySource::Server while PeerConnStatus::Relayed"
    );

    // Lift direct block and verify upgrade to Connected + DecodedVideoFrame delivery
    block_direct.store(false, Ordering::Release);
    let upgrade_deadline = Instant::now() + Duration::from_secs(6);
    let mut c_upgraded = false;
    while Instant::now() < upgrade_deadline && !c_upgraded {
        while let Ok(ev) = node_c.evt_rx.try_recv() {
            if let MeshEvent::PeersUpdated(peers) = ev {
                if peers
                    .iter()
                    .any(|p| p.peer_id == peer_a_id && p.status == PeerConnStatus::Connected)
                {
                    c_upgraded = true;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        c_upgraded,
        "PeerC must upgrade from Relayed to Connected once direct reachability is restored"
    );

    node_c
        .cmd_tx
        .send(MeshCommand::WatchPeerStream {
            peer_id: Some(peer_a_id),
        })
        .await
        .unwrap();
    let frame_c = wait_for_video_frame(&mut node_c, peer_a_id, Duration::from_secs(5)).await;
    assert!(
        frame_c.is_some(),
        "PeerC must receive DecodedVideoFrame after route upgrades to Connected"
    );

    node_a.shutdown().await;
    node_c.shutdown().await;
    let _ = proxy_shutdown_tx.send(true);
    let _ = proxy_task.await;
    let _ = rv_shutdown_tx.send(true);
    let _ = rv_task.await;
}

#[tokio::test]
async fn t3_late_joiner_recovers_from_dropped_stream_subscribe() {
    let rv_sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let rv_addr = rv_sock.local_addr().unwrap();
    let (rv_shutdown_tx, rv_shutdown_rx) = watch::channel(false);
    let rv_task = tokio::spawn(run_server_with_config(
        Arc::clone(&rv_sock),
        RendezvousConfig { enable_relay: true },
        rv_shutdown_rx,
    ));

    let dropped_sub_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (rv_a_addr, rv_c_addr, proxy_shutdown_tx, proxy_task) =
        spawn_drop_first_subscribe_proxy(rv_addr, Arc::clone(&dropped_sub_count)).await;

    let invite = InviteLink::generate();
    let mut node_a = TestMeshNode::spawn();
    node_a
        .cmd_tx
        .send(MeshCommand::JoinRoom {
            rendezvous_server: rv_a_addr.to_string(),
            invite: invite.clone(),
            nickname: "StreamerA".to_string(),
            client_uuid: [0x21u8; 16],
            initial_volumes: HashMap::new(),
            is_lan_room: false,
            room_name: None,
        })
        .await
        .unwrap();
    let peer_a_id = wait_joined_room(&mut node_a, "StreamerA").await;

    node_a
        .cmd_tx
        .send(MeshCommand::StartScreenShare {
            config: synthetic_stream_config(false),
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(250)).await;

    let mut node_c = TestMeshNode::spawn();
    node_c
        .cmd_tx
        .send(MeshCommand::JoinRoom {
            rendezvous_server: rv_c_addr.to_string(),
            invite,
            nickname: "ViewerC".to_string(),
            client_uuid: [0x22u8; 16],
            initial_volumes: HashMap::new(),
            is_lan_room: false,
            room_name: None,
        })
        .await
        .unwrap();
    let _peer_c_id = wait_joined_room(&mut node_c, "ViewerC").await;

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut c_ready = false;
    while Instant::now() < deadline && !c_ready {
        while let Ok(ev) = node_c.evt_rx.try_recv() {
            match ev {
                MeshEvent::RemoteStreamUpdated {
                    peer_id,
                    stream: Some(_),
                } if peer_id == peer_a_id => {
                    c_ready = true;
                }
                MeshEvent::PeersUpdated(peers)
                    if peers.iter().any(|p| {
                        p.peer_id == peer_a_id
                            && p.status == PeerConnStatus::Connected
                            && p.stream_info.is_some()
                    }) =>
                {
                    c_ready = true;
                }
                _ => {}
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        c_ready,
        "ViewerC must connect to StreamerA and see stream announce via bridge"
    );

    node_c
        .cmd_tx
        .send(MeshCommand::WatchPeerStream {
            peer_id: Some(peer_a_id),
        })
        .await
        .unwrap();

    let frame = wait_for_video_frame(&mut node_c, peer_a_id, Duration::from_secs(4)).await;
    assert!(
        dropped_sub_count.load(Ordering::Acquire) >= 1,
        "proxy must have intercepted and dropped the first StreamSubscribe packet"
    );
    assert!(
        frame.is_some(),
        "ViewerC must recover from dropped initial StreamSubscribe via retry and decode video frame"
    );

    node_a.shutdown().await;
    node_c.shutdown().await;
    let _ = proxy_shutdown_tx.send(true);
    let _ = proxy_task.await;
    let _ = rv_shutdown_tx.send(true);
    let _ = rv_task.await;
}

#[tokio::test]
async fn t3_friend_call_and_same_room_invite_suppression() {
    let rv_sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let rv_addr = rv_sock.local_addr().unwrap();
    let rv_str = rv_addr.to_string();
    let (rv_shutdown_tx, rv_shutdown_rx) = watch::channel(false);
    let rv_task = tokio::spawn(run_server(Arc::clone(&rv_sock), rv_shutdown_rx));

    let sec_a = [0x41u8; 32];
    let sec_b = [0x42u8; 32];
    let sec_c = [0x43u8; 32];
    let pk_a = PublicKey::from(&StaticSecret::from(sec_a)).to_bytes();
    let pk_b = PublicKey::from(&StaticSecret::from(sec_b)).to_bytes();
    let pk_c = PublicKey::from(&StaticSecret::from(sec_c)).to_bytes();

    let mut node_a = TestMeshNode::spawn();
    let mut node_b = TestMeshNode::spawn();
    let mut node_c = TestMeshNode::spawn();

    node_a
        .cmd_tx
        .send(MeshCommand::UpdatePresenceConfig {
            rendezvous_server: rv_str.clone(),
            nickname: "Alice".to_string(),
            client_uuid: [0xA1; 16],
            identity_secret: sec_a,
            friend_pub_keys: vec![pk_b, pk_c],
        })
        .await
        .unwrap();
    node_b
        .cmd_tx
        .send(MeshCommand::UpdatePresenceConfig {
            rendezvous_server: rv_str.clone(),
            nickname: "Bob".to_string(),
            client_uuid: [0xB2; 16],
            identity_secret: sec_b,
            friend_pub_keys: vec![pk_a],
        })
        .await
        .unwrap();
    node_c
        .cmd_tx
        .send(MeshCommand::UpdatePresenceConfig {
            rendezvous_server: rv_str.clone(),
            nickname: "Charlie".to_string(),
            client_uuid: [0xC3; 16],
            identity_secret: sec_c,
            friend_pub_keys: vec![pk_a],
        })
        .await
        .unwrap();

    // Wait until Alice sees Bob online
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut bob_online = false;
    while Instant::now() < deadline && !bob_online {
        while let Ok(ev) = node_a.evt_rx.try_recv() {
            if let MeshEvent::FriendsStatusUpdated(friends) = ev {
                if friends.iter().any(|f| f.pub_key == pk_b && f.online) {
                    bob_online = true;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(bob_online, "Alice must observe Bob online");

    // Alice calls Bob
    node_a
        .cmd_tx
        .send(MeshCommand::CallOrInviteFriend {
            target_pub_key: pk_b,
        })
        .await
        .unwrap();

    let mut bob_calls = Vec::new();
    let call_deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < call_deadline {
        while let Ok(ev) = node_b.evt_rx.try_recv() {
            if let MeshEvent::IncomingFriendCall(call) = ev {
                bob_calls.push(call);
            }
        }
        if !bob_calls.is_empty() {
            // Drain briefly to ensure no duplicate IncomingFriendCall arrives
            tokio::time::sleep(Duration::from_millis(100)).await;
            while let Ok(ev) = node_b.evt_rx.try_recv() {
                if let MeshEvent::IncomingFriendCall(call) = ev {
                    bob_calls.push(call);
                }
            }
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        bob_calls.len(),
        1,
        "Bob must receive single IncomingFriendCall"
    );
    let call_ab = bob_calls.remove(0);
    assert!(call_ab.is_known_friend);
    assert!(!call_ab.is_party_invite);

    // Alice and Bob join the room
    for (node, uuid, nick) in [
        (&mut node_a, [0xA1; 16], "Alice"),
        (&mut node_b, [0xB2; 16], "Bob"),
    ] {
        node.cmd_tx
            .send(MeshCommand::JoinRoom {
                rendezvous_server: rv_str.clone(),
                invite: call_ab.invite.clone(),
                nickname: nick.to_string(),
                client_uuid: uuid,
                initial_volumes: HashMap::new(),
                is_lan_room: false,
                room_name: None,
            })
            .await
            .unwrap();
    }
    wait_joined_room(&mut node_a, "Alice").await;
    wait_joined_room(&mut node_b, "Bob").await;

    // Wait for Alice to receive Bob's E2EE IdentityAnnounce (populating pub_key)
    let id_deadline = Instant::now() + Duration::from_secs(4);
    let mut a_knows_bob = false;
    while Instant::now() < id_deadline && !a_knows_bob {
        while let Ok(ev) = node_a.evt_rx.try_recv() {
            if let MeshEvent::PeersUpdated(peers) = ev {
                if peers.iter().any(|p| p.pub_key == Some(pk_b)) {
                    a_knows_bob = true;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(a_knows_bob, "Alice must receive Bob's IdentityAnnounce");

    // Alice invites Charlie into active party room
    node_a
        .cmd_tx
        .send(MeshCommand::CallOrInviteFriend {
            target_pub_key: pk_c,
        })
        .await
        .unwrap();

    let party_deadline = Instant::now() + Duration::from_secs(3);
    let mut c_party_call = None;
    while Instant::now() < party_deadline && c_party_call.is_none() {
        while let Ok(ev) = node_c.evt_rx.try_recv() {
            if let MeshEvent::IncomingFriendCall(call) = ev {
                c_party_call = Some(call);
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let c_call = c_party_call.expect("Charlie must receive party invite");
    assert!(c_call.is_party_invite);
    assert_eq!(c_call.invite, call_ab.invite);

    // Alice re-invites Bob who is already in the room -> MeshEvent::Error & zero duplicate calls
    node_a
        .cmd_tx
        .send(MeshCommand::CallOrInviteFriend {
            target_pub_key: pk_b,
        })
        .await
        .unwrap();

    let dup_deadline = Instant::now() + Duration::from_millis(300);
    let mut a_got_err = false;
    let mut b_got_dup = false;
    while Instant::now() < dup_deadline {
        while let Ok(ev) = node_a.evt_rx.try_recv() {
            if matches!(ev, MeshEvent::Error(ref msg) if msg.contains("already in the room")) {
                a_got_err = true;
            }
        }
        while let Ok(ev) = node_b.evt_rx.try_recv() {
            if matches!(ev, MeshEvent::IncomingFriendCall(_)) {
                b_got_dup = true;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(a_got_err, "Alice must receive Error when re-inviting Bob");
    assert!(
        !b_got_dup,
        "Bob must not receive duplicate IncomingFriendCall"
    );

    // Receiver-side same-room invite suppression: Charlie (in another session) sends a valid
    // X25519 invite for Bob's current room directly via rendezvous; Bob must suppress it.
    let (eph_pub, nonce, ciphertext) = seal_friend_invite(
        &StaticSecret::from(sec_c),
        &pk_b,
        &call_ab.invite,
        "Charlie",
        true,
    )
    .expect("Charlie should seal invite for Bob's room");
    let spoof_relay = SignalingPacket::C2sRelayFriendInvite {
        sender_pub_key: pk_c,
        target_pub_key: pk_b,
        ephemeral_pub_key: eph_pub,
        nonce,
        ciphertext,
    };
    let raw_c_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    raw_c_sock
        .send_to(&spoof_relay.encode().unwrap(), rv_addr)
        .await
        .unwrap();

    let rx_suppression_deadline = Instant::now() + Duration::from_millis(250);
    let mut b_got_receiver_dup = false;
    while Instant::now() < rx_suppression_deadline {
        while let Ok(ev) = node_b.evt_rx.try_recv() {
            if matches!(ev, MeshEvent::IncomingFriendCall(_)) {
                b_got_receiver_dup = true;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !b_got_receiver_dup,
        "Bob must suppress incoming friend invite for the room he is already in"
    );

    node_a.shutdown().await;
    node_b.shutdown().await;
    node_c.shutdown().await;
    let _ = rv_shutdown_tx.send(true);
    let _ = rv_task.await;
}

#[tokio::test]
async fn t4_audio_engine_route_switch_aec_and_stream_audio_lifecycle() {
    let (tx, mut rx) = mpsc::channel(32);
    let handle = AudioEngineHandle::start(AudioEngineConfig::default(), tx)
        .expect("AudioEngineHandle::start");

    handle.set_streams_active(true);
    tokio::time::sleep(Duration::from_millis(80)).await;

    handle.set_aec_enabled(true);
    handle.set_audio_devices(Some("@id:42".to_string()), Some("@id:7".to_string()));
    tokio::time::sleep(Duration::from_millis(40)).await;
    handle.set_audio_devices(None, None);
    tokio::time::sleep(Duration::from_millis(40)).await;
    handle.set_aec_enabled(false);

    let mut enc = OpusEncoder10ms::new_audio(96_000).unwrap();
    let tone = make_sine_frame(440.0, 0.5);
    handle.set_share_stream_audio(true);
    handle.set_stream_gain(0.8);
    for seq in 1..=5u64 {
        if let Some(opus) = enc.encode_frame(&tone).unwrap() {
            handle.push_remote_stream_opus(1, seq, &opus);
        }
    }
    handle.set_deafened(true);
    assert!(handle.is_deafened());
    handle.set_deafened(false);
    assert!(!handle.is_deafened());
    handle.set_mic_muted(true);
    assert!(handle.is_mic_muted());
    handle.set_mic_muted(false);
    assert!(!handle.is_mic_muted());
    assert!(handle.device_list_gen() >= 1);
    assert!(handle.local_mic_level_db().is_finite());

    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(
        !matches!(rx.try_recv(), Err(mpsc::error::TryRecvError::Disconnected)),
        "AudioEngineHandle worker must stay alive through active stream audio & route switches"
    );
    handle.clear_stream_audio();
    handle.set_share_stream_audio(false);
    handle.set_streams_active(false);
    tokio::time::sleep(Duration::from_millis(30)).await;
    handle.shutdown();
}

#[tokio::test]
async fn t4_stream_stop_clears_state_before_late_joiner() {
    let rv_sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let rv_addr = rv_sock.local_addr().unwrap();
    let rv_str = rv_addr.to_string();
    let (rv_shutdown_tx, rv_shutdown_rx) = watch::channel(false);
    let rv_task = tokio::spawn(run_server_with_config(
        Arc::clone(&rv_sock),
        RendezvousConfig { enable_relay: true },
        rv_shutdown_rx,
    ));

    let invite = InviteLink::generate();
    let mut node_a = TestMeshNode::spawn();
    let mut node_b = TestMeshNode::spawn();

    for (node, uuid, nick) in [
        (&mut node_a, [0x51u8; 16], "PeerA"),
        (&mut node_b, [0x52u8; 16], "PeerB"),
    ] {
        node.cmd_tx
            .send(MeshCommand::JoinRoom {
                rendezvous_server: rv_str.clone(),
                invite: invite.clone(),
                nickname: nick.to_string(),
                client_uuid: uuid,
                initial_volumes: HashMap::new(),
                is_lan_room: false,
                room_name: None,
            })
            .await
            .unwrap();
    }

    let peer_a_id = wait_joined_room(&mut node_a, "PeerA").await;
    wait_joined_room(&mut node_b, "PeerB").await;

    // Wait for B -> A Connected
    let conn_deadline = Instant::now() + Duration::from_secs(5);
    let mut connected = false;
    while Instant::now() < conn_deadline && !connected {
        while let Ok(ev) = node_b.evt_rx.try_recv() {
            if let MeshEvent::PeersUpdated(peers) = ev {
                if peers
                    .iter()
                    .any(|p| p.peer_id == peer_a_id && p.status == PeerConnStatus::Connected)
                {
                    connected = true;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(connected, "PeerB must connect to PeerA");

    node_a
        .cmd_tx
        .send(MeshCommand::StartScreenShare {
            config: synthetic_stream_config(true),
        })
        .await
        .unwrap();

    let start_deadline = Instant::now() + Duration::from_secs(5);
    let mut b_saw_start = false;
    while Instant::now() < start_deadline && !b_saw_start {
        while let Ok(ev) = node_b.evt_rx.try_recv() {
            if matches!(
                ev,
                MeshEvent::RemoteStreamUpdated {
                    peer_id,
                    stream: Some(_)
                } if peer_id == peer_a_id
            ) {
                b_saw_start = true;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(b_saw_start, "PeerB must observe StartScreenShare");

    node_a
        .cmd_tx
        .send(MeshCommand::StopScreenShare)
        .await
        .unwrap();

    let stop_deadline = Instant::now() + Duration::from_secs(5);
    let mut b_saw_stop = false;
    while Instant::now() < stop_deadline && !b_saw_stop {
        while let Ok(ev) = node_b.evt_rx.try_recv() {
            if matches!(
                ev,
                MeshEvent::RemoteStreamUpdated {
                    peer_id,
                    stream: None
                } if peer_id == peer_a_id
            ) {
                b_saw_stop = true;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        b_saw_stop,
        "PeerB must observe StopScreenShare (stream: None)"
    );

    // Late joiner C joins after StopScreenShare and must see stream_info == None
    let mut node_c = TestMeshNode::spawn();
    node_c
        .cmd_tx
        .send(MeshCommand::JoinRoom {
            rendezvous_server: rv_str,
            invite,
            nickname: "PeerC".to_string(),
            client_uuid: [0x53u8; 16],
            initial_volumes: HashMap::new(),
            is_lan_room: false,
            room_name: None,
        })
        .await
        .unwrap();
    wait_joined_room(&mut node_c, "PeerC").await;

    let c_deadline = Instant::now() + Duration::from_secs(4);
    let mut c_connected_a = false;
    let mut c_saw_ghost_stream = false;
    while Instant::now() < c_deadline {
        while let Ok(ev) = node_c.evt_rx.try_recv() {
            match ev {
                MeshEvent::PeersUpdated(peers) => {
                    if let Some(p) = peers.iter().find(|p| p.peer_id == peer_a_id) {
                        if p.stream_info.is_some() {
                            c_saw_ghost_stream = true;
                        }
                        if p.status == PeerConnStatus::Connected {
                            c_connected_a = true;
                        }
                    }
                }
                MeshEvent::RemoteStreamUpdated {
                    peer_id,
                    stream: Some(_),
                } if peer_id == peer_a_id => {
                    c_saw_ghost_stream = true;
                }
                _ => {}
            }
        }
        if c_connected_a && Instant::now() + Duration::from_millis(400) > c_deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(c_connected_a, "PeerC must connect to PeerA");
    assert!(
        !c_saw_ghost_stream,
        "PeerC must not receive stale stream_info after StopScreenShare"
    );

    node_a.shutdown().await;
    node_b.shutdown().await;
    node_c.shutdown().await;
    let _ = rv_shutdown_tx.send(true);
    let _ = rv_task.await;
}
