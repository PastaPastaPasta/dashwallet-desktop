#!/usr/bin/env bash
# Gate G-UI, U7 (Linux): builds the Flatpak (flatpak/org.dash.wallet.desktop.CrossUI.yml) from the
# release tarball, exports a bundle, installs it and launches it (demo mode) under Xvfb, all in a
# privileged container (flatpak/Dockerfile). Writes the bundle, flatpak.txt and a screenshot.
#
#   ci/gate/crossui/linux-flatpak.sh TARBALL OUT_DIR
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TARBALL=$(realpath "${1:?usage: linux-flatpak.sh TARBALL OUT_DIR}")
OUT=$(mkdir -p "${2:?}" && realpath "$2")
IMAGE=dwd-gate-flatpak
docker image inspect "$IMAGE" >/dev/null 2>&1 || docker build -t "$IMAGE" "$HERE/flatpak"

docker run --rm --privileged \
  -v "$HERE/flatpak":/manifest:ro -v "$TARBALL":/in/dash-wallet-linux-x86_64.tar.gz:ro -v "$OUT":/out \
  -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
  "$IMAGE" bash -euo pipefail -c '
    app=org.dash.wallet.desktop.CrossUI
    mkdir -p /build && cd /build
    cp /manifest/$app.yml /in/dash-wallet-linux-x86_64.tar.gz .
    t0=$(date +%s)
    flatpak-builder --disable-rofiles-fuse --force-clean --repo=repo build-dir $app.yml >/out/flatpak-build.log 2>&1
    flatpak build-bundle repo /out/dash-wallet-crossui.flatpak $app
    t1=$(date +%s)
    flatpak install --system -y --noninteractive /out/dash-wallet-crossui.flatpak >>/out/flatpak-build.log 2>&1
    {
      echo "build+bundle: $((t1 - t0)) s; bundle $(stat -c %s /out/dash-wallet-crossui.flatpak) bytes"
      flatpak info $app | sed "s/^/  /"
    } >/out/flatpak.txt
    # Launch the installed app (demo mode) on Xvfb; GateProbe marks the first frame.
    export DISPLAY=:91
    Xvfb $DISPLAY -screen 0 1280x860x24 -nolisten tcp >/dev/null 2>&1 &
    for _ in $(seq 50); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.1; done
    dbus-run-session -- flatpak run --env=DWD_GATE=1 $app --demo >/out/flatpak-run.log 2>&1 &
    seen=""
    for _ in $(seq 240); do
      grep -q "^gate-mark overview-paint " /out/flatpak-run.log && { seen=1; break; }
      sleep 0.5
    done
    sleep 2
    xwd -root -silent | xwdtopnm 2>/dev/null | pnmtopng >/out/flatpak-demo.png
    if [[ -n "$seen" ]]; then echo "launch: ok (overview-paint)" >>/out/flatpak.txt
    else echo "launch: FAIL no overview-paint in 120 s" >>/out/flatpak.txt; fi
    chown -R "$HOST_UID:$HOST_GID" /out
    [[ -n "$seen" ]]'
cat "$OUT/flatpak.txt"
