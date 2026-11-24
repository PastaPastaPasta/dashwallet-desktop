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
  tauri_conf=${TAURI_CONF:-../../rust/crates/dw-app/tauri.conf.json}
  python3 - "$tauri_conf" <<'PY'
import base64, json, sys
from pathlib import Path

retired = {"ED322B963B527B21", "7A4629E8E5438F3C"}
conf_path = Path(sys.argv[1])
if not conf_path.is_file():
    raise SystemExit(f"tauri-build: missing tauri.conf.json at {conf_path}")
pubkey_b64 = json.loads(conf_path.read_text()).get("plugins", {}).get("updater", {}).get("pubkey", "")
raw = base64.b64decode(pubkey_b64.strip()).decode("utf-8")
lines = [line.strip() for line in raw.splitlines() if line.strip()]
key_bytes = base64.b64decode(lines[1])
key_id_hex = key_bytes[2:10][::-1].hex().upper()
if key_id_hex in retired:
    raise SystemExit(
        f"tauri-build: refusing DW_ENABLE_UPDATER=1 with retired updater key ID {key_id_hex} "
        f"in {conf_path} (rotate to a local-only stable updater key before tagging 1.0; docs/release/SIGNING.md)"
    )
PY
  echo "tauri-build: DW_ENABLE_UPDATER=1: embedding stable updater endpoint (unsigned; signed locally)"
  exec pnpm tauri build "$@" \
    --config ../../rust/crates/dw-app/tauri.updater.conf.json \
    --config '{"bundle":{"createUpdaterArtifacts":false}}'
fi

echo "tauri-build: plain build without updater endpoint"
exec pnpm tauri build "$@"
