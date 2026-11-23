#!/usr/bin/env bash
# The Linux screen-reader check in one command: builds the debug app (production renderer, real
# engine) and dwcli in Docker and runs e2e-tauri/atspi-walk.ts there under Xvfb, with a private
# session bus and the AT-SPI bus. It fails when an interactive node on any screen has no
# accessible name (docs/qa/screen-reader-linux.md).
#
#   apps/desktop/scripts/atspi-walk-docker.sh OUT_DIR    run the check; results in OUT_DIR
#   apps/desktop/scripts/atspi-walk-docker.sh --clean    remove this checkout's volumes
#
# Image: ci/linux/Dockerfile.atspi, Ubuntu 24.04 with the apt set of CI's Tauri E2E job
# (ci/github/linux-host-deps.sh tauri), so a result here matches that runner. The Rust-only builder
# (ci/linux/Dockerfile) is Debian bookworm without WebKitWebDriver or AT-SPI. The image is tagged
# by the Dockerfile's hash, so an edit builds a new one.
#
# The repo is mounted read-only and copied into the container. The cargo registry and git caches
# are the shared dwd-cargo-* volumes; the target dir and the pnpm store are this checkout's
# (scripts/docker-volumes.sh): `--clean` removes them, or
#   docker volume rm dwd-atspi-target-<tag> dwd-atspi-cache-<tag>
# The container runs as root (the shared volumes are root's) and hands OUT_DIR back to the caller.
# Nothing reaches the host display: the X server is the container's own Xvfb.
#
#   DWD_MIN_FREE_GB   scripts/disk-guard.sh's floor (default 15)
#   DWD_ATSPI_IMAGE   another image
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
# shellcheck source=../../../scripts/docker-volumes.sh
source "$ROOT/scripts/docker-volumes.sh"

if [[ "${1:-}" == --clean ]]; then
  docker volume rm "$DWD_VOLUME_ATSPI_TARGET" "$DWD_VOLUME_ATSPI_CACHE"
  exit 0
fi
OUT="${1:?usage: $0 OUT_DIR | --clean}"

"$ROOT/scripts/disk-guard.sh" "${DWD_MIN_FREE_GB:-15}" "$ROOT"

dockerfile="$ROOT/ci/linux/Dockerfile.atspi"
IMAGE="${DWD_ATSPI_IMAGE:-dwd-atspi:$(sha256sum "$dockerfile" | cut -c1-12)}"
if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  docker build -f "$dockerfile" -t "$IMAGE" "$ROOT/ci"
fi

mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
docker run --rm --init \
  -v "$ROOT":/src:ro \
  -v "$OUT":/out \
  -v dwd-cargo-registry:/cargo-home/registry \
  -v dwd-cargo-git:/cargo-home/git \
  -v "$DWD_VOLUME_ATSPI_TARGET":/target \
  -v "$DWD_VOLUME_ATSPI_CACHE":/cache \
  -e CARGO_HOME=/cargo-home \
  -e CARGO_TARGET_DIR=/target \
  -e CARGO_BUILD_JOBS="$DWD_DOCKER_JOBS" \
  -e CARGO_INCREMENTAL=0 \
  -e DW_E2E_OUT=/out \
  -e DW_E2E_SCRIPT=e2e-tauri/atspi-walk.ts \
  -e OWNER="$(id -u):$(id -g)" \
  "$IMAGE" bash -euo pipefail -c '
    trap "chown -R \"\$OWNER\" /out" EXIT
    mkdir -p /work /cache/home
    tar -C /src --exclude=./rust/target --exclude=node_modules --exclude=./apps/desktop/dist \
      --exclude=./apps/desktop/test-results --exclude=./apps/desktop/.e2e-cache --exclude=.build \
      --exclude=./Artifacts --exclude=./.claude -cf - . | tar -C /work -xf -
    Xvfb :99 -screen 0 1280x800x24 -nolisten tcp >/out/xvfb.log 2>&1 &
    export DISPLAY=:99 HOME=/cache/home
    cd /work/apps/desktop
    pnpm install --frozen-lockfile --store-dir /cache/pnpm-store
    bash scripts/e2e-tauri.sh 2>&1 | tee /out/run.log
  '
