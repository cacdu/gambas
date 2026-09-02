// Canvas renderer: the board lives in an offscreen SIZE×SIZE canvas that is
// blitted to the screen with a pan/zoom transform (nearest-neighbor scaling).
import { PALETTE, SIZE } from "./board";

export class Renderer {
  private readonly ctx: CanvasRenderingContext2D;
  private readonly off: HTMLCanvasElement;
  private readonly offCtx: CanvasRenderingContext2D;
  private readonly image: ImageData;

  // Transform: screen = board * scale + offset (CSS pixels).
  scale = 2;
  offsetX = 0;
  offsetY = 0;

  // Minimum zoom: the largest integer scale where the whole board fits without padding.
  private fitScale = 2;
  private fitOffsetX = 0;
  private fitOffsetY = 0;

  hover: { x: number; y: number } | null = null;

  constructor(private readonly canvas: HTMLCanvasElement) {
    this.ctx = canvas.getContext("2d")!;
    this.off = document.createElement("canvas");
    this.off.width = SIZE;
    this.off.height = SIZE;
    this.offCtx = this.off.getContext("2d")!;
    this.image = this.offCtx.createImageData(SIZE, SIZE);
    this.setBoard(new Uint8Array(SIZE * SIZE));

    new ResizeObserver(() => this.resize()).observe(canvas);
    this.resize();
    this.fit();
  }

  setBoard(board: Uint8Array): void {
    for (let i = 0; i < board.length; i++) {
      this.putColor(i, board[i]);
    }
    this.flush();
  }

  setPixel(x: number, y: number, color: number): void {
    this.putColor(y * SIZE + x, color);
    this.flush();
  }

  /** Recompute the fit transform for the current viewport, without applying it. */
  private computeFit(): void {
    const { width, height } = this.viewport();
    this.fitScale = Math.max(1, Math.floor(Math.min(width, height) / SIZE));
    this.fitOffsetX = (width - SIZE * this.fitScale) / 2;
    this.fitOffsetY = (height - SIZE * this.fitScale) / 2;
  }

  private applyFit(): void {
    this.scale = this.fitScale;
    this.offsetX = this.fitOffsetX;
    this.offsetY = this.fitOffsetY;
  }

  /** Center the board at the largest integer scale that fills the viewport with no extra padding. */
  fit(): void {
    this.computeFit();
    this.applyFit();
    this.draw();
  }

  zoomAt(screenX: number, screenY: number, factor: number): void {
    const raw = this.scale * factor;
    if (raw <= this.fitScale) {
      // Snap back to perfect fit so repeated zoom-out cycles don't drift.
      this.applyFit();
    } else {
      const next = Math.min(64, raw);
      // Keep the board point under the cursor fixed while scaling.
      this.offsetX = screenX - ((screenX - this.offsetX) / this.scale) * next;
      this.offsetY = screenY - ((screenY - this.offsetY) / this.scale) * next;
      this.scale = next;
    }
    this.draw();
  }

  panBy(dx: number, dy: number): void {
    this.offsetX += dx;
    this.offsetY += dy;
    this.draw();
  }

  /** Screen (CSS px) → board coordinates, or null when outside the board. */
  pick(screenX: number, screenY: number): { x: number; y: number } | null {
    const x = Math.floor((screenX - this.offsetX) / this.scale);
    const y = Math.floor((screenY - this.offsetY) / this.scale);
    return x >= 0 && x < SIZE && y >= 0 && y < SIZE ? { x, y } : null;
  }

  draw(): void {
    const { width, height } = this.viewport();
    const dpr = window.devicePixelRatio || 1;
    this.ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    this.ctx.imageSmoothingEnabled = false;
    this.ctx.clearRect(0, 0, width, height);
    this.ctx.drawImage(
      this.off,
      this.offsetX,
      this.offsetY,
      SIZE * this.scale,
      SIZE * this.scale,
    );

    if (this.hover && this.scale >= 4) {
      this.ctx.strokeStyle = "#94E044";
      this.ctx.lineWidth = 1.5;
      this.ctx.strokeRect(
        this.offsetX + this.hover.x * this.scale,
        this.offsetY + this.hover.y * this.scale,
        this.scale,
        this.scale,
      );
    }
  }

  private putColor(index: number, color: number): void {
    const hex = PALETTE[color] ?? PALETTE[0];
    const base = index * 4;
    this.image.data[base] = parseInt(hex.slice(1, 3), 16);
    this.image.data[base + 1] = parseInt(hex.slice(3, 5), 16);
    this.image.data[base + 2] = parseInt(hex.slice(5, 7), 16);
    this.image.data[base + 3] = 255;
  }

  private flush(): void {
    this.offCtx.putImageData(this.image, 0, 0);
    this.draw();
  }

  private viewport(): { width: number; height: number } {
    return { width: this.canvas.clientWidth, height: this.canvas.clientHeight };
  }

  private resize(): void {
    const dpr = window.devicePixelRatio || 1;
    this.canvas.width = this.canvas.clientWidth * dpr;
    this.canvas.height = this.canvas.clientHeight * dpr;

    // A resize must not throw away the user's zoom/pan: only re-center when the
    // view was already at fit, or when the new viewport leaves it below fit.
    const wasAtFit = this.scale <= this.fitScale;
    this.computeFit();
    if (wasAtFit || this.scale < this.fitScale) {
      this.applyFit();
    }
    this.draw();
  }
}
