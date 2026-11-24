#!/usr/bin/env bash
# Attach and verify detached release signatures against an exact unsigned CI build, producing
# the final signed release directory ready for ci/github/release-publish.sh.
#
# Runs with ZERO secrets on either macOS or Linux.
#
#   ci/github/release-attach.sh \
#     --version <X.Y.Z> \
#     --unsigned-dir <dir> \
#     --sigs-dir <dir> \
#     --out-dir <dir> \
#     [--signed-dmg <path>] \
#     [--repo PastaPastaPasta/dashwallet-desktop] \
#     [--tauri-conf rust/crates/dw-app/tauri.conf.json] \
#     [--pgp-pubkey <path>] \
#     [--pgp-fingerprint <40-hex>] \
#     [--win-ca-file <path>] \
#     [--win-leaf-hash <sha256:HEX>] \
#     [--skip-win-chain-verify]
#
# Layout expected in --sigs-dir (mirroring dashpay/dash-detached-sigs):
#   unsigned.SHA256SUMS                                    # binds sigs to the exact unsigned CI build
#   win/dash-wallet-<ver>-windows-x64-setup.exe.pem        # osslsigncode detached PEM signature
#   osx/Dash Wallet.app/manifest.json (+ .cs, CodeResources) # macos-detached-sig.py payload
#   updater/latest.json                                    # signed with local stable updater key
#   SHA256SUMS                                             # final hashes of all 9 release files
#   SHA256SUMS.asc                                         # detached armored PGP signature over SHA256SUMS
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd "$here/../.." && pwd)

version=
unsigned_dir=
sigs_dir=
out_dir=
signed_dmg=
repo=PastaPastaPasta/dashwallet-desktop
tauri_conf="$repo_root/rust/crates/dw-app/tauri.conf.json"
pgp_pubkey=
pgp_fingerprint=
win_ca_file=
win_leaf_hash=
skip_win_chain_verify=0

while (($#)); do
  case "$1" in
    --version) version=${2:?}; shift 2 ;;
    --unsigned-dir) unsigned_dir=${2:?}; shift 2 ;;
    --sigs-dir) sigs_dir=${2:?}; shift 2 ;;
    --out-dir) out_dir=${2:?}; shift 2 ;;
    --signed-dmg) signed_dmg=${2:?}; shift 2 ;;
    --repo) repo=${2:?}; shift 2 ;;
    --tauri-conf) tauri_conf=${2:?}; shift 2 ;;
    --pgp-pubkey) pgp_pubkey=${2:?}; shift 2 ;;
    --pgp-fingerprint) pgp_fingerprint=${2:?}; shift 2 ;;
    --win-ca-file) win_ca_file=${2:?}; shift 2 ;;
    --win-leaf-hash) win_leaf_hash=${2:?}; shift 2 ;;
    --skip-win-chain-verify) skip_win_chain_verify=1; shift ;;
    *) echo "release-attach: unknown option: $1" >&2; exit 1 ;;
  esac
done

die() { echo "release-attach: $*" >&2; exit 1; }

[[ -n "$version" && "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "--version must be a stable X.Y.Z"
[[ -d "$unsigned_dir" ]] || die "--unsigned-dir $unsigned_dir does not exist"
[[ -d "$sigs_dir" ]] || die "--sigs-dir $sigs_dir does not exist"
[[ -n "$out_dir" ]] || die "--out-dir is required"

sha256_check() {
  local sums=$1 dir=$2
  if command -v sha256sum >/dev/null 2>&1; then
    (cd "$dir" && sha256sum -c "$sums")
  else
    (cd "$dir" && shasum -a 256 -c "$sums")
  fi
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

work=$(mktemp -d)
cleanup() {
  if [[ -n "${dmg_mnt:-}" && -d "${dmg_mnt:-}" ]]; then
    hdiutil detach "$dmg_mnt" >/dev/null 2>&1 || true
  fi
  if [[ -n "${gpg_tmp:-}" && -d "${gpg_tmp:-}" ]]; then
    GNUPGHOME=$gpg_tmp gpgconf --kill gpg-agent >/dev/null 2>&1 || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT

# 1. Verify unsigned CI build matches sigs/unsigned.SHA256SUMS verbatim.
unsigned_sums="$sigs_dir/unsigned.SHA256SUMS"
[[ -s "$unsigned_sums" ]] || die "missing $unsigned_sums"
unsigned_sums_abs=$(cd "$(dirname "$unsigned_sums")" && pwd)/$(basename "$unsigned_sums")
echo "release-attach: verifying unsigned CI inputs against unsigned.SHA256SUMS"
sha256_check "$unsigned_sums_abs" "$unsigned_dir"

dist_out="$out_dir/dist"
feed_out="$out_dir/feed"
rm -rf "$dist_out" "$feed_out"
mkdir -p "$dist_out" "$feed_out"

# 2. Copy the 5 unchanged CI files (Linux bundles + SBOMs).
for suffix in linux-x86_64.AppImage linux-amd64.deb linux-x86_64.flatpak sbom-rust.cdx.json sbom-npm.cdx.json; do
  src="$unsigned_dir/dash-wallet-$version-$suffix"
  [[ -f "$src" ]] || die "missing unsigned CI file $src"
  cp "$src" "$dist_out/"
done

# 3. Windows: attach detached PEM Authenticode signature and verify.
win_name="dash-wallet-$version-windows-x64-setup.exe"
win_unsigned="$unsigned_dir/$win_name"
win_sig="$sigs_dir/win/$win_name.pem"
win_signed="$dist_out/$win_name"
[[ -f "$win_unsigned" ]] || die "missing unsigned Windows installer $win_unsigned"
[[ -s "$win_sig" ]] || die "missing detached Windows signature $win_sig"
command -v osslsigncode >/dev/null 2>&1 || die "osslsigncode is required to attach/verify Windows signatures"

attach_args=(attach-signature -in "$win_unsigned" -sigin "$win_sig" -out "$win_signed")
verify_args=(verify -in "$win_signed")
if [[ -n "$win_ca_file" ]]; then
  attach_args+=(-CAfile "$win_ca_file")
  verify_args+=(-CAfile "$win_ca_file")
fi
if [[ -n "$win_leaf_hash" ]]; then
  attach_args+=(-require-leaf-hash "$win_leaf_hash")
  verify_args+=(-require-leaf-hash "$win_leaf_hash")
fi
if ((skip_win_chain_verify)); then
  verify_args+=(-ignore-timestamp -ignore-crl)
fi

echo "release-attach: attaching detached Authenticode signature to $win_name"
osslsigncode "${attach_args[@]}"
echo "release-attach: verifying signed $win_name"
osslsigncode "${verify_args[@]}"

# 4. macOS .app: extract unsigned .app, attach detached signatures, verify, pack deterministic .app.tar.gz.
mac_unsigned_tar="$unsigned_dir/dash-wallet-$version-macos-unsigned.tar.gz"
mac_sigs="$sigs_dir/osx/Dash Wallet.app"
[[ -f "$mac_unsigned_tar" ]] || die "missing unsigned macOS app archive $mac_unsigned_tar"
[[ -f "$mac_sigs/manifest.json" ]] || die "missing macOS detached signatures $mac_sigs/manifest.json"

mkdir -p "$work/mac-unsigned" "$work/mac-signed"
tar -xzf "$mac_unsigned_tar" -C "$work/mac-unsigned"
[[ -d "$work/mac-unsigned/Dash Wallet.app" ]] || die "$mac_unsigned_tar did not contain Dash Wallet.app"

echo "release-attach: applying detached macOS signatures to Dash Wallet.app"
python3 "$here/macos-detached-sig.py" apply "$work/mac-unsigned/Dash Wallet.app" "$mac_sigs" "$work/mac-signed/Dash Wallet.app"
python3 "$here/macos-detached-sig.py" verify-signed "$work/mac-signed/Dash Wallet.app"

if [[ "$(uname -s)" == Darwin ]]; then
  echo "release-attach: verifying macOS codesign, Gatekeeper (spctl), and notarization staple"
  codesign --verify --deep --strict --verbose=2 "$work/mac-signed/Dash Wallet.app"
  spctl --assess --type execute -vv "$work/mac-signed/Dash Wallet.app"
  xcrun stapler validate "$work/mac-signed/Dash Wallet.app"
fi

mac_app_tar="$feed_out/dash-wallet-$version-macos-universal.app.tar.gz"
python3 "$here/macos-detached-sig.py" pack-app "$work/mac-signed/Dash Wallet.app" "$mac_app_tar"

# 5. macOS .dmg: verify signed .dmg contains the exact attached Dash Wallet.app, and stage it.
dmg_name="dash-wallet-$version-macos-universal.dmg"
if [[ -z "$signed_dmg" && -f "$sigs_dir/$dmg_name" ]]; then
  signed_dmg="$sigs_dir/$dmg_name"
fi
[[ -n "$signed_dmg" && -f "$signed_dmg" ]] || die "missing signed macOS .dmg (pass --signed-dmg <path>)"

if [[ "$(uname -s)" == Darwin ]]; then
  echo "release-attach: verifying $dmg_name signature, staple, and interior Dash Wallet.app byte-identity"
  codesign --verify --verbose=2 "$signed_dmg"
  xcrun stapler validate "$signed_dmg"
  dmg_mnt="$work/dmg-mnt"
  mkdir -p "$dmg_mnt"
  hdiutil attach "$signed_dmg" -mountpoint "$dmg_mnt" -nobrowse -readonly >/dev/null
  [[ -d "$dmg_mnt/Dash Wallet.app" ]] || die "$signed_dmg does not contain Dash Wallet.app"
  diff -qr "$work/mac-signed/Dash Wallet.app" "$dmg_mnt/Dash Wallet.app" ||
    die "Dash Wallet.app inside $signed_dmg differs from CI unsigned build + detached signatures"
  hdiutil detach "$dmg_mnt" >/dev/null
  dmg_mnt=
fi
cp "$signed_dmg" "$dist_out/$dmg_name"

# 6. Updater feed: verify all 4 minisign signatures against tauri.conf.json pubkey and version.
latest_src="$sigs_dir/updater/latest.json"
[[ -s "$latest_src" ]] || die "missing $latest_src"
cp "$latest_src" "$feed_out/latest.json"
echo "release-attach: verifying updater latest.json minisign signatures"
python3 "$here/verify-minisign.py" \
  --pubkey-from-tauri "$tauri_conf" \
  --latest-json "$feed_out/latest.json" \
  --version "$version" \
  --files-dir "$dist_out" \
  --files-dir "$feed_out" \
  --repo "$repo"

# 7. Final SHA256SUMS + SHA256SUMS.asc verification.
final_sums="$sigs_dir/SHA256SUMS"
final_asc="$sigs_dir/SHA256SUMS.asc"
[[ -s "$final_sums" ]] || die "missing $final_sums"
[[ -s "$final_asc" ]] || die "missing $final_asc"
cp "$final_sums" "$dist_out/SHA256SUMS"
cp "$final_asc" "$dist_out/SHA256SUMS.asc"

# Verify SHA256SUMS lists exactly the 9 expected files (5 bundles + 2 SBOMs + latest.json + .app.tar.gz)
# and every file's actual hash matches.
expected_names=(
  "dash-wallet-$version-linux-amd64.deb"
  "dash-wallet-$version-linux-x86_64.AppImage"
  "dash-wallet-$version-linux-x86_64.flatpak"
  "dash-wallet-$version-macos-universal.app.tar.gz"
  "dash-wallet-$version-macos-universal.dmg"
  "dash-wallet-$version-sbom-npm.cdx.json"
  "dash-wallet-$version-sbom-rust.cdx.json"
  "dash-wallet-$version-windows-x64-setup.exe"
  "latest.json"
)
mapfile -t actual_names < <(awk '{print $2}' "$dist_out/SHA256SUMS" | LC_ALL=C sort)
[[ ${#actual_names[@]} -eq ${#expected_names[@]} ]] ||
  die "SHA256SUMS must list ${#expected_names[@]} files, found ${#actual_names[@]}"
for i in "${!expected_names[@]}"; do
  [[ "${actual_names[$i]}" == "${expected_names[$i]}" ]] ||
    die "SHA256SUMS entry mismatch: got '${actual_names[$i]}', expected '${expected_names[$i]}'"
done

while read -r want_hash fname; do
  [[ -n "$want_hash" && -n "$fname" ]] || continue
  if [[ -f "$dist_out/$fname" ]]; then
    got_hash=$(sha256_of "$dist_out/$fname")
  elif [[ -f "$feed_out/$fname" ]]; then
    got_hash=$(sha256_of "$feed_out/$fname")
  else
    die "SHA256SUMS references missing file $fname"
  fi
  [[ "$got_hash" == "$want_hash" ]] || die "SHA-256 mismatch for $fname: got $got_hash, expected $want_hash"
done < "$dist_out/SHA256SUMS"
echo "release-attach: all 9 final release files match SHA256SUMS"

# Verify PGP signature when a public key or fingerprint is supplied (or gpg keyring is configured).
if [[ -n "$pgp_pubkey" || -n "$pgp_fingerprint" ]]; then
  command -v gpg >/dev/null 2>&1 || die "gpg is required to verify SHA256SUMS.asc"
  gpg_args=(--batch --status-fd 1)
  if [[ -n "$pgp_pubkey" ]]; then
    gpg_tmp="$work/gnupg"
    (umask 077 && mkdir -p "$gpg_tmp")
    chmod 700 "$gpg_tmp"
    GNUPGHOME=$gpg_tmp gpg --batch --quiet --import "$pgp_pubkey"
    verify_out=$(GNUPGHOME=$gpg_tmp gpg "${gpg_args[@]}" --verify "$dist_out/SHA256SUMS.asc" "$dist_out/SHA256SUMS")
  else
    verify_out=$(gpg "${gpg_args[@]}" --verify "$dist_out/SHA256SUMS.asc" "$dist_out/SHA256SUMS")
  fi
  sig_primary=$(awk '$1 == "[GNUPG:]" && $2 == "VALIDSIG" { print toupper($12); exit }' <<<"$verify_out")
  [[ -n "$sig_primary" ]] || die "PGP verification failed for SHA256SUMS.asc"
  if [[ -n "$pgp_fingerprint" ]]; then
    want_fpr=$(printf '%s' "$pgp_fingerprint" | tr -d '[:space:]' | tr '[:lower:]' '[:upper:]')
    [[ "$sig_primary" == "$want_fpr" ]] ||
      die "SHA256SUMS.asc signed by primary key $sig_primary, expected $want_fpr"
  fi
  echo "release-attach: verified PGP signature SHA256SUMS.asc (primary key $sig_primary)"
fi

jq -n \
  --arg version "$version" \
  --arg windows_sha256 "$(sha256_of "$win_signed")" \
  --arg dmg_sha256 "$(sha256_of "$dist_out/$dmg_name")" \
  --arg app_tar_sha256 "$(sha256_of "$mac_app_tar")" \
  --arg sums_sha256 "$(sha256_of "$dist_out/SHA256SUMS")" \
  --arg asc_sha256 "$(sha256_of "$dist_out/SHA256SUMS.asc")" \
  '{schema: 1, version: $version, windows_sha256: $windows_sha256, dmg_sha256: $dmg_sha256, app_tar_sha256: $app_tar_sha256, sums_sha256: $sums_sha256, asc_sha256: $asc_sha256}' \
  > "$out_dir/signed-manifest.json"

echo "release-attach: ready to publish from DIST=$dist_out FEED=$feed_out"
