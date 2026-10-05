#!/usr/bin/env bash
# Build the Linux variant of the Rust core and run the headless Swift tests
# inside Docker (OrbStack on the dev Mac), host architecture.
#
#   scripts/linux-docker-test.sh [extra swift test args...]
#
# The repo is mounted read-only and copied into the container, so Linux
# builds never touch the host .build/ or Package.resolved (headless
# resolution drops the SwiftCrossUI pins). Only Artifacts/ is shared.
# Cargo caches and build dirs live in named volumes.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${DWD_LINUX_IMAGE:-dwd-linux-swift}"

"$ROOT/scripts/disk-guard.sh"

if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  docker build -f "$ROOT/ci/linux/Dockerfile.swift" -t "$IMAGE" "$ROOT/ci/linux"
fi

filter="${DWD_SWIFT_TEST_FILTER:-DashKitTests|RepoChecksTests}"

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
  -v dwd-linux-swift-target:/target \
  -v dwd-linux-swiftpm:/swiftpm \
  -e CARGO_TARGET_DIR=/target \
  -e CARGO_BUILD_JOBS="${DWD_LINUX_JOBS:-8}" \
  -e DWD_HEADLESS=1 \
  -e DWD_SWIFT_TEST_FILTER="$filter" \
  "$IMAGE" bash -euo pipefail -c '
    mkdir -p /work
    tar -C /src --exclude=./.build --exclude=./Artifacts --exclude=./.build-logs --exclude=./.swiftpm -cf - . \
      | tar -C /work -xf -
    cd /work
    start=$(date +%s)
    scripts/build-core.sh --check-bindings
    echo "linux: build-core took $(( $(date +%s) - start ))s; target volume $(du -sh /target | cut -f1)"
    swift build --scratch-path /swiftpm/.build --build-tests
    swift test --scratch-path /swiftpm/.build --skip-build --filter "$DWD_SWIFT_TEST_FILTER" "$@"
  ' bash "$@"
