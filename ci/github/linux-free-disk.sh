#!/usr/bin/env bash
# GitHub Actions (ubuntu runners, not job containers): delete preinstalled toolchains this
# project never uses, to make room for the Rust target dir and Docker images.
set -euo pipefail
sudo rm -rf /usr/share/dotnet /usr/local/lib/android /opt/ghc /opt/hostedtoolcache/CodeQL
df -h /
