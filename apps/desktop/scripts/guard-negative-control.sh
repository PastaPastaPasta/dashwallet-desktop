#!/usr/bin/env bash
# The negative controls of the production fixture guard (U-07, Sol r2 finding 2, r3 findings 1 and
# 2). Each works in its own copy of the app: fixture code is copied to src/guard-copy/ without its
# marker line and main.tsx imports it dynamically. Neither the marker nor the `src/fixture/` path
# names it, so Vite's noFixtureModules passes it; scripts/check-bundle.mjs must then fail
# `pnpm build` on the fixture's literals. Each fails unless the build fails at that check and on a
# literal of the expected fixture module:
# - wallet: a copy of src/fixture/wallet.ts;
# - mts: a new marked fixture module of another type, src/fixture/guard-extra.mts, and its copy;
# - errors: a copy of src/fixture/errors.ts, whose error codes are the generated bindings' and
#   exempt, but whose messages are not;
# - data-raw, data-url, data-inline, data-url-inline, data-json (Sol r4): a copy of the test data
#   src/fixture/vectors/accept.json, which carries no marker, loaded by an unmarked module as
#   `?raw` (a string in a chunk; a bare `import "…?raw"` ships nothing, Rollup drops it), `?url` (an asset file of its own), `?inline` on a known asset
#   type (`.txt`: a base64 data: URL; Vite parses `x.json?inline` as JavaScript and fails the
#   build), `?url&inline` (a base64 data: URL) or a plain JSON import (an object in a chunk).
#   `@ts-ignore` keeps tsc from standing in for the guard.
# Run by ci.yml (desktop-renderer).
#
#   scripts/guard-negative-control.sh
set -euo pipefail

app="$(cd "$(dirname "$0")/.." && pwd)"
work=

unmarked() { # <fixture source> <copy>
  grep -v '^// dw-fixture-module' "$1" >"$2"
  if grep -q 'dw-fixture-module' "$2"; then
    echo "guard-negative-control: the copy still carries the fixture marker" >&2
    exit 1
  fi
}

control() { # <name> <fixture module the found literals come from> <expected literal>
  local name=$1 source=$2 literal=$3
  # A sibling of the app, so its `../..` (the icon alias, the token sources) is still the repo.
  work="$(mktemp -d "$app/../.guard-control.XXXXXX")"
  tar -C "$app" --exclude=./node_modules --exclude=./dist --exclude=./dist-fixture \
    --exclude=./test-results --exclude=./playwright-report -cf - . | tar -C "$work" -xf -
  ln -s "$app/node_modules" "$work/node_modules"
  mkdir "$work/src/guard-copy"
  case $name in
  wallet)
    unmarked "$app/src/fixture/wallet.ts" "$work/src/guard-copy/wallet.ts"
    printf '\nvoid import("./guard-copy/wallet");\n' >>"$work/src/main.tsx"
    ;;
  mts)
    printf '// dw-fixture-module: a fixture module the guard must read.\nexport const GUARD_EXTRA = "guard extra fixture literal, a module of type .mts";\n' \
      >"$work/src/fixture/guard-extra.mts"
    unmarked "$work/src/fixture/guard-extra.mts" "$work/src/guard-copy/guard-extra.mts"
    printf '\nvoid import("./guard-copy/guard-extra.mjs");\n' >>"$work/src/main.tsx"
    ;;
  errors)
    unmarked "$app/src/fixture/errors.ts" "$work/src/guard-copy/errors.ts"
    printf '\nvoid import("./guard-copy/errors");\n' >>"$work/src/main.tsx"
    ;;
  data-*)
    local file=accept.json query
    case $name in
    data-raw) query='?raw' ;;
    data-url) query='?url' ;;
    data-inline) file=accept.txt query='?inline' ;;
    data-url-inline) query='?url&inline' ;;
    data-json) query='' ;;
    esac
    cp "$app/src/fixture/vectors/accept.json" "$work/src/guard-copy/$file"
    printf '// @ts-ignore\nimport data from "./%s%s";\nexport default data;\n' "$file" "$query" >"$work/src/guard-copy/data.ts"
    printf '\nvoid import("./guard-copy/data");\n' >>"$work/src/main.tsx"
    ;;
  esac

  local log="$work/build.log"
  if (cd "$work" && pnpm build) >"$log" 2>&1; then
    cat "$log"
    echo "guard-negative-control: FAIL ($name): a production build holding an unmarked fixture copy passed" >&2
    exit 1
  fi
  # Vite built it (the marker and path checks did not see it); check-bundle refused it.
  if ! grep -q '✓ built in' "$log" ||
    ! grep -q 'is a production renderer but contains [0-9]* fixture sentinel' "$log" ||
    ! grep -qF "$literal ($source)" "$log"; then
    cat "$log"
    echo "guard-negative-control: FAIL ($name): the build failed, but not at check-bundle's sentinels of $source" >&2
    exit 1
  fi
  grep 'fixture sentinel' "$log"
  grep -F "$literal ($source)" "$log"
  echo "guard-negative-control: $name: the unmarked copy in src/guard-copy/ fails the production build"
  rm -rf "$work"
  work=
}

trap '[[ -z $work ]] || rm -rf "$work"' EXIT
control wallet src/fixture/wallet.ts '"Coffee refund"'
control mts src/fixture/guard-extra.mts '"guard extra fixture literal, a module of type .mts"'
control errors src/fixture/errors.ts '"Block height out of range"'
for query in raw url inline url-inline json; do
  control "data-$query" src/fixture/vectors/accept.json '"lease-0123456789abcdef0123456789abcdef"'
done
