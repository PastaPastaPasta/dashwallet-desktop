#!/usr/bin/env bash
# GitHub Actions: install protoc $PROTOC_VERSION (set by the workflow) from the protobuf release
# zip into $RUNNER_TEMP/protoc and put it on the job's PATH. Linux, macOS and Windows (Git Bash).
# The zip is checked against the SHA-256 below (protobuf publishes no checksums; these were
# computed once from the release assets). A new version needs its five hashes added here.
# With PINNED_CACHE_DIR set (.github/actions/pinned-sources), the zip is read from there when the
# Actions cache restored it, and downloaded into it otherwise; it is checked on every use.
set -euo pipefail

version="${PROTOC_VERSION:?set PROTOC_VERSION}"
case "$(uname -s)/$(uname -m)" in
  Linux/x86_64) asset=linux-x86_64 ;;
  Linux/aarch64) asset=linux-aarch_64 ;;
  Darwin/arm64) asset=osx-aarch_64 ;;
  Darwin/x86_64) asset=osx-x86_64 ;;
  MINGW*/* | MSYS*/*) asset=win64 ;;
  *) echo "install-protoc: unsupported host $(uname -s)/$(uname -m)" >&2; exit 1 ;;
esac

case "$version/$asset" in
  29.3/linux-x86_64) sha256=3e866620c5be27664f3d2fa2d656b5f3e09b5152b42f1bedbf427b333e90021a ;;
  29.3/linux-aarch_64) sha256=6427349140e01f06e049e707a58709a4f221ae73ab9a0425bc4a00c8d0e1ab32 ;;
  29.3/osx-aarch_64) sha256=2b8a3403cd097f95f3ba656e14b76c732b6b26d7f183330b11e36ef2bc028765 ;;
  29.3/osx-x86_64) sha256=9a788036d8f9854f7b03c305df4777cf0e54e5b081e25bf15252da87e0e90875 ;;
  29.3/win64) sha256=57ea59e9f551ad8d71ffaa9b5cfbe0ca1f4e720972a1db7ec2d12ab44bff9383 ;;
  *) echo "install-protoc: no pinned SHA-256 for protoc $version ($asset)" >&2; exit 1 ;;
esac

dest="${RUNNER_TEMP:?}/protoc"
cache="${PINNED_CACHE_DIR:-}"
zip="${cache:-$RUNNER_TEMP}/protoc-${version}-${asset}.zip"
from="https://github.com/protocolbuffers/protobuf/releases/download/v${version}/protoc-${version}-${asset}.zip"
if [[ -n "$cache" && -f "$zip" ]]; then
  from="the Actions cache"
else
  mkdir -p "$(dirname "$zip")"
  # Retried: a runner that cannot reach the host for a few seconds must not fail the job.
  curl -fsSL --retry 5 --retry-all-errors --retry-delay 5 --connect-timeout 30 -o "$zip.part" "$from"
  mv "$zip.part" "$zip"
fi
# From stdin: given a path with backslashes, Git Bash's sha256sum prefixes the hash with "\".
if command -v sha256sum >/dev/null; then actual="$(sha256sum <"$zip")"; else actual="$(shasum -a 256 <"$zip")"; fi
if [[ "${actual%% *}" != "$sha256" ]]; then
  hint=
  [[ "$from" != "the Actions cache" ]] || hint=" The cache entry is corrupt or was tampered with: delete it (gh cache delete '${PINNED_CACHE_KEY:-pinned-sources-…}') and rerun."
  echo "::error::install-protoc: SHA-256 mismatch for protoc-${version}-${asset}.zip from $from: ${actual%% *}, pinned $sha256.$hint" >&2
  exit 1
fi
echo "install-protoc: protoc-${version}-${asset}.zip from $from, SHA-256 matches the pin"
rm -rf "$dest"
if command -v unzip >/dev/null; then unzip -q -d "$dest" "$zip"; else 7z x -o"$dest" "$zip" >/dev/null; fi
[[ -n "$cache" ]] || rm "$zip"
echo "$dest/bin" >> "$GITHUB_PATH"
"$dest/bin/protoc" --version
