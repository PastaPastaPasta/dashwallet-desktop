#!/usr/bin/env bash
# Host-side entry point for the G2 probe. Builds and tests the probe in the
# dwd-linux-swift-gtk image, then runs the Xvfb + AT-SPI smoke test.
#
#   probes/crossui-linux/scripts/run-in-container.sh [OUT_DIR] [--platform linux/amd64]
#
# OUT_DIR (default: <repo>/scratch/crossui-linux-<arch>) receives the AT-SPI dumps,
# screenshots and logs. SwiftPM's .build lives in a named Docker volume per arch so
# the git tree stays clean. With --platform the image defaults to
# dwd-linux-swift-gtk:<arch>, built with
#   docker build --platform linux/amd64 -f ci/linux/Dockerfile.swift-gtk -t dwd-linux-swift-gtk:amd64 ci/linux
set -euo pipefail
probe_dir=$(cd "$(dirname "$0")/.." && pwd)
repo_dir=$(cd "$probe_dir/../.." && pwd)
platform_args=()
arch=$(uname -m)
image=${IMAGE:-dwd-linux-swift-gtk}
out_dir=""
while [[ $# -gt 0 ]]; do
    case $1 in
        --platform)
            platform_args=(--platform "$2"); arch=${2#linux/}
            image=${IMAGE:-dwd-linux-swift-gtk:$arch}; shift 2 ;;
        *) out_dir=$1; shift ;;
    esac
done
out_dir=${out_dir:-$repo_dir/scratch/crossui-linux-$arch}
mkdir -p "$out_dir"
volume=dwd-crossui-probe-build-$arch

run() {
    # The +-expansion keeps macOS's bash 3.2 happy with an empty array under set -u.
    docker run --rm ${platform_args[@]+"${platform_args[@]}"} \
        -v "$probe_dir:/work" -v "$volume:/work/.build" -v "$out_dir:/out" \
        "$image" "$@"
}

run /work/scripts/container-build-test.sh 2>&1 | tee "$out_dir/build-test.log"
run /work/scripts/container-a11y-smoke.sh 2>&1 | tee "$out_dir/a11y-smoke.log"
