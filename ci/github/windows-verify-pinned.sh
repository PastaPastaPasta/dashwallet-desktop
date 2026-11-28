#!/usr/bin/env bash
# Verify a signed Windows installer against the pinned signer. release-attach.sh runs it before a release
# is published, and release-sign.yml before SHA256SUMS is PGP-signed, so both ask the same question.
#
#   ci/github/windows-verify-pinned.sh --exe <signed.exe> \
#     (--leaf-hash <sha256:64-hex> | --signer-org <organization> --signer-eku <OID> [--expect-leaf <sha256:64-hex>]) \
#     [--ca-file <roots.pem>] [--tsa-ca-file <roots.pem>] [--skip-chain-verify]
#
# The pinned signer must have made the installer's primary signature, the one Windows shows, and the installer
# must carry no other: osslsigncode alone is satisfied by a pinned signature nested under someone else's.
#
# The signature must also carry a timestamp that verifies. Without one the installer's signature stops
# verifying when the certificate expires, and osslsigncode accepts a signature with no timestamp, or with one it
# could not verify, while the certificate is still current. A timestamp is not covered by the signature, so it
# can be cut out of a detached signature after the signer made it.
#
# --leaf-hash    the signer is that one long-lived certificate. The timestamp is verified under osslsigncode's
#                default roots, or under --tsa-ca-file.
# --signer-org   the signer names that organization and carries that EKU (windows-signer-leaf.py) and chains to
#                --ca-file (required), which is also the only trust for the timestamp. The EKU must be one
#                identity's own (1.3.6.1.4.1.311.97.<a>.<b>.<c>.<d>).
# --skip-chain-verify   test suites only: no timestamp, no CRL.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

exe=
leaf_hash=
signer_org=
signer_eku=
expect_leaf=
ca_file=
tsa_ca_file=
skip_chain_verify=0

while (($#)); do
  case "$1" in
    --exe) exe=${2:?}; shift 2 ;;
    --leaf-hash) leaf_hash=${2:?}; shift 2 ;;
    --signer-org) signer_org=${2:?}; shift 2 ;;
    --signer-eku) signer_eku=${2:?}; shift 2 ;;
    --expect-leaf) expect_leaf=${2:?}; shift 2 ;;
    --ca-file) ca_file=${2:?}; shift 2 ;;
    --tsa-ca-file) tsa_ca_file=${2:?}; shift 2 ;;
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

if [[ -n "$tsa_ca_file" ]]; then
  [[ -f "$tsa_ca_file" && ! -L "$tsa_ca_file" ]] || die "--tsa-ca-file not found or is a symlink: $tsa_ca_file"
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
verify_help=$(osslsigncode verify --help 2>&1 || true)

if [[ -n "$signer_org$signer_eku" ]]; then
  [[ -z "$leaf_hash" ]] || die "pass --leaf-hash or --signer-org with --signer-eku, not both"
  [[ -n "$signer_org" && -n "$signer_eku" ]] || die "--signer-org and --signer-eku go together"
  [[ "$signer_org" != *REPLACE_* && "$signer_org" != *$'\n'* ]] || die "--signer-org is a placeholder or spans lines"
  [[ "$signer_eku" =~ ^1\.3\.6\.1\.4\.1\.311\.97\.[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
    die "--signer-eku must be the identity's own EKU, 1.3.6.1.4.1.311.97.<a>.<b>.<c>.<d> (got '$signer_eku')"
  [[ -n "$ca_file" ]] || die "--signer-org needs --ca-file: an organization and an EKU identify the signer only under a pinned root"
  [[ -z "$tsa_ca_file" ]] || die "--tsa-ca-file goes with --leaf-hash: under --signer-org the timestamp's roots are --ca-file's"
  tsa_ca_file=$ca_file
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

[[ "$verify_help" == *"-index"* ]] || die "this osslsigncode cannot verify one chosen signature (-index)"
verify_args=(verify -in "$exe" -index 0 -require-leaf-hash "$leaf_hash")
[[ -z "$ca_file" ]] || verify_args+=(-CAfile "$ca_file")
if [[ -n "$tsa_ca_file" ]]; then
  [[ "$verify_help" == *"-TSA-CAfile"* ]] || die "this osslsigncode cannot pin the timestamp root (-TSA-CAfile)"
  verify_args+=(-TSA-CAfile "$tsa_ca_file")
fi
if ((skip_chain_verify)); then
  verify_args+=(-ignore-timestamp)
  # -ignore-crl was added in osslsigncode 2.9 (Ubuntu 24.04 ships 2.8-2).
  [[ "$verify_help" != *"-ignore-crl"* ]] || verify_args+=(-ignore-crl)
fi

# The status lines read below are all on stdout; stderr is kept apart so nothing can be interleaved into them.
rc=0
osslsigncode "${verify_args[@]}" > "$work/verify.out" 2> "$work/verify.err" || rc=$?
cat "$work/verify.out"
cat "$work/verify.err" >&2
((rc == 0)) || die "osslsigncode verify failed for $exe (exit $rc)"

if grep -qE '^Warning: signature verification at index [0-9]+ was skipped' "$work/verify.out"; then
  die "$exe carries more than one signature; a release installer carries the pinned signer's only"
fi
if ((!skip_chain_verify)); then
  if ! grep -qx 'Timestamp Server Signature verification: ok' "$work/verify.out" ||
     grep -qE '^Timestamp (is not available|Server Signature verification: failed|Server Signature verification is disabled)' "$work/verify.out"; then
    # One refusal, three causes: say which, because only the first needs a new signature.
    if grep -qx 'Timestamp is not available' "$work/verify.out"; then
      why="it carries no timestamp: sign it again"
    elif grep -qx 'Timestamp Server Signature CRL verification: failed' "$work/verify.out" || grep -q 'CRL' "$work/verify.err"; then
      why="the timestamp authority's revocation check failed: if its list could not be fetched, run this again"
    else
      why="its timestamp does not verify under ${tsa_ca_file:-the default roots of osslsigncode}"
    fi
    die "the signature of $exe carries no verified timestamp, so it would stop verifying when its certificate expires ($why)"
  fi
fi
echo "windows-verify-pinned: $exe is signed by the pinned signer (leaf $leaf_hash)"
