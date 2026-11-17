use crate::net::nat::discover_lan_broadcast_addrs;
use anyhow::Result;
use socket2::{Domain, Protocol, Socket, Type};
use squad_proto::{LanDiscoveryPacket, RoomId, LAN_DISCOVERY_PORT};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{info, warn};

pub const DEFAULT_RENDEZVOUS_PORT: u16 = 3478;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredLanRoom {
    pub room_id: RoomId,
    pub secret: [u8; 32],
    pub host_addr: SocketAddr,
    pub room_name: String,
    pub host_nickname: String,
    pub peer_count: u8,
    pub max_peers: u8,
}

pub struct EmbeddedRendezvousHandle {
    pub port: u16,
    shutdown_tx: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
}

impl EmbeddedRendezvousHandle {
    pub async fn stop(mut self) {
        let _ = self.shutdown_tx.send(true);
        if let Some(handle) = self.task.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_millis(120), handle).await;
        }
    }
}

impl Drop for EmbeddedRendezvousHandle {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(true);
    }
}

/// Starts an embedded `squad-rendezvous` server inside `squad-client`.
/// Tries the default port 3478 first and falls back to an OS-assigned ephemeral port on conflict.
pub async fn start_embedded_rendezvous() -> Result<EmbeddedRendezvousHandle> {
    let socket = match UdpSocket::bind(("0.0.0.0", DEFAULT_RENDEZVOUS_PORT)).await {
        Ok(s) => s,
        Err(err) => {
            warn!(
                "Default rendezvous port {} busy ({}); binding ephemeral LAN port",
                DEFAULT_RENDEZVOUS_PORT, err
            );
            UdpSocket::bind("0.0.0.0:0").await?
        }
    };
    let port = socket.local_addr()?.port();
    let sock_arc = Arc::new(socket);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let task = tokio::spawn(async move {
        if let Err(err) = squad_rendezvous::run_server(sock_arc, shutdown_rx).await {
            warn!("Embedded LAN rendezvous server exited with error: {err}");
        }
    });

    info!("Started embedded LAN rendezvous server on UDP port {port}");
    Ok(EmbeddedRendezvousHandle {
        port,
        shutdown_tx,
        task: Some(task),
    })
}

/// Binds a broadcast-capable UDP discovery listener with `SO_REUSEADDR` (and `SO_REUSEPORT` on Unix)
/// so multiple local clients can coexist cleanly on the same machine.
pub fn bind_lan_discovery_listener(port: u16) -> Result<UdpSocket> {
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    sock.set_reuse_address(true)?;
    #[cfg(all(unix, not(target_os = "solaris"), not(target_os = "illumos")))]
    {
        let _ = sock.set_reuse_port(true);
    }
    sock.set_broadcast(true)?;
    sock.set_nonblocking(true)?;
    let bind_addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port);
    sock.bind(&bind_addr.into())?;
    let std_sock: std::net::UdpSocket = sock.into();
    Ok(UdpSocket::from_std(std_sock)?)
}

pub fn bind_lan_broadcast_sender() -> Result<UdpSocket> {
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    sock.set_broadcast(true)?;
    sock.set_nonblocking(true)?;
    let bind_addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
    sock.bind(&bind_addr.into())?;
    let std_sock: std::net::UdpSocket = sock.into();
    Ok(UdpSocket::from_std(std_sock)?)
}

pub async fn broadcast_lan_packet(tx_sock: &UdpSocket, pkt: &LanDiscoveryPacket) {
    let Ok(bytes) = pkt.encode() else {
        return;
    };
    for dst in discover_lan_broadcast_addrs(LAN_DISCOVERY_PORT) {
        let _ = tx_sock.send_to(&bytes, dst).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use squad_proto::{SignalingPacket, MAX_UDP_PAYLOAD};
    use std::time::Duration;

    #[tokio::test]
    async fn test_embedded_rendezvous_port_fallback_and_clean_shutdown() {
        // Hold default 3478 or simulate busy default socket first
        let rv_a = start_embedded_rendezvous().await.unwrap();
        let rv_b = start_embedded_rendezvous().await.unwrap();
        assert_ne!(
            rv_a.port, 0,
            "First embedded rendezvous must bind a valid port"
        );
        assert_ne!(
            rv_b.port, rv_a.port,
            "Second embedded rendezvous must fall back to an ephemeral port without conflict"
        );

        let cli = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let reg = SignalingPacket::C2sRegister {
            room_id: [0x77; 16],
            client_uuid: [0x88u8; 16],
            nickname: "LanHostTest".into(),
            lan_addrs: Vec::new(),
        };
        cli.send_to(&reg.encode().unwrap(), ("127.0.0.1", rv_b.port))
            .await
            .unwrap();

        let mut buf = [0u8; MAX_UDP_PAYLOAD];
        let (len, _) = tokio::time::timeout(Duration::from_secs(1), cli.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
        match SignalingPacket::decode(&buf[..len]).unwrap() {
            SignalingPacket::S2cRoomState {
                assigned_peer_id, ..
            } => assert_eq!(assigned_peer_id, 1),
            other => panic!("Expected S2cRoomState, got {other:?}"),
        }

        rv_b.stop().await;
        rv_a.stop().await;
    }
}
