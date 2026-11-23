#!/usr/bin/env bash
# GitHub Actions: launch the SwiftCrossUI app (GtkBackend) under Xvfb and a private
# D-Bus session, take a screenshot of its window, and fail if it exits early or never
# shows a window.
#
#   ci/github/linux-launch-screenshot.sh BIN OUT_DIR [SECONDS] [APP ARGS...]
#
# Default app arguments: --demo. Writes OUT_DIR/linux-demo.png, app.log and xvfb.log.
set -euo pipefail

if [[ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]]; then
  exec dbus-run-session -- "$0" "$@"
fi

bin="$1"
out="$2"
seconds="${3:-20}"
shift $(( $# >= 3 ? 3 : $# ))
(( $# )) || set -- --demo
mkdir -p "$out"

export DISPLAY=:99
Xvfb "$DISPLAY" -screen 0 1280x860x24 -nolisten tcp >"$out/xvfb.log" 2>&1 &
xvfb=$!
trap 'kill "$xvfb" 2>/dev/null || true' EXIT
for _ in $(seq 50); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.1; done

# The live mode writes under XDG_DATA_HOME; keep it out of the workspace.
export XDG_DATA_HOME="${RUNNER_TEMP:-/tmp}/dwd-xdg"
mkdir -p "$XDG_DATA_HOME" && chmod 700 "$XDG_DATA_HOME"

"$bin" "$@" >"$out/app.log" 2>&1 &
pid=$!
echo "launched $bin $* (pid $pid); waiting ${seconds}s"

window=""
for _ in $(seq "$seconds"); do
  sleep 1
  kill -0 "$pid" 2>/dev/null || break
  # The largest "Dash Wallet…" window: GTK also maps a 1x1 leader window with the same title.
  window="$(xwininfo -root -tree | awk '/"Dash Wallet/ && match($0, / [0-9]+x[0-9]+\+/) {
    split(substr($0, RSTART + 1, RLENGTH - 2), d, "x")
    if (d[1] * d[2] > best) { best = d[1] * d[2]; id = $1 }
  } END { if (best > 1) print id }')"
  [[ -n "$window" ]] && break
done
# Let the first screen load: the demo shows "Loading wallet…" for 10–45 s on agentbox before the
# Overview. DWD_SHOT_SETTLE (seconds) overrides.
[[ -n "$window" ]] && sleep "${DWD_SHOT_SETTLE:-45}"

status=0
if ! kill -0 "$pid" 2>/dev/null; then
  code=0
  wait "$pid" || code=$?
  echo "::error::dash-wallet exited early with status $code"
  status=1
elif [[ -z "$window" ]]; then
  echo "::error::dash-wallet shows no \"Dash Wallet\" window after ${seconds}s"
  status=1
else
  xwd -id "$window" -silent | xwdtopnm 2>/dev/null | pnmtopng >"$out/linux-demo.png"
  echo "screenshot of window $window: $out/linux-demo.png; rss $(ps -o rss= -p "$pid" | tr -d ' ') KB"
fi
kill "$pid" 2>/dev/null || true
wait "$pid" 2>/dev/null || true
echo "--- app.log (tail)"
tail -n 40 "$out/app.log" || true
exit "$status"
