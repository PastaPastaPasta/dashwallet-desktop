#!/usr/bin/env bash
# Windows Authenticode signing step. Two key holders:
#   - a YubiKey PIV certificate via osslsigncode + PKCS#11, on the release signer's Mac (mirrors Dash
#     Core's contrib/windeploy/detached-sig-create.sh);
#   - a cloud-held key via jsign (--jsign-jar): any jsign store type. No workflow uses this today
#     (docs/release/CI-SIGNING.md, appendix).
#
# Signs dash-wallet-<version>-windows-x64-setup.exe, extracts the detached PEM signature into
# <sigs-dir>/win/dash-wallet-<version>-windows-x64-setup.exe.pem, and round-trip verifies that
# osslsigncode attach-signature on the unsigned CI .exe reproduces the exact signed .exe.
#
#   ci/release/sign-windows.sh \
#     --version <X.Y.Z> \
#     --unsigned-dir <dir> \
#     --sigs-dir <dir> \
#     --out-dir <dir> \
#     --pkcs11-module <path-to-libykcs11.dylib-or-opensc-pkcs11.so> \
#     --pkcs11-cert "<pkcs11:URI>" \
#     --pkcs11-key "<pkcs11:URI>" \
#     [--pkcs11-provider <path-to-pkcs11prov.dylib>] \
#     [--chain <intermediates.pem>] \
#     [--tsa <http://timestamp.digicert.com>]
#
#   ci/release/sign-windows.sh \
#     --version <X.Y.Z> --unsigned-dir <dir> --sigs-dir <dir> --out-dir <dir> \
#     --jsign-jar <jsign.jar> \
#     --jsign-storetype <TRUSTEDSIGNING|...> \
#     --jsign-keystore <endpoint-or-keystore> \
#     --jsign-alias <account/profile-or-key-alias> \
#     --jsign-storepass-env <NAME of the environment variable holding the token or password> \
#     [--ca-file <roots.pem>] \
#     [--tsa <http://timestamp.acs.microsoft.com>]
#
# --ca-file holds the roots the final `osslsigncode verify` trusts for the signer and timestamp chains;
# without it the verify uses osslsigncode's default bundle.
set -euo pipefail

version=
unsigned_dir=
sigs_dir=
out_dir=
pkcs11_module=
pkcs11_provider=
pkcs11_cert=
pkcs11_key=
chain_file=
tsa_url=http://timestamp.digicert.com
jsign_jar=
jsign_storetype=
jsign_keystore=
jsign_alias=
jsign_storepass_env=
ca_file=

while (($#)); do
  case "$1" in
    --version) version=${2:?}; shift 2 ;;
    --unsigned-dir) unsigned_dir=${2:?}; shift 2 ;;
    --sigs-dir) sigs_dir=${2:?}; shift 2 ;;
    --out-dir) out_dir=${2:?}; shift 2 ;;
    --pkcs11-module) pkcs11_module=${2:?}; shift 2 ;;
    --pkcs11-provider) pkcs11_provider=${2:?}; shift 2 ;;
    --pkcs11-cert) pkcs11_cert=${2:?}; shift 2 ;;
    --pkcs11-key) pkcs11_key=${2:?}; shift 2 ;;
    --chain) chain_file=${2:?}; shift 2 ;;
    --tsa) tsa_url=${2:?}; shift 2 ;;
    --jsign-jar) jsign_jar=${2:?}; shift 2 ;;
    --jsign-storetype) jsign_storetype=${2:?}; shift 2 ;;
    --jsign-keystore) jsign_keystore=${2:?}; shift 2 ;;
    --jsign-alias) jsign_alias=${2:?}; shift 2 ;;
    --jsign-storepass-env) jsign_storepass_env=${2:?}; shift 2 ;;
    --ca-file) ca_file=${2:?}; shift 2 ;;
    *) echo "sign-windows: unknown option: $1" >&2; exit 1 ;;
  esac
done

die() { echo "sign-windows: $*" >&2; exit 1; }

[[ -n "$version" && "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$ ]] || die "--version must be X.Y.Z or X.Y.Z-rc.N"
[[ -d "$unsigned_dir" ]] || die "--unsigned-dir $unsigned_dir not found"
[[ -n "$sigs_dir" && -n "$out_dir" ]] || die "--sigs-dir and --out-dir are required"
if [[ -n "$jsign_jar" ]]; then
  [[ -z "$pkcs11_module$pkcs11_cert$pkcs11_key$pkcs11_provider$chain_file" ]] ||
    die "--jsign-jar and the --pkcs11-* / --chain options are two different key holders; pass one set"
  [[ -f "$jsign_jar" && ! -L "$jsign_jar" ]] || die "--jsign-jar not found: $jsign_jar"
  [[ -n "$jsign_storetype" && -n "$jsign_keystore" && -n "$jsign_alias" ]] ||
    die "--jsign-storetype, --jsign-keystore and --jsign-alias are required with --jsign-jar"
  [[ "$jsign_storepass_env" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]] ||
    die "--jsign-storepass-env must name an environment variable (the value never goes on a command line)"
  [[ -n "${!jsign_storepass_env:-}" ]] || die "environment variable $jsign_storepass_env is empty or unset"
  command -v java >/dev/null 2>&1 || die "java not found (jsign needs a Java runtime)"
else
  [[ -z "$jsign_storetype$jsign_keystore$jsign_alias$jsign_storepass_env" ]] ||
    die "--jsign-storetype, --jsign-keystore, --jsign-alias and --jsign-storepass-env need --jsign-jar"
  [[ -n "$pkcs11_module" && -f "$pkcs11_module" ]] || die "--pkcs11-module not found: ${pkcs11_module:-<unset>}"
  [[ -n "$pkcs11_cert" && -n "$pkcs11_key" ]] || die "--pkcs11-cert and --pkcs11-key PKCS#11 URIs are required"
fi
if [[ -n "$ca_file" ]]; then
  [[ -f "$ca_file" && ! -L "$ca_file" ]] || die "--ca-file not found or is a symlink: $ca_file"
fi
command -v osslsigncode >/dev/null 2>&1 || die "osslsigncode not found (brew install osslsigncode libp11 yubico-piv-tool)"

exe_name="dash-wallet-$version-windows-x64-setup.exe"
unsigned_exe="$unsigned_dir/$exe_name"
[[ -f "$unsigned_exe" && ! -L "$unsigned_exe" ]] || die "missing or symlink $unsigned_exe"
unsigned_sums="$unsigned_dir/unsigned.SHA256SUMS"
[[ -f "$unsigned_sums" && ! -L "$unsigned_sums" ]] || die "missing $unsigned_sums (run ci/release/fetch-unsigned.sh first)"
if [[ -f "$sigs_dir/unsigned.SHA256SUMS" ]]; then
  cmp -s "$unsigned_sums" "$sigs_dir/unsigned.SHA256SUMS" ||
    die "$unsigned_sums differs from $sigs_dir/unsigned.SHA256SUMS"
fi
# L3: Re-verify unsigned .exe hash against unsigned.SHA256SUMS right before invoking YubiKey signing.
want_exe_sha=$(awk -v f="$exe_name" '$2 == f { print $1; exit }' "$unsigned_sums")
[[ "$want_exe_sha" =~ ^[0-9a-f]{64}$ ]] || die "$exe_name not listed in $unsigned_sums"
if command -v sha256sum >/dev/null 2>&1; then
  got_exe_sha=$(sha256sum "$unsigned_exe" | awk '{print $1}')
else
  got_exe_sha=$(shasum -a 256 "$unsigned_exe" | awk '{print $1}')
fi
[[ "$got_exe_sha" == "$want_exe_sha" ]] ||
  die "unsigned input $exe_name changed since fetch (got $got_exe_sha != expected $want_exe_sha in $unsigned_sums)"

# Auto-detect Homebrew libp11 OpenSSL provider on macOS when not passed explicitly.
if [[ -z "$jsign_jar" && -z "$pkcs11_provider" ]] && command -v brew >/dev/null 2>&1; then
  brew_libp11=$(brew --prefix libp11 2>/dev/null || true)
  for cand in "$brew_libp11/lib/ossl-modules/pkcs11prov.dylib" "$brew_libp11/lib/ossl-modules/pkcs11prov.so"; do
    if [[ -f "$cand" ]]; then
      pkcs11_provider=$cand
      break
    fi
  done
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

mkdir -p "$sigs_dir/win" "$out_dir/dist"
signed_direct="$work/$exe_name"
sig_pem="$sigs_dir/win/$exe_name.pem"
signed_out="$out_dir/dist/$exe_name"

app_name="DashPay Desktop"
app_url="https://github.com/PastaPastaPasta/dashwallet-desktop"

if [[ -n "$jsign_jar" ]]; then
  # jsign signs in place. The token or password is read by jsign from the named environment variable.
  cp "$unsigned_exe" "$signed_direct"
  echo "sign-windows: signing $exe_name with jsign ($jsign_storetype, $jsign_alias)"
  java -jar "$jsign_jar" \
    --storetype "$jsign_storetype" \
    --keystore "$jsign_keystore" \
    --alias "$jsign_alias" \
    --storepass "env:$jsign_storepass_env" \
    --alg SHA-256 \
    --name "$app_name" \
    --url "$app_url" \
    --tsaurl "$tsa_url" \
    --tsmode RFC3161 \
    "$signed_direct"
else
  sign_args=(
    sign
    -pkcs11module "$pkcs11_module"
    -pkcs11cert "$pkcs11_cert"
    -key "$pkcs11_key"
    -h sha256
    -n "$app_name"
    -i "$app_url"
    -ts "$tsa_url"
    -askpass
    -in "$unsigned_exe"
    -out "$signed_direct"
  )
  if [[ -n "$pkcs11_provider" ]]; then
    sign_args=(sign -provider "$pkcs11_provider" "${sign_args[@]:1}")
  fi
  if [[ -n "$chain_file" ]]; then
    [[ -f "$chain_file" ]] || die "--chain file not found: $chain_file"
    sign_args+=(-ac "$chain_file")
  fi

  echo "sign-windows: signing $exe_name with PKCS#11 token (enter YubiKey PIV PIN when prompted)"
  osslsigncode "${sign_args[@]}"
fi

# Extracted, re-attached and verified in the working directory: --sigs-dir and --out-dir get a signature only
# once it has passed every check below, so a failed run leaves nothing there to push.
sig_staged="$work/detached.pem"
attached_staged="$work/attached.exe"
echo "sign-windows: extracting the detached PEM signature"
osslsigncode extract-signature -pem -in "$signed_direct" -out "$sig_staged"

echo "sign-windows: round-trip verifying attach-signature and verify"
attach_args=(attach-signature -in "$unsigned_exe" -sigin "$sig_staged" -out "$attached_staged")
verify_args=(verify -in "$attached_staged")
if [[ -n "$ca_file" ]]; then
  # osslsigncode <= 2.13 verifies the chain inside attach-signature as well (ci/github/release-attach.sh).
  attach_help=$(osslsigncode attach-signature --help 2>&1 || true)
  [[ "$attach_help" != *"-CAfile"* ]] || attach_args+=(-CAfile "$ca_file")
  [[ "$attach_help" != *"-TSA-CAfile"* ]] || attach_args+=(-TSA-CAfile "$ca_file")
  verify_args+=(-CAfile "$ca_file" -TSA-CAfile "$ca_file")
fi
osslsigncode "${attach_args[@]}"
cmp -s "$signed_direct" "$attached_staged" || die "round-trip attached .exe differed from directly signed .exe"
# The publish side refuses a signature without a verified timestamp (ci/github/windows-verify-pinned.sh, the
# same test as here); osslsigncode alone does not. Find that out here, not after the signature is pushed.
osslsigncode "${verify_args[@]}" | tee "$work/verify.out"
if ! grep -qx 'Timestamp Server Signature verification: ok' "$work/verify.out" ||
   grep -qE '^Timestamp (is not available|Server Signature verification: failed|Server Signature verification is disabled)' "$work/verify.out"; then
  die "the signature carries no verified timestamp (the timestamp server $tsa_url, or its revocation list, was unreachable, or its root is not trusted here); sign again"
fi
cp "$sig_staged" "$sig_pem"
cp "$attached_staged" "$signed_out"

echo "sign-windows: done"
echo "  detached sig: $sig_pem"
echo "  signed .exe:  $signed_out"
