#!/usr/bin/env bash
# GitHub Actions: apt packages on an ubuntu-24.04 runner.
#
#   ci/github/linux-host-deps.sh rust|tauri|flatpak|swiftcrossui|mingw
#
#   rust          what `cargo build --workspace` needs beyond the runner image: the WebKitGTK
#                 development packages of the Tauri crate rust/crates/dw-app (G-01; list from
#                 apps/desktop/README.md), installed before it lands so that merge needs no CI change,
#                 and dbus-daemon for dw-app's portal tests on a private bus;
#   tauri         those plus Xvfb, AT-SPI, xdotool, the WebKitGTK WebDriver, D-Bus and xdg-utils for
#                 the gate harness and the link checks (apps/desktop/README.md);
#   flatpak       flatpak-builder and the launch check's Xvfb, D-Bus and ImageMagick (release.yml);
#   swiftcrossui  GTK 4, Xvfb and the AT-SPI stack (ci/linux/Dockerfile.swift-gtk);
#   mingw         the MinGW-w64 C and C++ compilers for the Windows cross-check (x86_64-pc-windows-gnu).
set -euo pipefail

webkit=(libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev
  librsvg2-dev)
gui=(xvfb xauth x11-utils x11-apps netpbm dbus-x11 at-spi2-core gir1.2-atspi-2.0 python3-pyatspi python3-gi)

case "${1:?usage: $0 rust|tauri|flatpak|swiftcrossui|mingw}" in
  rust) packages=("${webkit[@]}" dbus) ;;
  # Ubuntu 24.04 calls the WebDriver package webkit2gtk-driver (26.04: webkitgtk-webdriver).
  # dbus and xdg-utils: second launches and links the OS opens (U-01, ci/github/deep-links.sh).
  tauri) packages=("${webkit[@]}" "${gui[@]}" xdotool imagemagick webkit2gtk-driver dbus xdg-utils) ;;
  flatpak) packages=(flatpak flatpak-builder "${gui[@]}" imagemagick dbus) ;;
  swiftcrossui) packages=(libgtk-4-dev "${gui[@]}" gsettings-desktop-schemas adwaita-icon-theme fonts-dejavu-core) ;;
  mingw) packages=(gcc-mingw-w64-x86-64 g++-mingw-w64-x86-64) ;;
  *) echo "unknown package set $1" >&2; exit 2 ;;
esac

apt_opts=(-o Acquire::Retries=3 -o Acquire::http::Timeout=30 -o Acquire::https::Timeout=30)
install=(apt-get "${apt_opts[@]}" install -y --no-install-recommends)

# DW_APT_CACHE (.github/actions/linux-host-deps): a directory restored from the Actions cache.
# When it holds .deb files, ci/github/apt-cache.sh verifies them against the image's Ubuntu
# keyring and they are installed without the network; a break in that chain fails the job. When
# it is empty, the packages are downloaded as below and saved into it.
# DW_APT_LIVE_FIRST=1 (Release): with a cache, one live attempt under short deadlines comes first,
# and the cached files install only if it fails. DW_APT_SAVE=0 (pull requests, which never save an
# entry): a download is not copied into the cache directory.
cache=${DW_APT_CACHE:-}
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
apt_cache=${DW_APT_CACHE_TOOL:-$here/apt-cache.sh}
from_cache() {
  local none status
  "$apt_cache" verify "$cache"
  # The files themselves, with no source configured, so apt can fetch nothing and needs no index
  # (it refuses local files with --no-download).
  none=$(mktemp -d)
  sudo DEBIAN_FRONTEND=noninteractive "${install[@]}" -qq -o Dir::Etc::SourceList=/dev/null \
    -o Dir::Etc::SourceParts="$none" "$cache"/archives/*.deb
  rmdir "$none"
  if ! status=$(dpkg-query -W -f '${Status}\n' "${packages[@]}") || grep -vqx 'install ok installed' <<< "$status"; then
    echo "::error::linux-host-deps: the cached .deb files did not install every package of the $pkgset set" >&2
    exit 1
  fi
  echo "linux-host-deps: the $pkgset set from the Actions cache, no download"
}
pkgset=$1
cached=
if [[ -n "$cache" ]] && compgen -G "$cache/archives/*.deb" > /dev/null; then cached=1; fi
if [[ -n "$cached" && "${DW_APT_LIVE_FIRST:-}" != 1 ]]; then
  from_cache
  exit 0
fi
# No package of these sets comes from packages.microsoft.com, a source the runner image adds. On
# 2026-11-27 its noble index answered 403 Forbidden, and apt-get update fails on any source it cannot
# read, before a download starts. So the live download reads the Ubuntu archive only.
# DW_APT_SOURCE_PARTS: the directory of extra sources (default /etc/apt/sources.list.d).
parts=${DW_APT_SOURCE_PARTS:-/etc/apt/sources.list.d}
for part in "$parts"/*.list "$parts"/*.sources; do
  if [ -f "$part" ] && grep -q 'packages[.]microsoft[.]com' "$part"; then
    sudo rm -f "$part"
    echo "apt: dropped $part, a packages.microsoft.com source" >&2
  fi
done
attempts=3 update_deadline=120 download_deadline=${DW_APT_DOWNLOAD_DEADLINE:-360}
if [[ -n "$cached" ]]; then
  attempts=1 update_deadline=60 download_deadline=180
fi

# Only the network steps run under a deadline and are tried again. A deadline that fired while
# dpkg was unpacking would leave a journal that apt refuses to continue from.
# The runner image lists its regional mirror first (mirror+file:/etc/apt/apt-mirrors.txt), then
# archive.ubuntu.com and security.ubuntu.com, and reads noble-security from security.ubuntu.com.
# apt leaves a stalled mirror too slowly to finish under the deadline, and on 2026-11-19 all three
# were down together. So a failed attempt puts the Azure mirror first, drops the mirrors listed
# ahead of archive.ubuntu.com, and reads noble-security from the same mirror list. When the mirror
# that failed is the Azure one (it is the regional mirror of some runners, and on 2026-11-20 it
# served 70 MB too slowly for three attempts), it goes last, so archive.ubuntu.com serves the next
# attempt. apt keeps what an attempt downloaded.
# DW_APT_DOWNLOAD_DEADLINE (seconds, default 360) bounds each download attempt; a live-first
# attempt has 60 s for the update and 180 s for the download.
mirrors=${DW_APT_MIRRORS:-/etc/apt/apt-mirrors.txt}
sources=${DW_APT_SOURCES:-/etc/apt/sources.list.d/ubuntu.sources}
fallback=http://azure.archive.ubuntu.com/ubuntu/
use_fallback_mirror() {
  local main='^https?://archive[.]ubuntu[.]com/' list skip=0 rest new text=
  [ -f "$mirrors" ] || return 0
  list=$(cat "$mirrors") || return 1
  if [ "$(awk 'NF { print $1; exit }' <<< "$list")" = "$fallback" ]; then
    rest=$(awk -v fallback="$fallback" 'NF && $1 != fallback { print $1 "\tpriority:" ++n }' <<< "$list") || return 1
    [ -n "$rest" ] || return 0
    new=$(printf '%s\n%s\tpriority:%s' "$rest" "$fallback" "$(($(wc -l <<< "$rest") + 1))")
  else
    if grep -Eq "$main" <<< "$list"; then skip=1; fi
    rest=$(awk -v main="$main" -v skip="$skip" -v fallback="$fallback" '
      $1 ~ main { skip = 0 }
      skip || !NF || $1 == fallback { next }
      { print $1 "\tpriority:" ++n + 1 }' <<< "$list") || return 1
    new=$(printf '%s\tpriority:1\n%s\n' "$fallback" "$rest" | sed '/^$/d')
  fi
  if [ -f "$sources" ]; then
    text=$(sed -E "s#https?://security[.]ubuntu[.]com/ubuntu/?#mirror+file:$mirrors#" "$sources") || return 1
    [ -n "$text" ] || return 1
  fi
  printf '%s\n' "$new" | sudo tee "$mirrors" > /dev/null || return 1
  if [ -n "$text" ]; then
    printf '%s\n' "$text" | sudo tee "$sources" > /dev/null || return 1
  fi
  echo "apt mirrors now: $(tr '\n' ' ' < "$mirrors")" >&2
}
downloaded=
for ((attempt = 1; attempt <= attempts; attempt++)); do
  if sudo timeout --verbose "$update_deadline" apt-get "${apt_opts[@]}" update &&
    sudo DEBIAN_FRONTEND=noninteractive timeout --verbose "$download_deadline" "${install[@]}" --download-only "${packages[@]}"; then
    downloaded=1
    break
  fi
  echo "apt download attempt $attempt failed" >&2
  # A live-first attempt is the only one: the verified cache follows, not another mirror.
  [[ -n "$cached" ]] || use_fallback_mirror || echo "could not switch apt mirrors" >&2
done
if [ -z "$downloaded" ]; then
  [[ -n "$cached" ]] || exit 1
  echo "::warning::linux-host-deps: the live download failed; installing the verified $pkgset set from the Actions cache"
  from_cache
  # Greppable in the run summary, so a release whose WebKitGTK may predate a security update is found.
  [[ -z "${GITHUB_STEP_SUMMARY:-}" ]] ||
    echo "apt: cache fallback used: the live download failed, so the verified cached $pkgset set was installed" >> "$GITHUB_STEP_SUMMARY"
  exit 0
fi
if [[ -n "$cache" && -z "$cached" && "${DW_APT_SAVE:-1}" == 1 ]]; then
  if "$apt_cache" save "$cache"; then
    [[ -z "${GITHUB_OUTPUT:-}" ]] || echo "saved=1" >> "$GITHUB_OUTPUT"
  else
    rm -rf "$cache"
    echo "::warning::linux-host-deps: the downloaded .deb files are not cached (see above)"
  fi
fi
sudo DEBIAN_FRONTEND=noninteractive "${install[@]}" -qq --no-download "${packages[@]}"
[[ -z "$cached" ]] || echo "linux-host-deps: the $pkgset set downloaded live; the cached set was not needed"
