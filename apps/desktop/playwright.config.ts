import { defineConfig } from "@playwright/test";

// Runs of the fixture build (`pnpm build:fixture`, served by `vite preview --mode fixture`) on
// the fixture backend alone, no engine (U-07), in Chromium (the engine inside WebView2) and
// WebKit (close to WKWebView and WebKitGTK). The viewport equals the Tauri window's inner
// size, so element boxes compare across engines and with `--selftest` (U3c). The gate runs the
// style spec again at its self-test size: DW_VIEWPORT=1000x620 (as the G-02 gate did).
const [width, height] = (process.env.DW_VIEWPORT ?? "1280x820").split("x").map(Number);
export default defineConfig({
  testDir: "e2e",
  outputDir: "test-results",
  fullyParallel: true,
  reporter: [["list"], ["html", { open: "never" }]],
  webServer: { command: "pnpm preview:fixture", port: 1420, reuseExistingServer: !process.env.CI },
  use: {
    baseURL: "http://localhost:1420",
    viewport: { width, height },
    deviceScaleFactor: 1,
    locale: "en-US",
  },
  projects: [
    { name: "chromium", use: { browserName: "chromium" } },
    { name: "webkit", use: { browserName: "webkit" } },
  ],
});
