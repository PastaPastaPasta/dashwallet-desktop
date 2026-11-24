#!/usr/bin/env bash
# Local final SHA256SUMS + PGP detached signature step (runs on the release signer's Mac with his
# real PGP key).
#
# Writes SHA256SUMS over all 9 final release files (5 bundles, 2 SBOMs, latest.json, and macOS
# .app.tar.gz), signs it into SHA256SUMS.asc with `gpg --detach-sign --armor`, and verifies the
# signature against --gpg-key.
#
#   ci/release/sign-checksums.sh \
#     --version <X.Y.Z> \
#     --dist-dir <dir> \
#     --feed-dir <dir> \
#     --sigs-dir <dir> \
#     --gpg-key <PRIMARY_FINGERPRINT_OR_KEY_ID>
set -euo pipefail

version=
dist_dir=
feed_dir=
sigs_dir=
gpg_key=

while (($#)); do
  case "$1" in
    --version) version=${2:?}; shift 2 ;;
    --dist-dir) dist_dir=${2:?}; shift 2 ;;
    --feed-dir) feed_dir=${2:?}; shift 2 ;;
    --sigs-dir) sigs_dir=${2:?}; shift 2 ;;
    --gpg-key) gpg_key=${2:?}; shift 2 ;;
    *) echo "sign-checksums: unknown option: $1" >&2; exit 1 ;;
  esac
done

die() { echo "sign-checksums: $*" >&2; exit 1; }

[[ -n "$version" && "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "--version must be X.Y.Z"
[[ -d "$dist_dir" && -d "$feed_dir" ]] || die "--dist-dir and --feed-dir must exist"
[[ -n "$sigs_dir" && -n "$gpg_key" ]] || die "--sigs-dir and --gpg-key are required"

sha256_line() {
  local dir=$1 name=$2
  if command -v sha256sum >/dev/null 2>&1; then
    (cd "$dir" && sha256sum "$name")
  else
    (cd "$dir" && shasum -a 256 "$name")
  fi
}

mkdir -p "$sigs_dir"
sums="$sigs_dir/SHA256SUMS"
asc="$sigs_dir/SHA256SUMS.asc"
: > "$sums"

dist_names=(
  "dash-wallet-$version-linux-amd64.deb"
  "dash-wallet-$version-linux-x86_64.AppImage"
  "dash-wallet-$version-linux-x86_64.flatpak"
  "dash-wallet-$version-macos-universal.dmg"
  "dash-wallet-$version-sbom-npm.cdx.json"
  "dash-wallet-$version-sbom-rust.cdx.json"
  "dash-wallet-$version-windows-x64-setup.exe"
)
feed_names=(
  "dash-wallet-$version-macos-universal.app.tar.gz"
  "latest.json"
)

for name in "${dist_names[@]}"; do
  [[ -f "$dist_dir/$name" ]] || die "missing $dist_dir/$name"
  sha256_line "$dist_dir" "$name" >> "$sums"
done
for name in "${feed_names[@]}"; do
  [[ -f "$feed_dir/$name" ]] || die "missing $feed_dir/$name"
  sha256_line "$feed_dir" "$name" >> "$sums"
done
LC_ALL=C sort -k2 "$sums" -o "$sums"

rm -f "$asc"
gpg --armor --detach-sign --digest-algo SHA256 --local-user "$gpg_key" --output "$asc" "$sums"
gpg --verify "$asc" "$sums"

cp "$sums" "$dist_dir/SHA256SUMS"
cp "$asc" "$dist_dir/SHA256SUMS.asc"

echo "sign-checksums: wrote and verified $sums and $asc"
cat "$sums"
