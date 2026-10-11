#!/usr/bin/env bash
# Gate G-UI on windows-latest (ci/gate/swiftcrossui.sh): SwiftCrossUI's dash-wallet with WinUIBackend.
#   U2  build; an archive (zip) with the Swift runtime, unpacked and started with the toolchain off
#       PATH; open the gate wallet; Overview renders; no crash in 10 minutes
#   U3  the four screens in light and dark (whole-screen captures)
#   U4  the swift-cross-ui#787 UI Automation walk on Overview and on Transactions
#   U7  the same archive unpacks and launches
# Runs in Git Bash; the launches go through windows-run.ps1 (pwsh). GateProbe's hooks unlock the
# vault and open a transaction's details (no AT-SPI here).
# With DWD_CROSS_BACKEND=gtk (swiftcrossui.sh --backend gtk) the app is built on GtkBackend with
# gvsbuild's GTK 4 (MSVC), whose DLLs go into the archive, instead of WinUIBackend.
set -euo pipefail
: "${GATE_ROOT:?}" "${GATE_OUT:?}" "${GATE_WORK:?}" "${GATE_WALLET:?}" "${GATE_PASSPHRASE_FILE:?}"
cd "$GATE_ROOT"
OUT="$GATE_OUT"
results="$OUT/results.txt"
: >"$results"
result() { echo "$*" | tee -a "$results"; }
win() { cygpath -w "$1"; }

backend=winui
if [[ "${DWD_CROSS_BACKEND:-}" == gtk ]]; then
  backend=gtk
  # gvsbuild's GTK 4 release, unpacked at the prefix its .pc files name.
  gtk_zip="$GATE_WORK/GTK4_Gvsbuild_2026.8.0_x64.zip"
  curl -fsSL -o "$gtk_zip" https://github.com/wingtk/gvsbuild/releases/download/2026.8.0/GTK4_Gvsbuild_2026.8.0_x64.zip
  echo "1f95a92d037f5292da05e6ab1037032ff21ddb7b20d4ac8e83e3674c864c07b0  $gtk_zip" | sha256sum -c -
  gtk_prefix=/c/gtk-build/gtk/x64/release
  mkdir -p "$gtk_prefix"
  7z x -y -bd -o"$(win "$gtk_prefix")" "$(win "$gtk_zip")" >/dev/null
  # SwiftPM's pkg-config parser reads any line with a ':' as a "Key: value" field, so the drive
  # letter in "prefix=C:/..." left ${prefix} undefined (and no include paths). The prefix is
  # relative to pcfiledir instead, which pkgconf reads too.
  sed -i 's|C:/gtk-build/gtk/x64/release|${pcfiledir}/../..|g; s/\r$//' "$gtk_prefix"/lib/pkgconfig/*.pc
  export PKG_CONFIG_PATH="$(win "$gtk_prefix/lib/pkgconfig")"
  export PATH="$gtk_prefix/bin:$PATH"
  result "GTK: gvsbuild 2026.8.0, GTK $(pkgconf --modversion gtk4)"
else
# WinUIBackend needs the Windows App Runtime 1.5 (as in ci.yml's windows-swift job, same pin).
pwsh -NoProfile -Command '
  $url = "https://download.microsoft.com/download/4f5a8b87-f017-485d-8cf7-c16e82671dbe/WindowsAppRuntimeInstall-x64.exe"
  $sha256 = "7418DC8DC18CFEE1CDC1B087A326EA6FD66E2E3A5E1C76D49FA948737A83A45E"
  $installer = Join-Path $env:RUNNER_TEMP "WindowsAppRuntimeInstall-x64.exe"
  Invoke-WebRequest -Uri $url -OutFile $installer
  $hash = (Get-FileHash $installer -Algorithm SHA256).Hash
  if ($hash -ne $sha256) { throw "Windows App Runtime installer SHA-256 $hash, expected $sha256" }
  $signature = Get-AuthenticodeSignature $installer
  if ($signature.Status -ne "Valid" -or $signature.SignerCertificate.Subject -notmatch "^CN=Microsoft Corporation,") {
    throw "Windows App Runtime installer signature: $($signature.Status)"
  }
  & $installer --quiet --force
  if ($LASTEXITCODE -ne 0) { throw "Windows App Runtime installer exited with $LASTEXITCODE" }'
fi

# --- build (release, WinUIBackend) ---
t0=$(date +%s)
scripts/build-core.sh --profile release --no-bindings
t1=$(date +%s)
swift build -c release --product dash-wallet
t2=$(date +%s)
bin_dir=$(swift build -c release --show-bin-path)
result "build ($backend): core $((t1 - t0)) s, swift $((t2 - t1)) s (release)"
# The main thread's stack reserve from the PE header (Package.swift links /STACK:8388608).
stack=$(python -c 'import struct, sys; d = open(sys.argv[1], "rb").read(); pe = struct.unpack_from("<I", d, 0x3C)[0]; print(struct.unpack_from("<Q", d, pe + 24 + 72)[0])' "$(win "$bin_dir/dash-wallet.exe")")
result "stack reserve: $stack bytes"

# --- U2/U7: the archive: the exe, SwiftPM's resource bundles, any DLLs the build put next to it,
# and the Swift runtime's DLLs (the toolchain's runtime directory, found through swiftCore.dll).
runtime_dir=$(dirname "$(cygpath -u "$(where.exe swiftCore.dll | head -1 | tr -d '\r')")")
# The launches run without these on PATH: the archive must carry what they hold.
drop_from_path="$(win "$runtime_dir")${gtk_prefix:+;$(win "$gtk_prefix/bin")}"
stage="$GATE_WORK/archive/dash-wallet"
rm -rf "$GATE_WORK/archive" && mkdir -p "$stage"
cp "$bin_dir/dash-wallet.exe" "$stage/"
cp "$bin_dir"/*.dll "$stage/" 2>/dev/null || true
for bundle in "$bin_dir"/*.resources; do [[ -e "$bundle" ]] && cp -R "$bundle" "$stage/"; done
cp "$runtime_dir"/*.dll "$stage/"
if [[ $backend == gtk ]]; then
  cp "$gtk_prefix"/bin/*.dll "$stage/"
  mkdir -p "$stage/share/glib-2.0"
  cp -R "$gtk_prefix/share/glib-2.0/schemas" "$stage/share/glib-2.0/"
  zip="$OUT/dash-wallet-windows-gtk-x86_64.zip"
else
  zip="$OUT/dash-wallet-windows-x86_64.zip"
fi
pwsh -NoProfile -Command "Compress-Archive -Path '$(win "$stage")' -DestinationPath '$(win "$zip")' -Force"
unpacked="$GATE_WORK/unpacked"
rm -rf "$unpacked"
pwsh -NoProfile -Command "Expand-Archive -Path '$(win "$zip")' -DestinationPath '$(win "$unpacked")'"
exe="$unpacked/dash-wallet/dash-wallet.exe"
# The build directory's bundles must not stand in for missing ones (SwiftPM's accessor falls back
# to the build path).
for bundle in "$bin_dir"/*.resources; do [[ -e "$bundle" ]] && mv "$bundle" "$bundle.moved"; done
result "U7 zip: $(stat -c %s "$zip") bytes, $(ls "$stage" | wc -l) entries ($(ls "$stage"/*.dll | wc -l) DLLs from $runtime_dir and the build)"

# Crash dumps (WER LocalDumps): a crash leaves a minidump; its stack is printed at the end
# (docs/ci.md, known gap 1: the debug build's launch died with 0xC00000FD).
dumps="$GATE_WORK/dumps"
mkdir -p "$dumps"
pwsh -NoProfile -Command "
  \$key = 'HKLM:\SOFTWARE\Microsoft\Windows\Windows Error Reporting\LocalDumps\dash-wallet.exe'
  New-Item -Path \$key -Force | Out-Null
  Set-ItemProperty -Path \$key -Name DumpFolder -Value '$(win "$dumps")' -Type ExpandString
  Set-ItemProperty -Path \$key -Name DumpType -Value 1 -Type DWord
  Set-ItemProperty -Path \$key -Name DumpCount -Value 5 -Type DWord"

status=0
run() { # NAME MARK [windows-run.ps1 switches...]
  local name=$1 mark=$2
  shift 2
  pwsh -NoProfile -File ci/gate/crossui/windows-run.ps1 -Name "$name" -OutDir "$(win "$OUT")" -Exe "$(win "$exe")" \
    -Datadir "$(win "$GATE_WALLET")" -PassphraseFile "$(win "$GATE_PASSPHRASE_FILE")" -Mark "$mark" -DropFromPath "$drop_from_path" "$@" || status=1
}

# U2: unlock, Overview, then keep it running for 10 minutes (600 s from launch).
run u2-overview overview-paint -Unlock -HoldSeconds 600
# U4: the #787 walks (the lock screen needs no unlock).
run u4-unlock lock-paint -Walk
run u4-overview overview-paint -Unlock -Walk
run u4-transactions transactions-paint -Unlock -Page transactions -Walk
# U3: the four screens, light and dark.
for appearance in light dark; do
  run "$appearance-unlock" lock-paint -Appearance "$appearance"
  run "$appearance-overview" overview-paint -Appearance "$appearance" -Unlock
  run "$appearance-transactions" transactions-paint -Appearance "$appearance" -Unlock -Page transactions
  run "$appearance-detail" detail-paint -Appearance "$appearance" -Unlock -OpenDetail -Page transactions
done
# The stack of every crash (cdb from the runner's Windows SDK; symbols from the build's PDBs).
cdb="/c/Program Files (x86)/Windows Kits/10/Debuggers/x64/cdb.exe"
# WER wrote no dump for the stack overflows (0xC00000FD) of run 38114456775, so one unlock to
# Overview runs again under cdb, which prints the stack at the first stack overflow ("sov").
if [[ -x "$cdb" ]] && grep -q "0xC00000FD" "$results"; then
  datadir="$GATE_WORK/cdb-datadir"
  rm -rf "$datadir" && cp -R "$GATE_WALLET" "$datadir"
  DWD_GATE=1 DWD_GATE_UNLOCK_FILE="$(win "$GATE_PASSPHRASE_FILE")" DWD_GATE_HIDE_SYNC_OVERLAY=1 \
    timeout 600 "$cdb" -y "$(win "$bin_dir")" -c 'sxe -c "kn 250; !uniqstack; q" sov; g' \
    "$(win "$exe")" --network regtest --datadir "$(win "$datadir")" --connect 127.0.0.1:1 \
    --dapi http://127.0.0.1:1 >"$OUT/overview-cdb.txt" 2>&1 || true
  result "overview under cdb: $(wc -l <"$OUT/overview-cdb.txt") lines in overview-cdb.txt"
fi
for dump in "$dumps"/*.dmp; do
  [[ -e "$dump" ]] || continue
  cp "$dump" "$OUT/"
  if [[ -x "$cdb" ]]; then
    "$cdb" -z "$(win "$dump")" -y "$(win "$bin_dir")" -i "$(win "$bin_dir")" \
      -c ".ecxr; kn 60; !uniqstack; q" >"$OUT/$(basename "$dump" .dmp)-stack.txt" 2>&1 || true
    result "crash dump $(basename "$dump"): stack in $(basename "$dump" .dmp)-stack.txt"
  else
    result "crash dump $(basename "$dump") (no cdb on the runner)"
  fi
done
cat "$results"
exit "$status"
