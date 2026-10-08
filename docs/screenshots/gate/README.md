# Gate G-UI screenshots: the Tauri candidate (G-01)

All taken on agentbox (Linux, no GPU, Xvfb), 1280 × 820 at 1x. Notes and numbers:
[`docs/research/g-01-tauri-spike.md`](../../research/g-01-tauri-spike.md).

| Folder | What | How |
|---|---|---|
| `webkitgtk/` | The real app (release build, WebKitGTK 2.52.6), fixture wallet, testnet: Overview, Transactions, transaction detail and lock screen, in light and dark | `apps/desktop/scripts/gate/linux-gate.sh` (self-test with `--selftest-pause`) |
| `webkitgtk/engine-regtest40k-*` | The real app on the real engine: the rust-dashcore regtest fixture wallet (40,000 blocks, ~40k transactions), encrypted vault (locked at launch, then unlocked with keystrokes) | the same harness with `--datadir`, and `PASSPHRASE_FILE` set |
| `playwright-chromium/`, `playwright-webkit/` | The renderer in Chromium (the engine inside WebView2) and Playwright's WebKit, fixture wallet; including the 920 × 600 minimum window | `DW_WRITE_SCREENSHOTS=1 pnpm e2e` |

macOS (WKWebView) and Windows (WebView2) screenshots come from G-02 on the CI runners.
