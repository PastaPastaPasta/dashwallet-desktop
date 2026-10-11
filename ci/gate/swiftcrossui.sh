#!/usr/bin/env bash
# Gate G-UI measurements for SwiftCrossUI (roadmap G-03; DASHPAY §2.1; docs/ci.md "gate.yml").
# gate.yml runs this on each OS with GATE_OS and GATE_OUT set (everything in GATE_OUT is uploaded
# as the artifact gate-swiftcrossui-<os>):
#
#   macos:   U8, U3 and U7: release build (AppKitBackend), an unsigned .app and dmg, installed
#            and launched; the four screens in light and dark
#   windows: U2, U3, U4 and U7: release build (WinUIBackend), a zip with the Swift runtime,
#            unpacked and run for 10 minutes; the screens in light and dark; the #787 UI
#            Automation walk
#
# Both open the gate's regtest wallet, the fixture G-02 measures Tauri on: dw-engine's datadir
# ci/gate/fixtures/regtest40k-dw.tar.gz (made by ci/gate/make-regtest-fixture.sh), whose vault
# passphrase is ci/gate/fixtures/regtest40k-passphrase.txt.
#
# Linux U1, U3, U4, U5 and U7 run on agentbox (ci/gate/crossui/linux-gate.sh), not here.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
: "${GATE_OS:?}" "${GATE_OUT:?}"
(($# == 0)) || { echo "swiftcrossui.sh takes no arguments" >&2; exit 2; }

export GATE_ROOT="$ROOT"
temp="${RUNNER_TEMP:-$ROOT/.gate}"
# Git Bash: a "D:\..." path would read as a remote host to tar.
[[ "$GATE_OS" != windows ]] || temp=$(cygpath -u "$temp")
export GATE_WORK="$temp/gate-work"
mkdir -p "$GATE_WORK"

# The runner, for the ADR's raw data.
{
  echo "date: $(date -u +%FT%TZ)"
  echo "runner: ${RUNNER_NAME:-?} ${ImageOS:-?} ${ImageVersion:-?} (${RUNNER_OS:-?} ${RUNNER_ARCH:-?})"
  echo "commit: $(git -C "$ROOT" rev-parse HEAD)"
  case "$GATE_OS" in
    linux) nproc; free -m | head -2; lsb_release -ds 2>/dev/null || true; uptime ;;
    macos) sw_vers; sysctl -n machdep.cpu.brand_string hw.ncpu hw.memsize; uptime ;;
    windows) powershell.exe -NoProfile -Command \
      "Get-CimInstance Win32_OperatingSystem | Select-Object Caption,Version,TotalVisibleMemorySize | Format-List; Get-CimInstance Win32_Processor | Select-Object Name,NumberOfLogicalProcessors | Format-List" ;;
  esac
} >"$GATE_OUT/runner.txt" 2>&1 || true
cat "$GATE_OUT/runner.txt"

export GATE_WALLET="$GATE_WORK/wallet"
export GATE_PASSPHRASE_FILE="$ROOT/ci/gate/fixtures/regtest40k-passphrase.txt"
rm -rf "$GATE_WALLET" && mkdir -p "$GATE_WALLET"
tar -C "$GATE_WALLET" -xzf "$ROOT/ci/gate/fixtures/regtest40k-dw.tar.gz"

case "$GATE_OS" in
  macos) exec bash "$ROOT/ci/gate/crossui/macos.sh" ;;
  windows) exec bash "$ROOT/ci/gate/crossui/windows.sh" ;;
  *) echo "swiftcrossui.sh: the Linux measurements run on agentbox (ci/gate/crossui/linux-gate.sh)" >&2; exit 2 ;;
esac
