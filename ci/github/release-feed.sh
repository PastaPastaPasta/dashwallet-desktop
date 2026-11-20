#!/usr/bin/env bash
# GitHub Actions (release.yml): the update feed (PK-03, docs/release/updater.md). Runs only in a build
# made with the updater signing key (TAURI_SIGNING_PRIVATE_KEY), which only a pushed release tag gets, from
# the `release` environment.
#
#   ci/github/release-feed.sh stage OUT_DIR VERSION OS   after a build job's `release-stage.sh`
#   ci/github/release-feed.sh feed IN_DIR OUT_DIR VERSION REPO   in the checksums job
#
# stage: copies this OS's updater artifacts to OUT_DIR under the release names: the signature of the
# staged bundle (<bundle>.sig) for linux (the AppImage) and windows (the NSIS installer), and the
# macOS app archive with its signature (dash-wallet-<version>-macos-universal.app.tar.gz[.sig]). The
# signature must be of the very bytes the release publishes, so the staged bundle is compared with
# the one Tauri signed.
#
# feed: writes OUT_DIR/latest.json and copies the macOS archive there, from the three OSes' staged
# files, when VERSION is a stable X.Y.Z: a pre-release never becomes the feed. With no updater files
# (a build without the key) it writes nothing and succeeds; with some OSes' files missing it fails.
# The entries are keyed `<os>-<arch>-<bundle>`, so only the AppImage, the NSIS installer and the
# macOS app update themselves (a .deb, an .rpm or a Flatpak never matches).
set -euo pipefail

mode=${1:?usage: $0 stage OUT_DIR VERSION OS | feed IN_DIR OUT_DIR VERSION REPO}
die() { echo "::error::release-feed: $*" >&2; exit 1; }

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

case "$mode" in
stage)
  out=${2:?} version=${3:?} os=${4:?}
  case "$os" in
  linux) suffix=linux-x86_64.AppImage glob='release/bundle/appimage/*.AppImage' ;;
  windows) suffix=windows-x64-setup.exe glob='release/bundle/nsis/*-setup.exe' ;;
  macos) suffix=macos-universal.app.tar.gz glob='universal-apple-darwin/release/bundle/macos/*.app.tar.gz' ;;
  *) die "unknown OS $os" ;;
  esac
  if [[ $os == macos ]]; then
    "$here/release-stage.sh" "$out" "$version" "$suffix" "$glob" > /dev/null
  else
    staged=${DIST_DIR:-$RUNNER_TEMP/dist}/dash-wallet-$version-$suffix
    signed=$("$here/release-stage.sh" "$(mktemp -d)" "$version" "$suffix" "$glob")
    cmp -s "$signed" "$staged" || die "$staged is not the bundle Tauri signed"
  fi
  "$here/release-stage.sh" "$out" "$version" "$suffix.sig" "$glob.sig" > /dev/null
  ls -l "$out"
  ;;

feed)
  in=${2:?} out=${3:?} version=${4:?} repo=${5:?}
  shopt -s nullglob
  sigs=("$in"/*.sig)
  if ((${#sigs[@]} == 0)); then
    echo "release-feed: no updater artifacts (a build without the updater key); no feed"
    exit 0
  fi
  if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "release-feed: $version is not a stable release; no feed"
    exit 0
  fi
  base=https://github.com/$repo/releases/download/v$version
  linux=dash-wallet-$version-linux-x86_64.AppImage
  windows=dash-wallet-$version-windows-x64-setup.exe
  macos=dash-wallet-$version-macos-universal.app.tar.gz
  for f in "$linux.sig" "$windows.sig" "$macos" "$macos.sig"; do
    [[ -s "$in/$f" ]] || die "$f is missing: every OS's build must carry the updater artifacts, or none"
  done
  [[ ${#sigs[@]} -eq 3 ]] || die "expected 3 signatures in $in, found ${#sigs[@]}"
  sig() { tr -d '\r\n' < "$in/$1.sig"; }
  mkdir -p "$out"
  cp "$in/$macos" "$out/$macos"
  jq -n --arg version "$version" --arg notes "https://github.com/$repo/releases/tag/v$version" \
    --arg date "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg lu "$base/$linux" --arg ls "$(sig "$linux")" \
    --arg wu "$base/$windows" --arg ws "$(sig "$windows")" \
    --arg mu "$base/$macos" --arg ms "$(sig "$macos")" \
    '{version: $version, notes: $notes, pub_date: $date, platforms: {
      "linux-x86_64-appimage": {url: $lu, signature: $ls},
      "windows-x86_64-nsis": {url: $wu, signature: $ws},
      "darwin-aarch64-app": {url: $mu, signature: $ms},
      "darwin-x86_64-app": {url: $mu, signature: $ms}}}' > "$out/latest.json"
  cat "$out/latest.json"
  [[ -z "${GITHUB_OUTPUT:-}" ]] || echo "feed=1" >> "$GITHUB_OUTPUT"
  ;;
*) die "unknown mode $mode" ;;
esac
