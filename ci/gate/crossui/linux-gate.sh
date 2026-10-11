#!/usr/bin/env bash
# Gate G-UI on Linux (G-03): SwiftCrossUI's dash-wallet (GtkBackend) from the release tarball
# (linux-package.sh), on the gate's regtest wallet (ci/gate/fixtures/regtest40k-dw.tar.gz),
# driven through AT-SPI and xdotool by linux_gate.py. Runs on the host's X display (DISPLAY, e.g.
# an agent-gui session) inside a private D-Bus session with its own AT-SPI bus, so nothing else
# on the desktop is seen.
#
#   ci/gate/crossui/linux-gate.sh APP_TARBALL WALLET_TARBALL PASSPHRASE_FILE OUT_DIR [options...]
#
# Options go to linux_gate.py (--phases, --runs).
#
# DWCLI names the dwcli binary used for U1's reference (default: rust/target/release/dwcli).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ -z "${GATE_SESSION:-}" ]]; then
  : "${DISPLAY:?run on an X display (agent-gui env N)}"
  usage="usage: linux-gate.sh APP_TARBALL WALLET_TARBALL PASSPHRASE_FILE OUT_DIR [options...]"
  tarball=$(realpath "${1:?$usage}")
  wallet=$(realpath "${2:?$usage}")
  passphrase=$(realpath "${3:?$usage}")
  out=$(mkdir -p "${4:?$usage}" && realpath "$4")
  shift 4
  exec env GATE_SESSION=1 dbus-run-session -- "$0" "$tarball" "$wallet" "$passphrase" "$out" "$@"
fi

tarball=$1 wallet=$2 passphrase=$3 out=$4
shift 4
/usr/libexec/at-spi-bus-launcher --launch-immediately >"$out/at-spi.log" 2>&1 &
for _ in $(seq 50); do
  dbus-send --session --print-reply --dest=org.a11y.Bus /org/a11y/bus org.a11y.Bus.GetAddress >/dev/null 2>&1 && break
  sleep 0.1
done
dbus-update-activation-environment DISPLAY XAUTHORITY

# U7: the tarball unpacks anywhere and runs from there.
install_dir="$out/install"
rm -rf "$install_dir" && mkdir -p "$install_dir"
tar -C "$install_dir" -xzf "$tarball"
{
  echo "host: $(uname -srm); $(. /etc/os-release && echo "$PRETTY_NAME")"
  echo "cpu: $(nproc) x $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2 | sed 's/^ //')"
  echo "memory: $(free -g | awk '/^Mem:/ {print $2}') GiB"
  echo "gtk: $(pkg-config --modversion gtk4 2>/dev/null || echo unknown)"
  echo "display: $DISPLAY $(xdpyinfo | awk '/dimensions:/ {print $2}')"
} >"$out/runner.txt"
cat "$out/runner.txt"

export PYTHONDONTWRITEBYTECODE=1  # it imports the AT-SPI helpers from the checkout
exec python3 "$HERE/linux_gate.py" --binary "$install_dir/dash-wallet/dash-wallet" --wallet "$wallet" \
  --passphrase-file "$passphrase" --out "$out" \
  --dwcli "${DWCLI:-$HERE/../../../rust/target/release/dwcli}" "$@"
