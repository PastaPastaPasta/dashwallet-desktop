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

# On a hosted runner the agent's console and system dialogs sit in front: restore the app's own
# window and make it topmost, so the screen shows it.
Add-Type -Namespace Win -Name User32 -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool ShowWindow(System.IntPtr h, int cmd);
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(System.IntPtr h);
[DllImport("user32.dll")] public static extern bool SetWindowPos(System.IntPtr h, System.IntPtr after, int x, int y, int cx, int cy, uint flags);
'@
for ($i = 0; $i -lt 15 -and -not $process.HasExited; $i++) {
    $process.Refresh()
    if ($process.MainWindowHandle -ne [System.IntPtr]::Zero) { break }
    Start-Sleep -Seconds 1
}
if ($process.HasExited) {
    Write-Host "::error::$Name exited while the screen was prepared (code $($process.ExitCode))"
    exit 1
}
$handle = $process.MainWindowHandle
Write-Host "${Name}: main window handle $handle, title '$($process.MainWindowTitle)'"
if ($handle -eq [System.IntPtr]::Zero) {
    Write-Host "::warning::${Name}: the process has no main window"
} else {
    [void][Win.User32]::ShowWindow($handle, 9)                                    # SW_RESTORE
    [void][Win.User32]::SetWindowPos($handle, [System.IntPtr](-1), 0, 0, 0, 0, 0x43)  # HWND_TOPMOST, no move/size, show
    [void][Win.User32]::SetForegroundWindow($handle)
}
Start-Sleep -Seconds 3

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
