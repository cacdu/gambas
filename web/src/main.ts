import { PALETTE } from "./board";
import { connectStream, fetchStatus, paint } from "./net";
import { Renderer } from "./render";

const canvas = document.getElementById("canvas") as HTMLCanvasElement;
const paletteEl = document.getElementById("palette")!;
const cooldownEl = document.getElementById("cooldown")!;
const nodeEl = document.getElementById("node")!;
const coordsEl = document.getElementById("coords")!;
const connEl = document.getElementById("conn")!;
const toastEl = document.getElementById("toast")!;

const renderer = new Renderer(canvas);
let selectedColor = 5; // red
let cooldownUntil = 0;

// ── palette ──────────────────────────────────────────────────────────────────

PALETTE.forEach((hex, i) => {
  const btn = document.createElement("button");
  btn.style.background = hex;
  btn.title = hex;
  btn.onclick = () => {
    selectedColor = i;
    paletteEl.querySelector(".selected")?.classList.remove("selected");
    btn.classList.add("selected");
  };
  if (i === selectedColor) btn.classList.add("selected");
  paletteEl.appendChild(btn);
});

// ── live board stream ────────────────────────────────────────────────────────

connectStream({
  onBoard: (board) => renderer.setBoard(board),
  onDelta: ({ x, y, color }) => renderer.setPixel(x, y, color),
  onConnection: (online) => connEl.classList.toggle("online", online),
});

// ── pan / zoom / paint ───────────────────────────────────────────────────────

let dragging = false;
let dragMoved = false;
let lastX = 0;
let lastY = 0;

canvas.addEventListener("pointerdown", (e) => {
  dragging = true;
  dragMoved = false;
  lastX = e.clientX;
  lastY = e.clientY;
  canvas.setPointerCapture(e.pointerId);
});

canvas.addEventListener("pointermove", (e) => {
  const rect = canvas.getBoundingClientRect();
  const sx = e.clientX - rect.left;
  const sy = e.clientY - rect.top;

  if (dragging) {
    const dx = e.clientX - lastX;
    const dy = e.clientY - lastY;
    if (Math.abs(dx) + Math.abs(dy) > 2) dragMoved = true;
    renderer.panBy(dx, dy);
    lastX = e.clientX;
    lastY = e.clientY;
  }

  const cell = renderer.pick(sx, sy);
  renderer.hover = cell;
  coordsEl.textContent = cell ? `(${cell.x}, ${cell.y})` : "";
  renderer.draw();
});

canvas.addEventListener("pointerup", async (e) => {
  dragging = false;
  if (dragMoved) return; // it was a pan, not a click

  const rect = canvas.getBoundingClientRect();
  const cell = renderer.pick(e.clientX - rect.left, e.clientY - rect.top);
  if (!cell) return;

  if (Date.now() < cooldownUntil) {
    toast("hold on — cooldown");
    return;
  }

  const result = await paint(cell.x, cell.y, selectedColor);
  if (result.ok) {
    // Optimistic paint; the authoritative delta arrives over the WS.
    renderer.setPixel(cell.x, cell.y, selectedColor);
  } else {
    toast(result.error === "cooldown" ? "hold on — cooldown" : `paint failed: ${result.error}`);
  }
  if (result.cooldownMs > 0) {
    cooldownUntil = Date.now() + result.cooldownMs;
  }
});

canvas.addEventListener("pointerleave", () => {
  renderer.hover = null;
  coordsEl.textContent = "";
  renderer.draw();
});

canvas.addEventListener(
  "wheel",
  (e) => {
    e.preventDefault();
    const rect = canvas.getBoundingClientRect();
    const factor = e.deltaY < 0 ? 1.2 : 1 / 1.2;
    renderer.zoomAt(e.clientX - rect.left, e.clientY - rect.top, factor);
  },
  { passive: false },
);

// ── status bar ───────────────────────────────────────────────────────────────

function toast(message: string): void {
  toastEl.textContent = message;
  toastEl.hidden = false;
  setTimeout(() => (toastEl.hidden = true), 1500);
}

setInterval(() => {
  const remaining = cooldownUntil - Date.now();
  if (remaining > 0) {
    cooldownEl.textContent = `next pixel in ${(remaining / 1000).toFixed(1)}s`;
    cooldownEl.classList.add("active");
  } else {
    cooldownEl.textContent = "ready to paint";
    cooldownEl.classList.remove("active");
  }
}, 100);

async function refreshStatus(): Promise<void> {
  const status = await fetchStatus();
  if (!status) return;
  if (status.cooldown_remaining_ms) {
    cooldownUntil = Math.max(cooldownUntil, Date.now() + status.cooldown_remaining_ms);
  }
  const role = status.is_leader ? "leader" : `follower of #${status.leader_id ?? "?"}`;
  nodeEl.textContent = `you are on node #${status.node_id} (${role})`;
}

refreshStatus();
setInterval(refreshStatus, 10_000);
