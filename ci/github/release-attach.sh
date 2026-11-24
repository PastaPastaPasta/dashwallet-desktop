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
#     --signed-dmg <path> \
#     [--repo PastaPastaPasta/dashwallet-desktop] \
#     [--tauri-conf rust/crates/dw-app/tauri.conf.json] \
#     [--identities-file docs/release/signing-identities.json] \
#     [--pgp-pubkey docs/release/release-key.asc] \
#     [--pgp-fingerprint <40-hex>] \
#     [--apple-team-id <10-alnum>] \
#     [--win-leaf-hash <sha256:64-hex>] \
#     [--win-ca-file <path>] \
#     [--skip-win-chain-verify] \
#     [--allow-non-darwin]
#
# Pinned signer identities (PGP public key + primary fingerprint, Apple Team ID, Windows leaf
# cert SHA-256) are loaded from the main repository at the tag (docs/release/signing-identities.json
# and docs/release/release-key.asc) and NEVER from --sigs-dir. All three identities fail closed
# until populated.
#
# Must run on macOS (Darwin) for codesign, spctl, stapler, and .dmg layout verification; non-Darwin
# hosts fail closed unless --allow-non-darwin is explicitly passed (for Linux test suites only).
#
# Layout expected in --sigs-dir (mirroring dashpay/dash-detached-sigs):
#   unsigned.SHA256SUMS                                    # binds sigs to the exact unsigned CI build
#   win/dash-wallet-<ver>-windows-x64-setup.exe.pem        # osslsigncode detached PEM signature
#   osx/Dash Wallet.app/manifest.json (+ .cs, CodeResources) # macos-detached-sig.py payload
#   updater/latest.json                                    # signed with local stable updater key
#   SHA256SUMS                                             # final hashes of all 9 release files
#   SHA256SUMS.asc                                         # detached armored PGP signature over SHA256SUMS
set -euo pipefail
umask 022

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd "$here/../.." && pwd)

version=
unsigned_dir=
sigs_dir=
out_dir=
signed_dmg=
repo=PastaPastaPasta/dashwallet-desktop
tauri_conf="$repo_root/rust/crates/dw-app/tauri.conf.json"
identities_file="$repo_root/docs/release/signing-identities.json"
pgp_pubkey="$repo_root/docs/release/release-key.asc"
pgp_fingerprint=
apple_team_id=
win_ca_file=
win_leaf_hash=
skip_win_chain_verify=0
allow_non_darwin=0

while (($#)); do
  case "$1" in
    --version) version=${2:?}; shift 2 ;;
    --unsigned-dir) unsigned_dir=${2:?}; shift 2 ;;
    --sigs-dir) sigs_dir=${2:?}; shift 2 ;;
    --out-dir) out_dir=${2:?}; shift 2 ;;
    --signed-dmg) signed_dmg=${2:?}; shift 2 ;;
    --repo) repo=${2:?}; shift 2 ;;
    --tauri-conf) tauri_conf=${2:?}; shift 2 ;;
    --identities-file) identities_file=${2:?}; shift 2 ;;
    --pgp-pubkey) pgp_pubkey=${2:?}; shift 2 ;;
    --pgp-fingerprint) pgp_fingerprint=${2:?}; shift 2 ;;
    --apple-team-id) apple_team_id=${2:?}; shift 2 ;;
    --win-ca-file) win_ca_file=${2:?}; shift 2 ;;
    --win-leaf-hash) win_leaf_hash=${2:?}; shift 2 ;;
    --skip-win-chain-verify) skip_win_chain_verify=1; shift ;;
    --allow-non-darwin) allow_non_darwin=1; shift ;;
    *) echo "release-attach: unknown option: $1" >&2; exit 1 ;;
  esac
done

die() { echo "release-attach: $*" >&2; exit 1; }

# N4: Fail closed off macOS unless --allow-non-darwin is explicitly requested (Linux tests only).
if [[ "$(uname -s)" != Darwin ]] && ((!allow_non_darwin)); then
  die "macOS codesign, spctl, stapler, and .dmg layout verification require macOS (pass --allow-non-darwin only for Linux test suites, never for publishing)"
fi

[[ -n "$version" && "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "--version must be a stable X.Y.Z"
[[ -d "$unsigned_dir" && ! -L "$unsigned_dir" ]] || die "--unsigned-dir $unsigned_dir does not exist"
[[ -d "$sigs_dir" && ! -L "$sigs_dir" ]] || die "--sigs-dir $sigs_dir does not exist"
[[ -n "$out_dir" ]] || die "--out-dir is required"
[[ -n "$signed_dmg" && -f "$signed_dmg" && ! -L "$signed_dmg" ]] || die "missing signed macOS .dmg (pass --signed-dmg <path>)"

# H1: Refuse any symlink anywhere inside --sigs-dir (or --unsigned-dir) before reading any file.
if find "$sigs_dir" -type l -print -quit | grep -q .; then
  die "refusing sigs directory containing symlinks ($sigs_dir)"
fi
if find "$unsigned_dir" -type l -print -quit | grep -q .; then
  die "refusing unsigned directory containing symlinks ($unsigned_dir)"
fi

# H2 / M4 / Decision 5: Load pinned signer identities from the main repo at the tag and fail closed.
if [[ -f "$identities_file" ]]; then
  [[ ! -L "$identities_file" ]] || die "refusing symlink identities file $identities_file"
  [[ -n "$pgp_fingerprint" ]] || pgp_fingerprint=$(jq -r '.pgp_fingerprint // empty' "$identities_file")
  [[ -n "$apple_team_id" ]] || apple_team_id=$(jq -r '.apple_team_id // empty' "$identities_file")
  [[ -n "$win_leaf_hash" ]] || win_leaf_hash=$(jq -r '.windows_leaf_sha256 // empty' "$identities_file")
fi

want_fpr=$(printf '%s' "$pgp_fingerprint" | tr -d '[:space:]' | tr '[:lower:]' '[:upper:]')
[[ "$want_fpr" =~ ^[0-9A-F]{40}$ ]] ||
  die "pinned PGP primary fingerprint must be 40 hex chars (got '${pgp_fingerprint:-<empty>}'; populate docs/release/signing-identities.json)"

[[ "$apple_team_id" =~ ^[A-Z0-9]{10}$ ]] ||
  die "pinned Apple Team ID must be 10 uppercase alphanumerics (got '${apple_team_id:-<empty>}'; populate docs/release/signing-identities.json)"

win_leaf_hash=$(printf '%s' "$win_leaf_hash" | tr '[:upper:]' '[:lower:]')
[[ "$win_leaf_hash" =~ ^sha256:[0-9a-f]{64}$ ]] ||
  die "pinned Windows leaf cert hash must be sha256:<64-hex> (got '${win_leaf_hash:-<empty>}'; populate docs/release/signing-identities.json)"

[[ -n "$pgp_pubkey" && -f "$pgp_pubkey" && ! -L "$pgp_pubkey" ]] ||
  die "pinned PGP public key file not found: ${pgp_pubkey:-<empty>} (populate docs/release/release-key.asc)"

# Never allow the PGP public key to come from inside --sigs-dir.
python3 - "$pgp_pubkey" "$sigs_dir" <<'PY' || die "refusing PGP public key loaded from inside --sigs-dir ($pgp_pubkey)"
import os, sys
from pathlib import Path
pub = Path(os.path.realpath(sys.argv[1]))
sigs = Path(os.path.realpath(sys.argv[2]))
try:
    pub.relative_to(sigs)
    sys.exit(1)
except ValueError:
    sys.exit(0)
PY

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
    GNUPGHOME=$gpg_tmp gpgconf --kill all >/dev/null 2>&1 || true
    rm -rf "$gpg_tmp"
  fi
  rm -rf "$work"
}
trap cleanup EXIT

# 1. Verify unsigned CI build matches sigs/unsigned.SHA256SUMS byte-for-byte with CI's copy (N5) and by hash.
ci_unsigned_sums="$unsigned_dir/unsigned.SHA256SUMS"
unsigned_sums="$sigs_dir/unsigned.SHA256SUMS"
[[ -s "$ci_unsigned_sums" ]] || die "missing CI unsigned.SHA256SUMS at $ci_unsigned_sums"
[[ -s "$unsigned_sums" ]] || die "missing $unsigned_sums"
cmp -s "$ci_unsigned_sums" "$unsigned_sums" ||
  die "sigs/unsigned.SHA256SUMS differs from CI's attested unsigned.SHA256SUMS ($ci_unsigned_sums)"
unsigned_sums_abs=$(cd "$(dirname "$unsigned_sums")" && pwd)/$(basename "$unsigned_sums")
echo "release-attach: verifying unsigned CI inputs against unsigned.SHA256SUMS (byte-identical to CI copy)"
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

# 3. Windows: attach detached PEM Authenticode signature and verify against pinned leaf hash.
win_name="dash-wallet-$version-windows-x64-setup.exe"
win_unsigned="$unsigned_dir/$win_name"
win_sig="$sigs_dir/win/$win_name.pem"
win_signed="$dist_out/$win_name"
[[ -f "$win_unsigned" ]] || die "missing unsigned Windows installer $win_unsigned"
[[ -s "$win_sig" ]] || die "missing detached Windows signature $win_sig"
command -v osslsigncode >/dev/null 2>&1 || die "osslsigncode is required to attach/verify Windows signatures"

attach_help=$(osslsigncode attach-signature --help 2>&1 || true)
verify_help=$(osslsigncode verify --help 2>&1 || true)

attach_args=(attach-signature -in "$win_unsigned" -sigin "$win_sig" -out "$win_signed")
verify_args=(verify -in "$win_signed" -require-leaf-hash "$win_leaf_hash")
if [[ -n "$win_ca_file" ]]; then
  [[ -f "$win_ca_file" && ! -L "$win_ca_file" ]] || die "--win-ca-file not found or is a symlink: $win_ca_file"
  if ! { grep -qx -- '-----BEGIN CERTIFICATE-----' "$win_ca_file" &&
         grep -qx -- '-----END CERTIFICATE-----' "$win_ca_file" &&
         openssl x509 -noout -in "$win_ca_file" >/dev/null 2>&1; }; then
    die "--win-ca-file $win_ca_file contains no valid PEM certificate"
  fi
  python3 - "$win_ca_file" "$sigs_dir" <<'PY' || die "refusing --win-ca-file loaded from inside --sigs-dir ($win_ca_file)"
import os, sys
from pathlib import Path
ca = Path(os.path.realpath(sys.argv[1]))
sigs = Path(os.path.realpath(sys.argv[2]))
try:
    ca.relative_to(sigs)
    sys.exit(1)
except ValueError:
    sys.exit(0)
PY
  # osslsigncode <= 2.13 verifies the cert chain inside attach-signature and requires -CAfile there;
  # >= 2.14 does digest-only attach and accepts -CAfile/-require-leaf-hash on verify only.
  [[ "$attach_help" == *"-CAfile"* ]] && attach_args+=(-CAfile "$win_ca_file")
  verify_args+=(-CAfile "$win_ca_file")
fi
[[ "$attach_help" == *"-require-leaf-hash"* ]] && attach_args+=(-require-leaf-hash "$win_leaf_hash")
if ((skip_win_chain_verify)); then
  verify_args+=(-ignore-timestamp)
  # -ignore-crl was added in osslsigncode 2.9 (Ubuntu 24.04 ships 2.8-2).
  [[ "$verify_help" == *"-ignore-crl"* ]] && verify_args+=(-ignore-crl)
fi

echo "release-attach: attaching detached Authenticode signature to $win_name"
osslsigncode "${attach_args[@]}"
echo "release-attach: verifying signed $win_name (pinned leaf $win_leaf_hash)"
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

# Pin both Apple Developer ID Application cert type (OIDs 1.2.840.113635.100.6.2.6 + .6.1.13) and Team ID.
apple_req="anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] /* exists */ and certificate leaf[field.1.2.840.113635.100.6.1.13] /* exists */ and certificate leaf[subject.OU] = \"$apple_team_id\""
if [[ "$(uname -s)" == Darwin ]]; then
  echo "release-attach: verifying macOS codesign (Developer ID, Team ID $apple_team_id), Gatekeeper (spctl), and notarization staple"
  codesign --verify --deep --strict --verbose=2 -R="$apple_req" "$work/mac-signed/Dash Wallet.app"
  spctl --assess --type execute -vv "$work/mac-signed/Dash Wallet.app"
  xcrun stapler validate "$work/mac-signed/Dash Wallet.app"
else
  echo "release-attach: WARNING: skipping macOS codesign/spctl/stapler/.dmg mount verification (--allow-non-darwin)" >&2
fi

mac_app_tar="$feed_out/dash-wallet-$version-macos-universal.app.tar.gz"
python3 "$here/macos-detached-sig.py" pack-app "$work/mac-signed/Dash Wallet.app" "$mac_app_tar"

# 5. macOS .dmg: verify signed .dmg Developer ID cert + Team ID, spctl primary-signature, staple, exact top-level layout, and interior Dash Wallet.app.
dmg_name="dash-wallet-$version-macos-universal.dmg"
if [[ "$(uname -s)" == Darwin ]]; then
  echo "release-attach: verifying $dmg_name signature (Developer ID, Team ID $apple_team_id), spctl primary-signature, staple, top-level layout, and interior .app"
  codesign --verify --verbose=2 -R="$apple_req" "$signed_dmg"
  spctl --assess --type open --context context:primary-signature -vv "$signed_dmg"
  xcrun stapler validate "$signed_dmg"
  dmg_mnt="$work/dmg-mnt"
  mkdir -p "$dmg_mnt"
  hdiutil attach "$signed_dmg" -mountpoint "$dmg_mnt" -nobrowse -readonly >/dev/null
  python3 "$here/macos-detached-sig.py" verify-dmg-layout "$dmg_mnt" "$work/mac-signed/Dash Wallet.app"
  codesign --verify --deep --strict --verbose=2 -R="$apple_req" "$dmg_mnt/Dash Wallet.app"
  hdiutil detach "$dmg_mnt" >/dev/null
  dmg_mnt=
fi
cp "$signed_dmg" "$dist_out/$dmg_name"

# 6. Updater feed: verify all 4 exact URLs and minisign signatures against tauri.conf.json pubkey and version.
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

# 7. Final SHA256SUMS + mandatory pinned PGP SHA256SUMS.asc verification (rejecting revoked/expired keys).
final_sums="$sigs_dir/SHA256SUMS"
final_asc="$sigs_dir/SHA256SUMS.asc"
[[ -s "$final_sums" ]] || die "missing $final_sums"
[[ -s "$final_asc" ]] || die "missing $final_asc"
cp "$final_sums" "$dist_out/SHA256SUMS"
cp "$final_asc" "$dist_out/SHA256SUMS.asc"

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
actual_names=()
while IFS= read -r fname; do
  [[ -n "$fname" ]] && actual_names+=("$fname")
done < <(awk '{print $2}' "$dist_out/SHA256SUMS" | LC_ALL=C sort)
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

command -v gpg >/dev/null 2>&1 || die "gpg is required to verify SHA256SUMS.asc"
# Short /tmp path keeps gpg-agent/keyboxd sockets well under macOS's 104-byte UNIX_PATH_MAX.
gpg_tmp=$(umask 077 && mktemp -d /tmp/dwgpg.XXXXXX)
chmod 700 "$gpg_tmp"
GNUPGHOME=$gpg_tmp gpg --batch --quiet --import "$pgp_pubkey" ||
  die "failed to import pinned PGP public key from $pgp_pubkey"

gpg_status="$work/gpg-status.txt"
gpg_stderr="$work/gpg-stderr.txt"
gpg_rc=0
GNUPGHOME=$gpg_tmp gpg --batch --status-file "$gpg_status" --verify "$dist_out/SHA256SUMS.asc" "$dist_out/SHA256SUMS" \
  >/dev/null 2>"$gpg_stderr" || gpg_rc=$?
if ((gpg_rc != 0)); then
  cat "$gpg_stderr" "$gpg_status" >&2 || true
  die "gpg --verify failed for SHA256SUMS.asc (exit $gpg_rc)"
fi

# L1: Reject revoked or expired keys/signatures explicitly, and require GOODSIG + pinned primary VALIDSIG.
# Use POSIX ERE `( |$)` rather than GNU `\b` so macOS BSD grep behaves identically to Linux GNU grep.
if grep -qE '^\[GNUPG:\] (REVKEYSIG|KEYREVOKED|EXPKEYSIG|EXPSIG|BADSIG|ERRSIG)( |$)' "$gpg_status"; then
  cat "$gpg_stderr" "$gpg_status" >&2 || true
  die "PGP signature on SHA256SUMS.asc rejected (revoked, expired, or bad key/signature)"
fi
grep -qE '^\[GNUPG:\] GOODSIG( |$)' "$gpg_status" || {
  cat "$gpg_stderr" "$gpg_status" >&2 || true
  die "PGP verification for SHA256SUMS.asc did not emit GOODSIG"
}
sig_primary=$(awk '$1 == "[GNUPG:]" && $2 == "VALIDSIG" { print toupper($12); exit }' "$gpg_status")
[[ -n "$sig_primary" ]] || die "PGP verification failed for SHA256SUMS.asc (missing VALIDSIG)"
[[ "$sig_primary" == "$want_fpr" ]] ||
  die "SHA256SUMS.asc signed by primary key $sig_primary, expected $want_fpr"
echo "release-attach: verified PGP signature SHA256SUMS.asc (GOODSIG + pinned primary key $sig_primary)"

# L4 / H2: Record pinned identities and SHA-256 of ALL 11 published files in signed-manifest.json.
python3 - "$version" "$sig_primary" "$apple_team_id" "$win_leaf_hash" "$dist_out" "$feed_out" "$out_dir/signed-manifest.json" <<'PY'
import hashlib, json, sys
from pathlib import Path

version, pgp_fpr, apple_team_id, win_leaf, dist_dir, feed_dir, out_path = sys.argv[1:]
expected = [
    f"dash-wallet-{version}-linux-amd64.deb",
    f"dash-wallet-{version}-linux-x86_64.AppImage",
    f"dash-wallet-{version}-linux-x86_64.flatpak",
    f"dash-wallet-{version}-macos-universal.app.tar.gz",
    f"dash-wallet-{version}-macos-universal.dmg",
    f"dash-wallet-{version}-sbom-npm.cdx.json",
    f"dash-wallet-{version}-sbom-rust.cdx.json",
    f"dash-wallet-{version}-windows-x64-setup.exe",
    "latest.json",
    "SHA256SUMS",
    "SHA256SUMS.asc",
]
files = {}
for name in sorted(expected):
    p = Path(dist_dir) / name
    if not p.is_file():
        p = Path(feed_dir) / name
    if not p.is_file():
        raise SystemExit(f"signed-manifest: missing {name}")
    h = hashlib.sha256()
    with p.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 16), b""):
            h.update(chunk)
    files[name] = h.hexdigest()

manifest = {
    "schema": 1,
    "version": version,
    "pgp_fpr": pgp_fpr,
    "apple_team_id": apple_team_id,
    "windows_leaf_sha256": win_leaf,
    "files": files,
}
Path(out_path).write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
PY

echo "release-attach: ready to publish from DIST=$dist_out FEED=$feed_out"
