#!/usr/bin/env bash
# release.yml: `pnpm tauri build "$@"`, with the update feed when the updater key is set (docs/release/updater.md).
set -euo pipefail

# Tauri's bundler checks std::env::var_os("APPLE_*"), which treats "" as set rather than absent.
# Unset empty Apple signing/notarization variables so tauri.conf.json's ad-hoc "-" identity keeps working.
for v in APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY \
  APPLE_API_ISSUER APPLE_API_KEY APPLE_API_KEY_PATH APPLE_TEAM_ID; do
  [[ -n "${!v:-}" ]] || unset "$v"
done

if [[ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]]; then
  echo "tauri-build: the updater signing key is set: a build with the update feed"
  [[ -z "${GITHUB_ENV:-}" ]] || echo "UPDATER=1" >> "$GITHUB_ENV"
  exec pnpm tauri build "$@" --config ../../rust/crates/dw-app/tauri.updater.conf.json
fi
if [[ "${GITHUB_EVENT_NAME:-}" == push && "${GITHUB_REF:-}" =~ ^refs/tags/v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  if [[ "${DW_UPDATER_REQUIRED:-}" == 1 ]]; then
    echo "::error::tauri-build: no updater signing key on ${GITHUB_REF#refs/tags/}, and DW_UPDATER_REQUIRED=1: installed copies would get no update. Check the release environment's secrets." >&2
    exit 1
  fi
  echo "::warning::tauri-build: no updater signing key on ${GITHUB_REF#refs/tags/}: this release has no updater and publishes no update feed."
fi
echo "tauri-build: no updater signing key: a build without the updater"
exec pnpm tauri build "$@"
