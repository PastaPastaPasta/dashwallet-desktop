#!/usr/bin/env bash
# tauri-driver end-to-end smoke on Linux (U-07): builds the app (production renderer, debug
# binary with the real engine) and dwcli, then runs e2e-tauri/smoke.ts. CI runs exactly this
# under xvfb-run. Needs a private X display in $DISPLAY (xvfb-run, or `agent-gui start N` on
# agentbox), WebKitWebDriver (Ubuntu: webkit2gtk-driver) and tauri-driver on PATH or in
# $TAURI_DRIVER (`cargo install tauri-driver --version 2.1.0 --locked`).
#
#   DW_E2E_SKIP_BUILD=1   reuse <target>/debug/{dash-wallet,dwcli} (<target>: $CARGO_TARGET_DIR,
#                         else rust/target)
#   DW_E2E_OUT=<dir>      screenshots, logs and result.json (default test-results/tauri-e2e)
#   DW_E2E_SCRIPT=<file>  the script to run instead of e2e-tauri/smoke.ts
#                         (scripts/e2e-tauri-onboarding.sh: e2e-tauri/onboarding.ts)
set -euo pipefail
cd "$(dirname "$0")/.."
repo=$(cd ../.. && pwd)

# The single-instance plugin (a second launch hands its links over) needs a session bus: a
# private one, so the smoke never meets an app on the desktop's bus.
if [[ -z "${DW_E2E_PRIVATE_BUS:-}" ]]; then
  command -v dbus-run-session >/dev/null || { echo "e2e-tauri: dbus-run-session not found (apt install dbus)" >&2; exit 2; }
  DW_E2E_PRIVATE_BUS=1 exec dbus-run-session -- "$PWD/scripts/e2e-tauri.sh" "$@"
fi

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

target=${CARGO_TARGET_DIR:-$repo/rust/target}
DW_APP="$target/debug/dash-wallet" DWCLI="$target/debug/dwcli" TAURI_DRIVER="$driver" \
  node "${DW_E2E_SCRIPT:-e2e-tauri/smoke.ts}"
