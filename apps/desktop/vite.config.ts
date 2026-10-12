import { resolve } from "node:path";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

const repo = resolve(import.meta.dirname, "../..");

// Tauri expects a fixed dev port and serves `dist/` in release builds.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  resolve: {
    alias: { "@icons": resolve(repo, "Resources/Icons") },
  },
  server: {
    port: 1420,
    strictPort: true,
    fs: { allow: [repo] },
  },
  preview: { port: 1420, strictPort: true },
  build: {
    // WebKitGTK 2.4x, WKWebView (macOS 14) and WebView2 all run ES2022.
    target: ["es2022", "safari16"],
    // Icons stay files: the CSP allows `img-src 'self'`, and nothing is inlined into JS.
    assetsInlineLimit: 0,
    sourcemap: false,
  },
  test: {
    include: ["src/**/*.test.ts"],
    environment: "node",
  },
});
