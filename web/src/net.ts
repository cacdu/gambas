// Server I/O: WebSocket stream (full board + deltas) and the paint endpoint.
//
// Binary frames from the server:
//   [0x01][SIZE*SIZE bytes]                 full board
//   [0x02][x: u16 BE][y: u16 BE][color u8]  single pixel
import type { Delta } from "./board";

const FRAME_BOARD = 0x01;
const FRAME_DELTA = 0x02;

export interface StreamHandlers {
  onBoard(board: Uint8Array): void;
  onDelta(delta: Delta): void;
  onConnection(online: boolean): void;
}

export function connectStream(handlers: StreamHandlers): void {
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  let retryMs = 500;

  const open = () => {
    const ws = new WebSocket(`${proto}//${location.host}/ws`);
    ws.binaryType = "arraybuffer";

    ws.onopen = () => {
      retryMs = 500;
      handlers.onConnection(true);
    };

    ws.onmessage = (ev: MessageEvent<ArrayBuffer>) => {
      const view = new DataView(ev.data);
      switch (view.getUint8(0)) {
        case FRAME_BOARD:
          handlers.onBoard(new Uint8Array(ev.data, 1));
          break;
        case FRAME_DELTA:
          handlers.onDelta({
            x: view.getUint16(1),
            y: view.getUint16(3),
            color: view.getUint8(5),
          });
          break;
      }
    };

    ws.onclose = () => {
      handlers.onConnection(false);
      setTimeout(open, retryMs);
      retryMs = Math.min(retryMs * 2, 10_000);
    };
  };

  open();
}

export interface PaintResult {
  ok: boolean;
  cooldownMs: number;
  error?: string;
}

export async function paint(x: number, y: number, color: number): Promise<PaintResult> {
  const resp = await fetch("/api/pixel", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ x, y, color }),
  });
  const body = await resp.json().catch(() => ({}));
  if (resp.ok) {
    return { ok: true, cooldownMs: body.cooldown_ms ?? 0 };
  }
  return {
    ok: false,
    cooldownMs: body.retry_after_ms ?? 0,
    error: body.error ?? `HTTP ${resp.status}`,
  };
}

export interface NodeStatus {
  node_id: number;
  is_leader: boolean;
  leader_id: number | null;
  cooldown_remaining_ms: number | null;
  canvas: { cooldown_ms: number };
}

export async function fetchStatus(): Promise<NodeStatus | null> {
  try {
    const resp = await fetch("/api/status");
    return resp.ok ? await resp.json() : null;
  } catch {
    return null;
  }
}
