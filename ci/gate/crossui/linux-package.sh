#!/usr/bin/env bash
# Gate G-UI, U7 (Linux): the release build of `dash-wallet` (SwiftCrossUI, GtkBackend) with the
# Swift runtime linked statically, packed as a portable tarball. Builds in the dwd-linux-crossui
# image with the checkout's Docker volumes (scripts/docker-volumes.sh), like
# scripts/crossui-linux-demo.sh.
#
#   ci/gate/crossui/linux-package.sh OUT_DIR
#
# OUT_DIR gets the tarball and build.json from linux-pack.sh.
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
    CORE_SECONDS=$((t1 - t0)) SWIFT_SECONDS=$((t2 - t1)) ci/gate/crossui/linux-pack.sh "$bin_dir" /out
    chown -R "$HOST_UID:$HOST_GID" /out
  '
