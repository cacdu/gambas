// Must mirror src/board.rs on the server.
export const SIZE = 256;

export const PALETTE = [
  "#FFFFFF", "#E4E4E4", "#888888", "#222222",
  "#FFA7D1", "#E50000", "#E59500", "#A06A42",
  "#E5D900", "#94E044", "#02BE01", "#00D3DD",
  "#0083C7", "#0000EA", "#CF6EE4", "#820080",
] as const;

export interface Delta {
  x: number;
  y: number;
  color: number;
}
