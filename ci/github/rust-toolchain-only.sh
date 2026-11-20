#!/usr/bin/env bash
# GitHub Actions, from rust/: install rust-toolchain.toml's toolchain, make it the default and
# uninstall every other toolchain (the runner image's stable).
#
# Swatinem/rust-cache hashes `rustc -vV` of every installed toolchain into its cache key. With the
# image's stable installed, an image update that bumps it (1.99.1 to 1.100.0 on 2026-11-19) moves
# every job to a key no run has saved yet, and they build the whole dependency graph cold.
set -euo pipefail
rustup toolchain install
active=$(rustup show active-toolchain)
active=${active%% *}
rustup default "$active"
rustup toolchain list --quiet | while read -r toolchain _; do
  if [ "$toolchain" != "$active" ]; then
    rustup toolchain uninstall "$toolchain"
  fi
done
rustup toolchain list
rustc --version
