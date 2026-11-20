#!/usr/bin/env bash
# Writes testdata/upgrade/<tag>/ with the released Linux app of <tag> (e2e-tauri/upgrade-fixture.ts).
# The .deb and SHA256SUMS are downloaded into a new empty directory, the .deb is checked against the
# SHA-256 the caller pins (second argument or DW_UPGRADE_SHA256), against SHA256SUMS and against the
# release workflow's build attestation for the tag, and unpacked into another; nothing runs from
# either but the release's own binary.
# The writer always runs under a fresh session bus (dbus-run-session) and a new Xvfb display
# (xvfb-run -a), with the caller's DISPLAY, WAYLAND_DISPLAY, XAUTHORITY and session bus removed and GTK
# held to X11, so no window can reach a real screen; it refuses to run any other way. An existing
# testdata/upgrade/<tag> is refused before anything is downloaded.
# Linux x86_64 only. Needs gh (or DW_UPGRADE_ASSETS=<dir> holding the two files), dpkg-deb, node 24,
# WebKitWebDriver, tauri-driver, dbus-run-session, xvfb-run and a dashd with dash-cli next to it
# ($DASHD, else $DASHD_PATH, else dashd on PATH).
#
#   scripts/upgrade-fixture.sh v0.2.0 981af3a867cdd8560af5feb73c66cac443923d5541ed70c3440ff4a6b64f6e0f
set -euo pipefail
tag=${1:?usage: upgrade-fixture.sh <release tag, e.g. v0.2.0> <sha256 of its linux-amd64 .deb>}
[[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "upgrade-fixture: $tag is not a release tag" >&2; exit 2; }
pinned=${2:-${DW_UPGRADE_SHA256:-}}
[[ $pinned =~ ^[0-9a-f]{64}$ ]] || { echo "upgrade-fixture: give the .deb's SHA-256 as the second argument or DW_UPGRADE_SHA256 (testdata/upgrade/README.md says where it comes from)" >&2; exit 2; }
cd "$(dirname "$0")/.."
desktop=$PWD
repo=$(cd ../.. && pwd)

out="$repo/testdata/upgrade/$tag"
if [[ -e "$out" || -L "$out" ]]; then
  echo "upgrade-fixture: $out exists; remove it or keep it, nothing was written" >&2
  exit 2
fi
command -v gh >/dev/null || { echo "upgrade-fixture: gh not found (it verifies the build attestation)" >&2; exit 2; }
command -v dbus-run-session >/dev/null || { echo "upgrade-fixture: dbus-run-session not found (apt install dbus)" >&2; exit 2; }
command -v xvfb-run >/dev/null || { echo "upgrade-fixture: xvfb-run not found (apt install xvfb)" >&2; exit 2; }
command -v WebKitWebDriver >/dev/null || { echo "upgrade-fixture: WebKitWebDriver not found (apt install webkit2gtk-driver)" >&2; exit 2; }
driver=${TAURI_DRIVER:-$(command -v tauri-driver || true)}
[[ -x "$driver" ]] || { echo "upgrade-fixture: tauri-driver not found; set TAURI_DRIVER" >&2; exit 2; }
dashd=${DASHD:-${DASHD_PATH:-$(command -v dashd || true)}}
[[ -x "$dashd" && -x "$(dirname "$dashd")/dash-cli" ]] || { echo "upgrade-fixture: set DASHD to a dashd with dash-cli next to it" >&2; exit 2; }

version=${tag#v}
deb="dash-wallet-${version}-linux-amd64.deb"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/assets" "$work/release"
if [[ -n "${DW_UPGRADE_ASSETS:-}" ]]; then
  cp "$DW_UPGRADE_ASSETS/SHA256SUMS" "$DW_UPGRADE_ASSETS/$deb" "$work/assets/"
else
  gh release download "$tag" -R PastaPastaPasta/dashwallet-desktop -D "$work/assets" -p SHA256SUMS -p "$deb"
fi
want=$(awk -v f="$deb" '$2 == f { print $1 }' "$work/assets/SHA256SUMS")
got=$(sha256sum "$work/assets/$deb" | awk '{ print $1 }')
[[ "$got" == "$pinned" ]] || { echo "upgrade-fixture: $deb is $got, not the pinned $pinned" >&2; exit 1; }
[[ -n "$want" && "$want" == "$got" ]] || { echo "upgrade-fixture: $deb does not match SHA256SUMS ($got, want ${want:-none})" >&2; exit 1; }
gh attestation verify "$work/assets/$deb" -R PastaPastaPasta/dashwallet-desktop \
  --signer-workflow PastaPastaPasta/dashwallet-desktop/.github/workflows/release.yml --source-ref "refs/tags/$tag" >&2 ||
  { echo "upgrade-fixture: $deb has no build attestation from the release workflow at $tag" >&2; exit 1; }
dpkg-deb -x "$work/assets/$deb" "$work/release"
app="$work/release/usr/bin/dash-wallet"
[[ -x "$app" ]] || { echo "upgrade-fixture: $deb has no usr/bin/dash-wallet" >&2; exit 1; }
echo "upgrade-fixture: $deb sha256 $got (pinned, SHA256SUMS and attestation of $tag)"

env -u DISPLAY -u WAYLAND_DISPLAY -u XAUTHORITY -u DBUS_SESSION_BUS_ADDRESS \
  GDK_BACKEND=x11 DW_UPGRADE_PRIVATE_SESSION=1 \
  DW_APP="$app" DWCLI=/bin/false TAURI_DRIVER="$driver" DASHD="$dashd" DW_UPGRADE_VERSION="$tag" DW_UPGRADE_OUT="$out" \
  DW_UPGRADE_ASSET="$deb" DW_UPGRADE_ASSET_SHA256="$got" \
  dbus-run-session -- xvfb-run -a node "$desktop/e2e-tauri/upgrade-fixture.ts"
