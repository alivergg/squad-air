use anyhow::Result;
use squad_rendezvous::RendezvousConfig;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "squad_rendezvous=info".into()),
        )
        .init();

    let bind_addr = parse_bind_arg().unwrap_or_else(|| "0.0.0.0:3478".to_string());
    let enable_relay = parse_enable_relay_arg();
    let socket = Arc::new(UdpSocket::bind(&bind_addr).await?);
    info!(
        "SquadAir Rendezvous server listening on {} (server relay: {})",
        socket.local_addr()?,
        if enable_relay {
            "enabled for voice & chat; video stream blocked"
        } else {
            "disabled (signaling-only; pass --enable-relay to allow voice/chat relay)"
        }
    );

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = shutdown_tx.send(true);
    });

    squad_rendezvous::run_server_with_config(
        socket,
        RendezvousConfig { enable_relay },
        shutdown_rx,
    )
    .await
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

fn parse_enable_relay_arg() -> bool {
    std::env::args().any(|arg| arg == "--enable-relay" || arg == "--relay")
}
