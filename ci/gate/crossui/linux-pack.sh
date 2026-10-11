#!/usr/bin/env bash
# Gate G-UI, U7 (Linux): packs a release build of `dash-wallet` (SwiftCrossUI, GtkBackend, Swift
# runtime linked statically) as the portable tarball. Used by linux-package.sh (in Docker, for
# agentbox) and crossui/linux.sh (on the gate's ubuntu runner).
#
#   ci/gate/crossui/linux-pack.sh BIN_DIR OUT_DIR
#
# BIN_DIR is `swift build -c release --show-bin-path`. OUT_DIR gets dash-wallet-linux-x86_64.tar.gz
# (one top directory, dash-wallet/, holding the binary and SwiftPM's resource bundles next to it,
# where Bundle.module looks) and build.json (sizes, the binary's shared-library needs, and the
# build times CORE_SECONDS and SWIFT_SECONDS from the environment).
set -euo pipefail
bin_dir=${1:?usage: linux-pack.sh BIN_DIR OUT_DIR}
out=${2:?usage: linux-pack.sh BIN_DIR OUT_DIR}
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
pkg=$stage/dash-wallet
mkdir -p "$pkg" "$out"
cp "$bin_dir/dash-wallet" "$pkg/"
strip "$pkg/dash-wallet"
# The app ID GTK registers on D-Bus. SwiftCrossUI reads it from the Swift Bundler metadata trailer
# (the JSON, its length as a big-endian UInt64, "SBUNMETA") at the end of the executable; without
# one it is com.example.SwiftCrossUIApp, which a Flatpak may not own.
meta='{"appIdentifier":"org.dash.wallet.desktop.CrossUI","appVersion":"0.1.0"}'
printf "%s" "$meta" >>"$pkg/dash-wallet"
perl -e 'print pack(q(Q>), shift), q(SBUNMETA)' "${#meta}" >>"$pkg/dash-wallet"
for bundle in "$bin_dir"/*.resources; do cp -R "$bundle" "$pkg/"; done
tarball=$out/dash-wallet-linux-x86_64.tar.gz
tar -C "$stage" -czf "$tarball" dash-wallet
needs=$(objdump -p "$pkg/dash-wallet" | awk '/NEEDED/ {print $2}' | paste -sd, -)
swift_libs=$(ldd "$pkg/dash-wallet" | grep -c -i swift || true)
cat >"$out/build.json" <<JSON
{
  "coreReleaseSeconds": ${CORE_SECONDS:-null},
  "swiftReleaseSeconds": ${SWIFT_SECONDS:-null},
  "binaryBytes": $(stat -c %s "$pkg/dash-wallet"),
  "tarballBytes": $(stat -c %s "$tarball"),
  "resourceBundles": "$(cd "$pkg" && ls -d -- *.resources | paste -sd, -)",
  "needed": "$needs",
  "swiftLibrariesInLdd": $swift_libs,
  "swift": "$(swift --version 2>&1 | head -1)",
  "gtk": "$(pkg-config --modversion gtk4)"
}
JSON
cat "$out/build.json"
