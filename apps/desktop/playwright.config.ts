import { defineConfig } from "@playwright/test";

// Fixture runs of the built renderer (`vite preview`) in Chromium (the engine inside WebView2)
// and WebKit (close to WKWebView and WebKitGTK). The viewport equals the Tauri window's inner
// size, so element boxes compare across engines and with `--selftest` (U3c).
export default defineConfig({
  testDir: "e2e",
  outputDir: "test-results",
  fullyParallel: true,
  reporter: [["list"]],
  webServer: { command: "pnpm preview", port: 1420, reuseExistingServer: true },
  use: {
    baseURL: "http://localhost:1420",
    viewport: { width: 1280, height: 820 },
    deviceScaleFactor: 1,
    locale: "en-US",
  },
  projects: [
    { name: "chromium", use: { browserName: "chromium" } },
    { name: "webkit", use: { browserName: "webkit" } },
  ],
});
