#!/usr/bin/env bash
# Enforce the allowed-imports table of DESIGN-opus §1.6.
#
#   scripts/lint-imports.sh [repo_root]
#
# Every directory under Sources/ must have an entry below; a new target without
# one fails the lint, so the table stays complete. Test targets under Tests/
# named <X>Tests may import Testing, XCTest, Foundation, Observation, <X>, and
# whatever <X> may import. Other test targets may import Testing, XCTest and
# Foundation only, unless listed.
set -euo pipefail

root="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

# Apple frameworks PlatformServicesMac may use (DESIGN-opus §1.6).
apple_frameworks="AppKit Security LocalAuthentication UserNotifications ServiceManagement ScreenCaptureKit AVFoundation CoreLocation CoreGraphics CoreImage IOKit"

allowed_for() {
  case "$1" in
    DashWalletCore)          echo "Foundation DashWalletCoreFFI" ;;
    DashKit)                 echo "Foundation DashWalletCore" ;;
    WalletRuntime)           echo "Foundation Observation DashKit PlatformServices" ;;
    AppServices)             echo "Foundation Observation DashKit PlatformServices" ;;
    PlatformServices)        echo "Foundation" ;;
    PlatformServicesMac)     echo "Foundation Observation PlatformServices $apple_frameworks" ;;
    PlatformServicesDesktop) echo "Foundation DashKit PlatformServices" ;;
    WalletFeatures)          echo "Foundation Observation WalletRuntime AppServices PlatformServices DesignTokens" ;;
    DesignTokens)            echo "Foundation" ;;
    DashUIMac)               echo "Foundation SwiftUI DashUIKit DesignTokens" ;;
    MacUI)                   echo "Foundation SwiftUI DashUIMac WalletFeatures PlatformServicesMac" ;;
    DashUICross)             echo "Foundation SwiftCrossUI DesignTokens" ;;
    CrossUI)                 echo "Foundation SwiftCrossUI DashUICross WalletFeatures PlatformServicesDesktop" ;;
    DashWalletCross)         echo "Foundation SwiftCrossUI DefaultBackend CrossUI DashUICross WalletFeatures WalletRuntime AppServices PlatformServices PlatformServicesDesktop DashKit DesignTokens" ;;
    RepoChecksTests)         echo "Foundation Testing" ;;
    *)                       return 1 ;;
  esac
}

# Prints "<file>:<line>:<module>" for every import declaration in a target dir.
# Handles attributes with or without arguments (@testable, @preconcurrency,
# @_spi(Name), @_implementationOnly),
# access modifiers (public/internal/package/...), and kind imports
# (`import struct Foundation.URL`). Submodules count as their top module.
imports_in() {
  local dir="$1"
  find "$dir" -name '*.swift' -print0 | while IFS= read -r -d '' f; do
    awk -v file="$f" '
      {
        line = $0
        sub(/\/\/.*/, "", line)
        if (line !~ /^[ \t]*(@[A-Za-z_]+(\([^)]*\)[ \t]*|[ \t]+))*((public|internal|package|private|fileprivate)[ \t]+)?import[ \t]/) next
        sub(/^.*import[ \t]+/, "", line)
        sub(/^(typealias|struct|class|enum|protocol|let|var|func)[ \t]+/, "", line)
        split(line, parts, /[ \t.;]/)
        if (parts[1] != "") print file ":" NR ":" parts[1]
      }' "$f"
  done
}

status=0
check_target() {
  local dir="$1" allowed="$2"
  local entry file_line module
  while IFS= read -r entry; do
    [[ -n "$entry" ]] || continue
    module="${entry##*:}"
    file_line="${entry%:*}"
    if [[ " $allowed " != *" $module "* ]]; then
      echo "lint-imports: ${file_line#"$root"/} imports '$module', not allowed in $(basename "$dir") (allowed: $allowed)" >&2
      status=1
    fi
  done < <(imports_in "$dir")
}

shopt -s nullglob
for dir in "$root"/Sources/*/; do
  dir="${dir%/}"
  name="$(basename "$dir")"
  if ! allowed="$(allowed_for "$name")"; then
    echo "lint-imports: no allowed-imports entry for Sources/$name; add one to scripts/lint-imports.sh" >&2
    status=1
    continue
  fi
  check_target "$dir" "$allowed"
done

for dir in "$root"/Tests/*/; do
  dir="${dir%/}"
  name="$(basename "$dir")"
  if allowed="$(allowed_for "$name")"; then
    check_target "$dir" "$allowed Testing XCTest"
    continue
  fi
  base="${name%Tests}"
  if [[ "$base" != "$name" ]] && under_test="$(allowed_for "$base")"; then
    check_target "$dir" "Foundation Observation Testing XCTest $base $under_test"
  else
    check_target "$dir" "Foundation Testing XCTest"
  fi
done

if (( status == 0 )); then
  echo "lint-imports: OK"
fi
exit "$status"
