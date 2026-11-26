#!/usr/bin/env bash
# The Linux screen-reader check in one command: builds the debug app (production renderer, real
# engine) and dwcli in Docker and runs e2e-tauri/atspi-walk.ts there under Xvfb, with a private
# session bus and the AT-SPI bus. It fails when an interactive node on any screen has no
# accessible name (docs/qa/screen-reader-linux.md).
#
#   apps/desktop/scripts/atspi-walk-docker.sh OUT_DIR    run the check; results in a new
#                                                        OUT_DIR/docker-<time>
#   apps/desktop/scripts/atspi-walk-docker.sh --clean    remove this checkout's volumes
#
# Image: ci/linux/Dockerfile.atspi, Ubuntu 24.04 with the apt set of CI's Tauri E2E job
# (ci/github/linux-host-deps.sh tauri), so a result here matches that runner. The Rust-only builder
# (ci/linux/Dockerfile) is Debian bookworm without WebKitWebDriver or AT-SPI. The image is tagged
# by the hash of the Dockerfile and the linux-host-deps.sh it copies, so an edit to either builds a
# new one.
#
# The repo is mounted read-only and copied into the container. The cargo registry and git caches
# are the shared dwd-cargo-* volumes; the target dir and the pnpm store are this checkout's
# (scripts/docker-volumes.sh): `--clean` removes them, or
#   docker volume rm dwd-atspi-target-<tag> dwd-atspi-cache-<tag>
# Each run gets a directory of its own, OUT_DIR/docker-<time>, made here: the container sees only
# that one, as /out, and the walk writes to a directory it makes in it (atspi-<time>). The
# container runs as root (the shared volumes are root's) and hands that run directory, and nothing
# else, back to the caller.
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

dockerfile="$ROOT/ci/linux/Dockerfile.atspi"
if [[ -n "${DWD_ATSPI_IMAGE:-}" ]]; then
  IMAGE="$DWD_ATSPI_IMAGE"
else
  if command -v sha256sum >/dev/null; then
    hash=(sha256sum)
  elif command -v shasum >/dev/null; then
    hash=(shasum -a 256)
  else
    echo "atspi-walk-docker: neither sha256sum nor shasum is installed; set DWD_ATSPI_IMAGE" >&2
    exit 2
  fi
  IMAGE="dwd-atspi:$(cat "$dockerfile" "$ROOT/ci/github/linux-host-deps.sh" | "${hash[@]}" | cut -c1-12)"
fi

"$ROOT/scripts/disk-guard.sh" "${DWD_MIN_FREE_GB:-15}" "$ROOT"

if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  docker build -f "$dockerfile" -t "$IMAGE" "$ROOT/ci"
fi

mkdir -p "$OUT"
RUN="$(cd "$OUT" && pwd)/docker-$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -m 700 "$RUN"
echo "atspi-walk-docker: results in $RUN"
docker run --rm --init \
  -v "$ROOT":/src:ro \
  -v "$RUN":/out \
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
