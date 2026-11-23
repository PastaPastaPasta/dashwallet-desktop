#!/usr/bin/env bash
# Builds the Linux core bundle and the SwiftCrossUI app (GtkBackend) in Docker,
# runs `dash-wallet --demo` under Xvfb, dumps its AT-SPI tree and takes
# screenshots.
#
#   scripts/crossui-linux-demo.sh [OUT_DIR]     (default docs/screenshots/m1/linux)
#
# The repo is mounted read-only and copied into the container (untracked files
# included). Artifacts/ is NOT shared: the Linux bundle is built inside the
# container. Cargo caches and build dirs live in the named volumes that
# scripts/linux-docker-test.sh also uses (scripts/docker-volumes.sh).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${DWD_CROSSUI_IMAGE:-dwd-linux-crossui}"
OUT="${1:-$ROOT/docs/screenshots/m1/linux}"
# shellcheck source=scripts/docker-volumes.sh
source "$ROOT/scripts/docker-volumes.sh"

"$ROOT/scripts/disk-guard.sh" "${DWD_MIN_FREE_GB:-15}"

if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  docker build -f "$ROOT/ci/linux/Dockerfile.crossui" -t "$IMAGE" "$ROOT/ci/linux"
fi

mkdir -p "$OUT"
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
  -e DWD_CROSSUI_SUITE="${DWD_CROSSUI_SUITE:-m1}" \
  "$IMAGE" bash /src/ci/linux/crossui/container-demo.sh 2>&1 | tee "$OUT/run.log"
