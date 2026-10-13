#!/usr/bin/env bash
# tauri-driver end-to-end smoke on Linux (U-07): builds the app (production renderer, debug
# binary with the real engine) and dwcli, then runs e2e-tauri/smoke.ts. CI runs exactly this
# under xvfb-run. Needs a private X display in $DISPLAY (xvfb-run, or `agent-gui start N` on
# agentbox), WebKitWebDriver (Ubuntu: webkit2gtk-driver) and tauri-driver on PATH or in
# $TAURI_DRIVER (`cargo install tauri-driver --version 2.1.0 --locked`).
#
#   DW_E2E_SKIP_BUILD=1   reuse rust/target/debug/{dash-wallet,dwcli}
#   DW_E2E_OUT=<dir>      screenshots, logs and result.json (default test-results/tauri-e2e)
set -euo pipefail
cd "$(dirname "$0")/.."
repo=$(cd ../.. && pwd)

if [[ -z "${DISPLAY:-}" ]]; then
  echo "e2e-tauri: set DISPLAY to a private X display (xvfb-run, agent-gui)" >&2
  exit 2
fi
command -v WebKitWebDriver >/dev/null || { echo "e2e-tauri: WebKitWebDriver not found (apt install webkit2gtk-driver)" >&2; exit 2; }
driver=${TAURI_DRIVER:-$(command -v tauri-driver || true)}
[[ -x "$driver" ]] || { echo "e2e-tauri: tauri-driver not found; set TAURI_DRIVER" >&2; exit 2; }

if [[ "${DW_E2E_SKIP_BUILD:-}" != 1 ]]; then
  # `--debug` keeps the build quick; the renderer is still the production `dist/` (`pnpm build`,
  # with its fixture-free check) embedded through Tauri's custom protocol.
  pnpm tauri build --debug --no-bundle
  (cd "$repo/rust" && cargo build -p dwcli --locked)
fi

DW_APP="$repo/rust/target/debug/dash-wallet" DWCLI="$repo/rust/target/debug/dwcli" TAURI_DRIVER="$driver" \
  node e2e-tauri/smoke.ts
