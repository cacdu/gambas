import { defineConfig } from "vite";

// Dev-only proxy to a locally running gambas node (see README).
export default defineConfig({
  server: {
    proxy: {
      "/api": "http://127.0.0.1:8081",
      "/ws": { target: "ws://127.0.0.1:8081", ws: true },
    },
  },
});
