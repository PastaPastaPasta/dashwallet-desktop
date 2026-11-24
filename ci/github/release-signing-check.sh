#!/usr/bin/env bash
# GitHub Actions (release.yml): validate signing credentials early for macos, windows, or pgp.
# Appends MACOS_SIGN=0|1, WINDOWS_SIGN=0|1, or PGP_SIGN=0|1 to $GITHUB_ENV when set.
#
#   ci/github/release-signing-check.sh <macos|windows|pgp>
#
# Never echoes any secret value; error and warning messages name only missing secret/variable names.
#
# Behaviour:
#   - When DW_SIGNING_ENABLED is not 'true' (main, PRs, schedule, dispatch, dry-run tags, or -rc.N
#     without DW_SIGN_CANDIDATES=1): sets <TARGET>_SIGN=0 and exits 0 quietly.
#   - When DW_SIGNING_ENABLED is 'true':
#       * All secrets and variables in the set are present: sets <TARGET>_SIGN=1 and exits 0.
#       * Some secrets are present but one or more secrets/variables are missing: fails early with
#         only the missing names (never ships a half-configured build).
#       * No secrets are present:
#           - On a final vX.Y.Z tag push with DW_SIGNING_REQUIRED=1: fails early naming the missing
#             secrets/variables.
#           - On a final vX.Y.Z tag push without DW_SIGNING_REQUIRED=1: warns once and sets 0.
#           - On a candidate (-rc.N, with DW_SIGN_CANDIDATES=1): warns and sets 0; candidates never
#             require signing even when DW_SIGNING_REQUIRED=1.
set -euo pipefail

target=${1:?usage: $0 macos|windows|pgp}

case "$target" in
  macos)
    env_flag=MACOS_SIGN
    secrets=(
      APPLE_CERTIFICATE
      APPLE_CERTIFICATE_PASSWORD
      APPLE_SIGNING_IDENTITY
      APPLE_API_ISSUER
      APPLE_API_KEY
      APPLE_API_KEY_P8
    )
    vars=(
      APPLE_TEAM_ID
    )
    ;;
  windows)
    env_flag=WINDOWS_SIGN
    secrets=(
      AZURE_TENANT_ID
      AZURE_CLIENT_ID
      AZURE_CLIENT_SECRET
    )
    vars=(
      AZURE_SIGNING_ENDPOINT
      AZURE_SIGNING_ACCOUNT
      AZURE_SIGNING_PROFILE
    )
    ;;
  pgp)
    env_flag=PGP_SIGN
    secrets=(
      RELEASE_PGP_PRIVATE_KEY
      RELEASE_PGP_PASSPHRASE
    )
    vars=(
      RELEASE_PGP_FINGERPRINT
    )
    ;;
  *)
    echo "::error::release-signing-check: unknown target '$target' (expected macos, windows, or pgp)" >&2
    exit 1
    ;;
esac

set_flag() {
  [[ -z "${GITHUB_ENV:-}" ]] || echo "$env_flag=$1" >> "$GITHUB_ENV"
}

event=${GITHUB_EVENT_NAME:-}
ref=${GITHUB_REF:-}
is_final=0
if [[ "$event" == push && "$ref" =~ ^refs/tags/v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  is_final=1
fi

# When release.yml calls this script, DW_SIGNING_ENABLED comes from needs.decide.outputs.signing.
# If left unset in a standalone invocation, default to enabled on a final tag push only.
signing_enabled=${DW_SIGNING_ENABLED:-}
if [[ -z "$signing_enabled" ]]; then
  if ((is_final)); then signing_enabled=true; else signing_enabled=false; fi
fi

if [[ "$signing_enabled" != true ]]; then
  set_flag 0
  echo "release-signing-check ($target): signing disabled for this run; continuing unsigned"
  exit 0
fi

present_secrets=0
missing_secrets=()
for name in "${secrets[@]}"; do
  if [[ -n "${!name:-}" ]]; then
    present_secrets=$((present_secrets + 1))
  else
    missing_secrets+=("$name")
  fi
done

missing_vars=()
for name in "${vars[@]}"; do
  if [[ -z "${!name:-}" ]]; then
    missing_vars+=("$name")
  fi
done

missing_all=("${missing_secrets[@]}" "${missing_vars[@]}")

if ((${#missing_all[@]} == 0)); then
  set_flag 1
  echo "release-signing-check ($target): all signing credentials present; signing enabled"
  exit 0
fi

missing_list=$(IFS=', '; echo "${missing_all[*]}")
tag_label=${ref#refs/tags/}

if ((present_secrets > 0)); then
  echo "::error::release-signing-check ($target): incomplete signing configuration on ${tag_label:-this run}; missing: $missing_list" >&2
  exit 1
fi

if ((is_final)) && [[ "${DW_SIGNING_REQUIRED:-}" == 1 ]]; then
  echo "::error::release-signing-check ($target): DW_SIGNING_REQUIRED=1 on $tag_label, but signing credentials are missing: $missing_list" >&2
  exit 1
fi

if ((is_final)); then
  echo "::warning::release-signing-check ($target): signing credentials missing on $tag_label ($missing_list); this release will be unsigned for $target."
else
  echo "::warning::release-signing-check ($target): signing enabled on candidate ${tag_label:-this run}, but signing credentials are missing ($missing_list); continuing unsigned (candidates never require signing)."
fi
set_flag 0
