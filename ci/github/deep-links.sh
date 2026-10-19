#!/usr/bin/env bash
# Deep links through the OS (U-01): the packaged app registers dash:, pay:, dashwallet: and
# dashpay: (tauri.conf.json plugins.deep-link.desktop.schemes, written by the bundler into the
# .desktop file, Info.plist or the NSIS installer's registry keys), and a link the OS opens
# reaches the running app and is routed.
#
#   ci/github/deep-links.sh linux|macos|windows <app bundle or binary>
#
#   linux    the .deb: extracted, its .desktop file checked and registered in a private
#            XDG_DATA_HOME/XDG_CONFIG_HOME, links opened with xdg-open. Run it under
#            `dbus-run-session -- xvfb-run -a` (the single-instance plugin's D-Bus name).
#   macos    the .app: copied to ~/Applications, registered with LaunchServices, links opened
#            with `open` (the app gets them as `RunEvent::Opened`).
#   windows  the NSIS installer: installed silently per user (HKCU\Software\Classes), links
#            opened with `cmd /c start` (a second launch that forwards them).
#
# The app must be built with the `selftest` feature (`--selftest-link-trace`). It is launched
# once with a link argument on regtest; the script waits for each step in the link trace:
# `received <source> <scheme>` from the shell and `routed <route>` from the renderer
# (dw-app shell/links.rs, apps/desktop/src/app/shellBridge.ts). The data root is the platform
# default, as for a link the OS opens: the opened launch has no `--datadir`, and only a launch
# on the same data root hands its link over (U-01 D2).
#
# CI only on macOS and Windows: there it replaces ~/Applications/Dash Wallet.app or installs into
# %LOCALAPPDATA%, claims the schemes for the user and uses the default data root. Set
# DW_LINKS_LOCAL=1 to run it anyway. The Linux leg keeps everything under its work directory.
set -euo pipefail

os=${1:?usage: $0 linux|macos|windows <bundle>}
bundle=${2:?usage: $0 linux|macos|windows <bundle>}
if [[ "$os" != linux && -z "${CI:-}" && -z "${DW_LINKS_LOCAL:-}" ]]; then
  echo "deep-links: $os changes the user's apps and URL handlers; CI only (DW_LINKS_LOCAL=1 to override)" >&2
  exit 2
fi
work=${DW_LINKS_WORK:-$(mktemp -d)}
trace="$work/links.trace"
out=${DW_LINKS_OUT:-$work/out}
mkdir -p "$out"
: > "$trace"

# A Base58Check regtest address (version 140, hash160 of twenty 0x11 bytes) and a Base58
# identity id (32 bytes of 7).
ADDRESS=yMsgnH1xKGa85n4bq2imrZbG2KgrmGttAV
IDENTITY=US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx
CHAIN=(--network regtest --no-platform --dapi https://127.0.0.1:1)
TIMEOUT=${DW_LINKS_TIMEOUT:-240}

fail() {
  echo "deep-links: FAIL: $*" >&2
  echo "--- trace" >&2
  cat "$trace" >&2 || true
  echo "--- app log (tail)" >&2
  tail -n 80 "$out/app.log" >&2 || true
  exit 1
}

# Waits until the trace holds `line` at least `count` times.
expect() {
  local line=$1 count=${2:-1} waited=0
  until [[ $(grep -cxF -- "$line" "$trace" || true) -ge $count ]]; do
    ((waited++ < TIMEOUT)) || fail "no \"$line\" (×$count) in the trace after ${TIMEOUT}s"
    sleep 1
  done
  echo "deep-links: ok: $line (×$count)"
}

app_pid=
stop_app() {
  [[ -n "$app_pid" ]] || return 0
  case "$os" in
    windows) kill "$app_pid" 2>/dev/null || taskkill //IM dash-wallet.exe //F >/dev/null 2>&1 || true ;;
    *) kill "$app_pid" 2>/dev/null || true ;;
  esac
  wait "$app_pid" 2>/dev/null || true
}
trap stop_app EXIT

case "$os" in
  linux)
    root="$work/root"
    dpkg-deb -x "$bundle" "$root"
    desktop=$(find "$root/usr/share/applications" -name '*.desktop' | head -n 1)
    [[ -n "$desktop" ]] || fail "no .desktop file in $bundle"
    cp "$desktop" "$out/"
    for scheme in dash pay dashwallet dashpay; do
      grep -qE "^MimeType=(.*;)?x-scheme-handler/$scheme(;|$)" "$desktop" || fail "$desktop does not claim x-scheme-handler/$scheme"
    done
    grep -q '^Exec=.* %u$' "$desktop" || fail "$desktop does not pass the link (%u): $(grep '^Exec=' "$desktop")"
    export XDG_DATA_HOME="$work/share" XDG_CONFIG_HOME="$work/config" HOME="$work/home"
    mkdir -p "$XDG_DATA_HOME/applications" "$XDG_CONFIG_HOME" "$HOME"
    # Registered as `dash-wallet.desktop`: the bundle names the file after productName
    # ("Dash Wallet.desktop"), and xdg-utils' generic path (no desktop environment, as here)
    # splits a desktop-file id at the space (U-01 D21; GNOME and KDE resolve it through gio/kio).
    name=dash-wallet.desktop
    cp "$desktop" "$XDG_DATA_HOME/applications/$name"
    for scheme in dash pay dashwallet dashpay; do
      xdg-mime default "$name" "x-scheme-handler/$scheme"
      [[ $(xdg-mime query default "x-scheme-handler/$scheme") == "$name" ]] || fail "xdg-mime did not take $name for $scheme"
    done
    export PATH="$root/usr/bin:$PATH"
    exe=$(sed -n 's/^Exec=\([^ ]*\).*/\1/p' "$desktop")
    open_link() { xdg-open "$1"; }
    ;;
  macos)
    apps="$HOME/Applications"
    mkdir -p "$apps"
    rm -rf "$apps/Dash Wallet.app"
    ditto "$bundle" "$apps/Dash Wallet.app"
    plist="$apps/Dash Wallet.app/Contents/Info.plist"
    cp "$plist" "$out/"
    for scheme in dash pay dashwallet dashpay; do
      plutil -extract CFBundleURLTypes json -o - "$plist" | grep -q "\"$scheme\"" || fail "Info.plist does not claim $scheme"
    done
    /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$apps/Dash Wallet.app"
    exe="$apps/Dash Wallet.app/Contents/MacOS/dash-wallet"
    open_link() { open "$1"; }
    ;;
  windows)
    "$bundle" //S
    # NSIS installs per user by default, into %LOCALAPPDATA%\<productName>.
    exe="$(cygpath -u "$LOCALAPPDATA")/Dash Wallet/dash-wallet.exe"
    [[ -f "$exe" ]] || fail "the installer put no $exe"
    for scheme in dash pay dashwallet dashpay; do
      reg query "HKCU\\Software\\Classes\\$scheme\\shell\\open\\command" //ve | tee -a "$out/registry.txt" | grep -qi 'dash-wallet.exe" "%1"' \
        || fail "HKCU\\Software\\Classes\\$scheme does not open dash-wallet.exe with the link"
    done
    # Not `cmd /c start`: Git Bash does not quote an argument without spaces, so cmd would
    # split the link at `&`. The URL handler takes the rest of its command line as is.
    open_link() { rundll32 url.dll,FileProtocolHandler "$1"; }
    ;;
  *)
    echo "unknown OS $os" >&2
    exit 2
    ;;
esac

# The first launch: a link argument, as when a click starts the app. The data root is new, so
# the network has no wallet: a payment or contact link is kept in the link stash for after
# onboarding (`routed saved`, DP5-01 D2), and an invitation always is (`routed invitation`).
"$exe" "${CHAIN[@]}" --selftest-link-trace "$trace" "dash:$ADDRESS?amount=0.5&label=Launch" > "$out/app.log" 2>&1 &
app_pid=$!
expect "received launch dash"
expect "routed saved"
expect "routed ready"

# A running app: the OS hands each link to it (macOS: an Apple Event; Linux and Windows: a
# second launch that forwards it and exits).
source=forwarded
[[ "$os" != macos ]] || source=opened
open_link "pay:$ADDRESS?label=Second"
expect "received $source pay"
expect "routed saved" 2
open_link "dashpay://user?id=$IDENTITY&username=bob"
expect "received $source dashpay"
expect "routed saved" 3
open_link "dashpay://invite?du=alice&pk=cDeepLinksPlantedKey&assetlocktx=00&islock=00"
expect "received $source dashpay" 2
expect "routed invitation"
open_link "dashwallet://scanqr"
expect "received $source dashwallet"
expect "routed scanQr"
# A link the wallet reads but refuses (the address is not a regtest one) is still routed.
open_link "dash:XwnLY9Tf7Zsef8gMGL2fhWA9ZmMjt4KPwg"
expect "routed rejected payment invalidAddress"

kill -0 "$app_pid" 2>/dev/null || fail "the app is no longer running"
# DP5-01 D14: no link text in the trace or the app's log.
for secret in "$ADDRESS" "$IDENTITY" cDeepLinksPlantedKey; do
  ! grep -qF -- "$secret" "$trace" "$out/app.log" || fail "a log carries $secret"
done
cp "$trace" "$out/"
echo "deep-links: PASS ($os)"
