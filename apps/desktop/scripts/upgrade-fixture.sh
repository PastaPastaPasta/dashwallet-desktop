#!/usr/bin/env bash
# Writes testdata/upgrade/<tag>/ with the released Linux app of <tag> (e2e-tauri/upgrade-fixture.ts).
# The .deb and SHA256SUMS are downloaded into a new empty directory, the .deb is checked against
# SHA256SUMS and unpacked into another; nothing runs from either but the release's own binary.
# Linux x86_64 only. Needs gh (or DW_UPGRADE_ASSETS=<dir> holding the two files), dpkg-deb, node 24,
# WebKitWebDriver, tauri-driver, dbus-run-session, xvfb-run (or a private $DISPLAY) and a dashd with
# dash-cli next to it ($DASHD, else $DASHD_PATH, else dashd on PATH).
#
#   scripts/upgrade-fixture.sh v0.1.0
set -euo pipefail
tag=${1:?usage: upgrade-fixture.sh <release tag, e.g. v0.1.0>}
[[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "upgrade-fixture: $tag is not a release tag" >&2; exit 2; }
cd "$(dirname "$0")/.."
desktop=$PWD
repo=$(cd ../.. && pwd)

if [[ -z "${DW_E2E_PRIVATE_BUS:-}" ]]; then
  command -v dbus-run-session >/dev/null || { echo "upgrade-fixture: dbus-run-session not found (apt install dbus)" >&2; exit 2; }
  export DW_E2E_PRIVATE_BUS=1
  if [[ -z "${DISPLAY:-}" ]]; then
    exec dbus-run-session -- xvfb-run -a "$desktop/scripts/upgrade-fixture.sh" "$@"
  fi
  exec dbus-run-session -- "$desktop/scripts/upgrade-fixture.sh" "$@"
fi

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
[[ -n "$want" && "$want" == "$got" ]] || { echo "upgrade-fixture: $deb does not match SHA256SUMS ($got, want ${want:-none})" >&2; exit 1; }
dpkg-deb -x "$work/assets/$deb" "$work/release"
app="$work/release/usr/bin/dash-wallet"
[[ -x "$app" ]] || { echo "upgrade-fixture: $deb has no usr/bin/dash-wallet" >&2; exit 1; }
echo "upgrade-fixture: $deb sha256 $got (SHA256SUMS of $tag)"

out="$repo/testdata/upgrade/$tag"
DW_APP="$app" DWCLI=/bin/false TAURI_DRIVER="$driver" DASHD="$dashd" DW_UPGRADE_VERSION="$tag" DW_UPGRADE_OUT="$out" \
  DW_UPGRADE_ASSET="$deb" DW_UPGRADE_ASSET_SHA256="$got" \
  node "$desktop/e2e-tauri/upgrade-fixture.ts"
