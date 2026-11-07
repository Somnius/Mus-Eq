// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
import { defineConfig } from "vite";

// In dev the page is served by Vite and proxies the API (and its WebSocket) to the server,
// so the login cookie stays same-origin. In production the server serves the built files itself.
const target = process.env.MUSEQ_URL ?? "http://127.0.0.1:8080";

export default defineConfig({
  server: { proxy: { "/v1": { target, ws: true, changeOrigin: false } } },
  build: { outDir: "dist", sourcemap: false },
});
