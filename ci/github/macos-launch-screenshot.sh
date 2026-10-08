#!/usr/bin/env bash
# GitHub Actions (macOS runner only; never on a developer's Mac, see CLAUDE.md):
# launch a GUI program, wait for its window, screenshot the window and the whole screen,
# and fail if the program exits early or never shows a window.
#
#   ci/github/macos-launch-screenshot.sh NAME OUT_DIR SECONDS APP_OR_BIN [ARGS...]
#
# An .app bundle is launched through LaunchServices (`open -n`), as a user would; anything
# else is executed directly. Writes OUT_DIR/NAME.png (the window), OUT_DIR/NAME-screen.png,
# OUT_DIR/NAME.log (stdout) and OUT_DIR/NAME.err.log.
set -euo pipefail

name="$1"
out="$2"
seconds="$3"
target="$4"
shift 4
mkdir -p "$out"
log="$out/$name.log"
err="$out/$name.err.log"

window_id="${RUNNER_TEMP:-/tmp}/macos-window-id"
[[ -x "$window_id" ]] || swiftc -O -o "$window_id" "$(dirname "${BASH_SOURCE[0]}")/macos-window-id.swift"

if [[ "$target" == *.app ]]; then
  open -n -a "$target" --stdout "$log" --stderr "$err" --args "$@"
  executable="$target/Contents/MacOS/$(defaults read "$target/Contents/Info.plist" CFBundleExecutable)"
  pid=""
  for _ in $(seq 20); do
    pid="$(pgrep -n -f "^$executable" || true)"
    [[ -n "$pid" ]] && break
    sleep 0.5
  done
  [[ -n "$pid" ]] || { echo "::error::$name: no process for $executable"; exit 1; }
else
  "$target" "$@" >"$log" 2>"$err" &
  pid=$!
fi
echo "$name: launched $target $* (pid $pid); waiting up to ${seconds}s for its window"

wait_for_window() {
  for _ in $(seq "$seconds"); do
    sleep 1
    kill -0 "$pid" 2>/dev/null || return 0
    window="$("$window_id" "$pid" || true)"
    [[ -z "$window" ]] || return 0
  done
}
window=""
wait_for_window
if [[ -z "$window" && "$target" == *.app ]] && kill -0 "$pid" 2>/dev/null; then
  # What a Dock click does: `open` of the running app sends it a reopen event, on which SwiftUI
  # presents the main window if none is open.
  echo "::warning::$name opened no window at launch; windows it owns: $("$window_id" --list "$pid" | tr '\n' ';')"
  echo "$name: sending a reopen event (open -a)"
  open -a "$target"
  wait_for_window
fi
# Let the first frame settle.
[[ -n "$window" ]] && sleep 3

status=0
if ! kill -0 "$pid" 2>/dev/null; then
  echo "::error::$name exited early"
  status=1
elif [[ -z "$window" ]]; then
  "$window_id" --list "$pid" || true
  screencapture -x "$out/$name-screen.png"
  echo "::error::$name shows no window after ${seconds}s (screen: $out/$name-screen.png)"
  status=1
else
  screencapture -x -o -l "$window" "$out/$name.png"
  screencapture -x "$out/$name-screen.png"
  echo "$name: screenshot of window $window: $out/$name.png; rss $(ps -o rss= -p "$pid" | tr -d ' ') KB"
fi
kill "$pid" 2>/dev/null || true
echo "--- $name.log, $name.err.log (tails)"
tail -n 20 "$log" "$err" || true
exit "$status"
