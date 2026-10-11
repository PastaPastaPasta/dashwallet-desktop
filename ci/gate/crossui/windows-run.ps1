# Gate G-UI on windows-latest (ci/gate/crossui/windows.sh): launch dash-wallet on the gate wallet,
# wait for a GateProbe mark, screenshot the screen, optionally keep it running and walk its UI
# Automation tree (the swift-cross-ui#787 walk), then stop it. One line per run is appended to
# results.txt; the UIA walk is written to <Name>-uia.txt.
#
#   pwsh windows-run.ps1 -Name N -OutDir DIR -Exe PATH -Datadir DIR -PassphraseFile FILE -Mark MARK [-Appearance light]
#        [-Unlock] [-OpenDetail] [-Page transactions] [-HoldSeconds 0] [-Walk] [-DropFromPath 'DIR;DIR']
param(
    [Parameter(Mandatory)] [string] $Name,
    [Parameter(Mandatory)] [string] $OutDir,
    [Parameter(Mandatory)] [string] $Exe,
    [Parameter(Mandatory)] [string] $Datadir,
    [Parameter(Mandatory)] [string] $PassphraseFile,
    [Parameter(Mandatory)] [string] $Mark,
    [string] $Appearance = 'light',
    [switch] $Unlock,
    [switch] $OpenDetail,
    [string] $Page = '',
    [int] $HoldSeconds = 0,
    [switch] $Walk,
    # Directories to drop from PATH (';'-separated; the toolchain's runtime, GTK's prefix), as well
    # as any with "Swift" in it: the archive must carry its runtime.
    [string] $DropFromPath = '',
    [int] $TimeoutSeconds = 240
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$results = Join-Path $OutDir 'results.txt'
function Result([string] $line) { Write-Host $line; Add-Content -Path $results -Value $line }
function ExitHex { '0x{0:X8}' -f $process.ExitCode }
function Shot([string] $file) {
    $bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $bitmap = New-Object System.Drawing.Bitmap $bounds.Width, $bounds.Height
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
    $bitmap.Save((Join-Path $OutDir $file), [System.Drawing.Imaging.ImageFormat]::Png)
    $graphics.Dispose(); $bitmap.Dispose()
}

# A fresh copy of the wallet per launch: a stopped run leaves an unclean shutdown behind.
$runData = Join-Path $env:RUNNER_TEMP "gate-run-datadir"
if (Test-Path $runData) { Remove-Item -Recurse -Force $runData }
Copy-Item -Recurse -Path $Datadir -Destination $runData
$argList = @('--network', 'regtest', '--datadir', $runData, '--connect', '127.0.0.1:1',
    '--dapi', 'http://127.0.0.1:1', '--appearance', $Appearance)
if ($Page) { $argList += @('--page', $Page) }
# The gate wallet has no peer: GateProbe hides the sync overlay that would cover the screens.
$environment = @{ DWD_GATE = '1'; DWD_GATE_HIDE_SYNC_OVERLAY = '1' }
if ($Unlock) { $environment.DWD_GATE_UNLOCK_FILE = $PassphraseFile }
if ($OpenDetail) { $environment.DWD_GATE_OPEN_DETAIL = '1' }
$drop = @($DropFromPath -split ';' | Where-Object { $_ } | ForEach-Object { $_.TrimEnd('\', '/') })
$environment.PATH = (($env:PATH -split ';') | Where-Object {
        $_ -and $_ -notmatch 'Swift' -and $drop -notcontains $_.TrimEnd('\', '/') }) -join ';'
$errLog = Join-Path $OutDir "$Name.err.log"
$outLog = Join-Path $OutDir "$Name.out.log"
$process = Start-Process -FilePath $Exe -ArgumentList $argList -Environment $environment -PassThru `
    -RedirectStandardError $errLog -RedirectStandardOutput $outLog
$start = Get-Date
$seen = $false
# Whether $Mark is GateProbe's latest screen (`*-paint`) mark. With -Unlock only marks after the
# unlock count: before the vault reports it is locked, the app can show Overview or Transactions
# (and the sync overlay, which GateProbe hides) and only then the lock screen.
function Shows {
    $text = Get-Content -Raw -Path $errLog -ErrorAction SilentlyContinue
    if (-not $text) { return $false }
    if ($Unlock) {
        $at = $text.IndexOf('gate-mark unlock-requested ')
        if ($at -lt 0) { return $false }
        $text = $text.Substring($at)
    }
    $paints = [regex]::Matches($text, '(?m)^gate-mark (\S+-paint) ')
    return $paints.Count -gt 0 -and $paints[$paints.Count - 1].Groups[1].Value -eq $Mark
}
while (((Get-Date) - $start).TotalSeconds -lt $TimeoutSeconds -and -not $process.HasExited) {
    if (Shows) { $seen = $true; break }
    Start-Sleep -Milliseconds 500
}
$waited = [math]::Round(((Get-Date) - $start).TotalSeconds, 1)
$status = 0
if (-not $seen) {
    Shot "$Name.png"
    if ($process.HasExited) { Result "${Name}: FAIL exited ($(ExitHex)) before '$Mark' after ${waited}s" }
    else { Result "${Name}: FAIL no '$Mark' in ${waited}s" }
    $status = 1
} else {
    Start-Sleep -Seconds 3
    if (-not (Shows)) {
        Result "${Name}: WARN the screen changed after '$Mark'"
    }
    Shot "$Name.png"
    $process.Refresh()
    Result "${Name}: ok '$Mark' after ${waited}s; working set $([int]($process.WorkingSet64 / 1MB)) MB"
    # Every DLL the app loaded comes from the archive or from Windows (WinUI's runtime is a
    # system package under WindowsApps), so a DLL missing from the archive cannot hide on PATH.
    $archive = Split-Path -Parent $Exe
    try {
        $foreign = @($process.Modules | ForEach-Object { $_.FileName } | Where-Object {
                -not $_.StartsWith($archive, 'OrdinalIgnoreCase') -and
                -not $_.StartsWith($env:SystemRoot, 'OrdinalIgnoreCase') -and $_ -notmatch '\\WindowsApps\\' })
    } catch {
        $foreign = @("(modules not listed: $($_.Exception.Message))")
    }
    if ($foreign.Count -gt 0) {
        Result "${Name}: FAIL loaded from outside the archive: $($foreign -join ', ')"
        $status = 1
    }
    if ($Walk -and -not $process.HasExited) {
        # The swift-cross-ui#787 walk: every descendant of the main window, with its control type
        # and name. The app crashing during the walk is the #787 failure.
        Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
        $walkOut = Join-Path $OutDir "$Name-uia.txt"
        try {
            $process.Refresh()
            $root = [System.Windows.Automation.AutomationElement]::FromHandle($process.MainWindowHandle)
            $all = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants,
                [System.Windows.Automation.Condition]::TrueCondition)
            $lines = foreach ($e in $all) {
                $c = $e.Current
                "{0}`t{1}`t'{2}'`tfocusable={3}" -f $c.ControlType.ProgrammaticName, $c.ClassName, $c.Name, $c.IsKeyboardFocusable
            }
            $lines | Set-Content $walkOut
            $interactive = @($all | Where-Object { $_.Current.IsKeyboardFocusable -or $_.Current.ControlType.ProgrammaticName -match 'Button|Edit|List|CheckBox|ComboBox|Hyperlink|Tab|MenuItem' })
            $unnamed = @($interactive | Where-Object { -not $_.Current.Name })
            $verdict = if ($interactive.Count -gt 0 -and $unnamed.Count -eq 0) { 'ok' } else { 'FAIL' }
            Result "${Name}: UIA walk $verdict $($all.Count) elements, $($interactive.Count) interactive, $($unnamed.Count) unnamed"
            if ($verdict -eq 'FAIL') { $status = 1 }
        } catch {
            Result "${Name}: UIA walk FAIL $($_.Exception.GetType().Name): $($_.Exception.Message)"
            $status = 1
        }
        Start-Sleep -Seconds 5
        $process.Refresh()
        if ($process.HasExited) {
            Result "${Name}: FAIL the app exited during the UIA walk ($(ExitHex))"
            $status = 1
        }
    }
    if ($HoldSeconds -gt 0 -and -not $process.HasExited) {
        $deadline = $start.AddSeconds($HoldSeconds)
        $peak = 0
        while ((Get-Date) -lt $deadline -and -not $process.HasExited) {
            Start-Sleep -Seconds 15
            $process.Refresh()
            if (-not $process.HasExited) { $peak = [math]::Max($peak, $process.WorkingSet64) }
        }
        $held = [math]::Round(((Get-Date) - $start).TotalSeconds)
        if ($process.HasExited) {
            Result "${Name}: FAIL exited ($(ExitHex)) after ${held}s"
            $status = 1
        } else {
            Shot "$Name-end.png"
            Result "${Name}: alive after ${held}s; peak working set $([int]($peak / 1MB)) MB"
        }
    }
}
if (-not $process.HasExited) { Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue }
$process.WaitForExit(10000) | Out-Null
exit $status
