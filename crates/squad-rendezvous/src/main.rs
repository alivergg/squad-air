pub mod state;

use anyhow::Result;
use squad_proto::{SignalingErrorCode, SignalingPacket, MAX_UDP_PAYLOAD};
use state::RendezvousState;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tracing::{debug, info, warn};

const PEER_TTL: Duration = Duration::from_secs(6);
const CLEANUP_INTERVAL: Duration = Duration::from_secs(2);

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "squad_rendezvous=info".into()),
        )
        .init();

    let bind_addr = parse_bind_arg().unwrap_or_else(|| "0.0.0.0:3478".to_string());
    let socket = Arc::new(UdpSocket::bind(&bind_addr).await?);
    info!(
        "Squad-Air Rendezvous server listening on {}",
        socket.local_addr()?
    );

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = shutdown_tx.send(true);
    });

    run_server(socket, shutdown_rx).await
}

fn parse_bind_arg() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len() {
        if args[i] == "--bind" && i + 1 < args.len() {
            return Some(args[i + 1].clone());
        }
    }
    None
}

pub async fn run_server(
    socket: Arc<UdpSocket>,
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
                    let updates = state_reaper.evict_expired(Instant::now(), PEER_TTL);
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

    let mut buf = vec![0u8; MAX_UDP_PAYLOAD + 64];
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
                handle_datagram(&socket, &state, &buf[..len], src_addr).await;
            }
            _ = shutdown_rx.changed() => {
                info!("Shutting down Squad-Air Rendezvous server");
                break;
            }
        }
    }

    reaper_handle.abort();
    Ok(())
}

async fn handle_datagram(
    socket: &UdpSocket,
    state: &RendezvousState,
    datagram: &[u8],
    src_addr: SocketAddr,
) {
    let now = Instant::now();
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
}
