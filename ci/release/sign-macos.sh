#!/usr/bin/env bash
# Local macOS signing step (runs on the release signer's Mac; mirrors Dash Core's detached-sig-create.sh).
#
# Signs Dash Wallet.app with Developer ID Application from the local Keychain, notarizes and staples
# it with Apple, extracts the small detached signature payload into <sigs-dir>/osx/Dash Wallet.app,
# round-trip verifies re-attachment onto the unsigned CI .app, builds the deterministic updater archive
# (.app.tar.gz), and builds + signs + notarizes + staples the final .dmg.
#
#   ci/release/sign-macos.sh \
#     --version <X.Y.Z> \
#     --unsigned-dir <dir> \
#     --sigs-dir <dir> \
#     --out-dir <dir> \
#     --identity "Developer ID Application: ..." \
#     (--notary-profile <keychain-profile> | --api-key-path <p8> --api-key-id <id> --api-issuer <uuid>)
set -euo pipefail
umask 022

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
gh_dir=$(cd "$here/../github" && pwd)

version=
unsigned_dir=
sigs_dir=
out_dir=
identity=
notary_profile=
api_key_path=
api_key_id=
api_issuer=

while (($#)); do
  case "$1" in
    --version) version=${2:?}; shift 2 ;;
    --unsigned-dir) unsigned_dir=${2:?}; shift 2 ;;
    --sigs-dir) sigs_dir=${2:?}; shift 2 ;;
    --out-dir) out_dir=${2:?}; shift 2 ;;
    --identity) identity=${2:?}; shift 2 ;;
    --notary-profile) notary_profile=${2:?}; shift 2 ;;
    --api-key-path) api_key_path=${2:?}; shift 2 ;;
    --api-key-id) api_key_id=${2:?}; shift 2 ;;
    --api-issuer) api_issuer=${2:?}; shift 2 ;;
    *) echo "sign-macos: unknown option: $1" >&2; exit 1 ;;
  esac
done

die() { echo "sign-macos: $*" >&2; exit 1; }

[[ "$(uname -s)" == Darwin ]] || die "must run on macOS"
[[ -n "$version" && "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "--version must be X.Y.Z"
[[ -d "$unsigned_dir" ]] || die "--unsigned-dir $unsigned_dir not found"
[[ -n "$sigs_dir" && -n "$out_dir" && -n "$identity" ]] || die "--sigs-dir, --out-dir, and --identity are required"

notary_auth=()
if [[ -n "$notary_profile" ]]; then
  notary_auth=(--keychain-profile "$notary_profile")
elif [[ -n "$api_key_path" && -n "$api_key_id" && -n "$api_issuer" ]]; then
  notary_auth=(--key "$api_key_path" --key-id "$api_key_id" --issuer "$api_issuer")
else
  die "provide either --notary-profile <profile> or --api-key-path + --api-key-id + --api-issuer"
fi

unsigned_tar="$unsigned_dir/dash-wallet-$version-macos-unsigned.tar.gz"
[[ -f "$unsigned_tar" && ! -L "$unsigned_tar" ]] || die "missing or symlink $unsigned_tar"
unsigned_sums="$unsigned_dir/unsigned.SHA256SUMS"
[[ -f "$unsigned_sums" && ! -L "$unsigned_sums" ]] || die "missing $unsigned_sums (run ci/release/fetch-unsigned.sh first)"
if [[ -f "$sigs_dir/unsigned.SHA256SUMS" ]]; then
  cmp -s "$unsigned_sums" "$sigs_dir/unsigned.SHA256SUMS" ||
    die "$unsigned_sums differs from $sigs_dir/unsigned.SHA256SUMS"
fi
# L3: Re-verify unsigned input hash against unsigned.SHA256SUMS right before invoking Apple signing.
want_tar_sha=$(awk -v f="${unsigned_tar##*/}" '$2 == f { print $1; exit }' "$unsigned_sums")
[[ "$want_tar_sha" =~ ^[0-9a-f]{64}$ ]] || die "${unsigned_tar##*/} not listed in $unsigned_sums"
if command -v sha256sum >/dev/null 2>&1; then
  got_tar_sha=$(sha256sum "$unsigned_tar" | awk '{print $1}')
else
  got_tar_sha=$(shasum -a 256 "$unsigned_tar" | awk '{print $1}')
fi
[[ "$got_tar_sha" == "$want_tar_sha" ]] ||
  die "unsigned input ${unsigned_tar##*/} changed since fetch (got $got_tar_sha != expected $want_tar_sha in $unsigned_sums)"

notarize_and_staple() {
  local target=$1 submit_file=$2 label=$3
  local out rc=0 status sub_id
  out=$(mktemp)
  xcrun notarytool submit "$submit_file" \
    "${notary_auth[@]}" \
    --wait --timeout 60m \
    --output-format json | tee "$out" || rc=$?
  status=$(jq -r '.status // empty' "$out" 2>/dev/null || true)
  sub_id=$(jq -r '.id // empty' "$out" 2>/dev/null || true)
  rm -f "$out"
  if [[ $rc -ne 0 || "$status" != Accepted ]]; then
    if [[ -n "$sub_id" ]]; then
      xcrun notarytool log "$sub_id" "${notary_auth[@]}" || true
    fi
    die "notarization of $label failed (exit=$rc, status=${status:-unknown}, id=${sub_id:-unknown})"
  fi
  xcrun stapler staple "$target"
  xcrun stapler validate "$target"
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

mkdir -p "$work/unsigned" "$work/signed" "$work/roundtrip" "$out_dir/dist" "$out_dir/feed" "$sigs_dir/osx"
tar -xzf "$unsigned_tar" -C "$work/unsigned"
[[ -d "$work/unsigned/Dash Wallet.app" ]] || die "$unsigned_tar did not contain Dash Wallet.app"
cp -R "$work/unsigned/Dash Wallet.app" "$work/signed/Dash Wallet.app"

app="$work/signed/Dash Wallet.app"
xattr -crs "$app"

echo "sign-macos: codesigning Dash Wallet.app with $identity"
codesign --force --options runtime --timestamp --sign "$identity" "$app/Contents/MacOS/dash-wallet"
codesign --force --options runtime --timestamp --sign "$identity" "$app"
codesign --verify --deep --strict --verbose=2 "$app"

echo "sign-macos: notarizing and stapling Dash Wallet.app"
ditto -c -k --keepParent "$app" "$work/DashWallet-app.zip"
notarize_and_staple "$app" "$work/DashWallet-app.zip" "Dash Wallet.app"
spctl --assess --type execute -vv "$app"

echo "sign-macos: extracting detached signatures into $sigs_dir/osx/Dash Wallet.app"
python3 "$gh_dir/macos-detached-sig.py" extract "$work/unsigned/Dash Wallet.app" "$app" "$sigs_dir/osx/Dash Wallet.app"

echo "sign-macos: round-trip verifying detached signature attachment"
python3 "$gh_dir/macos-detached-sig.py" apply "$work/unsigned/Dash Wallet.app" "$sigs_dir/osx/Dash Wallet.app" "$work/roundtrip/Dash Wallet.app"
diff -qr "$app" "$work/roundtrip/Dash Wallet.app" || die "round-trip attached Dash Wallet.app differed from signed .app"
codesign --verify --deep --strict --verbose=2 "$work/roundtrip/Dash Wallet.app"
spctl --assess --type execute -vv "$work/roundtrip/Dash Wallet.app"
xcrun stapler validate "$work/roundtrip/Dash Wallet.app"

app_tar="$out_dir/feed/dash-wallet-$version-macos-universal.app.tar.gz"
python3 "$gh_dir/macos-detached-sig.py" pack-app "$work/roundtrip/Dash Wallet.app" "$app_tar"

echo "sign-macos: creating, signing, notarizing, and stapling the .dmg"
dmg_src="$work/dmg-src"
mkdir -p "$dmg_src"
cp -R "$work/roundtrip/Dash Wallet.app" "$dmg_src/Dash Wallet.app"
ln -s /Applications "$dmg_src/Applications"
dmg="$out_dir/dist/dash-wallet-$version-macos-universal.dmg"
rm -f "$dmg"
hdiutil create -volname "Dash Wallet" -srcfolder "$dmg_src" -ov -format UDZO "$dmg"
codesign --force --timestamp --sign "$identity" "$dmg"
codesign --verify --verbose=2 "$dmg"
notarize_and_staple "$dmg" "$dmg" "$(basename "$dmg")"

echo "sign-macos: done"
echo "  detached sigs: $sigs_dir/osx/Dash Wallet.app"
echo "  signed .dmg:   $dmg"
echo "  updater tar:   $app_tar"
