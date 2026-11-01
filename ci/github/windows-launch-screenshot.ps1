# GitHub Actions (windows-latest): launch a GUI program, wait, capture the primary screen, and
# fail if the process exits early.
#
#   pwsh ci/github/windows-launch-screenshot.ps1 -Name NAME -OutDir DIR -Seconds N -Exe PATH [-ArgumentList ARGS]
#
# Writes DIR/NAME.png, DIR/NAME.out.log and DIR/NAME.err.log.
param(
    [Parameter(Mandatory)] [string] $Name,
    [Parameter(Mandatory)] [string] $OutDir,
    [int] $Seconds = 20,
    [Parameter(Mandatory)] [string] $Exe,
    [string[]] $ArgumentList = @()
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
Add-Type -AssemblyName System.Windows.Forms, System.Drawing

$start = @{
    FilePath               = $Exe
    PassThru               = $true
    RedirectStandardOutput = (Join-Path $OutDir "$Name.out.log")
    RedirectStandardError  = (Join-Path $OutDir "$Name.err.log")
}
if ($ArgumentList.Count -gt 0) { $start.ArgumentList = $ArgumentList }
$process = Start-Process @start
Write-Host "${Name}: launched $Exe $ArgumentList (pid $($process.Id)); waiting ${Seconds}s"
for ($i = 0; $i -lt $Seconds -and -not $process.HasExited; $i++) { Start-Sleep -Seconds 1 }

if ($process.HasExited) {
    Get-Content (Join-Path $OutDir "$Name.err.log") -Tail 40 -ErrorAction SilentlyContinue
    Write-Host "::error::$Name exited early with code $($process.ExitCode)"
    exit 1
}

# The runner agent's console sits in front on a hosted runner: minimize every window, then bring
# the app's own window back (AppActivate restores a minimized window).
Add-Type -AssemblyName Microsoft.VisualBasic
(New-Object -ComObject Shell.Application).MinimizeAll()
Start-Sleep -Seconds 1
try { [Microsoft.VisualBasic.Interaction]::AppActivate($process.Id) } catch { Write-Host "::warning::${Name}: could not activate the app window: $_" }
Start-Sleep -Seconds 3
if ($process.HasExited) {
    Write-Host "::error::$Name exited while the screen was prepared (code $($process.ExitCode))"
    exit 1
}

$bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$bitmap = New-Object System.Drawing.Bitmap $bounds.Width, $bounds.Height
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
$png = Join-Path $OutDir "$Name.png"
$bitmap.Save($png, [System.Drawing.Imaging.ImageFormat]::Png)
$graphics.Dispose(); $bitmap.Dispose()
$process.Refresh()
Write-Host "${Name}: screenshot $png ($($bounds.Width)x$($bounds.Height)); working set $([int]($process.WorkingSet64 / 1MB)) MB"
Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
