#!/usr/bin/env bash
# GitHub Actions (release.yml, checksums job): produce a detached armored PGP signature
# (<file>.asc) over SHA256SUMS using an exported signing subkey in a throwaway GNUPGHOME,
# verify the imported primary fingerprint against RELEASE_PGP_FINGERPRINT, and verify the
# resulting signature before returning.
#
#   ci/github/release-pgp-sign.sh <path-to-SHA256SUMS>
#
# Never echoes secret values, never passes the passphrase on argv, and never enables set -x.
set -euo pipefail

sums=${1:?usage: $0 <path-to-SHA256SUMS>}
[[ -f "$sums" ]] || { echo "::error::release-pgp-sign: $sums does not exist" >&2; exit 1; }

: "${RELEASE_PGP_PRIVATE_KEY:?RELEASE_PGP_PRIVATE_KEY must be set}"
: "${RELEASE_PGP_PASSPHRASE:?RELEASE_PGP_PASSPHRASE must be set}"
: "${RELEASE_PGP_FINGERPRINT:?RELEASE_PGP_FINGERPRINT must be set}"

# Normalize expected primary fingerprint: strip whitespace, uppercase, require 40 hex chars.
want_fpr=$(printf '%s' "$RELEASE_PGP_FINGERPRINT" | tr -d '[:space:]' | tr '[:lower:]' '[:upper:]')
if [[ ! "$want_fpr" =~ ^[0-9A-F]{40}$ ]]; then
  echo "::error::release-pgp-sign: RELEASE_PGP_FINGERPRINT must be a 40-hex-character fingerprint" >&2
  exit 1
fi

# Mask interior base64 lines of the multi-line armored private key.
while IFS= read -r line; do
  if [[ ${#line} -ge 16 && "$line" != -----* && "$line" != *:* ]]; then
    printf '::add-mask::%s\n' "$line"
  fi
done <<<"$RELEASE_PGP_PRIVATE_KEY"

gnupg_base=${RUNNER_TEMP:-$(dirname "$sums")}
gnupg_home="$gnupg_base/gnupg-release"
rm -rf "$gnupg_home"
(umask 077 && mkdir -p "$gnupg_home")
chmod 700 "$gnupg_home"
export GNUPGHOME="$gnupg_home"

cleanup() {
  gpgconf --kill gpg-agent >/dev/null 2>&1 || true
  rm -rf "$gnupg_home"
}
trap cleanup EXIT

gpg --batch --quiet --pinentry-mode loopback --passphrase-fd 3 --import \
  3<<<"$RELEASE_PGP_PASSPHRASE" <<<"$RELEASE_PGP_PRIVATE_KEY"

mapfile -t primary_fprs < <(
  gpg --batch --with-colons --list-secret-keys |
    awk -F: '$1 == "sec" { want = 1; next } want && $1 == "fpr" { print toupper($10); want = 0 }'
)
if [[ ${#primary_fprs[@]} -ne 1 ]]; then
  echo "::error::release-pgp-sign: expected 1 imported primary secret key, found ${#primary_fprs[@]}" >&2
  exit 1
fi
if [[ "${primary_fprs[0]}" != "$want_fpr" ]]; then
  echo "::error::release-pgp-sign: imported primary key fingerprint ${primary_fprs[0]} does not match RELEASE_PGP_FINGERPRINT $want_fpr" >&2
  exit 1
fi

asc="$sums.asc"
rm -f "$asc"
gpg --batch --yes --armor --detach-sign --digest-algo SHA256 \
  --local-user "$want_fpr" \
  --pinentry-mode loopback --passphrase-fd 3 \
  --output "$asc" "$sums" 3<<<"$RELEASE_PGP_PASSPHRASE"

[[ -s "$asc" ]] || { echo "::error::release-pgp-sign: $asc was not written" >&2; exit 1; }

verify_status=$(gpg --batch --status-fd 1 --verify "$asc" "$sums")
sig_primary=$(awk '$1 == "[GNUPG:]" && $2 == "VALIDSIG" { print toupper($12); exit }' <<<"$verify_status")
if [[ "$sig_primary" != "$want_fpr" ]]; then
  echo "::error::release-pgp-sign: signature verification failed (VALIDSIG primary '${sig_primary:-none}', expected '$want_fpr')" >&2
  exit 1
fi

echo "release-pgp-sign: signed and verified $asc with primary key $want_fpr"
