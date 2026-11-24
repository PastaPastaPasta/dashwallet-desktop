#!/usr/bin/env bash
# GitHub Actions (release.yml, macos job): write the App Store Connect API .p8 key to a 0600 file
# in $RUNNER_TEMP and snapshot existing user keychains for the always() cleanup step.
# Never echoes secret values and never enables set -x.
set -euo pipefail

: "${APPLE_API_KEY_P8:?APPLE_API_KEY_P8 must be set}" "${RUNNER_TEMP:?RUNNER_TEMP must be set}" "${GITHUB_ENV:?GITHUB_ENV must be set}"

# Mask interior base64 lines of the multi-line PEM secret.
while IFS= read -r line; do
  if [[ ${#line} -ge 16 && "$line" != -----* && "$line" != *:* ]]; then
    printf '::add-mask::%s\n' "$line"
  fi
done <<<"$APPLE_API_KEY_P8"

dir="$RUNNER_TEMP/apple-signing"
rm -rf "$dir"
(umask 077 && mkdir -p "$dir")
chmod 700 "$dir"

# Snapshot existing keychains so macos-signing-cleanup.sh can remove any temporary keychain
# Tauri created if the build is killed before Tauri's Drop impl runs.
shopt -s nullglob
: > "$dir/keychains-before.txt"
for kc in "$HOME"/Library/Keychains/*.keychain-db; do
  printf '%s\n' "$kc" >> "$dir/keychains-before.txt"
done

p8="$dir/AuthKey.p8"
(umask 077 && printf '%s\n' "$APPLE_API_KEY_P8" > "$p8")
chmod 600 "$p8"

first_line=$(awk 'NF { print; exit }' "$p8")
if [[ "$first_line" != "-----BEGIN "* ]]; then
  rm -rf "$dir"
  echo "::error::macos-signing-setup: APPLE_API_KEY_P8 does not look like a PEM .p8 private key" >&2
  exit 1
fi

echo "APPLE_API_KEY_PATH=$p8" >> "$GITHUB_ENV"
echo "macos-signing-setup: wrote App Store Connect API key (0600) to $p8"
