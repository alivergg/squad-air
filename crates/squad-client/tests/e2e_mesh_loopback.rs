use squad_dsp::{OpusDecoder10ms, OpusEncoder10ms, FRAME_SAMPLES};
use squad_proto::{InviteLink, P2pHeader, P2pMsgType, P2pPayload, RoomCipher, SignalingPacket};
use std::time::Duration;
use tokio::net::UdpSocket;

#[tokio::test]
async fn test_e2e_encrypted_hole_punch_and_opus_audio() {
    // 1. Генерируем инвайт комнаты и инициализируем шифры двух пиров
    let invite = InviteLink::generate();
    let cipher_a = RoomCipher::from_secret(&invite.secret);
    let cipher_b = RoomCipher::from_secret(&invite.secret);

    let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr_a = sock_a.local_addr().unwrap();
    let addr_b = sock_b.local_addr().unwrap();

    // 2. Проверяем сигналинг C2sRegister кодирование/декодирование
    let reg_a = SignalingPacket::C2sRegister {
        room_id: invite.room_id,
        client_uuid: [0x01; 16],
        nickname: "PeerA".into(),
        lan_addrs: Vec::new(),
    };
    let reg_bytes = reg_a.encode().unwrap();
    let decoded_reg = SignalingPacket::decode(&reg_bytes).unwrap();
    assert_eq!(reg_a, decoded_reg);

    // 3. Peer A отправляет зашифрованный PUNCH_SYN -> Peer B
    let syn_hdr = P2pHeader {
        msg_type: P2pMsgType::PunchSyn,
        room_prefix: invite.room_prefix(),
        sender_peer_id: 1,
        seq_num: 1,
    };
    let syn_body = P2pPayload::PunchSyn {
        client_uuid: [0x01; 16],
        timestamp_us: 123_456,
    };
    let mut wire = Vec::new();
    cipher_a
        .seal_packet(&syn_hdr, &syn_body, &mut wire)
        .unwrap();
    sock_a.send_to(&wire, addr_b).await.unwrap();

    let mut rx_buf = [0u8; 1500];
    let (len, from) = tokio::time::timeout(Duration::from_secs(1), sock_b.recv_from(&mut rx_buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(from, addr_a);

    let (dec_hdr, dec_body) = cipher_b.open_packet(&mut rx_buf[..len]).unwrap();
    assert_eq!(dec_hdr.sender_peer_id, 1);
    assert_eq!(dec_body, syn_body);

    // 4. Peer B кодирует 10 мс синусоиды 440 Гц в Opus, шифрует ChaCha20-Poly1305 и шлет Peer A
    let mut encoder = OpusEncoder10ms::new(40_000).unwrap();
    let mut decoder = OpusDecoder10ms::new().unwrap();

    let mut pcm_in = [0.0f32; FRAME_SAMPLES];
    for (i, sample) in pcm_in.iter_mut().enumerate() {
        let t = i as f32 / 48_000.0;
        *sample = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.6;
    }

    // Прогреваем первые 2 фрейма Opus и отправляем 3-й
    let _ = encoder.encode_frame(&pcm_in).unwrap();
    let _ = encoder.encode_frame(&pcm_in).unwrap();
    let opus_bytes = encoder
        .encode_frame(&pcm_in)
        .unwrap()
        .expect("non-DTX active sine frame");

    let audio_hdr = P2pHeader {
        msg_type: P2pMsgType::AudioOpus,
        room_prefix: invite.room_prefix(),
        sender_peer_id: 2,
        seq_num: 10,
    };
    let audio_body = P2pPayload::AudioOpus {
        opus_data: opus_bytes,
    };

    cipher_b
        .seal_packet(&audio_hdr, &audio_body, &mut wire)
        .unwrap();
    sock_b.send_to(&wire, addr_a).await.unwrap();

    let (len, _) = tokio::time::timeout(Duration::from_secs(1), sock_a.recv_from(&mut rx_buf))
        .await
        .unwrap()
        .unwrap();

    let (_, rx_payload) = cipher_a.open_packet(&mut rx_buf[..len]).unwrap();
    match rx_payload {
        P2pPayload::AudioOpus { opus_data } => {
            let mut pcm_out = [0.0f32; FRAME_SAMPLES];
            decoder.decode_frame(&opus_data, &mut pcm_out).unwrap();
            let rms = (pcm_out.iter().map(|&x| x * x).sum::<f32>() / (FRAME_SAMPLES as f32)).sqrt();
            assert!(
                rms > 0.05,
                "Decoded Opus audio frame should have audible RMS energy"
            );
        }
        other => panic!("Expected AudioOpus payload, got {other:?}"),
    }
}

#[tokio::test]
async fn test_e2e_chat_and_swarm_packets_loopback() {
    let invite = InviteLink::generate();
    let cipher_a = RoomCipher::from_secret(&invite.secret);
    let cipher_b = RoomCipher::from_secret(&invite.secret);

    let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr_a = sock_a.local_addr().unwrap();
    let addr_b = sock_b.local_addr().unwrap();

    let mut wire = Vec::new();
    let mut rx_buf = [0u8; 1500];

    let chat = P2pPayload::ChatText {
        sender_uuid: [0x11; 16],
        sender_nickname: "PeerA".to_string(),
        msg_id: 42,
        timestamp_ms: 1_700_000_000_000,
        text: "Rush B no stop!".to_string(),
    };
    cipher_a
        .seal_packet(
            &P2pHeader {
                msg_type: chat.msg_type(),
                room_prefix: invite.room_prefix(),
                sender_peer_id: 1,
                seq_num: 1,
            },
            &chat,
            &mut wire,
        )
        .unwrap();
    sock_a.send_to(&wire, addr_b).await.unwrap();

    let (len, _) = tokio::time::timeout(Duration::from_secs(1), sock_b.recv_from(&mut rx_buf))
        .await
        .unwrap()
        .unwrap();
    let (_, dec_chat) = cipher_b.open_packet(&mut rx_buf[..len]).unwrap();
    assert_eq!(dec_chat, chat);

    let ack = P2pPayload::ChatAck { msg_id: 42 };
    cipher_b
        .seal_packet(
            &P2pHeader {
                msg_type: ack.msg_type(),
                room_prefix: invite.room_prefix(),
                sender_peer_id: 2,
                seq_num: 1,
            },
            &ack,
            &mut wire,
        )
        .unwrap();
    sock_b.send_to(&wire, addr_a).await.unwrap();

    let (len, _) = tokio::time::timeout(Duration::from_secs(1), sock_a.recv_from(&mut rx_buf))
        .await
        .unwrap()
        .unwrap();
    let (_, dec_ack) = cipher_a.open_packet(&mut rx_buf[..len]).unwrap();
    assert_eq!(dec_ack, ack);
}

#[tokio::test]
async fn test_e2e_video_1080p_encode_fragment_cipher_reassemble_decode() {
    use squad_video::{
        fragment_frame, FrameReassembler, H264Decoder, H264Encoder, SyntheticPatternCapture,
    };

    let invite = InviteLink::generate();
    let cipher_a = RoomCipher::from_secret(&invite.secret);
    let cipher_b = RoomCipher::from_secret(&invite.secret);

    let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr_b = sock_b.local_addr().unwrap();

    let mut cap = SyntheticPatternCapture::new(1920, 1080);
    let frame = cap.next_frame();
    let mut enc = H264Encoder::new(60, 6_000_000);
    let encoded = enc.encode(&frame);
    assert!(encoded.is_keyframe);

    let frags = fragment_frame(1, encoded.is_keyframe, &encoded.annex_b);
    assert!(
        frags.len() >= 2,
        "must contain at least 1 data + 1 FEC parity"
    );

    let mut wire = Vec::new();
    let mut sent = 0usize;
    for (seq, frag) in frags.into_iter().enumerate() {
        // Simulate single packet loss inside the first FEC group (drop data shard #0, keep FEC parity).
        if frag.frag_idx == 0 && (frag.flags & squad_proto::VIDEO_FLAG_XOR_FEC) == 0 {
            continue;
        }
        let hdr = P2pHeader {
            msg_type: P2pMsgType::VideoFrameFrag,
            room_prefix: invite.room_prefix(),
            sender_peer_id: 1,
            seq_num: (seq + 1) as u64,
        };
        let payload = P2pPayload::VideoFrameFrag {
            frame_id: frag.frame_id,
            frag_idx: frag.frag_idx,
            frag_count: frag.frag_count,
            flags: frag.flags,
            data: frag.data,
        };
        cipher_a.seal_packet(&hdr, &payload, &mut wire).unwrap();
        sock_a.send_to(&wire, addr_b).await.unwrap();
        sent += 1;
    }

    let mut reassembler = FrameReassembler::new();
    let mut rx_buf = [0u8; 1500];
    let mut reassembled = None;
    for _ in 0..sent {
        let (len, _) = tokio::time::timeout(Duration::from_secs(1), sock_b.recv_from(&mut rx_buf))
            .await
            .unwrap()
            .unwrap();
        let (_, payload) = cipher_b.open_packet(&mut rx_buf[..len]).unwrap();
        if let P2pPayload::VideoFrameFrag {
            frame_id,
            frag_idx,
            frag_count,
            flags,
            data,
        } = payload
        {
            let (out, _need_pli) =
                reassembler.push_fragment(frame_id, frag_idx, frag_count, flags, data);
            if out.is_some() {
                reassembled = out;
            }
        } else {
            panic!("Expected VideoFrameFrag");
        }
    }

    let recovered = reassembled.expect("XOR-FEC must recover dropped fragment #0");
    assert!(recovered.is_keyframe);
    assert_eq!(recovered.annex_b_bytes, encoded.annex_b);

    let mut dec = H264Decoder::new();
    let rgba = dec
        .decode(&recovered.annex_b_bytes)
        .expect("1080p keyframe must decode");
    assert_eq!(rgba.width, 1920);
    assert_eq!(rgba.height, 1080);
    assert_eq!(rgba.rgba.len(), 1920 * 1080 * 4);
}

#[tokio::test]
async fn test_e2e_persistent_chat_identity_and_avatar_announce_loopback() {
    let invite = InviteLink::generate();
    let cipher_a = RoomCipher::from_secret(&invite.secret);
    let cipher_b = RoomCipher::from_secret(&invite.secret);

    let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr_b = sock_b.local_addr().unwrap();

    let packets = vec![
        (
            P2pMsgType::ChatText,
            P2pPayload::ChatText {
                msg_id: 42,
                timestamp_ms: 1_700_000_000_000,
                sender_uuid: [0xA1; 16],
                sender_nickname: "Alpha".into(),
                text: "Persistent UUID authorship test".into(),
            },
        ),
        (
            P2pMsgType::BlobOffer,
            P2pPayload::BlobOffer {
                msg_id: 43,
                timestamp_ms: 1_700_000_000_001,
                blob_id: [0xBB; 16],
                kind: 2,
                auto_download: false,
                filename: "replay.dem".into(),
                total_size: 4096,
                chunk_size: 1024,
                total_chunks: 4,
                blake3_root: [0xCC; 32],
                sender_uuid: [0xA1; 16],
                sender_nickname: "Alpha".into(),
            },
        ),
        (
            P2pMsgType::AvatarAnnounce,
            P2pPayload::AvatarAnnounce {
                avatar_hash: [0xDD; 32],
                total_size: 8192,
                total_chunks: 8,
            },
        ),
    ];

    let mut wire = Vec::new();
    let mut rx_buf = [0u8; 1500];
    for (idx, (msg_type, payload)) in packets.into_iter().enumerate() {
        let hdr = P2pHeader {
            msg_type,
            room_prefix: invite.room_prefix(),
            sender_peer_id: 9,
            seq_num: (idx + 1) as u64,
        };
        cipher_a.seal_packet(&hdr, &payload, &mut wire).unwrap();
        sock_a.send_to(&wire, addr_b).await.unwrap();

        let (len, _) = tokio::time::timeout(Duration::from_secs(1), sock_b.recv_from(&mut rx_buf))
            .await
            .unwrap()
            .unwrap();
        let (dec_hdr, dec_payload) = cipher_b.open_packet(&mut rx_buf[..len]).unwrap();
        assert_eq!(dec_hdr.sender_peer_id, 9);
        assert_eq!(dec_payload, payload);
    }
}

#[tokio::test]
async fn test_e2e_video_idr_gating_and_neutral_gray_accuracy() {
    use squad_video::{fragment_frame, FrameReassembler, H264Decoder, H264Encoder, RawRgbaFrame};

    let invite = InviteLink::generate();
    let cipher_a = RoomCipher::from_secret(&invite.secret);
    let cipher_b = RoomCipher::from_secret(&invite.secret);

    let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr_b = sock_b.local_addr().unwrap();

    let gray_frame = RawRgbaFrame {
        width: 256,
        height: 256,
        rgba: [128u8, 128, 128, 255].repeat(256 * 256),
    };

    let mut enc = H264Encoder::new(60, 8_000_000);
    let initial_idr = enc.encode(&gray_frame);
    assert!(initial_idr.is_keyframe);
    let p_frame_after_loss = enc.encode(&gray_frame);
    assert!(!p_frame_after_loss.is_keyframe);

    let mut wire = Vec::new();
    let mut seq = 1u64;
    let mut rx_buf = [0u8; 1500];
    let mut reassembler = FrameReassembler::new();

    // Send P-frame before receiver ever got an IDR: strict awaiting_idr must drop it and request PLI.
    let p_frags = fragment_frame(2, false, &p_frame_after_loss.annex_b);
    let p_sent = p_frags.len();
    for frag in p_frags {
        let hdr = P2pHeader {
            msg_type: P2pMsgType::VideoFrameFrag,
            room_prefix: invite.room_prefix(),
            sender_peer_id: 1,
            seq_num: seq,
        };
        seq += 1;
        let payload = P2pPayload::VideoFrameFrag {
            frame_id: frag.frame_id,
            frag_idx: frag.frag_idx,
            frag_count: frag.frag_count,
            flags: frag.flags,
            data: frag.data,
        };
        cipher_a.seal_packet(&hdr, &payload, &mut wire).unwrap();
        sock_a.send_to(&wire, addr_b).await.unwrap();
    }

    let mut requested_pli = false;
    for _ in 0..p_sent {
        let (len, _) = tokio::time::timeout(Duration::from_secs(1), sock_b.recv_from(&mut rx_buf))
            .await
            .unwrap()
            .unwrap();
        let (_, payload) = cipher_b.open_packet(&mut rx_buf[..len]).unwrap();
        if let P2pPayload::VideoFrameFrag {
            frame_id,
            frag_idx,
            frag_count,
            flags,
            data,
        } = payload
        {
            let (out, need_pli) =
                reassembler.push_fragment(frame_id, frag_idx, frag_count, flags, data);
            assert!(out.is_none(), "P-frame must be rejected while awaiting_idr");
            if need_pli {
                requested_pli = true;
            }
        }
    }
    assert!(
        requested_pli,
        "FrameReassembler must request PLI on dropped P-frame"
    );

    // Now encode a fresh IDR keyframe and drop one shard to also verify XOR-FEC recovery.
    enc.force_keyframe();
    let recovery_idr = enc.encode(&gray_frame);
    assert!(recovery_idr.is_keyframe);
    let idr_frags = fragment_frame(3, true, &recovery_idr.annex_b);
    let mut idr_sent = 0usize;
    for frag in idr_frags {
        if frag.frag_idx == 0 && (frag.flags & squad_proto::VIDEO_FLAG_XOR_FEC) == 0 {
            continue;
        }
        let hdr = P2pHeader {
            msg_type: P2pMsgType::VideoFrameFrag,
            room_prefix: invite.room_prefix(),
            sender_peer_id: 1,
            seq_num: seq,
        };
        seq += 1;
        let payload = P2pPayload::VideoFrameFrag {
            frame_id: frag.frame_id,
            frag_idx: frag.frag_idx,
            frag_count: frag.frag_count,
            flags: frag.flags,
            data: frag.data,
        };
        cipher_a.seal_packet(&hdr, &payload, &mut wire).unwrap();
        sock_a.send_to(&wire, addr_b).await.unwrap();
        idr_sent += 1;
    }

    let mut recovered = None;
    for _ in 0..idr_sent {
        let (len, _) = tokio::time::timeout(Duration::from_secs(1), sock_b.recv_from(&mut rx_buf))
            .await
            .unwrap()
            .unwrap();
        let (_, payload) = cipher_b.open_packet(&mut rx_buf[..len]).unwrap();
        if let P2pPayload::VideoFrameFrag {
            frame_id,
            frag_idx,
            frag_count,
            flags,
            data,
        } = payload
        {
            let (out, _) = reassembler.push_fragment(frame_id, frag_idx, frag_count, flags, data);
            if out.is_some() {
                recovered = out;
            }
        }
    }

    let frame_ok = recovered.expect("IDR frame must unlock reassembler");
    assert!(frame_ok.is_keyframe);
    let mut dec = H264Decoder::new();
    let decoded = dec
        .decode(&frame_ok.annex_b_bytes)
        .expect("IDR must decode");
    for px in decoded.rgba.as_chunks::<4>().0 {
        assert_eq!(
            px[0], px[1],
            "Neutral gray must have R == G (no purple tint)"
        );
        assert_eq!(
            px[1], px[2],
            "Neutral gray must have G == B (no purple tint)"
        );
        assert!(
            px[0].abs_diff(128) <= 2,
            "Neutral gray luminance must remain near 128, got {}",
            px[0]
        );
    }
}

#[tokio::test]
async fn test_e2e_h265_decoder_error_triggers_auto_h264_fallback_and_full_1080p() {
    use squad_video::{
        fragment_frame, DecoderError, FrameReassembler, H264Decoder, H264Encoder,
        SyntheticPatternCapture, VideoCodec,
    };

    let invite = InviteLink::generate();
    let cipher_a = RoomCipher::from_secret(&invite.secret);
    let cipher_b = RoomCipher::from_secret(&invite.secret);

    let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr_a = sock_a.local_addr().unwrap();
    let addr_b = sock_b.local_addr().unwrap();

    // 1. Verify StreamAnnounce (codec=H265, has_audio=true) and StreamAudioOpus roundtrip over UDP
    let announce = P2pPayload::StreamAnnounce {
        is_streaming: true,
        codec: VideoCodec::H265.as_u8(),
        has_audio: true,
        width: 1920,
        height: 1080,
        fps: 60,
        source_title: "Display 1".into(),
        encoder_backend: "NVIDIA NVENC".into(),
    };
    let mut wire = Vec::new();
    let mut seq = 1u64;
    let hdr_ann = P2pHeader {
        msg_type: P2pMsgType::StreamAnnounce,
        room_prefix: invite.room_prefix(),
        sender_peer_id: 1,
        seq_num: seq,
    };
    seq += 1;
    cipher_a
        .seal_packet(&hdr_ann, &announce, &mut wire)
        .unwrap();
    sock_a.send_to(&wire, addr_b).await.unwrap();

    let mut rx_buf = [0u8; 1500];
    let (len, _) = tokio::time::timeout(Duration::from_secs(1), sock_b.recv_from(&mut rx_buf))
        .await
        .unwrap()
        .unwrap();
    let (_, dec_ann) = cipher_b.open_packet(&mut rx_buf[..len]).unwrap();
    assert_eq!(dec_ann, announce);

    let audio_pkt = P2pPayload::StreamAudioOpus {
        opus_data: vec![0x42; 96],
    };
    let hdr_aud = P2pHeader {
        msg_type: P2pMsgType::StreamAudioOpus,
        room_prefix: invite.room_prefix(),
        sender_peer_id: 1,
        seq_num: seq,
    };
    seq += 1;
    cipher_a
        .seal_packet(&hdr_aud, &audio_pkt, &mut wire)
        .unwrap();
    sock_a.send_to(&wire, addr_b).await.unwrap();
    let (len, _) = tokio::time::timeout(Duration::from_secs(1), sock_b.recv_from(&mut rx_buf))
        .await
        .unwrap()
        .unwrap();
    let (_, dec_aud) = cipher_b.open_packet(&mut rx_buf[..len]).unwrap();
    assert_eq!(dec_aud, audio_pkt);

    // 2. Streamer encodes 1920x1080 in H.265
    let mut cap = SyntheticPatternCapture::new(1920, 1080);
    let f1 = cap.next_frame();
    let mut enc = H264Encoder::new_with_codec(60, 8_000_000, VideoCodec::H265);
    let hevc_frame = enc.encode(&f1);
    assert!(hevc_frame.is_keyframe);

    let frags_hevc = fragment_frame(1, true, &hevc_frame.annex_b);
    let frag_count_hevc = frags_hevc.len();
    for frag in frags_hevc {
        let hdr = P2pHeader {
            msg_type: P2pMsgType::VideoFrameFrag,
            room_prefix: invite.room_prefix(),
            sender_peer_id: 1,
            seq_num: seq,
        };
        seq += 1;
        let payload = P2pPayload::VideoFrameFrag {
            frame_id: frag.frame_id,
            frag_idx: frag.frag_idx,
            frag_count: frag.frag_count,
            flags: frag.flags,
            data: frag.data,
        };
        cipher_a.seal_packet(&hdr, &payload, &mut wire).unwrap();
        sock_a.send_to(&wire, addr_b).await.unwrap();
    }

    let mut reassembler = FrameReassembler::new();
    let mut reassembled_hevc = None;
    for _ in 0..frag_count_hevc {
        let (len, _) = tokio::time::timeout(Duration::from_secs(1), sock_b.recv_from(&mut rx_buf))
            .await
            .unwrap()
            .unwrap();
        let (_, payload) = cipher_b.open_packet(&mut rx_buf[..len]).unwrap();
        if let P2pPayload::VideoFrameFrag {
            frame_id,
            frag_idx,
            frag_count,
            flags,
            data,
        } = payload
        {
            let (out, _) = reassembler.push_fragment(frame_id, frag_idx, frag_count, flags, data);
            if out.is_some() {
                reassembled_hevc = out;
            }
        }
    }

    let hevc_assembled = reassembled_hevc.expect("H.265 IDR frame must reassemble");
    let mut viewer_dec = H264Decoder::new();
    viewer_dec.set_hevc_decode_supported(false);
    let err = viewer_dec
        .decode_checked(&hevc_assembled.annex_b_bytes)
        .unwrap_err();
    let DecoderError::UnsupportedCodec(reason) = err else {
        panic!("Expected UnsupportedCodec error, got {err:?}");
    };

    // 3. Viewer sends StreamDecoderError back to streamer; streamer falls back to H.264
    let err_hdr = P2pHeader {
        msg_type: P2pMsgType::StreamDecoderError,
        room_prefix: invite.room_prefix(),
        sender_peer_id: 2,
        seq_num: 1,
    };
    let err_payload = P2pPayload::StreamDecoderError {
        failed_codec: VideoCodec::H265.as_u8(),
        reason,
    };
    cipher_b
        .seal_packet(&err_hdr, &err_payload, &mut wire)
        .unwrap();
    sock_b.send_to(&wire, addr_a).await.unwrap();

    let (len, _) = tokio::time::timeout(Duration::from_secs(1), sock_a.recv_from(&mut rx_buf))
        .await
        .unwrap()
        .unwrap();
    let (_, recv_err) = cipher_a.open_packet(&mut rx_buf[..len]).unwrap();
    assert!(matches!(
        recv_err,
        P2pPayload::StreamDecoderError {
            failed_codec: 1,
            ..
        }
    ));

    enc.set_codec(VideoCodec::H264);
    assert_eq!(enc.codec(), VideoCodec::H264);
    let f2 = cap.next_frame();
    let h264_frame = enc.encode(&f2);
    assert!(h264_frame.is_keyframe);

    let frags_h264 = fragment_frame(2, true, &h264_frame.annex_b);
    let frag_count_h264 = frags_h264.len();
    for frag in frags_h264 {
        let hdr = P2pHeader {
            msg_type: P2pMsgType::VideoFrameFrag,
            room_prefix: invite.room_prefix(),
            sender_peer_id: 1,
            seq_num: seq,
        };
        seq += 1;
        let payload = P2pPayload::VideoFrameFrag {
            frame_id: frag.frame_id,
            frag_idx: frag.frag_idx,
            frag_count: frag.frag_count,
            flags: frag.flags,
            data: frag.data,
        };
        cipher_a.seal_packet(&hdr, &payload, &mut wire).unwrap();
        sock_a.send_to(&wire, addr_b).await.unwrap();
    }

    let mut reassembled_h264 = None;
    for _ in 0..frag_count_h264 {
        let (len, _) = tokio::time::timeout(Duration::from_secs(1), sock_b.recv_from(&mut rx_buf))
            .await
            .unwrap()
            .unwrap();
        let (_, payload) = cipher_b.open_packet(&mut rx_buf[..len]).unwrap();
        if let P2pPayload::VideoFrameFrag {
            frame_id,
            frag_idx,
            frag_count,
            flags,
            data,
        } = payload
        {
            let (out, _) = reassembler.push_fragment(frame_id, frag_idx, frag_count, flags, data);
            if out.is_some() {
                reassembled_h264 = out;
            }
        }
    }

    let h264_assembled = reassembled_h264.expect("H.264 fallback IDR must reassemble");
    let decoded = viewer_dec
        .decode_checked(&h264_assembled.annex_b_bytes)
        .expect("H.264 fallback must decode cleanly")
        .expect("H.264 fallback must output 1080p RGBA frame");
    assert_eq!(decoded.width, 1920);
    assert_eq!(decoded.height, 1080);
    assert_eq!(decoded.rgba.len(), 1920 * 1080 * 4);
}

#[tokio::test]
async fn test_e2e_relay_framing_opus_decode_and_handoff_dedup() {
    use squad_dsp::{JitterBuffer, JitterProfile};
    use squad_proto::{
        encode_relay_datagram_into, is_relay_datagram, parse_relay_header, rewrite_relay_peer_id,
        MAX_RELAY_UDP_PAYLOAD, RELAY_HEADER_LEN,
    };

    let invite = InviteLink::generate();
    let cipher_a = RoomCipher::from_secret(&invite.secret);
    let cipher_b = RoomCipher::from_secret(&invite.secret);

    let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let sock_relay = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr_a = sock_a.local_addr().unwrap();
    let addr_relay = sock_relay.local_addr().unwrap();
    let addr_b = sock_b.local_addr().unwrap();

    // Verify encrypted PunchSyn wrapped in a relay envelope with server peer-ID rewrite.
    let mut inner_buf = Vec::new();
    let mut relay_wire = Vec::with_capacity(MAX_RELAY_UDP_PAYLOAD);
    let mut rx_relay = [0u8; MAX_RELAY_UDP_PAYLOAD + 64];
    let mut rx_b = [0u8; MAX_RELAY_UDP_PAYLOAD + 64];

    let syn_hdr = P2pHeader {
        msg_type: P2pMsgType::PunchSyn,
        room_prefix: invite.room_prefix(),
        sender_peer_id: 1,
        seq_num: 1,
    };
    let syn_payload = P2pPayload::PunchSyn {
        client_uuid: [0xA1; 16],
        timestamp_us: 999_001,
    };
    cipher_a
        .seal_packet(&syn_hdr, &syn_payload, &mut inner_buf)
        .unwrap();
    encode_relay_datagram_into(&invite.room_id, 2, &inner_buf, &mut relay_wire).unwrap();
    assert!(is_relay_datagram(&relay_wire));
    sock_a.send_to(&relay_wire, addr_relay).await.unwrap();

    let (len_r, from_r) =
        tokio::time::timeout(Duration::from_secs(1), sock_relay.recv_from(&mut rx_relay))
            .await
            .unwrap()
            .unwrap();
    assert_eq!(from_r, addr_a);
    assert!(is_relay_datagram(&rx_relay[..len_r]));
    let (r_room, target_pid, _) = parse_relay_header(&rx_relay[..len_r]).unwrap();
    assert_eq!(r_room, invite.room_id);
    assert_eq!(target_pid, 2);
    rewrite_relay_peer_id(&mut rx_relay[..len_r], 1);
    sock_relay
        .send_to(&rx_relay[..len_r], addr_b)
        .await
        .unwrap();

    let (len_b, from_b) = tokio::time::timeout(Duration::from_secs(1), sock_b.recv_from(&mut rx_b))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(from_b, addr_relay);
    assert!(is_relay_datagram(&rx_b[..len_b]));
    let (b_room, relay_sender_pid, _) = parse_relay_header(&rx_b[..len_b]).unwrap();
    assert_eq!(b_room, invite.room_id);
    assert_eq!(relay_sender_pid, 1);
    let (dec_syn_hdr, dec_syn) = cipher_b
        .open_packet(&mut rx_b[RELAY_HEADER_LEN..len_b])
        .unwrap();
    assert_eq!(dec_syn_hdr.sender_peer_id, relay_sender_pid);
    assert_eq!(dec_syn, syn_payload);

    // Encode 5 active 10 ms Opus frames after warming up the encoder.
    let mut encoder = OpusEncoder10ms::new(40_000).unwrap();
    let mut pcm_in = [0.0f32; FRAME_SAMPLES];
    for (i, sample) in pcm_in.iter_mut().enumerate() {
        let t = i as f32 / 48_000.0;
        *sample = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.6;
    }
    let _ = encoder.encode_frame(&pcm_in).unwrap();
    let _ = encoder.encode_frame(&pcm_in).unwrap();
    let mut opus_frames = Vec::with_capacity(5);
    for _ in 0..5 {
        opus_frames.push(
            encoder
                .encode_frame(&pcm_in)
                .unwrap()
                .expect("non-DTX active sine frame"),
        );
    }

    // Phase 1: Deliver seq 1..=3 via the UDP relay path.
    let mut jb = JitterBuffer::with_profile(JitterProfile::Balanced);
    for seq in 1..=3u64 {
        let hdr = P2pHeader {
            msg_type: P2pMsgType::AudioOpus,
            room_prefix: invite.room_prefix(),
            sender_peer_id: 1,
            seq_num: seq,
        };
        let payload = P2pPayload::AudioOpus {
            opus_data: opus_frames[(seq - 1) as usize].clone(),
        };
        cipher_a
            .seal_packet(&hdr, &payload, &mut inner_buf)
            .unwrap();
        encode_relay_datagram_into(&invite.room_id, 2, &inner_buf, &mut relay_wire).unwrap();
        sock_a.send_to(&relay_wire, addr_relay).await.unwrap();

        let (len_r, _) =
            tokio::time::timeout(Duration::from_secs(1), sock_relay.recv_from(&mut rx_relay))
                .await
                .unwrap()
                .unwrap();
        let (room, pid, _) = parse_relay_header(&rx_relay[..len_r]).unwrap();
        assert_eq!(room, invite.room_id);
        assert_eq!(pid, 2);
        rewrite_relay_peer_id(&mut rx_relay[..len_r], 1);
        sock_relay
            .send_to(&rx_relay[..len_r], addr_b)
            .await
            .unwrap();

        let (len_b, from_b) =
            tokio::time::timeout(Duration::from_secs(1), sock_b.recv_from(&mut rx_b))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(from_b, addr_relay);
        let (rx_room, sender_pid, _) = parse_relay_header(&rx_b[..len_b]).unwrap();
        assert_eq!(rx_room, invite.room_id);
        assert_eq!(sender_pid, 1);
        let (dec_hdr, dec_payload) = cipher_b
            .open_packet(&mut rx_b[RELAY_HEADER_LEN..len_b])
            .unwrap();
        assert_eq!(dec_hdr.sender_peer_id, 1);
        let P2pPayload::AudioOpus { opus_data } = dec_payload else {
            panic!("Expected AudioOpus over relay");
        };
        jb.push_packet(dec_hdr.seq_num, opus_data);
    }

    // Pop seq 1 so next_expected_seq advances to 2 before direct handoff.
    let mut pcm_out = [0.0f32; FRAME_SAMPLES];
    assert!(jb.pop_pcm_frame(&mut pcm_out));
    let rms_1 = (pcm_out.iter().map(|&x| x * x).sum::<f32>() / (FRAME_SAMPLES as f32)).sqrt();
    assert!(rms_1 > 0.05, "Expected audible RMS for seq 1, got {rms_1}");

    // Phase 2: Simulate relay-to-direct handoff with duplicate seq 1 (stale) and seq 3 (in-window),
    // followed by direct seq 4 and seq 5 without relay encapsulation.
    for seq in [1u64, 3, 4, 5] {
        let hdr = P2pHeader {
            msg_type: P2pMsgType::AudioOpus,
            room_prefix: invite.room_prefix(),
            sender_peer_id: 1,
            seq_num: seq,
        };
        let payload = P2pPayload::AudioOpus {
            opus_data: opus_frames[(seq - 1) as usize].clone(),
        };
        cipher_a
            .seal_packet(&hdr, &payload, &mut inner_buf)
            .unwrap();
        sock_a.send_to(&inner_buf, addr_b).await.unwrap();

        let (len_b, from_b) =
            tokio::time::timeout(Duration::from_secs(1), sock_b.recv_from(&mut rx_b))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(from_b, addr_a);
        assert!(!is_relay_datagram(&rx_b[..len_b]));
        let (dec_hdr, dec_payload) = cipher_b.open_packet(&mut rx_b[..len_b]).unwrap();
        assert_eq!(dec_hdr.sender_peer_id, 1);
        let P2pPayload::AudioOpus { opus_data } = dec_payload else {
            panic!("Expected direct AudioOpus during handoff");
        };
        jb.push_packet(dec_hdr.seq_num, opus_data);
    }

    // Remaining seq 2, 3, 4, 5 must each decode cleanly once with zero duplicate replay.
    for expected_seq in 2..=5u64 {
        assert!(
            jb.pop_pcm_frame(&mut pcm_out),
            "Expected decoded frame for seq {expected_seq}"
        );
        let rms = (pcm_out.iter().map(|&x| x * x).sum::<f32>() / (FRAME_SAMPLES as f32)).sqrt();
        assert!(
            rms > 0.05,
            "Expected audible RMS for seq {expected_seq}, got {rms}"
        );
    }

    // Drain up to 3 PLC concealment frames, then verify no duplicate buffered packets remain.
    for _ in 0..3 {
        let _ = jb.pop_pcm_frame(&mut pcm_out);
    }
    assert!(
        !jb.pop_pcm_frame(&mut pcm_out),
        "Duplicate handoff packets must not replay after seq 5"
    );
    assert_eq!(jb.snapshot().loss_pct, 0.0);
}

#[tokio::test]
async fn test_e2e_spake2_six_digit_code_join_and_wrong_code_rejection() {
    use chacha20poly1305::{
        aead::{Aead, KeyInit, Payload},
        ChaCha20Poly1305, Nonce,
    };
    use rand::{rngs::OsRng, RngCore};
    use spake2::{Ed25519Group, Identity, Password, Spake2};
    use squad_proto::{SignalingErrorCode, Spake2Packet};
    use std::sync::Arc;
    use tokio::sync::watch;

    let rv_sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let rv_addr = rv_sock.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let rv_task = tokio::spawn(squad_rendezvous::run_server(
        Arc::clone(&rv_sock),
        shutdown_rx,
    ));

    let sock_host = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let sock_joiner = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let invite = InviteLink::generate();

    // Host registers room
    let reg_host = SignalingPacket::C2sRegister {
        room_id: invite.room_id,
        client_uuid: [0x11; 16],
        nickname: "CodeHost".to_string(),
        lan_addrs: Vec::new(),
    };
    sock_host
        .send_to(&reg_host.encode().unwrap(), rv_addr)
        .await
        .unwrap();
    let mut rx_host = [0u8; 2048];
    let (n, _) = tokio::time::timeout(Duration::from_secs(1), sock_host.recv_from(&mut rx_host))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        SignalingPacket::decode(&rx_host[..n]).unwrap(),
        SignalingPacket::S2cRoomState {
            assigned_peer_id: 1,
            ..
        }
    ));

    // Host registers 6-digit code
    let code = 482_910u32;
    let host_pk = [0xAA; 32];
    let reg_code = SignalingPacket::C2sRegisterShortCode {
        room_id: invite.room_id,
        host_pub_key: host_pk,
        code,
    };
    sock_host
        .send_to(&reg_code.encode().unwrap(), rv_addr)
        .await
        .unwrap();
    let (n, _) = tokio::time::timeout(Duration::from_secs(1), sock_host.recv_from(&mut rx_host))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        SignalingPacket::decode(&rx_host[..n]).unwrap(),
        SignalingPacket::S2cShortCodeRegistered {
            code,
            expires_in_secs: 600
        }
    );

    // Phase 1: Full SPAKE2 resolution + AEAD envelope check (correct vs wrong code)
    let pass = format!("{code:06}");
    let (state_a, msg_a) = Spake2::<Ed25519Group>::start_a(
        &Password::new(pass.as_bytes()),
        &Identity::new(b"squad-air-spake2-joiner-v1"),
        &Identity::new(b"squad-air-spake2-host-v1"),
    );
    let resolve = SignalingPacket::C2sResolveShortCode {
        code,
        joiner_pub_key: [0xBB; 32],
        joiner_lan_addrs: Vec::new(),
        spake2_msg_a: msg_a.clone(),
    };
    sock_joiner
        .send_to(&resolve.encode().unwrap(), rv_addr)
        .await
        .unwrap();

    // Host gets Challenge; Joiner gets JoinerInfo
    let (n, _) = tokio::time::timeout(Duration::from_secs(1), sock_host.recv_from(&mut rx_host))
        .await
        .unwrap()
        .unwrap();
    let SignalingPacket::S2cShortCodeChallenge {
        code: chal_code,
        joiner_public_addr,
        spake2_msg_a,
        ..
    } = SignalingPacket::decode(&rx_host[..n]).unwrap()
    else {
        panic!("Expected S2cShortCodeChallenge at host");
    };
    assert_eq!(chal_code, code);
    assert_eq!(spake2_msg_a, msg_a);

    let mut rx_joiner = [0u8; 2048];
    let (n, _) = tokio::time::timeout(
        Duration::from_secs(1),
        sock_joiner.recv_from(&mut rx_joiner),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(
        SignalingPacket::decode(&rx_joiner[..n]).unwrap(),
        SignalingPacket::S2cShortCodeJoinerInfo { code: c, .. } if c == code
    ));

    // Host completes SPAKE2 side B and seals InviteLink (zero visibility to Rendezvous)
    let (state_b, msg_b) = Spake2::<Ed25519Group>::start_b(
        &Password::new(pass.as_bytes()),
        &Identity::new(b"squad-air-spake2-joiner-v1"),
        &Identity::new(b"squad-air-spake2-host-v1"),
    );
    let key_b = state_b.finish(&spake2_msg_a).unwrap();
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let mut pt = [0u8; 48];
    pt[..16].copy_from_slice(&invite.room_id);
    pt[16..48].copy_from_slice(&invite.secret);
    let aad = format!("squad-air spake2 room invite v1:{code:06}");
    let enc = ChaCha20Poly1305::new_from_slice(&key_b[..32])
        .unwrap()
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &pt,
                aad: aad.as_bytes(),
            },
        )
        .unwrap();
    let reply = Spake2Packet::HostReply {
        code,
        spake2_msg_b: msg_b.clone(),
        nonce,
        encrypted_invite: enc.clone(),
    };
    sock_host
        .send_to(&reply.encode().unwrap(), joiner_public_addr)
        .await
        .unwrap();

    let (n, _) = tokio::time::timeout(
        Duration::from_secs(1),
        sock_joiner.recv_from(&mut rx_joiner),
    )
    .await
    .unwrap()
    .unwrap();
    let Spake2Packet::HostReply {
        spake2_msg_b,
        nonce: rx_nonce,
        encrypted_invite,
        ..
    } = Spake2Packet::decode(&rx_joiner[..n]).unwrap()
    else {
        panic!("Expected Spake2Packet::HostReply");
    };

    // Wrong password SPAKE2 side A must fail AEAD authentication
    let (wrong_state_a, _) = Spake2::<Ed25519Group>::start_a(
        &Password::new(b"999999"),
        &Identity::new(b"squad-air-spake2-joiner-v1"),
        &Identity::new(b"squad-air-spake2-host-v1"),
    );
    let wrong_key = wrong_state_a.finish(&spake2_msg_b).unwrap();
    assert!(ChaCha20Poly1305::new_from_slice(&wrong_key[..32])
        .unwrap()
        .decrypt(
            Nonce::from_slice(&rx_nonce),
            Payload {
                msg: &encrypted_invite,
                aad: aad.as_bytes(),
            },
        )
        .is_err());

    // Correct SPAKE2 decrypts exact RoomCipher secret and joins same room
    let key_a = state_a.finish(&spake2_msg_b).unwrap();
    let opened = ChaCha20Poly1305::new_from_slice(&key_a[..32])
        .unwrap()
        .decrypt(
            Nonce::from_slice(&rx_nonce),
            Payload {
                msg: &encrypted_invite,
                aad: aad.as_bytes(),
            },
        )
        .unwrap();
    let mut joined_secret = [0u8; 32];
    joined_secret.copy_from_slice(&opened[16..48]);
    assert_eq!(joined_secret, invite.secret);

    let cipher_joiner = RoomCipher::from_secret(&joined_secret);
    let cipher_host = RoomCipher::from_secret(&invite.secret);
    let mut wire = Vec::new();
    cipher_host
        .seal_packet(
            &P2pHeader {
                msg_type: P2pMsgType::ChatText,
                room_prefix: invite.room_prefix(),
                sender_peer_id: 1,
                seq_num: 1,
            },
            &P2pPayload::ChatText {
                sender_uuid: [0x11; 16],
                sender_nickname: "CodeHost".to_string(),
                msg_id: 7,
                timestamp_ms: 99,
                text: "SPAKE2 E2E ok".to_string(),
            },
            &mut wire,
        )
        .unwrap();
    let (_, chat_payload) = cipher_joiner.open_packet(&mut wire).unwrap();
    assert!(matches!(
        chat_payload,
        P2pPayload::ChatText { ref text, .. } if text == "SPAKE2 E2E ok"
    ));

    // Phase 2: 4 wrong attempts -> InvalidShortCode, 5th & 6th attempts -> RateLimited
    let wrong_resolve = SignalingPacket::C2sResolveShortCode {
        code: 482_911,
        joiner_pub_key: [0xCC; 32],
        joiner_lan_addrs: Vec::new(),
        spake2_msg_a: vec![0x42; 33],
    }
    .encode()
    .unwrap();
    for attempt in 1..=6 {
        sock_joiner.send_to(&wrong_resolve, rv_addr).await.unwrap();
        let (n, _) = tokio::time::timeout(
            Duration::from_secs(1),
            sock_joiner.recv_from(&mut rx_joiner),
        )
        .await
        .unwrap()
        .unwrap();
        let expected_err = if attempt < 5 {
            SignalingErrorCode::InvalidShortCode
        } else {
            SignalingErrorCode::RateLimited
        };
        assert_eq!(
            SignalingPacket::decode(&rx_joiner[..n]).unwrap(),
            SignalingPacket::S2cError { code: expected_err }
        );
    }

    let _ = shutdown_tx.send(true);
    let _ = rv_task.await;
}

#[tokio::test]
async fn test_e2e_x25519_friend_call_and_third_peer_party_invite() {
    use chacha20poly1305::{
        aead::{Aead, KeyInit, Payload},
        ChaCha20Poly1305, Nonce,
    };
    use rand::{rngs::OsRng, RngCore};
    use std::sync::Arc;
    use tokio::sync::watch;
    use x25519_dalek::{EphemeralSecret, PublicKey, StaticSecret};

    let rv_sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let rv_addr = rv_sock.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let rv_task = tokio::spawn(squad_rendezvous::run_server(
        Arc::clone(&rv_sock),
        shutdown_rx,
    ));

    let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let sock_c = UdpSocket::bind("127.0.0.1:0").await.unwrap();

    let sk_a = StaticSecret::random_from_rng(OsRng);
    let sk_b = StaticSecret::random_from_rng(OsRng);
    let sk_c = StaticSecret::random_from_rng(OsRng);
    let pk_a = PublicKey::from(&sk_a).to_bytes();
    let pk_b = PublicKey::from(&sk_b).to_bytes();
    let pk_c = PublicKey::from(&sk_c).to_bytes();

    for (sock, pk, nick) in [
        (&sock_a, pk_a, "Alice"),
        (&sock_b, pk_b, "Bob"),
        (&sock_c, pk_c, "Charlie"),
    ] {
        let ping = SignalingPacket::C2sPresencePing {
            pub_key: pk,
            nickname: nick.to_string(),
            in_room: false,
        };
        sock.send_to(&ping.encode().unwrap(), rv_addr)
            .await
            .unwrap();
    }
    tokio::time::sleep(Duration::from_millis(30)).await;

    // A queries B and C status
    let query = SignalingPacket::C2sQueryFriends {
        friend_pub_keys: vec![pk_b, pk_c],
    };
    sock_a
        .send_to(&query.encode().unwrap(), rv_addr)
        .await
        .unwrap();
    let mut rx_buf = [0u8; 2048];
    let (n, _) = tokio::time::timeout(Duration::from_secs(1), sock_a.recv_from(&mut rx_buf))
        .await
        .unwrap()
        .unwrap();
    let SignalingPacket::S2cFriendsStatus { online_friends } =
        SignalingPacket::decode(&rx_buf[..n]).unwrap()
    else {
        panic!("Expected S2cFriendsStatus");
    };
    assert_eq!(online_friends.len(), 2);

    let seal_invite = |sender_sk: &StaticSecret,
                       target_pk: &[u8; 32],
                       invite: &InviteLink,
                       nick: &str,
                       party: bool| {
        let target_pub = PublicKey::from(*target_pk);
        let sender_pub = PublicKey::from(sender_sk).to_bytes();
        let eph_sk = EphemeralSecret::random_from_rng(OsRng);
        let eph_pub = PublicKey::from(&eph_sk).to_bytes();
        let ss_s = sender_sk.diffie_hellman(&target_pub);
        let ss_e = eph_sk.diffie_hellman(&target_pub);
        let mut h = blake3::Hasher::new();
        h.update(b"squad-air friend invite x25519 v1");
        h.update(ss_s.as_bytes());
        h.update(ss_e.as_bytes());
        let key = h.finalize();
        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut nonce);
        let mut pt = Vec::new();
        pt.extend_from_slice(&invite.room_id);
        pt.extend_from_slice(&invite.secret);
        pt.push(u8::from(party));
        pt.push(nick.len() as u8);
        pt.extend_from_slice(nick.as_bytes());
        let mut aad = Vec::with_capacity(96);
        aad.extend_from_slice(&sender_pub);
        aad.extend_from_slice(target_pk);
        aad.extend_from_slice(&eph_pub);
        let ct = ChaCha20Poly1305::new_from_slice(key.as_bytes())
            .unwrap()
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &pt,
                    aad: &aad,
                },
            )
            .unwrap();
        (eph_pub, nonce, ct)
    };

    let open_invite = |recv_sk: &StaticSecret,
                       sender_pk: &[u8; 32],
                       eph_pk: &[u8; 32],
                       nonce: &[u8; 12],
                       ct: &[u8]|
     -> Result<(InviteLink, bool), ()> {
        let recv_pk = PublicKey::from(recv_sk).to_bytes();
        let ss_s = recv_sk.diffie_hellman(&PublicKey::from(*sender_pk));
        let ss_e = recv_sk.diffie_hellman(&PublicKey::from(*eph_pk));
        let mut h = blake3::Hasher::new();
        h.update(b"squad-air friend invite x25519 v1");
        h.update(ss_s.as_bytes());
        h.update(ss_e.as_bytes());
        let key = h.finalize();
        let mut aad = Vec::with_capacity(96);
        aad.extend_from_slice(sender_pk);
        aad.extend_from_slice(&recv_pk);
        aad.extend_from_slice(eph_pk);
        let pt = ChaCha20Poly1305::new_from_slice(key.as_bytes())
            .unwrap()
            .decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad: &aad })
            .map_err(|_| ())?;
        let mut room_id = [0u8; 16];
        let mut secret = [0u8; 32];
        room_id.copy_from_slice(&pt[..16]);
        secret.copy_from_slice(&pt[16..48]);
        Ok((InviteLink { room_id, secret }, pt[48] != 0))
    };

    // A calls B -> B decrypts; unauthorized C cannot decrypt B's payload
    let invite = InviteLink::generate();
    let (eph_b, nonce_b, ct_b) = seal_invite(&sk_a, &pk_b, &invite, "Alice", false);
    sock_a
        .send_to(
            &SignalingPacket::C2sRelayFriendInvite {
                sender_pub_key: pk_a,
                target_pub_key: pk_b,
                ephemeral_pub_key: eph_b,
                nonce: nonce_b,
                ciphertext: ct_b,
            }
            .encode()
            .unwrap(),
            rv_addr,
        )
        .await
        .unwrap();

    let (n, _) = tokio::time::timeout(Duration::from_secs(1), sock_b.recv_from(&mut rx_buf))
        .await
        .unwrap()
        .unwrap();
    let SignalingPacket::S2cIncomingFriendInvite {
        sender_pub_key,
        ephemeral_pub_key,
        nonce,
        ciphertext,
    } = SignalingPacket::decode(&rx_buf[..n]).unwrap()
    else {
        panic!("Expected S2cIncomingFriendInvite for Bob");
    };
    assert!(open_invite(
        &sk_c,
        &sender_pub_key,
        &ephemeral_pub_key,
        &nonce,
        &ciphertext
    )
    .is_err());
    let (b_invite, b_party) = open_invite(
        &sk_b,
        &sender_pub_key,
        &ephemeral_pub_key,
        &nonce,
        &ciphertext,
    )
    .unwrap();
    assert!(!b_party);
    assert_eq!(b_invite.to_uri(), invite.to_uri());

    // A invites C into the active room as a 3rd party member
    let (eph_c, nonce_c, ct_c) = seal_invite(&sk_a, &pk_c, &invite, "Alice", true);
    sock_a
        .send_to(
            &SignalingPacket::C2sRelayFriendInvite {
                sender_pub_key: pk_a,
                target_pub_key: pk_c,
                ephemeral_pub_key: eph_c,
                nonce: nonce_c,
                ciphertext: ct_c,
            }
            .encode()
            .unwrap(),
            rv_addr,
        )
        .await
        .unwrap();
    let (n, _) = tokio::time::timeout(Duration::from_secs(1), sock_c.recv_from(&mut rx_buf))
        .await
        .unwrap()
        .unwrap();
    let SignalingPacket::S2cIncomingFriendInvite {
        sender_pub_key,
        ephemeral_pub_key,
        nonce,
        ciphertext,
    } = SignalingPacket::decode(&rx_buf[..n]).unwrap()
    else {
        panic!("Expected S2cIncomingFriendInvite for Charlie");
    };
    let (c_invite, c_party) = open_invite(
        &sk_c,
        &sender_pub_key,
        &ephemeral_pub_key,
        &nonce,
        &ciphertext,
    )
    .unwrap();
    assert!(c_party);
    assert_eq!(c_invite.to_uri(), invite.to_uri());

    // All 3 peers register in the same room via Rendezvous and receive unified peer list
    for (idx, (sock, inv)) in [
        (&sock_a, &invite),
        (&sock_b, &b_invite),
        (&sock_c, &c_invite),
    ]
    .into_iter()
    .enumerate()
    {
        sock.send_to(
            &SignalingPacket::C2sRegister {
                room_id: inv.room_id,
                client_uuid: [(idx as u8) + 1; 16],
                nickname: format!("Peer{}", idx + 1),
                lan_addrs: Vec::new(),
            }
            .encode()
            .unwrap(),
            rv_addr,
        )
        .await
        .unwrap();
        let (n, _) = tokio::time::timeout(Duration::from_secs(1), sock.recv_from(&mut rx_buf))
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            SignalingPacket::decode(&rx_buf[..n]).unwrap(),
            SignalingPacket::S2cRoomState {
                assigned_peer_id,
                ..
            } if assigned_peer_id == (idx as u16) + 1
        ));
    }

    let _ = shutdown_tx.send(true);
    let _ = rv_task.await;
}
