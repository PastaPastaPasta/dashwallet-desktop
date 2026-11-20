#!/usr/bin/env bash
# GitHub Actions, from rust/: run one package's test binaries, built with the whole workspace
# selected, so the build resolves the workspace's features.
#
#   ci/github/cargo-test-package.sh <package> [cargo test options]
#   ci/github/cargo-test-package.sh dw-app --features dw-app/selftest
#
# `cargo test -p <package>` resolves features for that package alone, which compiles every
# workspace crate under it again (rust-cache keeps only the dependencies). Selecting the workspace
# compiles only the crates whose own features change. Each binary runs from the package's
# directory, as `cargo test` runs it. Doc tests are not run.
set -euo pipefail
pkg=${1:?usage: $0 <package> [cargo test options]}
shift

read -r manifest version < <(cargo metadata --no-deps --format-version 1 --locked |
  jq -r --arg pkg "$pkg" '.packages[] | select(.name == $pkg) | "\(.manifest_path) \(.version)"') ||
  { echo "::error::no workspace package $pkg"; exit 1; }

artifacts=$(mktemp)
cargo test --workspace --no-run --locked --message-format=json-render-diagnostics "$@" > "$artifacts"
mapfile -t executables < <(jq -r --arg manifest "$manifest" \
  'select(.reason == "compiler-artifact" and .manifest_path == $manifest and .profile.test and .executable != null) | .executable' \
  "$artifacts")
[ "${#executables[@]}" -gt 0 ] || { echo "::error::cargo built no test binary for $pkg"; exit 1; }

dir=$(dirname "$manifest")
status=0
for exe in "${executables[@]}"; do
  echo "     Running ${exe#"$PWD/"}"
  (cd "$dir" && CARGO_MANIFEST_DIR="$dir" CARGO_MANIFEST_PATH="$manifest" CARGO_PKG_NAME="$pkg" \
    CARGO_PKG_VERSION="$version" "$exe") || status=1
done
exit "$status"
