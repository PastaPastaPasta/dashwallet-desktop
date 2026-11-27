#!/usr/bin/env bash
# Verify a signed Windows installer against the pinned signer. release-attach.sh runs it before a release
# is published, and release-sign.yml before SHA256SUMS is PGP-signed, so both ask the same question.
#
#   ci/github/windows-verify-pinned.sh --exe <signed.exe> \
#     (--leaf-hash <sha256:64-hex> | --signer-org <organization> --signer-eku <OID> [--expect-leaf <sha256:64-hex>]) \
#     [--ca-file <roots.pem>] [--skip-chain-verify]
#
# --leaf-hash    the signer is that one long-lived certificate.
# --signer-org   the signer names that organization and carries that EKU (windows-signer-leaf.py), chains to
#                --ca-file (required), and its signature carries an RFC 3161 timestamp that verifies under
#                --ca-file too. Such a certificate lives for days: without the timestamp the installer's
#                signature would stop verifying when it expires, and osslsigncode accepts a signature with no
#                timestamp, or with one it could not verify, while the certificate is still current.
# --skip-chain-verify   test suites only: no timestamp, no CRL.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

exe=
leaf_hash=
signer_org=
signer_eku=
expect_leaf=
ca_file=
skip_chain_verify=0

while (($#)); do
  case "$1" in
    --exe) exe=${2:?}; shift 2 ;;
    --leaf-hash) leaf_hash=${2:?}; shift 2 ;;
    --signer-org) signer_org=${2:?}; shift 2 ;;
    --signer-eku) signer_eku=${2:?}; shift 2 ;;
    --expect-leaf) expect_leaf=${2:?}; shift 2 ;;
    --ca-file) ca_file=${2:?}; shift 2 ;;
    --skip-chain-verify) skip_chain_verify=1; shift ;;
    *) echo "windows-verify-pinned: unknown option: $1" >&2; exit 1 ;;
  esac
done

die() { echo "windows-verify-pinned: $*" >&2; exit 1; }

[[ -n "$exe" && -f "$exe" && ! -L "$exe" ]] || die "--exe not found or is a symlink: ${exe:-<unset>}"
command -v osslsigncode >/dev/null 2>&1 || die "osslsigncode is required"
if [[ -n "$ca_file" ]]; then
  [[ -f "$ca_file" && ! -L "$ca_file" ]] || die "--ca-file not found or is a symlink: $ca_file"
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
verify_help=$(osslsigncode verify --help 2>&1 || true)

signer=0
if [[ -n "$signer_org$signer_eku" ]]; then
  signer=1
  [[ -z "$leaf_hash" ]] || die "pass --leaf-hash or --signer-org with --signer-eku, not both"
  [[ -n "$signer_org" && -n "$signer_eku" ]] || die "--signer-org and --signer-eku go together"
  [[ -n "$ca_file" ]] || die "--signer-org needs --ca-file: an organization and an EKU identify the signer only under a pinned root"
  [[ "$verify_help" == *"-TSA-CAfile"* ]] || die "this osslsigncode cannot pin the timestamp root (-TSA-CAfile)"
  osslsigncode extract-signature -pem -in "$exe" -out "$work/sig.pem" >/dev/null ||
    die "$exe carries no signature to read"
  leaf_hash=$(python3 "$here/windows-signer-leaf.py" "$work/sig.pem" "$signer_org" "$signer_eku") ||
    die "no certificate in the signature of $exe matches the pinned windows_signer ($signer_org, EKU $signer_eku)"
  [[ -z "$expect_leaf" || "$leaf_hash" == "$expect_leaf" ]] ||
    die "the attached signature's leaf $leaf_hash is not the detached signature's $expect_leaf"
else
  [[ -z "$expect_leaf" ]] || die "--expect-leaf goes with --signer-org"
fi
[[ "$leaf_hash" =~ ^sha256:[0-9a-f]{64}$ ]] || die "the leaf hash must be sha256:<64 lower-case hex> (got '${leaf_hash:-<empty>}')"

verify_args=(verify -in "$exe" -require-leaf-hash "$leaf_hash")
[[ -z "$ca_file" ]] || verify_args+=(-CAfile "$ca_file")
((!signer)) || verify_args+=(-TSA-CAfile "$ca_file")
if ((skip_chain_verify)); then
  verify_args+=(-ignore-timestamp)
  # -ignore-crl was added in osslsigncode 2.9 (Ubuntu 24.04 ships 2.8-2).
  [[ "$verify_help" != *"-ignore-crl"* ]] || verify_args+=(-ignore-crl)
fi

rc=0
osslsigncode "${verify_args[@]}" > "$work/verify.log" 2>&1 || rc=$?
cat "$work/verify.log"
((rc == 0)) || die "osslsigncode verify failed for $exe (exit $rc)"

if ((signer && !skip_chain_verify)); then
  if ! grep -qx 'Timestamp Server Signature verification: ok' "$work/verify.log" ||
     grep -qE '^Timestamp (is not available|Server Signature verification: failed|Server Signature verification is disabled)' "$work/verify.log"; then
    die "the signature of $exe carries no verified RFC 3161 timestamp; its certificate expires within days and the signature with it"
  fi
fi
echo "windows-verify-pinned: $exe is signed by the pinned signer (leaf $leaf_hash)"
