#!/usr/bin/env bash
# GitHub Actions (release.yml): `pnpm tauri build "$@"` from apps/desktop. With the updater signing key
# (TAURI_SIGNING_PRIVATE_KEY, only on release tags) it merges rust/crates/dw-app/tauri.updater.conf.json:
# the feed URL and `bundle.createUpdaterArtifacts`, so Tauri signs the updater artifacts (PK-03,
# docs/release/updater.md), and it records `updater=1` in the step's outputs and UPDATER=1 in the job's
# environment. Without the key the build is the plain one, with no updater in it.
set -euo pipefail

if [[ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]]; then
  echo "tauri-build: the updater signing key is set: a build with the update feed"
  [[ -z "${GITHUB_OUTPUT:-}" ]] || echo "updater=1" >> "$GITHUB_OUTPUT"
  [[ -z "${GITHUB_ENV:-}" ]] || echo "UPDATER=1" >> "$GITHUB_ENV"
  exec pnpm tauri build "$@" --config ../../rust/crates/dw-app/tauri.updater.conf.json
fi
echo "tauri-build: no updater signing key: a build without the updater"
exec pnpm tauri build "$@"
