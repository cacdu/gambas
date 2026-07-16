/// WebSocket fan-out: every browser gets the full board on connect, then a
/// 6-byte delta per pixel as writes apply on *this* node.
///
/// The event subscription is opened *before* the board snapshot is taken, so
/// no write can fall between them — at worst a delta repeats a pixel already
/// in the snapshot, and repainting a pixel is idempotent.
///
/// Wire format (binary frames, server → client):
///   [0x01][width*height bytes]                  full board, row-major palette indices
///   [0x02][x: u16 BE][y: u16 BE][color: u8]     one pixel changed
use axum::extract::ws::{Message, WebSocket};
use tokio::sync::broadcast::error::RecvError;
use tracing::debug;

use raft_kv::Event;

use crate::{board, http::AppState, metrics};

const FRAME_BOARD: u8 = 0x01;
const FRAME_DELTA: u8 = 0x02;

pub async fn session(mut socket: WebSocket, state: AppState) {
    metrics::WS_CLIENTS.inc();
    let _guard = scopeguard();

    // Subscribe first, snapshot second — see module docs.
    let mut events = state.node.subscribe();
    if send_board(&mut socket, &state).await.is_err() {
        return;
    }

    loop {
        tokio::select! {
            event = events.recv() => {
                let frame = match event {
                    Ok(Event::Set { key, value }) => {
                        match delta_frame(&key, &value) {
                            Some(f) => f,
                            None => continue, // not a pixel key — some other tenant of the store
                        }
                    }
                    // Pixels are never deleted; ignore.
                    Ok(Event::Delete { .. }) => continue,
                    // Local state was replaced wholesale, or we missed events:
                    // the only correct move is a full resync.
                    Ok(Event::SnapshotApplied) | Err(RecvError::Lagged(_)) => {
                        debug!("ws client resync (snapshot or lag)");
                        if send_board(&mut socket, &state).await.is_err() {
                            break;
                        }
                        continue;
                    }
                    Err(RecvError::Closed) => break,
                };
                if socket.send(Message::Binary(frame.into())).await.is_err() {
                    break;
                }
            }
            msg = socket.recv() => {
                match msg {
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                    // Clients don't send data; axum answers pings itself.
                    Some(Ok(_)) => {}
                }
            }
        }
    }
}

async fn send_board(socket: &mut WebSocket, state: &AppState) -> Result<(), axum::Error> {
    let board = board::snapshot(&state.node).await;
    let mut frame = Vec::with_capacity(1 + board.len());
    frame.push(FRAME_BOARD);
    frame.extend_from_slice(&board);
    socket.send(Message::Binary(frame.into())).await
}

fn delta_frame(key: &str, value: &str) -> Option<Vec<u8>> {
    let (x, y) = board::parse_pixel_key(key)?;
    let color = board::parse_color(value)?;
    let mut f = Vec::with_capacity(6);
    f.push(FRAME_DELTA);
    f.extend_from_slice(&x.to_be_bytes());
    f.extend_from_slice(&y.to_be_bytes());
    f.push(color);
    Some(f)
}

fn scopeguard() -> impl Drop {
    struct Dec;
    impl Drop for Dec {
        fn drop(&mut self) {
            metrics::WS_CLIENTS.dec();
        }
    }
    Dec
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_frame_encodes_coordinates_big_endian() {
        let f = delta_frame("px:001:255", "15").unwrap();
        assert_eq!(f, vec![FRAME_DELTA, 0, 1, 0, 255, 15]);
    }

    #[test]
    fn delta_frame_ignores_foreign_keys() {
        assert_eq!(delta_frame("config:motd", "hi"), None);
        assert_eq!(delta_frame("px:999:000", "1"), None);
    }
}
