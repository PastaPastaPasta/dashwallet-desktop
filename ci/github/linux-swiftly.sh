#!/usr/bin/env bash
# GitHub Actions: install a Swift toolchain with swiftly on an ubuntu-24.04 runner and put it
# on the job's PATH (ci.yml's Linux Swift job, gate.yml's swiftcrossui runs).
#
#   ci/github/linux-swiftly.sh VERSION      (for example 6.3.3)
#
# swiftly itself is a pinned release, checked against the SHA-256 below before it runs (its
# release .sig verifies with "Swift Automatic Signing Key #4"; checked when pinning). swiftly
# then verifies the toolchain's GPG signature.
set -euo pipefail

version="${1:?usage: $0 VERSION}"
swiftly_version=1.1.1
case "$(uname -m)" in
  x86_64) swiftly_sha256=dc5f94308b33455530f4150b412527596a33c8525a7d59b025b598520e92d121 ;;
  aarch64) swiftly_sha256=e664899868b2dea9a1e7a7fbad0521c24aceeca14bbcbb22ba5cda1f62c37ed4 ;;
  *) echo "linux-swiftly: unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac

export SWIFTLY_HOME_DIR="$HOME/.local/share/swiftly" SWIFTLY_BIN_DIR="$HOME/.local/share/swiftly/bin"
scratch="$(mktemp -d)"
# From the scratch dir: `swiftly install --use` writes .swift-version into the current directory.
cd "$scratch"
curl -fsSL -o swiftly.tar.gz \
  "https://download.swift.org/swiftly/linux/swiftly-${swiftly_version}-$(uname -m).tar.gz"
echo "$swiftly_sha256  swiftly.tar.gz" | sha256sum -c -
tar -xzf swiftly.tar.gz
./swiftly init --assume-yes --no-modify-profile --skip-install --quiet-shell-followup
# swiftly lists the system packages the toolchain needs (libcurl4-openssl-dev on the runner)
# in a script instead of installing them.
"$SWIFTLY_BIN_DIR/swiftly" install --assume-yes --use --post-install-file post-install.sh "$version"
[[ ! -s post-install.sh ]] || sudo bash post-install.sh
cd /
rm -rf "$scratch"
echo "$SWIFTLY_BIN_DIR" >> "$GITHUB_PATH"
"$SWIFTLY_BIN_DIR/swift" --version
