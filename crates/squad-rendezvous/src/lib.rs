pub mod state;

pub use state::RendezvousState;

use anyhow::Result;
use squad_proto::{
    is_relay_datagram, parse_relay_header, rewrite_relay_peer_id, P2pHeader, SignalingErrorCode,
    SignalingPacket, MAX_RELAY_UDP_PAYLOAD,
};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tracing::{debug, info, warn};

pub const DEFAULT_PEER_TTL: Duration = Duration::from_secs(6);
const CLEANUP_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RendezvousConfig {
    pub enable_relay: bool,
}

pub async fn run_server(
    socket: Arc<UdpSocket>,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    run_server_with_config(socket, RendezvousConfig::default(), shutdown_rx).await
}

pub async fn run_server_with_config(
    socket: Arc<UdpSocket>,
    config: RendezvousConfig,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    let state = Arc::new(RendezvousState::new());
    let state_reaper = Arc::clone(&state);
    let socket_reaper = Arc::clone(&socket);
    let mut reaper_shutdown = shutdown_rx.clone();

    let reaper_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(CLEANUP_INTERVAL);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let updates = state_reaper.evict_expired(Instant::now(), DEFAULT_PEER_TTL);
                    for (dst, pkt) in updates {
                        if let Ok(bytes) = pkt.encode() {
                            let _ = socket_reaper.send_to(&bytes, dst).await;
                        }
                    }
                }
                _ = reaper_shutdown.changed() => {
                    break;
                }
            }
        }
    });

    let mut buf = vec![0u8; MAX_RELAY_UDP_PAYLOAD + 64];
    loop {
        tokio::select! {
            recv_res = socket.recv_from(&mut buf) => {
                let (len, src_addr) = match recv_res {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("UDP recv_from error: {e}");
                        continue;
                    }
                };
                handle_datagram_with_config(&socket, &state, config, &mut buf[..len], src_addr).await;
            }
            _ = shutdown_rx.changed() => {
                info!("Shutting down SquadAir Rendezvous server");
                break;
            }
        }
    }

    reaper_handle.abort();
    Ok(())
}

pub async fn handle_datagram(
    socket: &UdpSocket,
    state: &RendezvousState,
    datagram: &mut [u8],
    src_addr: SocketAddr,
) {
    handle_datagram_with_config(socket, state, RendezvousConfig::default(), datagram, src_addr).await;
}

pub async fn handle_datagram_with_config(
    socket: &UdpSocket,
    state: &RendezvousState,
    config: RendezvousConfig,
    datagram: &mut [u8],
    src_addr: SocketAddr,
) {
    let now = Instant::now();

    // Handle zero-amplification relay datagrams before signaling rate-limiting/decoding.
    if is_relay_datagram(datagram) {
        if !config.enable_relay {
            return;
        }
        let Ok((room_id, target_peer_id, inner)) = parse_relay_header(datagram) else {
            return;
        };
        let Ok(inner_hdr) = P2pHeader::decode(inner) else {
            return;
        };
        if !inner_hdr.msg_type.is_server_relay_allowed() {
            return;
        }
        if let Some((sender_peer_id, target_addr)) =
            state.authorize_relay_forward(src_addr, room_id, target_peer_id, now, DEFAULT_PEER_TTL)
        {
            rewrite_relay_peer_id(datagram, sender_peer_id);
            let _ = socket.send_to(datagram, target_addr).await;
        }
        return;
    }

    if !state.check_rate_limit(src_addr.ip(), now) {
        if let Ok(bytes) = (SignalingPacket::S2cError {
            code: SignalingErrorCode::RateLimited,
        })
        .encode()
        {
            let _ = socket.send_to(&bytes, src_addr).await;
        }
        return;
    }

    let packet = match SignalingPacket::decode(datagram) {
        Ok(p) => p,
        Err(e) => {
            debug!("Invalid signaling packet from {src_addr}: {e}");
            if let Ok(bytes) = (SignalingPacket::S2cError {
                code: SignalingErrorCode::InvalidPacket,
            })
            .encode()
            {
                let _ = socket.send_to(&bytes, src_addr).await;
            }
            return;
        }
    };

    let responses = match packet {
        SignalingPacket::C2sRegister {
            room_id,
            client_uuid,
            nickname,
            lan_addrs,
        } => state.handle_register(src_addr, room_id, client_uuid, nickname, lan_addrs, now),
        SignalingPacket::C2sLeave {
            room_id,
            client_uuid,
        } => state.handle_leave(room_id, client_uuid),
        _ => Vec::new(),
    };

    for (dst, pkt) in responses {
        if let Ok(bytes) = pkt.encode() {
            let _ = socket.send_to(&bytes, dst).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use squad_proto::{encode_relay_datagram_into, P2pMsgType, P2pPayload, RoomCipher};

    #[tokio::test]
    async fn test_ten_peers_registration_and_11th_rejected() {
        let server_sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let server_addr = server_sock.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let srv_task = tokio::spawn(run_server(server_sock, shutdown_rx));

        let room_id = [0x99u8; 16];
        let mut clients = Vec::new();

        for i in 0..10u8 {
            let cli = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let pkt = SignalingPacket::C2sRegister {
                room_id,
                client_uuid: [i + 1; 16],
                nickname: format!("Player-{}", i + 1),
                lan_addrs: Vec::new(),
            };
            cli.send_to(&pkt.encode().unwrap(), server_addr)
                .await
                .unwrap();

            let mut buf = [0u8; 1200];
            let (len, _) = tokio::time::timeout(Duration::from_secs(1), cli.recv_from(&mut buf))
                .await
                .unwrap()
                .unwrap();
            let resp = SignalingPacket::decode(&buf[..len]).unwrap();
            match resp {
                SignalingPacket::S2cRoomState {
                    assigned_peer_id,
                    peers,
                } => {
                    assert_eq!(assigned_peer_id, (i + 1) as u16);
                    assert!(!peers.is_empty());
                }
                other => panic!("unexpected response: {other:?}"),
            }
            clients.push(cli);
        }

        // 11th client must receive RoomFull
        let cli_11 = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let pkt_11 = SignalingPacket::C2sRegister {
            room_id,
            client_uuid: [0xFF; 16],
            nickname: "OverflowPlayer".into(),
            lan_addrs: Vec::new(),
        };
        cli_11
            .send_to(&pkt_11.encode().unwrap(), server_addr)
            .await
            .unwrap();

        let mut buf = [0u8; 1200];
        let (len, _) = tokio::time::timeout(Duration::from_secs(1), cli_11.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
        let resp = SignalingPacket::decode(&buf[..len]).unwrap();
        assert_eq!(
            resp,
            SignalingPacket::S2cError {
                code: SignalingErrorCode::RoomFull
            }
        );

        let _ = shutdown_tx.send(true);
        let _ = srv_task.await;
    }

    async fn register_client(
        sock: &UdpSocket,
        server_addr: SocketAddr,
        room_id: [u8; 16],
        client_uuid: [u8; 16],
        nickname: &str,
    ) -> u16 {
        let pkt = SignalingPacket::C2sRegister {
            room_id,
            client_uuid,
            nickname: nickname.to_string(),
            lan_addrs: Vec::new(),
        };
        sock.send_to(&pkt.encode().unwrap(), server_addr)
            .await
            .unwrap();

        let mut buf = [0u8; 1200];
        let (len, _) = tokio::time::timeout(Duration::from_secs(1), sock.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
        match SignalingPacket::decode(&buf[..len]).unwrap() {
            SignalingPacket::S2cRoomState {
                assigned_peer_id, ..
            } => assigned_peer_id,
            other => panic!("expected S2cRoomState, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_udp_relay_between_room_peers_and_rejects_outsider() {
        let server_sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let server_addr = server_sock.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let srv_task = tokio::spawn(run_server_with_config(
            server_sock,
            RendezvousConfig { enable_relay: true },
            shutdown_rx,
        ));

        let peer_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let outsider = UdpSocket::bind("127.0.0.1:0").await.unwrap();

        let room_id = [0xABu8; 16];
        let peer_a_id = register_client(&peer_a, server_addr, room_id, [1u8; 16], "Alice").await;
        let peer_b_id = register_client(&peer_b, server_addr, room_id, [2u8; 16], "Bob").await;

        // Drain the room-state broadcast peer_a received when peer_b joined.
        let mut drain_buf = [0u8; 1200];
        let (len, _) =
            tokio::time::timeout(Duration::from_secs(1), peer_a.recv_from(&mut drain_buf))
                .await
                .unwrap()
                .unwrap();
        assert!(matches!(
            SignalingPacket::decode(&drain_buf[..len]).unwrap(),
            SignalingPacket::S2cRoomState { peers, .. } if peers.len() == 2
        ));

        let cipher = RoomCipher::from_secret(&[0x55u8; 32]);
        let inner_hdr = P2pHeader {
            msg_type: P2pMsgType::AudioOpus,
            room_prefix: [0xAB, 0xAB, 0xAB, 0xAB],
            sender_peer_id: peer_a_id,
            seq_num: 42,
        };
        let inner_payload = P2pPayload::AudioOpus {
            opus_data: vec![10, 20, 30, 40],
        };
        let mut inner = Vec::new();
        cipher
            .seal_packet(&inner_hdr, &inner_payload, &mut inner)
            .unwrap();

        let mut wire = Vec::new();
        encode_relay_datagram_into(&room_id, peer_b_id, &inner, &mut wire).unwrap();
        peer_a.send_to(&wire, server_addr).await.unwrap();

        let mut recv_buf = [0u8; MAX_RELAY_UDP_PAYLOAD + 64];
        let (len, from) =
            tokio::time::timeout(Duration::from_secs(1), peer_b.recv_from(&mut recv_buf))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(from, server_addr);
        assert!(is_relay_datagram(&recv_buf[..len]));

        let (relayed_room, relayed_sender_id, relayed_inner) =
            parse_relay_header(&recv_buf[..len]).unwrap();
        assert_eq!(relayed_room, room_id);
        assert_eq!(relayed_sender_id, peer_a_id);
        assert_eq!(relayed_inner, inner.as_slice());

        let mut inner_copy = relayed_inner.to_vec();
        let (opened_hdr, opened_payload) = cipher.open_packet(&mut inner_copy).unwrap();
        assert_eq!(opened_hdr, inner_hdr);
        assert_eq!(opened_payload, inner_payload);

        // Outsider relay must be silently dropped with zero reflection/amplification.
        outsider.send_to(&wire, server_addr).await.unwrap();

        // Malformed inner P2P header from a valid peer must also be silently dropped.
        let mut bad_inner_wire = Vec::new();
        encode_relay_datagram_into(
            &room_id,
            peer_b_id,
            b"invalid-p2p-header-payload",
            &mut bad_inner_wire,
        )
        .unwrap();
        peer_a.send_to(&bad_inner_wire, server_addr).await.unwrap();

        // Self-relay must also be silently dropped.
        let mut self_relay_wire = Vec::new();
        encode_relay_datagram_into(&room_id, peer_a_id, &inner, &mut self_relay_wire).unwrap();
        peer_a.send_to(&self_relay_wire, server_addr).await.unwrap();

        let silence_window = Duration::from_millis(80);
        assert!(
            tokio::time::timeout(silence_window, outsider.recv_from(&mut recv_buf))
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(silence_window, peer_a.recv_from(&mut recv_buf))
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(silence_window, peer_b.recv_from(&mut recv_buf))
                .await
                .is_err()
        );

        let _ = shutdown_tx.send(true);
        let _ = srv_task.await;
    }

    #[tokio::test]
    async fn test_udp_relay_disabled_by_default_drops_relay_packets() {
        let server_sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let server_addr = server_sock.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let srv_task = tokio::spawn(run_server(server_sock, shutdown_rx));

        let peer_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let room_id = [0xBCu8; 16];
        let peer_a_id = register_client(&peer_a, server_addr, room_id, [1u8; 16], "Alice").await;
        let peer_b_id = register_client(&peer_b, server_addr, room_id, [2u8; 16], "Bob").await;

        let mut drain_buf = [0u8; 1200];
        let _ = tokio::time::timeout(Duration::from_secs(1), peer_a.recv_from(&mut drain_buf))
            .await
            .unwrap()
            .unwrap();

        let cipher = RoomCipher::from_secret(&[0x55u8; 32]);
        let mut inner = Vec::new();
        cipher
            .seal_packet(
                &P2pHeader {
                    msg_type: P2pMsgType::AudioOpus,
                    room_prefix: [0xBC; 4],
                    sender_peer_id: peer_a_id,
                    seq_num: 1,
                },
                &P2pPayload::AudioOpus {
                    opus_data: vec![1, 2, 3],
                },
                &mut inner,
            )
            .unwrap();
        let mut wire = Vec::new();
        encode_relay_datagram_into(&room_id, peer_b_id, &inner, &mut wire).unwrap();
        peer_a.send_to(&wire, server_addr).await.unwrap();

        let mut recv_buf = [0u8; MAX_RELAY_UDP_PAYLOAD + 64];
        assert!(
            tokio::time::timeout(Duration::from_millis(80), peer_b.recv_from(&mut recv_buf))
                .await
                .is_err(),
            "server without --enable-relay must drop all relay datagrams"
        );

        let _ = shutdown_tx.send(true);
        let _ = srv_task.await;
    }

    #[tokio::test]
    async fn test_udp_relay_with_flag_allows_voice_chat_and_blocks_video() {
        let server_sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let server_addr = server_sock.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let srv_task = tokio::spawn(run_server_with_config(
            server_sock,
            RendezvousConfig { enable_relay: true },
            shutdown_rx,
        ));

        let peer_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let room_id = [0xCDu8; 16];
        let peer_a_id = register_client(&peer_a, server_addr, room_id, [1u8; 16], "Alice").await;
        let peer_b_id = register_client(&peer_b, server_addr, room_id, [2u8; 16], "Bob").await;

        let mut drain_buf = [0u8; 1200];
        let _ = tokio::time::timeout(Duration::from_secs(1), peer_a.recv_from(&mut drain_buf))
            .await
            .unwrap()
            .unwrap();

        let cipher = RoomCipher::from_secret(&[0x55u8; 32]);
        let mut inner = Vec::new();
        let mut wire = Vec::new();

        // ChatText must pass through when relay is enabled
        cipher
            .seal_packet(
                &P2pHeader {
                    msg_type: P2pMsgType::ChatText,
                    room_prefix: [0xCD; 4],
                    sender_peer_id: peer_a_id,
                    seq_num: 1,
                },
                &P2pPayload::ChatText {
                    sender_uuid: [1u8; 16],
                    sender_nickname: "Alice".into(),
                    msg_id: 1,
                    timestamp_ms: 100,
                    text: "hello over relay".into(),
                },
                &mut inner,
            )
            .unwrap();
        encode_relay_datagram_into(&room_id, peer_b_id, &inner, &mut wire).unwrap();
        peer_a.send_to(&wire, server_addr).await.unwrap();

        let mut recv_buf = [0u8; MAX_RELAY_UDP_PAYLOAD + 64];
        let (len, _) =
            tokio::time::timeout(Duration::from_secs(1), peer_b.recv_from(&mut recv_buf))
                .await
                .unwrap()
                .unwrap();
        let (_, _, relayed_inner) = parse_relay_header(&recv_buf[..len]).unwrap();
        let mut inner_copy = relayed_inner.to_vec();
        let (hdr, _) = cipher.open_packet(&mut inner_copy).unwrap();
        assert_eq!(hdr.msg_type, P2pMsgType::ChatText);

        // VideoFrameFrag and StreamAudioOpus must be dropped by server relay even when enabled
        for (seq, payload) in [
            (
                2u64,
                P2pPayload::VideoFrameFrag {
                    frame_id: 1,
                    frag_idx: 0,
                    frag_count: 1,
                    flags: 1,
                    data: vec![0xAA; 100],
                },
            ),
            (
                3u64,
                P2pPayload::StreamAudioOpus {
                    opus_data: vec![1, 2, 3, 4],
                },
            ),
            (4u64, P2pPayload::StreamSubscribe { subscribe: true }),
            (5u64, P2pPayload::StreamPli),
        ] {
            cipher
                .seal_packet(
                    &P2pHeader {
                        msg_type: payload.msg_type(),
                        room_prefix: [0xCD; 4],
                        sender_peer_id: peer_a_id,
                        seq_num: seq,
                    },
                    &payload,
                    &mut inner,
                )
                .unwrap();
            encode_relay_datagram_into(&room_id, peer_b_id, &inner, &mut wire).unwrap();
            peer_a.send_to(&wire, server_addr).await.unwrap();
        }

        assert!(
            tokio::time::timeout(Duration::from_millis(80), peer_b.recv_from(&mut recv_buf))
                .await
                .is_err(),
            "server relay must block screen-share video/stream traffic"
        );

        let _ = shutdown_tx.send(true);
        let _ = srv_task.await;
    }
}
