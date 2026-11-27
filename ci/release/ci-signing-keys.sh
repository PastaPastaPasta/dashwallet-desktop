#!/usr/bin/env bash
# Key material for release-sign.yml, on its macOS runner only (docs/release/CI-SIGNING.md). Every value
# comes from the `release-signing` environment through the step's env; none is echoed, and `set -x` is
# never enabled.
#
#   ci/release/ci-signing-keys.sh apple <identities.json>   # APPLE_CERTIFICATE, APPLE_CERTIFICATE_PASSWORD, APPLE_API_KEY_P8
#   ci/release/ci-signing-keys.sh pgp <identities.json>     # RELEASE_PGP_PRIVATE_KEY, RELEASE_PGP_PASSPHRASE
#   ci/release/ci-signing-keys.sh cleanup
#
# apple: imports the Developer ID .p12 into a throwaway keychain and writes the App Store Connect API key
#        to a 0600 file; refuses a certificate that is not the one Developer ID Application identity of the
#        pinned apple_team_id. Exports APPLE_SIGNING_IDENTITY and APPLE_API_KEY_PATH.
# pgp:   imports the signing key into a throwaway GNUPGHOME; refuses a key whose primary fingerprint is not
#        the pinned pgp_fingerprint. Exports GNUPGHOME.
set -euo pipefail
umask 077

die() { echo "::error::ci-signing-keys: $*" >&2; exit 1; }

cmd=${1:?usage: ci-signing-keys.sh apple|pgp|cleanup [identities.json]}
: "${RUNNER_TEMP:?RUNNER_TEMP must be set}"
dir="$RUNNER_TEMP/dw-signing"
keychain="$dir/signing.keychain-db"
gpg_link="$dir/gnupg-home"

mask_lines() {
  local line
  while IFS= read -r line; do
    if [[ ${#line} -ge 16 && "$line" != -----* ]]; then printf '::add-mask::%s\n' "$line"; fi
  done
}

pinned() {
  local file=$1 key=$2 value
  [[ -f "$file" && ! -L "$file" ]] || die "identities file not found: $file"
  value=$(jq -r --arg k "$key" '.[$k] // empty' "$file")
  [[ -n "$value" && "$value" != *REPLACE_* ]] || die "$key is not pinned in $file; the public identities land on main before the tag"
  printf '%s' "$value"
}

case "$cmd" in
  apple)
    [[ "$(uname -s)" == Darwin ]] || die "apple runs on macOS"
    : "${APPLE_CERTIFICATE:?}" "${APPLE_CERTIFICATE_PASSWORD:?}" "${APPLE_API_KEY_P8:?}" "${GITHUB_ENV:?}"
    team=$(pinned "${2:?identities.json}" apple_team_id)
    mask_lines <<<"$APPLE_API_KEY_P8"
    mkdir -p "$dir"
    chmod 700 "$dir"

    keychain_pass=$(openssl rand -hex 24)
    printf '%s' "$APPLE_CERTIFICATE" | base64 --decode > "$dir/developer-id.p12" ||
      die "APPLE_CERTIFICATE is not base64 of a .p12"
    security create-keychain -p "$keychain_pass" "$keychain"
    security set-keychain-settings -lut 7200 "$keychain"
    security unlock-keychain -p "$keychain_pass" "$keychain"
    security import "$dir/developer-id.p12" -k "$keychain" -f pkcs12 -P "$APPLE_CERTIFICATE_PASSWORD" \
      -T /usr/bin/codesign >/dev/null || die "the .p12 did not import (wrong APPLE_CERTIFICATE_PASSWORD?)"
    rm -f "$dir/developer-id.p12"
    security set-key-partition-list -S apple-tool:,apple: -s -k "$keychain_pass" "$keychain" >/dev/null
    existing=()
    while IFS= read -r kc; do
      kc=${kc#"${kc%%[![:space:]]*}"}
      kc=${kc#\"}
      kc=${kc%\"}
      [[ -n "$kc" ]] && existing+=("$kc")
    done < <(security list-keychains -d user)
    security list-keychains -d user -s "$keychain" ${existing[@]+"${existing[@]}"}

    identities=$(security find-identity -v -p codesigning "$keychain" | awk -F'"' '/"Developer ID Application: /{print $2}')
    [[ -n "$identities" && "$identities" != *$'\n'* ]] ||
      die "the .p12 must hold exactly one Developer ID Application identity"
    [[ "$identities" == *"($team)" ]] ||
      die "the .p12's identity is not of the pinned Apple team $team"

    p8="$dir/AuthKey.p8"
    printf '%s\n' "$APPLE_API_KEY_P8" > "$p8"
    chmod 600 "$p8"
    [[ "$(awk 'NF { print; exit }' "$p8")" == "-----BEGIN "* ]] || die "APPLE_API_KEY_P8 is not a PEM .p8 key"
    {
      echo "APPLE_SIGNING_IDENTITY=$identities"
      echo "APPLE_API_KEY_PATH=$p8"
    } >> "$GITHUB_ENV"
    echo "ci-signing-keys: imported $identities"
    ;;
  pgp)
    : "${RELEASE_PGP_PRIVATE_KEY:?}" "${RELEASE_PGP_PASSPHRASE:?}" "${GITHUB_ENV:?}"
    want=$(pinned "${2:?identities.json}" pgp_fingerprint | tr -d '[:space:]' | tr '[:lower:]' '[:upper:]')
    mask_lines <<<"$RELEASE_PGP_PRIVATE_KEY"
    mkdir -p "$dir"
    chmod 700 "$dir"
    # A short path: gpg-agent's socket must fit macOS's 104-byte limit.
    home=$(mktemp -d /tmp/dwsign.XXXXXX)
    chmod 700 "$home"
    printf '%s\n' "$home" > "$gpg_link"
    GNUPGHOME=$home gpg --batch --quiet --pinentry-mode loopback --passphrase-fd 3 --import \
      3<<<"$RELEASE_PGP_PASSPHRASE" <<<"$RELEASE_PGP_PRIVATE_KEY" || die "the PGP key did not import"
    primaries=$(GNUPGHOME=$home gpg --batch --with-colons --list-secret-keys |
      awk -F: '$1 == "sec" { want = 1; next } want && $1 == "fpr" { print toupper($10); want = 0 }')
    [[ "$primaries" == "$want" ]] ||
      die "RELEASE_PGP_PRIVATE_KEY must hold exactly the pinned key $want (got: ${primaries//$'\n'/ })"
    echo "GNUPGHOME=$home" >> "$GITHUB_ENV"
    echo "ci-signing-keys: imported PGP key $want"
    ;;
  cleanup)
    if [[ -f "$keychain" ]]; then security delete-keychain "$keychain" >/dev/null 2>&1 || true; fi
    if [[ -f "$gpg_link" ]]; then
      home=$(cat "$gpg_link")
      if [[ "$home" == /tmp/dwsign.* && -d "$home" ]]; then
        GNUPGHOME=$home gpgconf --kill all >/dev/null 2>&1 || true
        rm -rf "$home"
      fi
    fi
    rm -rf "$dir"
    echo "ci-signing-keys: removed the keychain, the API key and the GnuPG home"
    ;;
  *) die "unknown command: $cmd" ;;
esac
