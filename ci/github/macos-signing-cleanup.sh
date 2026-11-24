#!/usr/bin/env bash
# GitHub Actions (release.yml, macos job, if: always()): remove the temporary .p8 key file and any
# leftover temporary keychain created during the build.
set -euo pipefail

dir="${RUNNER_TEMP:-}/apple-signing"
[[ -n "${RUNNER_TEMP:-}" && -d "$dir" ]] || exit 0

if [[ -f "$dir/keychains-before.txt" ]]; then
  shopt -s nullglob
  for kc in "$HOME"/Library/Keychains/*.keychain-db; do
    if ! grep -Fxq -- "$kc" "$dir/keychains-before.txt"; then
      echo "macos-signing-cleanup: deleting leftover temporary keychain $(basename "$kc")"
      security delete-keychain "$kc" || true
    fi
  done
fi

rm -rf "$dir"
echo "macos-signing-cleanup: removed $dir"
