#!/usr/bin/env bash
# Gate G-UI on the ubuntu runner (G-03), from swiftcrossui.sh: the release tarball built natively
# (linux-pack.sh), dwcli for U1's reference, then linux-gate.sh on a private Xvfb display. The
# quiet runner's numbers stand next to agentbox's, as the Tauri column does.
#
# Needs GATE_OUT, GATE_WORK, GATE_WALLET's fixture and GATE_PASSPHRASE_FILE (swiftcrossui.sh).
set -euo pipefail
ROOT="${GATE_ROOT:?}"
cd "$ROOT"
pkg="$GATE_WORK/pkg"
t0=$(date +%s)
scripts/build-core.sh --profile release --no-bindings
t1=$(date +%s)
swift build -c release --static-swift-stdlib --product dash-wallet
t2=$(date +%s)
CORE_SECONDS=$((t1 - t0)) SWIFT_SECONDS=$((t2 - t1)) \
  ci/gate/crossui/linux-pack.sh "$(swift build -c release --show-bin-path)" "$pkg"
cp "$pkg/build.json" "$GATE_OUT/"
# The build directory's bundles must not stand in for the tarball's (SwiftPM's accessor falls back
# to the build path).
for bundle in "$(swift build -c release --show-bin-path)"/*.resources; do
  [[ -e "$bundle" ]] && mv "$bundle" "$bundle.moved"
done
(cd rust && cargo build --release -p dwcli)

export DISPLAY=:91
Xvfb "$DISPLAY" -screen 0 1440x900x24 -nolisten tcp >/dev/null 2>&1 &
xvfb=$!
trap 'kill "$xvfb" 2>/dev/null || true' EXIT
for _ in $(seq 50); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.1; done

DWCLI="$ROOT/rust/target/release/dwcli" ci/gate/crossui/linux-gate.sh "$pkg/dash-wallet-linux-x86_64.tar.gz" \
  ci/gate/fixtures/regtest40k-dw.tar.gz "$GATE_PASSPHRASE_FILE" "$GATE_OUT/linux" --runs 10
