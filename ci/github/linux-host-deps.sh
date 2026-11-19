#!/usr/bin/env bash
# GitHub Actions: apt packages on an ubuntu-24.04 runner.
#
#   ci/github/linux-host-deps.sh rust|tauri|flatpak|swiftcrossui
#
#   rust          what `cargo build --workspace` needs beyond the runner image: the WebKitGTK
#                 development packages of the Tauri crate rust/crates/dw-app (G-01; list from
#                 apps/desktop/README.md), installed before it lands so that merge needs no CI change;
#   tauri         those plus Xvfb, AT-SPI, xdotool, the WebKitGTK WebDriver, D-Bus and xdg-utils for
#                 the gate harness and the link checks (apps/desktop/README.md);
#   flatpak       flatpak-builder and the launch check's Xvfb, D-Bus and ImageMagick (release.yml);
#   swiftcrossui  GTK 4, Xvfb and the AT-SPI stack (ci/linux/Dockerfile.swift-gtk).
set -euo pipefail

webkit=(libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev
  librsvg2-dev)
gui=(xvfb xauth x11-utils x11-apps netpbm dbus-x11 at-spi2-core gir1.2-atspi-2.0 python3-pyatspi python3-gi)

case "${1:?usage: $0 rust|tauri|flatpak|swiftcrossui}" in
  rust) packages=("${webkit[@]}") ;;
  # Ubuntu 24.04 calls the WebDriver package webkit2gtk-driver (26.04: webkitgtk-webdriver).
  # dbus and xdg-utils: second launches and links the OS opens (U-01, ci/github/deep-links.sh).
  tauri) packages=("${webkit[@]}" "${gui[@]}" xdotool imagemagick webkit2gtk-driver dbus xdg-utils) ;;
  flatpak) packages=(flatpak flatpak-builder "${gui[@]}" imagemagick dbus) ;;
  swiftcrossui) packages=(libgtk-4-dev "${gui[@]}" gsettings-desktop-schemas adwaita-icon-theme fonts-dejavu-core) ;;
  *) echo "unknown package set $1" >&2; exit 2 ;;
esac

apt_opts=(-o Acquire::Retries=3 -o Acquire::http::Timeout=30 -o Acquire::https::Timeout=30)
for attempt in 1 2 3; do
  if sudo timeout 120 apt-get "${apt_opts[@]}" update &&
    sudo DEBIAN_FRONTEND=noninteractive timeout 360 apt-get "${apt_opts[@]}" install -y -qq --no-install-recommends \
      "${packages[@]}"; then
    exit 0
  fi
  echo "apt attempt $attempt failed" >&2
done
exit 1
