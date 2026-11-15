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