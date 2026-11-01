#!/usr/bin/env bash
# GitHub Actions (release.yml): launch the built Linux AppImage under Xvfb and a private D-Bus
# session, wait, screenshot the whole screen, and fail if the process dies first or no window
# titled "Dash Wallet" ever appears.
#
#   ci/github/linux-release-launch.sh APPIMAGE OUT_DIR SECONDS [APP ARGS...]
#
# The AppImage runs extracted (APPIMAGE_EXTRACT_AND_RUN): the runner has no FUSE. HOME and the
# XDG directories are private and empty, so the app finds no data. Writes OUT_DIR/launch-linux.png,
# app.log and xvfb.log.
set -euo pipefail

if [[ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]]; then
  exec dbus-run-session -- "$0" "$@"
fi

app=$1
out=$2
seconds=$3
shift 3
mkdir -p "$out"

home=$(mktemp -d)
export HOME="$home" XDG_DATA_HOME="$home/data" XDG_CONFIG_HOME="$home/config" XDG_CACHE_HOME="$home/cache"
export APPIMAGE_EXTRACT_AND_RUN=1
mkdir -p "$XDG_DATA_HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME"

export DISPLAY=:98
Xvfb "$DISPLAY" -screen 0 1280x860x24 -nolisten tcp >"$out/xvfb.log" 2>&1 &
xvfb=$!
cleanup() { kill "$xvfb" 2>/dev/null || true; rm -rf "$home"; }
trap cleanup EXIT
for _ in $(seq 50); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.1; done

"$app" "$@" >"$out/app.log" 2>&1 &
pid=$!
echo "launched $app $* (pid $pid); waiting ${seconds}s"

window=""
for _ in $(seq "$seconds"); do
  sleep 1
  kill -0 "$pid" 2>/dev/null || break
  window=$(xwininfo -root -tree | grep -c '"Dash Wallet' || true)
  [[ "$window" -gt 0 ]] && break
  window=""
done
# Let the first screen load.
[[ -n "$window" ]] && sleep "${DWD_SHOT_SETTLE:-20}"

status=0
if ! kill -0 "$pid" 2>/dev/null; then
  code=0
  wait "$pid" || code=$?
  echo "::error::dash-wallet exited early with status $code"
  status=1
elif [[ -z "$window" ]]; then
  echo "::error::dash-wallet shows no \"Dash Wallet\" window after ${seconds}s"
  import -window root "$out/launch-linux.png" || true
  status=1
else
  import -window root "$out/launch-linux.png"
  echo "screenshot: $out/launch-linux.png; rss $(ps -o rss= -p "$pid" | tr -d ' ') KB"
  # The screenshot must not be the last thing the app did: it is still running.
  kill -0 "$pid" 2>/dev/null || { echo "::error::dash-wallet died while the screenshot was taken"; status=1; }
fi
kill "$pid" 2>/dev/null || true
wait "$pid" 2>/dev/null || true
echo "--- app.log (tail)"
tail -n 40 "$out/app.log" || true
exit "$status"
