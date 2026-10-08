#!/usr/bin/env bash
# Build libdashwallet_core (rust/crates/dw-ffi), regenerate the UniFFI Swift
# bindings and assemble the SE-0482 artifact bundle consumed by Package.swift.
#
#   scripts/build-core.sh [--triple <rust-triple>] [--profile dev|release|dist]
#                         [--no-bindings | --check-bindings]
#
# Outputs:
#   Sources/DashWalletCore/Generated/DashWalletCore.swift      (committed)
#   Artifacts/DashWalletCore.artifactbundle/<variant>/          (gitignored)
#       libdashwallet_core.a (dashwallet_core.lib on Windows),
#       source-stamp (hash of rust/ sources)
#       include/DashWalletCoreFFI.h, include/module.modulemap
#   Artifacts/DashWalletCore.artifactbundle/info.json           (variants built from
#                                                                the current sources)
#
# Env: CARGO_TARGET_DIR (default: see "Cargo target dir" below),
#      DWD_DEPS_DIR (shared dependency dir; its target/ becomes the default),
#      DWD_MIN_FREE_GB (disk guard threshold, default 15).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUST_DIR="$ROOT/rust"
BUNDLE="$ROOT/Artifacts/DashWalletCore.artifactbundle"
GEN_DIR="$ROOT/Sources/DashWalletCore/Generated"

triple=""
profile="dev"
bindings=1
check_bindings=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --triple) triple="$2"; shift 2 ;;
    --profile) profile="$2"; shift 2 ;;
    --no-bindings) bindings=0; shift ;;
    # Do not write bindings; fail if the committed ones differ (CI check).
    --check-bindings) bindings=0; check_bindings=1; shift ;;
    -h|--help) sed -n '2,19p' "$0"; exit 0 ;;
    *) echo "build-core: unknown argument $1" >&2; exit 2 ;;
  esac
done

case "$profile" in
  dev) profile_dir=debug ;;
  release|dist) profile_dir="$profile" ;;
  *) echo "build-core: profile must be dev, release or dist" >&2; exit 2 ;;
esac

host_triple="$(cd "$RUST_DIR" && rustc -vV | awk '/^host:/ {print $2}')"
[[ -n "$triple" ]] || triple="$host_triple"

case "$triple" in
  aarch64-apple-darwin)       variant=macos-arm64;    swift_triples='"arm64-apple-macosx"' ;;
  x86_64-apple-darwin)        variant=macos-x86_64;   swift_triples='"x86_64-apple-macosx"' ;;
  x86_64-unknown-linux-gnu)   variant=linux-x86_64;   swift_triples='"x86_64-unknown-linux-gnu"' ;;
  aarch64-unknown-linux-gnu)  variant=linux-aarch64;  swift_triples='"aarch64-unknown-linux-gnu"' ;;
  # Windows: CI's non-blocking Swift job (docs/ci.md, "Known gaps").
  x86_64-pc-windows-msvc)     variant=windows-x86_64; swift_triples='"x86_64-unknown-windows-msvc"' ;;
  *) echo "build-core: unsupported triple $triple" >&2; exit 2 ;;
esac

# Cargo target dir (DESIGN.md R3), first match wins:
#   1. CARGO_TARGET_DIR;
#   2. $DWD_DEPS_DIR/target when DWD_DEPS_DIR is set;
#   3. on macOS, ~/workspace/dashwallet-desktop-deps/target when that deps dir
#      exists (the dev Mac's one shared target dir);
#   4. rust/target. Linux hosts that build several worktrees at once (agentbox)
#      keep one target dir per checkout and share compiles through sccache.
# A relative CARGO_TARGET_DIR or DWD_DEPS_DIR is taken from the directory the
# script runs in: cargo runs in rust/ and would resolve it there, while the
# artifact is looked up from here.
if [[ -z "${CARGO_TARGET_DIR:-}" ]]; then
  if [[ -n "${DWD_DEPS_DIR:-}" ]]; then
    export CARGO_TARGET_DIR="$DWD_DEPS_DIR/target"
  elif [[ "$(uname -s)" == Darwin && -d "$HOME/workspace/dashwallet-desktop-deps" ]]; then
    export CARGO_TARGET_DIR="$HOME/workspace/dashwallet-desktop-deps/target"
  else
    export CARGO_TARGET_DIR="$RUST_DIR/target"
  fi
fi
[[ "$CARGO_TARGET_DIR" == /* ]] || export CARGO_TARGET_DIR="$PWD/$CARGO_TARGET_DIR"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-14.0}"

"$ROOT/scripts/disk-guard.sh"

# Source stamp: one hash over every file under rust/ except build output,
# followed by the cargo profile. A bundle variant is listed in info.json only
# when its stamp matches this build's, so a variant built from older sources
# or with another profile (for example a release Linux variant left by an
# earlier Docker run next to a dev macOS build) can never be linked silently.
source_stamp() {
  local hasher
  if command -v sha256sum >/dev/null 2>&1; then hasher="sha256sum"; else hasher="shasum -a 256"; fi
  (cd "$RUST_DIR" && find . -type f -not -path './target/*' -not -name '.DS_Store' -print0 \
      | LC_ALL=C sort -z | xargs -0 $hasher | $hasher | cut -d' ' -f1)
}
stamp="$(source_stamp) $profile"

# Building the host triple without --target shares target/<profile> with
# plain `cargo build`/`cargo test`, instead of compiling the graph twice.
target_args=()
out_dir="$CARGO_TARGET_DIR/$profile_dir"
if [[ "$triple" != "$host_triple" ]]; then
  target_args=(--target "$triple")
  out_dir="$CARGO_TARGET_DIR/$triple/$profile_dir"
fi

log="$(mktemp "${TMPDIR:-/tmp}/build-core.XXXXXX")"
trap 'rm -f "$log"' EXIT

echo "build-core: cargo rustc -p dw-ffi ($triple, $profile) -> $CARGO_TARGET_DIR"
# --print native-static-libs gives the exact system libraries the static
# library needs; they become `link` directives in the module map so Swift
# targets autolink them.
(cd "$RUST_DIR" && cargo rustc -p dw-ffi --lib --profile "$profile" ${target_args[@]+"${target_args[@]}"} \
    -- --print native-static-libs) 2>&1 | tee "$log"

# Without colour codes: under CARGO_TERM_COLOR=always (CI) the line ends in an ANSI reset, which
# would turn the last library, -lc, into a link directive for "c<ESC>[0m".
esc="$(printf '\033')"
native_libs="$(sed -n 's/.*native-static-libs: //p' "$log" | tail -1 | sed "s/${esc}\[[0-9;]*m//g")"
if [[ -z "$native_libs" ]]; then
  echo "build-core: rustc printed no native-static-libs line" >&2
  exit 1
fi

case "$triple" in
  *apple-darwin) lib_name=libdashwallet_core.a; dylib="$out_dir/libdashwallet_core.dylib" ;;
  *windows-msvc) lib_name=dashwallet_core.lib; dylib="$out_dir/dashwallet_core.dll" ;;
  *) lib_name=libdashwallet_core.a; dylib="$out_dir/libdashwallet_core.so" ;;
esac
lib="$out_dir/$lib_name"
[[ -f "$lib" ]] || { echo "build-core: $lib not found" >&2; exit 1; }

gen_tmp="$(mktemp -d "${TMPDIR:-/tmp}/uniffi-gen.XXXXXX")"
trap 'rm -f "$log"; rm -rf "$gen_tmp"' EXIT
echo "build-core: uniffi-bindgen generate (library mode)"
(cd "$RUST_DIR" && cargo run -q -p uniffi-bindgen --profile dev -- \
    generate --library "$dylib" --language swift --out-dir "$gen_tmp")

swift_files=("$gen_tmp"/*.swift)
if [[ ${#swift_files[@]} -ne 1 ]]; then
  echo "build-core: expected one generated .swift file, got ${#swift_files[@]}" >&2
  exit 1
fi
if (( bindings )); then
  mkdir -p "$GEN_DIR"
  cp "${swift_files[0]}" "$GEN_DIR/DashWalletCore.swift"
elif (( check_bindings )); then
  if ! diff -q "${swift_files[0]}" "$GEN_DIR/DashWalletCore.swift" >/dev/null; then
    echo "build-core: committed bindings differ from the generated ones; run scripts/build-core.sh and commit" >&2
    diff -u "$GEN_DIR/DashWalletCore.swift" "${swift_files[0]}" | head -40 >&2 || true
    exit 1
  fi
  echo "build-core: committed bindings match"
fi

vdir="$BUNDLE/$variant"
rm -rf "$vdir"
mkdir -p "$vdir/include"
cp "$gen_tmp/DashWalletCoreFFI.h" "$vdir/include/DashWalletCoreFFI.h"
# Same filesystem: hard link (dev archives are several hundred MB). Else copy.
# MSVC: some import libraries come from crates rather than the Windows SDK (windows-targets
# ships windows.0.52.0.lib in its own directory), and a Swift link does not search there. Merge
# them into the static library, and leave them out of the module map below.
merged_libs=" "
if [[ "$triple" == *windows-msvc ]]; then
  crate_libs=()
  # Git Bash: native Windows paths, and no MSYS rewriting of llvm-lib's -out: argument.
  winpath() { cygpath -w "$1"; }
  for native in $native_libs; do
    [[ "$native" == *.lib && "$merged_libs" != *" $native "* ]] || continue
    # The target architecture's copy (windows_x86_64_msvc-*/lib/, not windows_aarch64_msvc-*),
    # newest version first when the registry holds several.
    found="$( (find "${CARGO_HOME:-$HOME/.cargo}/registry/src" -path "*_${triple%%-*}_msvc-*/lib/$native" \
      2>/dev/null || true) | sort -V | tail -n 1)"
    if [[ -n "$found" ]]; then
      crate_libs+=("$(winpath "$found")"); merged_libs+="$native "
    elif [[ "$native" == windows*.lib ]]; then
      echo "build-core: warning: $native not found in the cargo registry; the Swift link will need it" >&2
    fi
  done
  if (( ${#crate_libs[@]} )); then
    echo "build-core: merging crate import libraries into $lib_name:${merged_libs% }"
    MSYS_NO_PATHCONV=1 llvm-lib "-out:$(winpath "$vdir/$lib_name")" "$(winpath "$lib")" "${crate_libs[@]}"
  fi
fi
[[ -f "$vdir/$lib_name" ]] || ln -f "$lib" "$vdir/$lib_name" 2>/dev/null || cp "$lib" "$vdir/$lib_name"

# Our own module map: uniffi's generated one carries `use "Darwin"`, which
# does not exist on Linux.
{
  echo "module DashWalletCoreFFI {"
  echo "    header \"DashWalletCoreFFI.h\""
  echo "    export *"
  set -- $native_libs
  while [[ $# -gt 0 ]]; do
    case "$1" in
      -framework) echo "    link framework \"$2\""; shift 2 ;;
      -l*) name="${1#-l}"
           # libc / libSystem are always linked by the Swift driver.
           [[ "$name" == "c" || "$name" == "System" ]] || echo "    link \"$name\""
           shift ;;
      # MSVC prints import libraries by file name (kernel32.lib) and /defaultlib: for the CRT,
      # which the Swift driver links itself.
      *.lib) [[ "$merged_libs" == *" $1 "* ]] || echo "    link \"${1%.lib}\""; shift ;;
      *) shift ;;
    esac
  done
  echo "}"
} > "$vdir/include/module.modulemap"

echo "$stamp" > "$vdir/source-stamp"

cat > "$vdir/variant.json" <<EOF
        {
          "path": "$variant/$lib_name",
          "supportedTriples": [$swift_triples],
          "staticLibraryMetadata": {
            "headerPaths": ["$variant/include"],
            "moduleMapPath": "$variant/include/module.modulemap"
          }
        }
EOF

# info.json lists every variant present in the bundle whose source stamp
# matches, so a macOS and a Linux build of the same sources (shared checkout,
# Docker) coexist, and stale variants are left out with a warning.
{
  echo '{'
  echo '  "schemaVersion": "1.0",'
  echo '  "artifacts": {'
  echo '    "DashWalletCoreFFI": {'
  echo '      "version": "0.1.0",'
  echo '      "type": "staticLibrary",'
  echo '      "variants": ['
  first=1
  for f in "$BUNDLE"/*/variant.json; do
    d="$(dirname "$f")"
    if [[ "$(cat "$d/source-stamp" 2>/dev/null)" != "$stamp" ]]; then
      echo "build-core: leaving stale variant $(basename "$d") out of info.json (other sources or profile; rebuild it with --triple ... --profile $profile)" >&2
      continue
    fi
    (( first )) || echo '        ,'
    cat "$f"
    first=0
  done
  echo '      ]'
  echo '    }'
  echo '  }'
  echo '}'
} > "$BUNDLE/info.json"

echo "build-core: native libs: $native_libs"
echo "build-core: bundle variant $variant ready: $(du -sh "$vdir" | cut -f1)"
