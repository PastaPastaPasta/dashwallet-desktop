#!/usr/bin/env bash
# release.yml: `pnpm tauri build "$@"`, optionally embedding the stable updater endpoint.
#
# Stable signing keys never live in CI (docs/release/SIGNING.md):
#   - DW_ENABLE_UPDATER=1 (set on stable vX.Y.Z tags): merges tauri.updater.conf.json (the stable
#     feed URL; the public key is already in tauri.conf.json) with createUpdaterArtifacts=false so
#     CI builds the unsigned app with the updater enabled and zero private keys.
#   - TAURI_SIGNING_PRIVATE_KEY (local developer builds only, docs/release/updater.md): merges
#     tauri.updater.conf.json with createUpdaterArtifacts=true and sets UPDATER=1.
#   - Otherwise (main, -rc.N, dry runs, dispatches): plain build without the updater endpoint.
set -euo pipefail

if [[ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]]; then
  echo "tauri-build: local updater signing key is set: building with updater artifacts"
  [[ -z "${GITHUB_ENV:-}" ]] || echo "UPDATER=1" >> "$GITHUB_ENV"
  exec pnpm tauri build "$@" --config ../../rust/crates/dw-app/tauri.updater.conf.json
fi

if [[ "${DW_ENABLE_UPDATER:-}" == 1 ]]; then
  echo "tauri-build: DW_ENABLE_UPDATER=1: embedding stable updater endpoint (unsigned; signed locally)"
  exec pnpm tauri build "$@" \
    --config ../../rust/crates/dw-app/tauri.updater.conf.json \
    --config '{"bundle":{"createUpdaterArtifacts":false}}'
fi

echo "tauri-build: plain build without updater endpoint"
exec pnpm tauri build "$@"
