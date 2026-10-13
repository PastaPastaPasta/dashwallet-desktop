#!/usr/bin/env bash
# The negative control of the production fixture guard (U-07, Sol r2 finding 2): in a copy of the
# app, src/fixture/wallet.ts is copied to src/guard-copy/ without its marker line and main.tsx
# imports it dynamically. Neither the marker nor the `src/fixture/` path names it, so Vite's
# noFixtureModules passes it; scripts/check-bundle.mjs must then fail `pnpm build` on the
# fixture's literals. This fails unless the build fails at that check and for that reason.
# Run by ci.yml (desktop-renderer).
#
#   scripts/guard-negative-control.sh
set -euo pipefail

app="$(cd "$(dirname "$0")/.." && pwd)"
# A sibling of the app, so its `../..` (the icon alias, the token sources) is still the repo.
work="$(mktemp -d "$app/../.guard-control.XXXXXX")"
trap 'rm -rf "$work"' EXIT

tar -C "$app" --exclude=./node_modules --exclude=./dist --exclude=./dist-fixture \
  --exclude=./test-results --exclude=./playwright-report -cf - . | tar -C "$work" -xf -
ln -s "$app/node_modules" "$work/node_modules"

mkdir "$work/src/guard-copy"
grep -v '^// dw-fixture-module' "$app/src/fixture/wallet.ts" >"$work/src/guard-copy/wallet.ts"
if grep -q 'dw-fixture-module' "$work/src/guard-copy/wallet.ts"; then
  echo "guard-negative-control: the copy still carries the fixture marker" >&2
  exit 1
fi
printf '\nvoid import("./guard-copy/wallet");\n' >>"$work/src/main.tsx"

log="$work/build.log"
if (cd "$work" && pnpm build) >"$log" 2>&1; then
  cat "$log"
  echo "guard-negative-control: FAIL: a production build holding an unmarked fixture copy passed" >&2
  exit 1
fi
# Vite built it (the marker and path checks did not see it); check-bundle refused it.
if ! grep -q '✓ built in' "$log" ||
  ! grep -q 'is a production renderer but contains [0-9]* fixture sentinel' "$log" ||
  ! grep -q '(src/fixture/wallet.ts)' "$log"; then
  cat "$log"
  echo "guard-negative-control: FAIL: the build failed, but not at check-bundle's fixture sentinels" >&2
  exit 1
fi
grep 'fixture sentinel' "$log"
echo "guard-negative-control: the unmarked copy in src/guard-copy/ fails the production build"
