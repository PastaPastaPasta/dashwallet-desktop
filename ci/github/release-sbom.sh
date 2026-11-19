#!/usr/bin/env bash
# GitHub Actions (release.yml, `sbom` job): CycloneDX JSON SBOMs of what the release bundles are built from,
# named like the bundles so the checksums and publish steps treat them the same, and echo their paths:
#   dash-wallet-<version>-sbom-rust.cdx.json  the app (crate dw-app, binary dash-wallet), default features,
#                                             dependencies for every target (cargo-cyclonedx, CycloneDX 1.5);
#   dash-wallet-<version>-sbom-npm.cdx.json   the renderer's production dependencies, the ones bundled into the
#                                             app, from apps/desktop/pnpm-lock.yaml (pnpm sbom, CycloneDX 1.6).
#
#   ci/github/release-sbom.sh OUT_DIR VERSION
#
# Needs cargo-cyclonedx and pnpm on PATH, and jq. The workspace crates' bom-refs carry the checkout's absolute
# path (path+file:///.../rust/crates/x): they are rewritten relative to the repository (path+file:rust/crates/x),
# and the script fails if either file still holds the checkout path, $HOME or a file:/// URL.
set -euo pipefail

out=${1:?usage: $0 OUT_DIR VERSION}
version=${2:?}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
mkdir -p "$out"
out=$(cd "$out" && pwd)
rust_sbom="$out/dash-wallet-$version-sbom-rust.cdx.json"
npm_sbom="$out/dash-wallet-$version-sbom-npm.cdx.json"

# cargo-cyclonedx writes one file next to every workspace member's Cargo.toml.
name="release-sbom-$$"
trap 'find "$root/rust" -name "$name.json" -delete' EXIT
SOURCE_DATE_EPOCH=$(git -C "$root" log -1 --format=%ct) \
  cargo cyclonedx --manifest-path "$root/rust/Cargo.toml" -f json --spec-version 1.5 --target all \
  --override-filename "$name" -q
jq --arg abs "file://$root/" 'walk(if type == "string" then (split($abs) | join("file:")) else . end)' \
  "$root/rust/crates/dw-app/$name.json" > "$rust_sbom"

(cd "$root/apps/desktop" && pnpm sbom --sbom-format cyclonedx --sbom-spec-version 1.6 --sbom-type application \
  --lockfile-only --prod) > "$npm_sbom"

for f in "$rust_sbom" "$npm_sbom"; do
  jq -e '.bomFormat == "CycloneDX" and (.components | length) > 0 and (.metadata.component.name | length) > 0' \
    "$f" > /dev/null || { echo "::error::release-sbom: $f is not a CycloneDX SBOM with components" >&2; exit 1; }
  if grep -qF -e "$root" -e "$HOME" -e "file:///" "$f"; then
    echo "::error::release-sbom: $f holds a machine-local path" >&2
    grep -oF -e "$root" -e "$HOME" -e "file:///" "$f" | sort | uniq -c >&2
    exit 1
  fi
  echo "release-sbom: $f ($(jq '.components | length' "$f") components, $(wc -c < "$f") bytes)" >&2
  echo "$f"
done
