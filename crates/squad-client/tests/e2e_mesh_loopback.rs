use squad_dsp::{OpusDecoder10ms, OpusEncoder10ms, FRAME_SAMPLES};
use squad_proto::{
    InviteLink, P2pHeader, P2pMsgType, P2pPayload, RoomCipher, SignalingPacket,
};
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
    cipher_a.seal_packet(&syn_hdr, &syn_body, &mut wire).unwrap();
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
    let audio_body = P2pPayload::AudioOpus { opus_data: opus_bytes };

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
            assert!(rms > 0.05, "Decoded Opus audio frame should have audible RMS energy");
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
    assert!(frags.len() >= 2, "must contain at least 1 data + 1 FEC parity");

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
    let rgba = dec.decode(&recovered.annex_b_bytes).expect("1080p keyframe must decode");
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
    use squad_video::{
        fragment_frame, FrameReassembler, H264Decoder, H264Encoder, RawRgbaFrame,
    };

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
    assert!(requested_pli, "FrameReassembler must request PLI on dropped P-frame");

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
    let decoded = dec.decode(&frame_ok.annex_b_bytes).expect("IDR must decode");
    for px in decoded.rgba.as_chunks::<4>().0 {
        assert_eq!(px[0], px[1], "Neutral gray must have R == G (no purple tint)");
        assert_eq!(px[1], px[2], "Neutral gray must have G == B (no purple tint)");
        assert!(
            px[0].abs_diff(128) <= 2,
            "Neutral gray luminance must remain near 128, got {}",
            px[0]
        );
    }
}
