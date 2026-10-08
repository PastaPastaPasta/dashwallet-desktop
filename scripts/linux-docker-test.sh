#!/usr/bin/env bash
# Build the Linux variant of the Rust core and run the headless Swift tests
# inside Docker (OrbStack on the dev Mac), host architecture.
#
#   scripts/linux-docker-test.sh [extra swift test args...]
#
# The repo is mounted read-only and copied into the container, so Linux
# builds never touch the host .build/ or Package.resolved. Only Artifacts/
# is shared. Every .build/ and .swiftpm/ is left out of the copy, including
# ones an editor's indexer creates inside Vendor/swift-cross-ui.
# Cargo caches and build dirs live in named volumes (scripts/docker-volumes.sh).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${DWD_LINUX_IMAGE:-dwd-linux-swift}"
# shellcheck source=scripts/docker-volumes.sh
source "$ROOT/scripts/docker-volumes.sh"

"$ROOT/scripts/disk-guard.sh"

if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  docker build -f "$ROOT/ci/linux/Dockerfile.swift" -t "$IMAGE" "$ROOT/ci/linux"
fi

# Every headless test target that has tests on Linux (MacUITests and
# DashUIMacSnapshotTests compile to nothing there).
filter="${DWD_SWIFT_TEST_FILTER:-DashKitTests|DesignTokensTests|RepoChecksTests|WalletRuntimeTests|WalletFeaturesTests|PlatformServicesDesktopTests}"

# Separate target volume from the Rust-only image (dwd-linux-target): this
# image is Ubuntu noble (glibc 2.39), that one Debian bookworm (glibc 2.36),
# and build-script binaries are not portable from the newer glibc to the older.
mkdir -p "$ROOT/Artifacts"
# Artifacts/ is mounted read-write so the Linux variant lands next to the
# macOS one and info.json lists both.
docker run --rm \
  -v "$ROOT":/src:ro \
  -v "$ROOT/Artifacts":/work/Artifacts \
  -v dwd-cargo-registry:/usr/local/cargo/registry \
  -v dwd-cargo-git:/usr/local/cargo/git \
  -v "$DWD_VOLUME_TARGET":/target \
  -v "$DWD_VOLUME_SWIFTPM":/swiftpm \
  -e CARGO_TARGET_DIR=/target \
  -e CARGO_BUILD_JOBS="$DWD_DOCKER_JOBS" \
  -e DWD_HEADLESS=1 \
  -e DWD_MIN_FREE_GB="${DWD_MIN_FREE_GB:-15}" \
  -e DWD_SWIFT_TEST_FILTER="$filter" \
  "$IMAGE" bash -euo pipefail -c '
    mkdir -p /work
    tar -C /src --exclude=.build --exclude=./.derived --exclude=./.claude --exclude=./Artifacts --exclude=./.build-logs --exclude=.swiftpm -cf - . \
      | tar -C /work -xf -
    cd /work
    start=$(date +%s)
    scripts/build-core.sh --check-bindings
    echo "linux: build-core took $(( $(date +%s) - start ))s; target volume $(du -sh /target | cut -f1)"
    swift build --scratch-path /swiftpm/.build --build-tests
    swift test --scratch-path /swiftpm/.build --skip-build --filter "$DWD_SWIFT_TEST_FILTER" "$@"
  ' bash "$@"
