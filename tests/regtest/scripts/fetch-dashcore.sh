#!/usr/bin/env sh
# Download and verify an official Dash Core release tarball, then unpack it.
#
# Usage: fetch-dashcore.sh <dest-dir> [triple]
#   dest-dir  directory that receives bin/, lib/, share/ of the release
#   triple    release triple; default is derived from `uname -s`/`uname -m`
#             (x86_64-linux-gnu, aarch64-linux-gnu, arm64-apple-darwin, x86_64-apple-darwin)
#
# Environment:
#   DASHCORE_CACHE  directory where downloaded tarballs are kept (default: $HOME/.cache/dwd-dashcore)
#
# The version and SHA-256 values below are copied from the release's SHA256SUMS.asc
# (https://github.com/dashpay/dash/releases/download/v24.0.0-rc.2/SHA256SUMS.asc).
# Bumping the version means replacing every value in this block together.
set -eu

DASHCORE_VERSION=24.0.0-rc.2
SHA256_x86_64_linux_gnu=626840b4d724b2b4238fc53fed6025ff72a6600be2d8b078441340950113f908
SHA256_aarch64_linux_gnu=05a97c68ea31985975f9669e34369dd6dd5bce5f278edfbab17d9ce572801a1b
SHA256_arm64_apple_darwin=c0e1aa38c851247a01c9f7bb6b177371993ef7ab45e45fa6dca159b7a21c5dcb
SHA256_x86_64_apple_darwin=285fbb4fba103d065b56be7658dcf38d94201a87f4bdf01972417c4e83e1095c

if [ "$#" -lt 1 ]; then
    echo "usage: $0 <dest-dir> [triple]" >&2
    exit 2
fi
dest=$1

if [ "$#" -ge 2 ]; then
    triple=$2
else
    os=$(uname -s)
    arch=$(uname -m)
    case "$os/$arch" in
        Linux/x86_64) triple=x86_64-linux-gnu ;;
        Linux/aarch64 | Linux/arm64) triple=aarch64-linux-gnu ;;
        Darwin/arm64) triple=arm64-apple-darwin ;;
        Darwin/x86_64) triple=x86_64-apple-darwin ;;
        *) echo "unsupported host $os/$arch" >&2; exit 1 ;;
    esac
fi

case "$triple" in
    x86_64-linux-gnu) want=$SHA256_x86_64_linux_gnu ;;
    aarch64-linux-gnu) want=$SHA256_aarch64_linux_gnu ;;
    arm64-apple-darwin) want=$SHA256_arm64_apple_darwin ;;
    x86_64-apple-darwin) want=$SHA256_x86_64_apple_darwin ;;
    *) echo "no pinned SHA-256 for triple $triple" >&2; exit 1 ;;
esac

file="dashcore-${DASHCORE_VERSION}-${triple}.tar.gz"
url="https://github.com/dashpay/dash/releases/download/v${DASHCORE_VERSION}/${file}"
cache=${DASHCORE_CACHE:-"$HOME/.cache/dwd-dashcore"}
mkdir -p "$cache"

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

if [ ! -f "$cache/$file" ] || [ "$(sha256_of "$cache/$file")" != "$want" ]; then
    echo "downloading $url" >&2
    curl -fsSL --retry 3 -o "$cache/$file.part" "$url"
    mv "$cache/$file.part" "$cache/$file"
fi

got=$(sha256_of "$cache/$file")
if [ "$got" != "$want" ]; then
    echo "SHA-256 mismatch for $file: got $got, want $want" >&2
    exit 1
fi
echo "verified $file sha256=$got" >&2

mkdir -p "$dest"
tar -xzf "$cache/$file" -C "$dest" --strip-components=1 \
    "dashcore-${DASHCORE_VERSION}/bin" \
    "dashcore-${DASHCORE_VERSION}/lib" \
    "dashcore-${DASHCORE_VERSION}/share"
# dash-qt and the unit-test binary are not needed for headless regtest.
rm -f "$dest/bin/dash-qt" "$dest/bin/test_dash"
case "$triple" in
    *-apple-darwin)
        # The release tarball's Mach-O binaries are unsigned; Apple Silicon kills unsigned
        # executables (exit 137). An ad-hoc signature lets them run locally. The tarball
        # itself was verified above.
        for bin in "$dest"/bin/*; do
            codesign --force --sign - "$bin"
        done
        ;;
esac
echo "$DASHCORE_VERSION" > "$dest/VERSION"
echo "installed Dash Core $DASHCORE_VERSION ($triple) into $dest" >&2
