#!/usr/bin/env bash
# Gate U7 on Linux: the .deb installs and the installed app launches; the AppImage runs.
#
# Installs the bundle with apt (needs sudo), runs the installed binary's fixture self-test under a
# private Xvfb display and D-Bus session, removes the package again, then runs the AppImage the
# same way (extract-and-run, so FUSE is not needed). Writes OUT_DIR/linux-packages.json.
#
# Usage (from the repo root, after `pnpm tauri build`): apps/desktop/scripts/gate/linux-packages.sh OUT_DIR
set -euo pipefail

if [[ -z "${DW_GATE_INNER:-}" ]]; then
  exec dbus-run-session -- env DW_GATE_INNER=1 "$0" "$@"
fi

OUT=$(realpath -m "${1:?usage: linux-packages.sh OUT_DIR}")
REPO=$(cd "$(dirname "$0")/../../../.." && pwd)
BUNDLE=$REPO/rust/target/release/bundle
DEB=$(ls "$BUNDLE"/deb/*.deb)
APPIMAGE=$(ls "$BUNDLE"/appimage/*.AppImage)
mkdir -p "$OUT"
cd "$OUT"

export DISPLAY=:${DW_GATE_DISPLAY:-79}
Xvfb "$DISPLAY" -screen 0 1440x900x24 -nolisten tcp >xvfb.log 2>&1 &
XVFB=$!
trap 'kill $XVFB 2>/dev/null || true' EXIT
for _ in $(seq 50); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.1; done
dbus-update-activation-environment DISPLAY

# selftest NAME COMMAND...: runs the fixture self-test, prints "true" when its final report arrived.
selftest() {
  local name=$1
  shift
  rm -f "$OUT/selftest-$name.json"
  # Absolute path: the AppImage's AppRun changes the working directory.
  timeout 180 "$@" --fixture --network testnet --selftest "$OUT/selftest-$name.json" >"$name.log" 2>&1 || true
  python3 -c "import json,sys; r=json.load(open(sys.argv[1])); print(str(r.get('final') is True and r['checksFailed'] == 0).lower())" \
    "$OUT/selftest-$name.json" 2>/dev/null || echo false
}

echo "== .deb: $(basename "$DEB")"
dpkg-deb -I "$DEB" >deb-info.txt
sudo -n DEBIAN_FRONTEND=noninteractive apt-get install -y "$DEB" >deb-install.log 2>&1 && deb_installed=true || deb_installed=false
deb_ok=false
if $deb_installed; then
  deb_ok=$(selftest deb /usr/bin/dash-wallet)
  desktop_file=$(dpkg -L dash-wallet | grep -m1 '\.desktop$' || true)
  sudo -n apt-get remove -y dash-wallet >deb-remove.log 2>&1 || true
fi

echo "== AppImage: $(basename "$APPIMAGE")"
appimage_ok=$(APPIMAGE_EXTRACT_AND_RUN=1 selftest appimage "$APPIMAGE")

cat >linux-packages.json <<JSON
{
  "deb": {"file": "$(basename "$DEB")", "bytes": $(stat -c %s "$DEB"), "installed": $deb_installed, "launchedAndSelftested": $deb_ok, "desktopFile": "${desktop_file:-}"},
  "appimage": {"file": "$(basename "$APPIMAGE")", "bytes": $(stat -c %s "$APPIMAGE"), "launchedAndSelftested": $appimage_ok}
}
JSON
cat linux-packages.json
