#!/usr/bin/env bash
# GitHub Actions: apt packages on an ubuntu-24.04 runner.
#
#   ci/github/linux-host-deps.sh rust|tauri|swiftcrossui
#
#   rust          what `cargo build --workspace` needs beyond the runner image: the WebKitGTK
#                 development packages of the Tauri crate rust/crates/dw-app (G-01; list from
#                 apps/desktop/README.md), installed before it lands so that merge needs no CI change;
#   tauri         those plus Xvfb, AT-SPI, xdotool, the WebKitGTK WebDriver, D-Bus and xdg-utils for
#                 the gate harness and the link checks (apps/desktop/README.md);
#   swiftcrossui  GTK 4, Xvfb and the AT-SPI stack (ci/linux/Dockerfile.swift-gtk).
set -euo pipefail

webkit=(libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev
  librsvg2-dev)
gui=(xvfb xauth x11-utils x11-apps netpbm dbus-x11 at-spi2-core gir1.2-atspi-2.0 python3-pyatspi python3-gi)

case "${1:?usage: $0 rust|tauri|swiftcrossui}" in
  rust) packages=("${webkit[@]}") ;;
  # Ubuntu 24.04 calls the WebDriver package webkit2gtk-driver (26.04: webkitgtk-webdriver).
  # dbus and xdg-utils: second launches and links the OS opens (U-01, ci/github/deep-links.sh).
  tauri) packages=("${webkit[@]}" "${gui[@]}" xdotool imagemagick webkit2gtk-driver dbus xdg-utils) ;;
  swiftcrossui) packages=(libgtk-4-dev "${gui[@]}" gsettings-desktop-schemas adwaita-icon-theme fonts-dejavu-core) ;;
  *) echo "unknown package set $1" >&2; exit 2 ;;
esac

apt_opts=(-o Acquire::Retries=3 -o Acquire::http::Timeout=30 -o Acquire::https::Timeout=30)
install=(apt-get "${apt_opts[@]}" install -y --no-install-recommends)
# Only the network steps run under a deadline and are tried again. A deadline that fired while
# dpkg was unpacking would leave a journal that apt refuses to continue from.
# The runner image lists its regional mirror first (mirror+file:/etc/apt/apt-mirrors.txt). When
# that mirror stalls without failing, apt never moves on to the next one, so a failed attempt
# drops every mirror listed ahead of archive.ubuntu.com.
mirrors=${DW_APT_MIRRORS:-/etc/apt/apt-mirrors.txt}
skip_stalled_mirror() {
  local main='^https?://archive\.ubuntu\.com/'
  sudo grep -Eq "$main" "$mirrors" 2>/dev/null || return 0
  sudo sed -i -E "0,\\#$main#{\\#$main#!d}" "$mirrors"
  echo "apt mirrors now: $(tr '\n' ' ' < "$mirrors")" >&2
}
downloaded=
for attempt in 1 2 3; do
  if sudo timeout --verbose 120 apt-get "${apt_opts[@]}" update &&
    sudo DEBIAN_FRONTEND=noninteractive timeout --verbose 360 "${install[@]}" --download-only "${packages[@]}"; then
    downloaded=1
    break
  fi
  echo "apt download attempt $attempt failed" >&2
  skip_stalled_mirror
done
[ -n "$downloaded" ] || exit 1
sudo DEBIAN_FRONTEND=noninteractive "${install[@]}" -qq --no-download "${packages[@]}"
