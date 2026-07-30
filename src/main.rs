/// gambas — a distributed pixel canvas.
///
/// Every replica of this binary is a full Raft node: pixels are keys in an
/// embedded raft-kv store, replicated by consensus, and fanned out to
/// browsers over WebSockets as they apply locally.
mod board;
mod cooldown;
mod http;
mod metrics;
mod ws;

use std::{collections::HashMap, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use anyhow::Result;
use clap::Parser;
use tracing::info;

use raft_kv::{NodeId, RaftKv, RaftKvOptions};

#[derive(Debug, Parser)]
#[command(
    name = "gambas",
    about = "Distributed pixel canvas on embedded raft-kv"
)]
struct Config {
    /// Unique node id within the cluster.
    #[arg(long)]
    id: NodeId,

    /// Bind address for Raft gRPC between replicas (e.g. 0.0.0.0:7001).
    #[arg(long)]
    raft_addr: String,

    /// Bind address for the web app.
    #[arg(long, default_value = "0.0.0.0:8080")]
    http_addr: String,

    /// Peer Raft addresses: repeated --peer id=host:port
    #[arg(long = "peer", value_parser = parse_peer)]
    peers: Vec<(NodeId, String)>,

    /// Peer app (HTTP) addresses for leader forwarding: repeated --app-peer id=host:port
    #[arg(long = "app-peer", value_parser = parse_peer)]
    app_peers: Vec<(NodeId, String)>,

    #[arg(long, default_value = "data")]
    data_dir: PathBuf,

    /// Seconds a client must wait between pixels. 0 disables the cooldown.
    #[arg(long, default_value_t = 5)]
    cooldown_secs: u64,

    /// Trust the configured client-IP header (set when behind the LB).
    #[arg(long, default_value_t = false)]
    behind_proxy: bool,

    /// Header carrying the real client IP when behind a proxy. Default
    /// x-forwarded-for (nginx); set to fly-client-ip on Fly.io.
    #[arg(long, default_value = "x-forwarded-for")]
    trusted_ip_header: String,

    /// Directory with the built frontend.
    #[arg(long, default_value = "web/dist")]
    static_dir: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("gambas=info".parse()?)
                .add_directive("raft_kv=info".parse()?),
        )
        .init();

    let cfg = Config::parse();
    info!(id = cfg.id, raft = %cfg.raft_addr, http = %cfg.http_addr, "starting gambas node");

    let node = RaftKv::start(RaftKvOptions {
        id: cfg.id,
        raft_addr: cfg.raft_addr.clone(),
        peers: cfg.peers.iter().cloned().collect::<HashMap<_, _>>(),
        app_addrs: cfg.app_peers.iter().cloned().collect::<HashMap<_, _>>(),
        data_dir: cfg.data_dir.clone(),
        learner: false,
    })
    .await?;

    let state = http::AppState {
        node,
        cooldown: Arc::new(cooldown::Cooldown::new(Duration::from_secs(
            cfg.cooldown_secs,
        ))),
        behind_proxy: cfg.behind_proxy,
        trusted_ip_header: cfg.trusted_ip_header.clone().into(),
        http_client: reqwest::Client::new(),
    };
    let app = http::router(state, cfg.static_dir.clone());

    let addr: SocketAddr = cfg.http_addr.parse()?;
    info!("serving canvas on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}

fn parse_peer(s: &str) -> Result<(NodeId, String), String> {
    let (id, addr) = s
        .split_once('=')
        .ok_or_else(|| format!("expected id=addr, got '{s}'"))?;
    let id: NodeId = id.parse().map_err(|_| format!("invalid node id: {id}"))?;
    Ok((id, addr.to_string()))
}
