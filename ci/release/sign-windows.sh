#!/usr/bin/env bash
# Local Windows Authenticode signing step (runs on the release signer's Mac using a YubiKey PIV
# certificate via osslsigncode + PKCS#11; mirrors Dash Core's contrib/windeploy/detached-sig-create.sh).
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
    *) echo "sign-windows: unknown option: $1" >&2; exit 1 ;;
  esac
done

die() { echo "sign-windows: $*" >&2; exit 1; }

[[ -n "$version" && "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "--version must be X.Y.Z"
[[ -d "$unsigned_dir" ]] || die "--unsigned-dir $unsigned_dir not found"
[[ -n "$sigs_dir" && -n "$out_dir" ]] || die "--sigs-dir and --out-dir are required"
[[ -n "$pkcs11_module" && -f "$pkcs11_module" ]] || die "--pkcs11-module not found: ${pkcs11_module:-<unset>}"
[[ -n "$pkcs11_cert" && -n "$pkcs11_key" ]] || die "--pkcs11-cert and --pkcs11-key PKCS#11 URIs are required"
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
if [[ -z "$pkcs11_provider" ]] && command -v brew >/dev/null 2>&1; then
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

sign_args=(
  sign
  -pkcs11module "$pkcs11_module"
  -pkcs11cert "$pkcs11_cert"
  -key "$pkcs11_key"
  -h sha256
  -n "Dash Wallet"
  -i "https://github.com/PastaPastaPasta/dashwallet-desktop"
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

echo "sign-windows: extracting detached PEM signature to $sig_pem"
osslsigncode extract-signature -pem -in "$signed_direct" -out "$sig_pem"

echo "sign-windows: round-trip verifying attach-signature and verify"
rm -f "$signed_out"
osslsigncode attach-signature -in "$unsigned_exe" -sigin "$sig_pem" -out "$signed_out"
cmp -s "$signed_direct" "$signed_out" || die "round-trip attached .exe differed from directly signed .exe"
osslsigncode verify -in "$signed_out"

echo "sign-windows: done"
echo "  detached sig: $sig_pem"
echo "  signed .exe:  $signed_out"
