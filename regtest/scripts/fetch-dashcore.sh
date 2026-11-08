#!/usr/bin/env sh
# Download and verify an official Dash Core release archive, then unpack it.
#
# Usage: fetch-dashcore.sh <dest-dir> [triple]
#   dest-dir  directory that receives bin/, lib/, share/ of the release
#   triple    release triple; default is derived from `uname -s`/`uname -m`
#             (x86_64-linux-gnu, aarch64-linux-gnu, arm64-apple-darwin, x86_64-apple-darwin,
#             win64 under Git Bash/MSYS: a .zip, unpacked with unzip or 7z)
#
# Environment:
#   DASHCORE_CACHE  directory where downloaded tarballs are kept (default: $HOME/.cache/dwd-dashcore)
#
# The version and SHA-256 values below are copied from the release's SHA256SUMS.asc
# (https://github.com/dashpay/dash/releases/download/v24.0.0/SHA256SUMS.asc).
# Bumping the version means replacing every value in this block together.
set -eu

DASHCORE_VERSION=24.0.0
SHA256_x86_64_linux_gnu=1ffce395e035fb2ee617c92e6eeaab139ca3d10db6f83d4e816136b726512c5e
SHA256_aarch64_linux_gnu=77783b233936be2c3c13046e87dd7d9162c2dbd93f8b81e9f7ca7420a3b0b1cb
SHA256_arm64_apple_darwin=93c6989985d6cc2e3405c55f7a62f5e9160593b11a8de048c6e1338540781e16
SHA256_x86_64_apple_darwin=e24380ba50473bc3f5144727f01ce0794cea1b057b58ee07df19aee7e90ba950
SHA256_win64=dc459ae59c30390e20d6c7f4952a110aaa53dec2d2ee611ab0e9777f9e0b9219

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
        MINGW64*/x86_64 | MSYS*/x86_64 | CYGWIN*/x86_64) triple=win64 ;;
        *) echo "unsupported host $os/$arch" >&2; exit 1 ;;
    esac
fi

case "$triple" in
    x86_64-linux-gnu) want=$SHA256_x86_64_linux_gnu ;;
    aarch64-linux-gnu) want=$SHA256_aarch64_linux_gnu ;;
    arm64-apple-darwin) want=$SHA256_arm64_apple_darwin ;;
    x86_64-apple-darwin) want=$SHA256_x86_64_apple_darwin ;;
    win64) want=$SHA256_win64 ;;
    *) echo "no pinned SHA-256 for triple $triple" >&2; exit 1 ;;
esac

case "$triple" in
    win64) ext=zip ;;
    *) ext=tar.gz ;;
esac
file="dashcore-${DASHCORE_VERSION}-${triple}.${ext}"
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
if [ "$ext" = zip ]; then
    # The zip has the same top directory as the tarballs.
    unpacked="$dest/.unpack"
    rm -rf "$unpacked"
    if command -v unzip >/dev/null 2>&1; then
        unzip -q "$cache/$file" -d "$unpacked"
    else
        7z x -bd -y "-o$unpacked" "$cache/$file" >/dev/null
    fi
    for part in bin lib share; do
        rm -rf "${dest:?}/$part"
        mv "$unpacked/dashcore-${DASHCORE_VERSION}/$part" "$dest/$part"
    done
    rm -rf "$unpacked"
else
    tar -xzf "$cache/$file" -C "$dest" --strip-components=1 \
        "dashcore-${DASHCORE_VERSION}/bin" \
        "dashcore-${DASHCORE_VERSION}/lib" \
        "dashcore-${DASHCORE_VERSION}/share"
fi
# dash-qt and the unit-test binary are not needed for headless regtest.
rm -f "$dest/bin/dash-qt" "$dest/bin/test_dash" "$dest/bin/dash-qt.exe" "$dest/bin/test_dash.exe"
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
