/// HTTP API.
///
/// POST /api/pixel   {x, y, color} → paint one pixel (cooldown per client IP)
/// GET  /api/status  → node role, canvas config, caller's remaining cooldown
/// GET  /ws          → WebSocket: full board + live pixel deltas
/// GET  /metrics     → Prometheus (gambas + embedded raft-kv metrics)
/// GET  /*           → static frontend
///
/// Any replica accepts paints. A follower forwards the write to the leader
/// over the internal network, marked with `x-gambas-internal` so the leader
/// skips its own cooldown (the edge node already charged the client) and
/// never re-forwards. Nodes must not be reachable except through the LB.
use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use axum::{
    extract::{ws::WebSocketUpgrade, ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tower_http::services::{ServeDir, ServeFile};
use tracing::warn;

use raft_kv::RaftKv;

use crate::{board, cooldown::Cooldown, metrics, ws};

const INTERNAL_HEADER: &str = "x-gambas-internal";

#[derive(Clone)]
pub struct AppState {
    pub node: RaftKv,
    pub cooldown: Arc<Cooldown>,
    pub behind_proxy: bool,
    /// Header to read the real client IP from when `behind_proxy` (e.g.
    /// `x-forwarded-for` behind nginx, `fly-client-ip` behind Fly's proxy).
    pub trusted_ip_header: Arc<str>,
    /// Shared secret proving a request is an internal leader-forward, set on
    /// every node of a cluster. `None` = single node: nothing is internal.
    pub internal_secret: Option<Arc<str>>,
    pub http_client: reqwest::Client,
}

pub fn router(state: AppState, static_dir: PathBuf) -> Router {
    let index = ServeFile::new(static_dir.join("index.html"));
    Router::new()
        .route("/api/pixel", post(paint))
        .route("/api/status", get(status))
        .route("/ws", get(ws_upgrade))
        .route("/metrics", get(metrics_handler))
        .fallback_service(ServeDir::new(static_dir).fallback(index))
        .with_state(state)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PaintRequest {
    pub x: u16,
    pub y: u16,
    pub color: u8,
}

async fn paint(
    State(s): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<PaintRequest>,
) -> Response {
    if req.x >= board::WIDTH || req.y >= board::HEIGHT || req.color as usize >= board::PALETTE.len()
    {
        metrics::PAINT_REJECTED
            .with_label_values(&["invalid"])
            .inc();
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "out of range" })),
        )
            .into_response();
    }

    // Forwarded writes were already charged a cooldown slot by the edge node.
    let internal = is_internal(&headers, s.internal_secret.as_deref());
    let ip = client_ip(&headers, peer, s.behind_proxy, &s.trusted_ip_header);
    if !internal {
        if let Err(wait) = s.cooldown.try_claim(ip) {
            metrics::PAINT_REJECTED
                .with_label_values(&["cooldown"])
                .inc();
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({ "error": "cooldown", "retry_after_ms": wait.as_millis() as u64 })),
            )
                .into_response();
        }
    }

    let key = board::pixel_key(req.x, req.y);
    let result = s.node.put(key, req.color.to_string()).await;

    let response = match result {
        Ok(()) => {
            metrics::PIXELS_PAINTED.inc();
            let cooldown_ms = s.cooldown.period().as_millis() as u64;
            (
                StatusCode::OK,
                Json(json!({ "ok": true, "cooldown_ms": cooldown_ms })),
            )
                .into_response()
        }
        Err(raft_kv::Error::NotLeader {
            leader_addr: Some(addr),
            ..
        }) if !internal => forward_to_leader(&s, &addr, req).await,
        Err(e) => {
            warn!("paint failed: {e}");
            metrics::PAINT_REJECTED
                .with_label_values(&["unavailable"])
                .inc();
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    };

    // Don't charge the client for a pixel that never committed.
    if !internal && !response.status().is_success() {
        s.cooldown.rollback(ip);
    }
    response
}

/// Relay a paint to the leader's app address. The client keeps talking to one
/// node; consensus topology stays invisible to the frontend.
async fn forward_to_leader(s: &AppState, leader_addr: &str, req: PaintRequest) -> Response {
    let url = format!("http://{leader_addr}/api/pixel");
    let mut builder = s
        .http_client
        .post(&url)
        .json(&req)
        .timeout(Duration::from_secs(6));
    if let Some(secret) = s.internal_secret.as_deref() {
        builder = builder.header(INTERNAL_HEADER, secret);
    }
    let sent = builder.send().await;
    match sent {
        Ok(resp) => {
            let status =
                StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let body = resp.bytes().await.unwrap_or_default();
            (status, [("content-type", "application/json")], body).into_response()
        }
        Err(e) => {
            warn!("forward to leader {leader_addr} failed: {e}");
            metrics::PAINT_REJECTED
                .with_label_values(&["forward"])
                .inc();
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "leader unreachable" })),
            )
                .into_response()
        }
    }
}

async fn status(
    State(s): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Json<serde_json::Value> {
    let raft = s.node.status().await;
    let ip = client_ip(&headers, peer, s.behind_proxy, &s.trusted_ip_header);
    let remaining_ms = s.cooldown.remaining(ip).map(|d| d.as_millis() as u64);
    Json(json!({
        "node_id": raft.id,
        "is_leader": raft.is_leader,
        "leader_id": raft.leader_id,
        "canvas": {
            "width": board::WIDTH,
            "height": board::HEIGHT,
            "palette": board::PALETTE,
            "cooldown_ms": s.cooldown.period().as_millis() as u64,
        },
        "cooldown_remaining_ms": remaining_ms,
    }))
}

async fn ws_upgrade(State(s): State<AppState>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| ws::session(socket, s))
}

async fn metrics_handler() -> Response {
    use prometheus::TextEncoder;
    match TextEncoder::new().encode_to_string(&prometheus::gather()) {
        Ok(body) => (
            StatusCode::OK,
            [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
            body,
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// A request is an internal leader-forward iff it carries the shared secret in
/// the internal header. Presence alone is not enough — nginx and Fly's proxy
/// both pass client-supplied headers through, so an unauthenticated header
/// would let any client skip the cooldown.
fn is_internal(headers: &HeaderMap, secret: Option<&str>) -> bool {
    let Some(secret) = secret else {
        return false;
    };
    headers
        .get(INTERNAL_HEADER)
        .map(|v| ct_eq(v.as_bytes(), secret.as_bytes()))
        .unwrap_or(false)
}

/// Constant-time byte equality: no early return on the first mismatch, so a
/// timing side-channel can't reveal how many leading bytes were guessed. The
/// length may short-circuit — it isn't secret.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Client IP for rate limiting: the first hop of the configured trusted header
/// when running behind the LB, the socket peer otherwise.
///
/// The header MUST be one the proxy sets itself and a client cannot forge past
/// it. Behind nginx that's `x-forwarded-for` (overwritten with `$remote_addr`);
/// behind Fly's proxy — which *appends* to a client-supplied XFF — it must be
/// `fly-client-ip`, which Fly sets to the real peer.
fn client_ip(headers: &HeaderMap, peer: SocketAddr, behind_proxy: bool, header: &str) -> IpAddr {
    if behind_proxy {
        if let Some(ip) = headers
            .get(header)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .and_then(|v| v.trim().parse().ok())
        {
            return ip;
        }
    }
    peer.ip()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer() -> SocketAddr {
        "192.168.1.9:55555".parse().unwrap()
    }

    #[test]
    fn client_ip_uses_peer_when_not_behind_proxy() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "1.2.3.4".parse().unwrap());
        let ip = client_ip(&headers, peer(), false, "x-forwarded-for");
        assert_eq!(ip, "192.168.1.9".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn client_ip_takes_first_forwarded_hop() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "1.2.3.4, 10.0.0.1".parse().unwrap());
        let ip = client_ip(&headers, peer(), true, "x-forwarded-for");
        assert_eq!(ip, "1.2.3.4".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn client_ip_falls_back_on_garbage_header() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "not-an-ip".parse().unwrap());
        let ip = client_ip(&headers, peer(), true, "x-forwarded-for");
        assert_eq!(ip, "192.168.1.9".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn client_ip_reads_the_configured_header_only() {
        let mut headers = HeaderMap::new();
        // A spoofed XFF is ignored when the trusted header is fly-client-ip...
        headers.insert("x-forwarded-for", "1.2.3.4".parse().unwrap());
        headers.insert("fly-client-ip", "203.0.113.7".parse().unwrap());
        let ip = client_ip(&headers, peer(), true, "fly-client-ip");
        assert_eq!(ip, "203.0.113.7".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn internal_requires_the_matching_secret() {
        let mut headers = HeaderMap::new();
        headers.insert(INTERNAL_HEADER, "s3cr3t".parse().unwrap());
        assert!(is_internal(&headers, Some("s3cr3t")));
    }

    #[test]
    fn internal_rejects_wrong_or_forged_values() {
        let mut headers = HeaderMap::new();
        headers.insert(INTERNAL_HEADER, "wrong".parse().unwrap());
        assert!(!is_internal(&headers, Some("s3cr3t")));
        // The old presence-only sentinel "1" must no longer pass.
        headers.insert(INTERNAL_HEADER, "1".parse().unwrap());
        assert!(!is_internal(&headers, Some("s3cr3t")));
    }

    #[test]
    fn internal_is_false_without_a_configured_secret() {
        let mut headers = HeaderMap::new();
        headers.insert(INTERNAL_HEADER, "anything".parse().unwrap());
        assert!(!is_internal(&headers, None));
    }

    #[test]
    fn internal_is_false_when_header_absent() {
        let headers = HeaderMap::new();
        assert!(!is_internal(&headers, Some("s3cr3t")));
    }
}
