#!/usr/bin/env bash
# Runs INSIDE the dwd-linux-swift-gtk container with the probe mounted at /work.
# Builds the probe and runs its headless tests, timing each step. Output goes to
# stdout and to /work/.build/logs/ (the .build volume, not the git tree).
set -euo pipefail
cd /work
mkdir -p .build/logs
echo "== host: $(uname -m), $(nproc) cpus, swift: $(swift --version 2>&1 | head -1)"
echo "== gtk4: $(pkg-config --modversion gtk4)"

timed() {
    local name=$1; shift
    local start end status
    start=$(date +%s%N)
    set +e
    "$@" 2>&1 | tee ".build/logs/$name.log"
    status=${PIPESTATUS[0]}
    set -e
    end=$(date +%s%N)
    echo "== $name: exit $status, $(( (end - start) / 1000000 )) ms"
    return "$status"
}

timed swift-build swift build "$@"
timed swift-test swift test "$@"

bin=$(swift build "$@" --show-bin-path)/CrossUILinuxProbe
echo "== binary: $bin"
ls -l "$bin"
file "$bin"
echo "== stripped size estimate:"
strip -o /tmp/probe-stripped "$bin" && ls -l /tmp/probe-stripped
echo "== shared libraries:"
ldd "$bin" | sed 's/ (0x[0-9a-f]*)//' | sort
