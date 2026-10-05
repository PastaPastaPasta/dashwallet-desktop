#!/usr/bin/env bash
# Runs INSIDE the dwd-linux-swift-gtk container with the probe mounted at /work and
# an output directory mounted at /out. Expects the probe to be built already
# (container-build-test.sh). Starts Xvfb, a D-Bus session bus and the AT-SPI bus,
# launches the probe, runs atspi_smoke.py against it, and reports whether the app
# survived the whole run.
set -euo pipefail
OUT=${OUT:-/out}
mkdir -p "$OUT"
bin=$(cd /work && swift build "$@" --show-bin-path)/CrossUILinuxProbe

if [[ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]]; then
    # Re-exec under a private session bus so everything below shares it.
    exec dbus-run-session -- "$0" "$@"
fi

export DISPLAY=:99
Xvfb "$DISPLAY" -screen 0 1280x800x24 -nolisten tcp >"$OUT/xvfb.log" 2>&1 &
xvfb_pid=$!
for _ in $(seq 50); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.1; done
xdpyinfo >/dev/null 2>&1 || { echo "Xvfb did not start"; cat "$OUT/xvfb.log"; exit 1; }

# The AT-SPI bus launcher registers org.a11y.Bus on the session bus; the registry
# daemon is D-Bus-activated on the accessibility bus when the first client connects.
/usr/libexec/at-spi-bus-launcher --launch-immediately >"$OUT/at-spi-bus.log" 2>&1 &
for _ in $(seq 50); do
    dbus-send --session --print-reply --dest=org.a11y.Bus /org/a11y/bus org.a11y.Bus.GetAddress \
        >/dev/null 2>&1 && break
    sleep 0.1
done

start_ns=$(date +%s%N)
G_MESSAGES_DEBUG=${G_MESSAGES_DEBUG:-} "$bin" >"$OUT/app.log" 2>&1 &
app_pid=$!
echo "== launched probe pid $app_pid"

set +e
if [[ -n "${SMOKE_SCRIPT:-}" ]]; then
    python3 "$SMOKE_SCRIPT" "$app_pid"   # ad-hoc AT-SPI debugging script
else
    python3 /work/scripts/atspi_smoke.py --pid "$app_pid" --out "$OUT"
fi
smoke_status=$?
set -e

if kill -0 "$app_pid" 2>/dev/null; then
    alive=yes
    rss_kb=$(ps -o rss= -p "$app_pid" | tr -d ' ')
    kill "$app_pid"; wait "$app_pid" 2>/dev/null || true
else
    alive=no
    app_status=0; wait "$app_pid" || app_status=$?
    echo "== probe exited early with status $app_status"
fi
elapsed_ms=$(( ($(date +%s%N) - start_ns) / 1000000 ))
echo "== probe alive at end of smoke run: $alive (rss ${rss_kb:-?} KB, run ${elapsed_ms} ms)"
echo "== app.log:"; cat "$OUT/app.log"
kill "$xvfb_pid" 2>/dev/null || true
[[ "$alive" == yes ]] || exit 2
exit "$smoke_status"
