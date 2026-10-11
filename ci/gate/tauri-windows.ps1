# Gate G-UI on windows-latest, the Tauri candidate (G-02); called by ci/gate/tauri.sh.
#
#   powershell.exe -NoProfile -ExecutionPolicy Bypass -File tauri-windows.ps1 -Repo R -OutDir O -App EXE
#       -Wallet DIR -PassphraseFile F [-PauseMs 2500] [-RunSeconds 600] [-WindowSize 1000x620]
#
# 1. U3/U8-style shots: --selftest on the fixture, the window captured on each screen.
# 2. --selftest on the copied regtest wallet (U1 rows, real history_page timings).
# 3. U7/U2: the NSIS installer (/S) and the MSI (msiexec /qn) install; the MSI's app launches.
# 4. U4: uia_walk.ps1 over the NSIS-installed app (fixture).
# 5. U2: the NSIS-installed app opens the copied wallet, is unlocked with keystrokes, shows
#    Overview, and runs for RunSeconds; it must not exit. Memory of its process tree is sampled.
# Writes windows-gate.json (and the files named above) to OutDir; exit 1 on any failure.
param(
    [Parameter(Mandatory)] [string] $Repo,
    [Parameter(Mandatory)] [string] $OutDir,
    [Parameter(Mandatory)] [string] $App,
    [Parameter(Mandatory)] [string] $Wallet,
    [Parameter(Mandatory)] [string] $PassphraseFile,
    [int] $PauseMs = 2500,
    [string] $WindowSize = '1000x620',
    [int] $RunSeconds = 600
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class Win {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
}
'@
# Not $shots: PowerShell names are case-insensitive, and a function's $Shots parameter would hide it.
$shotDir = Join-Path $OutDir 'shots'
New-Item -ItemType Directory -Force -Path $shotDir | Out-Null
$result = [ordered]@{}
$failed = New-Object System.Collections.ArrayList
function Fail([string] $What) { Write-Host "::error::$What"; [void]$failed.Add($What) }

# The window of the process (falls back to the whole screen) as a PNG.
function Shot($Process, [string] $File) {
    $Process.Refresh()
    $h = $Process.MainWindowHandle
    $r = New-Object Win+RECT
    if ($h -ne [IntPtr]::Zero -and [Win]::GetWindowRect($h, [ref]$r) -and ($r.Right - $r.Left) -gt 50) {
        $x = $r.Left; $y = $r.Top; $w = $r.Right - $r.Left; $hh = $r.Bottom - $r.Top
    } else {
        $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
        $x = $b.X; $y = $b.Y; $w = $b.Width; $hh = $b.Height
    }
    # A failed capture is evidence missing, not a failed run: warn and go on.
    try {
        $bmp = New-Object System.Drawing.Bitmap $w, $hh
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $g.CopyFromScreen($x, $y, 0, 0, (New-Object System.Drawing.Size $w, $hh))
        $bmp.Save($File, [System.Drawing.Imaging.ImageFormat]::Png)
        $g.Dispose(); $bmp.Dispose()
    } catch { Write-Host "::warning::screenshot $File ($w x $hh at $x,$y): $($_.Exception.Message)" }
}

function Start-App([string] $Exe, [string[]] $Arguments, [string] $Name) {
    $p = Start-Process -FilePath $Exe -ArgumentList $Arguments -PassThru `
        -RedirectStandardOutput (Join-Path $OutDir "$Name.out.log") -RedirectStandardError (Join-Path $OutDir "$Name.log")
    Write-Host "${Name}: started $Exe $Arguments (pid $($p.Id))"
    return $p
}

function Wait-Window($Process, [int] $Seconds) {
    for ($i = 0; $i -lt $Seconds * 4; $i++) {
        if ($Process.HasExited) { return $false }
        $Process.Refresh()
        if ($Process.MainWindowHandle -ne [IntPtr]::Zero) { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}

# Working set and private bytes (MB) of the process and its descendants (msedgewebview2.exe).
function Tree-Memory([int] $RootId) {
    $all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, WorkingSetSize, PrivatePageCount
    $ids = @($RootId); $i = 0
    while ($i -lt $ids.Count) { $ids += @($all | Where-Object { $_.ParentProcessId -eq $ids[$i] } | ForEach-Object { $_.ProcessId }); $i++ }
    $procs = @($all | Where-Object { $ids -contains $_.ProcessId })
    return [ordered]@{
        processes       = $procs.Count
        workingSetMB    = [math]::Round((($procs | Measure-Object WorkingSetSize -Sum).Sum) / 1MB, 1)
        privateBytesMB  = [math]::Round((($procs | Measure-Object PrivatePageCount -Sum).Sum) / 1MB, 1)
    }
}

# 1. --selftest on the fixture with a window capture per screen.
function Selftest-WithShots([string[]] $Arguments, [string] $Report, [string] $Name, [bool] $WithShots) {
    $log = Join-Path $OutDir "$Name.log"
    $p = Start-App $App ($Arguments + @('--window-size', $WindowSize, '--selftest', $Report, '--selftest-pause', "$PauseMs")) $Name
    $seen = 0
    $deadline = (Get-Date).AddSeconds(900)
    while (-not $p.HasExited -and (Get-Date) -lt $deadline) {
        if ($WithShots) {
            $screens = @(Select-String -Path $log -Pattern 'selftest-screen ([A-Za-z/]+)' -AllMatches -ErrorAction SilentlyContinue |
                ForEach-Object { $_.Matches } | ForEach-Object { $_.Groups[1].Value })
            while ($seen -lt $screens.Count) {
                Start-Sleep -Milliseconds 800
                Shot $p (Join-Path $shotDir ("windows-" + ($screens[$seen] -replace '/', '-') + '.png'))
                $seen++
            }
        }
        Start-Sleep -Milliseconds 200
    }
    if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force; Fail "${Name}: did not finish in 900 s" }
    elseif (-not (Test-Path $Report)) { Fail "${Name}: wrote no report (exit $($p.ExitCode))" }
    return $seen
}

$fixture = @('--fixture', '--network', 'testnet')
$engine = @('--datadir', $Wallet, '--network', 'regtest', '--no-platform', '--dapi', 'https://127.0.0.1:1443')
$result.selftestShots = Selftest-WithShots $fixture (Join-Path $OutDir 'selftest-windows.json') 'selftest-windows' $true
Selftest-WithShots $engine (Join-Path $OutDir 'selftest-windows-engine.json') 'selftest-windows-engine' $false | Out-Null

# 3. Installers.
$bundle = Join-Path $Repo 'rust\target\release\bundle'
$nsis = Get-ChildItem (Join-Path $bundle 'nsis') -Filter '*-setup.exe' | Select-Object -First 1
$msi = Get-ChildItem (Join-Path $bundle 'msi') -Filter '*.msi' | Select-Object -First 1
$result.nsis = [ordered]@{ file = $nsis.Name; bytes = $nsis.Length }
$result.msi = [ordered]@{ file = $msi.Name; bytes = $msi.Length }

$t = Measure-Command { $ip = Start-Process -FilePath $nsis.FullName -ArgumentList '/S' -Wait -PassThru }
$result.nsis.exitCode = $ip.ExitCode
$result.nsis.seconds = [math]::Round($t.TotalSeconds, 1)
$installed = Get-ChildItem -Path "$env:LOCALAPPDATA\Dash Wallet", "$env:ProgramFiles\Dash Wallet" -Filter 'dash-wallet.exe' -ErrorAction SilentlyContinue |
    Select-Object -First 1
if ($ip.ExitCode -ne 0 -or $null -eq $installed) { Fail "NSIS install failed (exit $($ip.ExitCode))" }
$result.nsis.installedExe = "$($installed.FullName)"
Write-Host "NSIS: installed $($installed.FullName) in $($result.nsis.seconds) s"

$msiLog = Join-Path $OutDir 'msi-install.log'
$t = Measure-Command { $mp = Start-Process msiexec.exe -ArgumentList '/i', "`"$($msi.FullName)`"", '/qn', '/norestart', '/l*v', "`"$msiLog`"" -Wait -PassThru }
$result.msi.exitCode = $mp.ExitCode
$result.msi.seconds = [math]::Round($t.TotalSeconds, 1)
# Where the MSI put the app: INSTALLDIR defaults to Program Files, but the MSI reuses the
# InstallDir the NSIS installer left in the registry, so read the target from the log.
$msiDir = Select-String -Path $msiLog -Pattern 'Dir \(target\): Key: INSTALLDIR\s*, Object: (.+)$' -ErrorAction SilentlyContinue |
    Select-Object -Last 1 | ForEach-Object { $_.Matches[0].Groups[1].Value.Trim() }
if (-not $msiDir) { $msiDir = Join-Path $env:ProgramFiles 'Dash Wallet' }
$msiExe = Join-Path $msiDir 'dash-wallet.exe'
$result.msi.installedExe = $msiExe
if ($mp.ExitCode -ne 0 -or -not (Test-Path $msiExe)) { Fail "MSI install failed (exit $($mp.ExitCode))" }
else {
    $p = Start-App $msiExe $fixture 'msi-app'
    $result.msi.window = Wait-Window $p 60
    Start-Sleep -Seconds 5
    if ($result.msi.window -and -not $p.HasExited) { Shot $p (Join-Path $shotDir 'windows-msi-app.png') } else { Fail 'the MSI-installed app shows no window' }
    Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
}

# 4. UIA walk over the installed app (fixture).
if ($installed) {
    $p = Start-App $installed.FullName $fixture 'uia-app'
    Wait-Window $p 60 | Out-Null
    Start-Sleep -Seconds 3
    $walk = Join-Path $Repo 'apps\desktop\scripts\gate\uia_walk.ps1'
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $walk -ProcessId $p.Id -OutDir (Join-Path $OutDir 'uia')
    $result.uiaExit = $LASTEXITCODE
    if ($LASTEXITCODE -ne 0) { Fail "UIA walk: exit $LASTEXITCODE" }
    Shot $p (Join-Path $shotDir 'windows-uia-end.png')
    Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
}

# 5. U2: the installed app on the copied wallet for RunSeconds.
if ($installed) {
    $u2 = [ordered]@{ samples = @() }
    $t0 = Get-Date
    $p = Start-App $installed.FullName $engine 'u2-app'
    $u2.window = Wait-Window $p 120
    $u2.windowSeconds = [math]::Round(((Get-Date) - $t0).TotalSeconds, 1)
    Start-Sleep -Seconds 10
    Shot $p (Join-Path $shotDir 'windows-wallet-locked.png')
    $p.Refresh()
    [Win]::ShowWindow($p.MainWindowHandle, 9) | Out-Null
    [Win]::SetForegroundWindow($p.MainWindowHandle) | Out-Null
    Start-Sleep -Milliseconds 500
    # The passphrase field has focus on the lock screen; SendKeys needs its specials escaped.
    $pass = (Get-Content $PassphraseFile -TotalCount 1) -replace '([+^%~(){}\[\]])', '{$1}'
    [System.Windows.Forms.SendKeys]::SendWait($pass)
    [System.Windows.Forms.SendKeys]::SendWait('{ENTER}')
    Start-Sleep -Seconds 20
    Shot $p (Join-Path $shotDir 'windows-wallet-overview.png')
    while (((Get-Date) - $t0).TotalSeconds -lt $RunSeconds -and -not $p.HasExited) {
        $sample = Tree-Memory $p.Id
        $sample.t = [int]((Get-Date) - $t0).TotalSeconds
        $u2.samples += , $sample
        Start-Sleep -Seconds 30
    }
    $u2.ranSeconds = [int]((Get-Date) - $t0).TotalSeconds
    $u2.exited = $p.HasExited
    if ($p.HasExited) { $u2.exitCode = $p.ExitCode; Fail "U2: the app exited after $($u2.ranSeconds) s (code $($p.ExitCode))" }
    else {
        Shot $p (Join-Path $shotDir 'windows-wallet-10min.png')
        Stop-Process -Id $p.Id -Force
    }
    $result.u2 = $u2
}

$result.failures = @($failed)
$result | ConvertTo-Json -Depth 8 | Set-Content -Encoding UTF8 (Join-Path $OutDir 'windows-gate.json')
Get-Content (Join-Path $OutDir 'windows-gate.json')
exit ([int]($failed.Count -gt 0))
