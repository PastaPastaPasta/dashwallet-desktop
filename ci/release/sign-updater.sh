#!/usr/bin/env bash
# Local updater feed signing step (runs on the release signer's Mac with the local stable updater key,
# never stored in CI).
#
# Signs the final Linux AppImage, signed Windows NSIS installer, and signed macOS .app.tar.gz using
# `pnpm tauri signer sign --app-version <VERSION>` (binding each signature to <VERSION> for
# `requireSignedVersion: true`), writes latest.json into <sigs-dir>/updater/latest.json and
# <feed-dir>/latest.json, and cryptographically verifies all 4 platform signatures against
# rust/crates/dw-app/tauri.conf.json's plugins.updater.pubkey.
#
#   ci/release/sign-updater.sh \
#     --version <X.Y.Z> \
#     --dist-dir <dir> \
#     --feed-dir <dir> \
#     --sigs-dir <dir> \
#     --key-file <path-to-stable-updater.key> \
#     [--repo PastaPastaPasta/dashwallet-desktop]
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd "$here/../.." && pwd)
gh_dir="$repo_root/ci/github"

version=
dist_dir=
feed_dir=
sigs_dir=
key_file=
repo=PastaPastaPasta/dashwallet-desktop

while (($#)); do
  case "$1" in
    --version) version=${2:?}; shift 2 ;;
    --dist-dir) dist_dir=${2:?}; shift 2 ;;
    --feed-dir) feed_dir=${2:?}; shift 2 ;;
    --sigs-dir) sigs_dir=${2:?}; shift 2 ;;
    --key-file) key_file=${2:?}; shift 2 ;;
    --repo) repo=${2:?}; shift 2 ;;
    *) echo "sign-updater: unknown option: $1" >&2; exit 1 ;;
  esac
done

die() { echo "sign-updater: $*" >&2; exit 1; }

[[ -n "$version" && "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "--version must be X.Y.Z"
[[ -d "$dist_dir" && -d "$feed_dir" ]] || die "--dist-dir and --feed-dir must exist"
[[ -n "$sigs_dir" ]] || die "--sigs-dir is required"
[[ -n "$key_file" && -f "$key_file" ]] || die "--key-file not found: ${key_file:-<unset>}"

linux_name="dash-wallet-$version-linux-x86_64.AppImage"
win_name="dash-wallet-$version-windows-x64-setup.exe"
mac_name="dash-wallet-$version-macos-universal.app.tar.gz"

for f in "$dist_dir/$linux_name" "$dist_dir/$win_name" "$feed_dir/$mac_name"; do
  [[ -f "$f" ]] || die "missing artifact to sign: $f"
done

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# Copy into a throwaway dir so `tauri signer sign` writes its .sig files outside dist/feed.
cp "$dist_dir/$linux_name" "$dist_dir/$win_name" "$feed_dir/$mac_name" "$work/"

# Prompt once for the updater key password unless already provided via environment.
if [[ -z "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD+x}" ]]; then
  printf 'Updater private key passphrase (leave empty if none): ' >&2
  read -rs pass
  echo >&2
  export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=$pass
fi

for name in "$linux_name" "$win_name" "$mac_name"; do
  echo "sign-updater: signing $name (version $version)"
  pnpm --dir "$repo_root/apps/desktop" tauri signer sign \
    --app-version "$version" \
    -f "$key_file" \
    "$work/$name" >/dev/null
  [[ -s "$work/$name.sig" ]] || die "tauri signer did not produce $name.sig"
done

mkdir -p "$sigs_dir/updater"
"$gh_dir/release-feed.sh" feed "$work" "$work/feed-out" "$version" "$repo" >/dev/null
cp "$work/feed-out/latest.json" "$sigs_dir/updater/latest.json"
cp "$work/feed-out/latest.json" "$feed_dir/latest.json"

python3 "$gh_dir/verify-minisign.py" \
  --pubkey-from-tauri "$repo_root/rust/crates/dw-app/tauri.conf.json" \
  --latest-json "$sigs_dir/updater/latest.json" \
  --version "$version" \
  --files-dir "$dist_dir" \
  --files-dir "$feed_dir" \
  --repo "$repo"

echo "sign-updater: wrote and verified $sigs_dir/updater/latest.json"
