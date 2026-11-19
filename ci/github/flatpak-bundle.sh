#!/usr/bin/env bash
# GitHub Actions (release.yml, `flatpak` job) and local builds: build the Flatpak from a release .deb
# with packaging/flatpak/org.dash.wallet.desktop.yml and export it as a single-file bundle.
#
#   ci/github/flatpak-bundle.sh DEB OUT_BUNDLE WORK_DIR
#
# The GNOME runtime and SDK come from the Flathub remote of the user installation (FLATPAK_USER_DIR
# when set), added here when missing. The build, its repository and flatpak-builder's state stay
# under WORK_DIR. The .deb is copied next to the manifest for the build and removed afterwards.
set -euo pipefail

deb=$(realpath "${1:?usage: $0 DEB OUT_BUNDLE WORK_DIR}")
out=${2:?}
work=${3:?}
app_id=org.dash.wallet.desktop
dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../packaging/flatpak" && pwd)

mkdir -p "$work" "$(dirname "$out")"
cp "$deb" "$dir/dash-wallet.deb"
trap 'rm -f "$dir/dash-wallet.deb"' EXIT

flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak-builder --user --install-deps-from=flathub --disable-rofiles-fuse --force-clean \
  --state-dir="$work/state" --repo="$work/repo" "$work/build" "$dir/$app_id.yml"
flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
  "$work/repo" "$out" "$app_id"
echo "flatpak-bundle: $out ($(wc -c <"$out") bytes)" >&2
