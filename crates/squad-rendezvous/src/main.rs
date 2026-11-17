use anyhow::Result;
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
    let socket = Arc::new(UdpSocket::bind(&bind_addr).await?);
    info!(
        "SquadAir Rendezvous server listening on {}",
        socket.local_addr()?
    );

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = shutdown_tx.send(true);
    });

    squad_rendezvous::run_server(socket, shutdown_rx).await
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
