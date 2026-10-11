#!/usr/bin/env bash
# Gate G-UI, U7 (Linux): the release build of `dash-wallet` (SwiftCrossUI, GtkBackend) with the
# Swift runtime linked statically, packed as a portable tarball. Builds in the dwd-linux-crossui
# image with the checkout's Docker volumes (scripts/docker-volumes.sh), like
# scripts/crossui-linux-demo.sh.
#
#   ci/gate/crossui/linux-package.sh OUT_DIR
#
# OUT_DIR gets dash-wallet-linux-x86_64.tar.gz (one top directory, dash-wallet/, holding the binary
# and SwiftPM's resource bundles next to it, where Bundle.module looks) and build.json (timings,
# sizes, the binary's shared-library needs).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
OUT="$(mkdir -p "${1:?usage: linux-package.sh OUT_DIR}" && cd "$1" && pwd)"
IMAGE="${DWD_CROSSUI_IMAGE:-dwd-linux-crossui}"
# shellcheck source=scripts/docker-volumes.sh
source "$ROOT/scripts/docker-volumes.sh"
"$ROOT/scripts/disk-guard.sh" "${DWD_MIN_FREE_GB:-15}"
if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  docker build -f "$ROOT/ci/linux/Dockerfile.crossui" -t "$IMAGE" "$ROOT/ci/linux"
fi

docker run --rm \
  -v "$ROOT":/src:ro \
  -v "$OUT":/out \
  -v dwd-cargo-registry:/usr/local/cargo/registry \
  -v dwd-cargo-git:/usr/local/cargo/git \
  -v "$DWD_VOLUME_TARGET":/target \
  -v "$DWD_VOLUME_SWIFTPM":/swiftpm \
  -e CARGO_TARGET_DIR=/target \
  -e CARGO_BUILD_JOBS="$DWD_DOCKER_JOBS" \
  -e DWD_MIN_FREE_GB="${DWD_MIN_FREE_GB:-15}" \
  -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
  "$IMAGE" bash -euo pipefail -c '
    mkdir -p /work
    tar -C /src --exclude=.build --exclude=./rust/target --exclude=./.derived --exclude=./.claude \
        --exclude=./Artifacts --exclude=./.build-logs --exclude=.swiftpm --exclude=./docs/screenshots -cf - . \
      | tar -C /work -xf -
    cd /work
    t0=$(date +%s)
    scripts/build-core.sh --profile release --no-bindings
    t1=$(date +%s)
    swift build --scratch-path /swiftpm/.build -c release --static-swift-stdlib --product dash-wallet
    t2=$(date +%s)
    bin_dir=$(swift build --scratch-path /swiftpm/.build -c release --show-bin-path)
    pkg=/tmp/pkg/dash-wallet
    rm -rf /tmp/pkg && mkdir -p "$pkg"
    cp "$bin_dir/dash-wallet" "$pkg/"
    strip "$pkg/dash-wallet"
    # The app ID GTK registers on D-Bus. SwiftCrossUI reads it from the Swift Bundler metadata
    # trailer (the JSON, its length as a big-endian UInt64, "SBUNMETA") at the end of the
    # executable; without one it is com.example.SwiftCrossUIApp, which a Flatpak may not own.
    meta="{\"appIdentifier\":\"org.dash.wallet.desktop.CrossUI\",\"appVersion\":\"0.1.0\"}"
    printf "%s" "$meta" >>"$pkg/dash-wallet"
    perl -e "print pack(q(Q>), shift), q(SBUNMETA)" "${#meta}" >>"$pkg/dash-wallet"
    for bundle in "$bin_dir"/*.resources; do cp -R "$bundle" "$pkg/"; done
    tar -C /tmp/pkg -czf /out/dash-wallet-linux-x86_64.tar.gz dash-wallet
    needs=$(objdump -p "$pkg/dash-wallet" | awk "/NEEDED/ {print \$2}" | paste -sd, -)
    swift_libs=$(ldd "$pkg/dash-wallet" | grep -c -i swift || true)
    cat >/out/build.json <<JSON
{
  "coreReleaseSeconds": $((t1 - t0)),
  "swiftReleaseSeconds": $((t2 - t1)),
  "binaryBytes": $(stat -c %s "$pkg/dash-wallet"),
  "tarballBytes": $(stat -c %s /out/dash-wallet-linux-x86_64.tar.gz),
  "resourceBundles": "$(cd "$pkg" && ls -d -- *.resources | paste -sd, -)",
  "needed": "$needs",
  "swiftLibrariesInLdd": $swift_libs,
  "swift": "$(swift --version 2>&1 | head -1)",
  "gtk": "$(pkg-config --modversion gtk4)"
}
JSON
    chown -R "$HOST_UID:$HOST_GID" /out
    cat /out/build.json
  '
