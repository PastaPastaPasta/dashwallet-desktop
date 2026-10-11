# Gate U4 on Windows: a UI Automation walk of the running app (DASHPAY §2.1, the #787 walk).
#
#   powershell.exe -NoProfile -ExecutionPolicy Bypass -File uia_walk.ps1 -ProcessId PID -OutDir DIR [-Passphrase demo]
#
# Windows PowerShell 5.1 (.NET Framework), which always has UIAutomationClient. Finds the
# process's top-level window, then for each gate screen dumps the UIA tree (raw view) and counts
# interactive elements without a name. It drives the app through UIA only, as Narrator or a test
# driver would: Invoke on the sidebar's "Transactions", on the first transaction row, on Close and
# on "Lock wallet"; the passphrase goes in with ValuePattern (or keystrokes if that is refused).
# The walk is the counterpart of atspi_walk.py; it names nothing Tauri-specific beyond the
# accessible names of the gate screens' controls, so the SwiftCrossUI app can be walked with it too.
#
# Writes DIR/uia-<screen>.txt and DIR/uia-summary.json. Exit status 0 when every hard check passed
# and the app is still running at the end (a UIA client that crashes the app fails the walk).
param(
    [Parameter(Mandatory)] [int] $ProcessId,
    [Parameter(Mandatory)] [string] $OutDir,
    [string] $Passphrase = 'demo',
    [int] $TimeoutSeconds = 60
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Windows.Forms
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$AE = [System.Windows.Automation.AutomationElement]
$CT = [System.Windows.Automation.ControlType]
$interactiveTypes = @(
    $CT::Button, $CT::CheckBox, $CT::ComboBox, $CT::Edit, $CT::Hyperlink, $CT::ListItem, $CT::MenuItem,
    $CT::RadioButton, $CT::Slider, $CT::Spinner, $CT::SplitButton, $CT::TabItem, $CT::TreeItem
)
$checks = New-Object System.Collections.ArrayList
$summary = [ordered]@{}

function Check([string] $Kind, [string] $What, [bool] $Ok, [string] $Detail = '') {
    $tag = if ($Ok) { 'PASS' } elseif ($Kind -eq 'hard') { 'FAIL' } else { 'GAP ' }
    Write-Host "[$tag] ${Kind}: $What $(if ($Detail) { "-- $Detail" })"
    [void]$checks.Add([ordered]@{ kind = $Kind; what = $What; ok = $Ok; detail = $Detail })
    return $Ok
}

function Alive { return $null -ne (Get-Process -Id $ProcessId -ErrorAction SilentlyContinue) }

function Find-Window {
    $cond = New-Object System.Windows.Automation.PropertyCondition($AE::ProcessIdProperty, $ProcessId)
    return $AE::RootElement.FindFirst([System.Windows.Automation.TreeScope]::Children, $cond)
}

function Info($e) {
    $c = $e.Current
    $patterns = @($e.GetSupportedPatterns() | ForEach-Object { $_.ProgrammaticName -replace 'Identifiers\.Pattern$', '' })
    return [ordered]@{
        type      = $c.ControlType.ProgrammaticName -replace '^ControlType\.', ''
        localized = $c.LocalizedControlType
        name      = $c.Name
        id        = $c.AutomationId
        offscreen = $c.IsOffscreen
        enabled   = $c.IsEnabled
        focusable = $c.IsKeyboardFocusable
        password  = $c.IsPassword
        patterns  = $patterns
        rect      = "$([int]$c.BoundingRectangle.X),$([int]$c.BoundingRectangle.Y) $([int]$c.BoundingRectangle.Width)x$([int]$c.BoundingRectangle.Height)"
        element   = $e
    }
}

# Depth-first walk of the raw view: [(depth, info)].
function Walk($root) {
    $walker = [System.Windows.Automation.TreeWalker]::RawViewWalker
    $out = New-Object System.Collections.ArrayList
    $stack = New-Object System.Collections.Stack
    $stack.Push(@($root, 0))
    while ($stack.Count -gt 0 -and $out.Count -lt 5000) {
        $pair = $stack.Pop()
        $e = $pair[0]; $depth = $pair[1]
        try { [void]$out.Add(@($depth, (Info $e))) } catch { continue }
        $kids = New-Object System.Collections.ArrayList
        try {
            $child = $walker.GetFirstChild($e)
            while ($null -ne $child) { [void]$kids.Add($child); $child = $walker.GetNextSibling($child) }
        } catch { }
        for ($i = $kids.Count - 1; $i -ge 0; $i--) { $stack.Push(@($kids[$i], $depth + 1)) }
    }
    return , $out
}

function Is-Interactive($info) {
    foreach ($t in $interactiveTypes) { if ($info.type -eq ($t.ProgrammaticName -replace '^ControlType\.', '')) { return $true } }
    return $false
}

function Dump([string] $Screen) {
    $win = Find-Window
    if ($null -eq $win) { Check 'hard' "${Screen}: the window is in the UIA tree" $false | Out-Null; return }
    $nodes = Walk $win
    $lines = foreach ($n in $nodes) {
        $i = $n[1]
        ('  ' * $n[0]) + "[$($i.type)/$($i.localized)] '$($i.name)' id=$($i.id) $($i.rect)$(if ($i.offscreen) { ' offscreen' })$(if ($i.password) { ' password' }) {$($i.patterns -join ',')}"
    }
    $lines | Set-Content -Encoding UTF8 (Join-Path $OutDir "uia-$Screen.txt")
    $items = @($nodes | Where-Object { (Is-Interactive $_[1]) -and -not $_[1].offscreen } | ForEach-Object { $_[1] })
    $unnamed = @($items | Where-Object { -not $_.name.Trim() })
    $summary[$Screen] = [ordered]@{
        nodes            = $nodes.Count
        interactive      = $items.Count
        unnamed          = $unnamed.Count
        unnamedDetail    = @($unnamed | ForEach-Object { "$($_.type) $($_.rect) id=$($_.id)" })
        interactiveNames = @($items | ForEach-Object { "$($_.type): $($_.name)" } | Sort-Object -Unique | Select-Object -First 80)
    }
    Write-Host "== dumped $($nodes.Count) nodes to uia-$Screen.txt ($($items.Count) interactive, $($unnamed.Count) unnamed)"
}

# The first on-screen element matching the predicate, polling until the timeout.
# Scriptblocks see variables by dynamic scope: callers pass closures (GetNewClosure), and the
# parameter names differ, so a predicate never resolves a name to this function's own variables.
function Wait-For([scriptblock] $Match, [int] $Seconds = 15) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    do {
        $win = Find-Window
        if ($null -ne $win) {
            foreach ($n in (Walk $win)) { $i = $n[1]; if (-not $i.offscreen -and (& $Match $i)) { return $i } }
        }
        Start-Sleep -Milliseconds 250
    } while ((Get-Date) -lt $deadline)
    return $null
}

function Press([scriptblock] $Want, [string] $Label) {
    $hit = Wait-For ({ param($i) $i.type -in @('Button', 'Hyperlink', 'ListItem', 'MenuItem') -and (& $Want $i) }.GetNewClosure())
    if ($null -eq $hit) { return Check 'hard' "find '$Label'" $false }
    $ok = $false
    try {
        $hit.element.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
        $ok = $true
    } catch { $detail = $_.Exception.Message }
    return Check 'hard' "Invoke '$($hit.name.Substring(0, [Math]::Min(60, $hit.name.Length)))' through UIA" $ok $detail
}

try {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ($null -eq (Find-Window) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 500 }
    $win = Find-Window
    if (Check 'hard' 'the app window is in the UIA tree' ($null -ne $win)) {
        $summary['window'] = $win.Current.Name
        Check 'soft' "the window is named 'Dash Wallet'" ($win.Current.Name -eq 'Dash Wallet') $win.Current.Name | Out-Null

        $content = Wait-For { param($i) $i.type -eq 'Button' -and $i.name -eq 'Transactions' } $TimeoutSeconds
        Check 'hard' 'web content is exposed (Overview controls found)' ($null -ne $content) | Out-Null
        Start-Sleep -Seconds 1
        Dump 'overview'

        Press { param($i) $i.name -eq 'Transactions' } 'Transactions' | Out-Null
        Wait-For { param($i) $i.type -eq 'Edit' -and $i.name -like '*Search*' } | Out-Null
        Start-Sleep -Seconds 1
        Dump 'transactions'

        # Rows are buttons named by their content ("Received 1:40 AM Pending +0.20 tDASH").
        Press { param($i) $i.name -match 'DASH' -and $i.name -match '\d' -and $i.name -notmatch '^Click' } 'first transaction row' | Out-Null
        # PowerShell's -and and -or bind equally (left to right): keep the parentheses.
        $dialog = Wait-For { param($i) ($i.localized -match 'dialog') -or ($i.type -eq 'Window' -and $i.name -eq 'Transaction details') }
        if (Check 'hard' 'the detail sheet is a dialog' ($null -ne $dialog)) {
            Check 'soft' 'the dialog is named' ([bool]$dialog.name) $dialog.name | Out-Null
        }
        Start-Sleep -Seconds 1
        Dump 'txDetail'
        Press { param($i) $i.name -eq 'Close' } 'Close' | Out-Null
        Start-Sleep -Seconds 1

        Press { param($i) $i.name -eq 'Lock wallet' } 'Lock wallet' | Out-Null
        $field = Wait-For { param($i) $i.type -eq 'Edit' -and $i.name -eq 'Passphrase' }
        Check 'hard' 'the passphrase field is named' ($null -ne $field) | Out-Null
        Start-Sleep -Seconds 1
        Dump 'unlock'
        if ($null -ne $field) {
            $typed = $false
            try {
                $field.element.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).SetValue($Passphrase)
                $typed = $true
            } catch { $why = $_.Exception.Message }
            Check 'soft' 'type the passphrase through UIA ValuePattern' $typed $why | Out-Null
            if (-not $typed) {
                $field.element.SetFocus()
                [System.Windows.Forms.SendKeys]::SendWait($Passphrase)
                Check 'hard' 'type the passphrase with the keyboard (SendKeys)' $true | Out-Null
            }
            Start-Sleep -Milliseconds 500
            Press { param($i) $i.name -eq 'Unlock' } 'Unlock' | Out-Null
            $gone = $false
            for ($k = 0; $k -lt 40 -and -not $gone; $k++) {
                Start-Sleep -Milliseconds 250
                $gone = $null -eq (Wait-For { param($i) $i.type -eq 'Edit' -and $i.name -eq 'Passphrase' } 0)
            }
            Check 'hard' 'unlocking through UIA removes the lock screen' $gone | Out-Null
        }

        foreach ($s in 'overview', 'transactions', 'txDetail', 'unlock') {
            if ($summary.Contains($s)) {
                $d = $summary[$s]
                Check 'hard' "${s}: 0 unnamed interactive elements ($($d.interactive) interactive)" ($d.unnamed -eq 0) (($d.unnamedDetail -join '; ')) | Out-Null
            }
        }
    }
} catch {
    Check 'hard' 'the walk ran to the end' $false $_.Exception.Message | Out-Null
}
Check 'hard' 'the app is still running after the walk (no UIA crash)' (Alive) | Out-Null

$hardFailures = @($checks | Where-Object { $_.kind -eq 'hard' -and -not $_.ok }).Count
[ordered]@{ summary = $summary; checks = $checks; hardFailures = $hardFailures } |
    ConvertTo-Json -Depth 6 | Set-Content -Encoding UTF8 (Join-Path $OutDir 'uia-summary.json')
exit ([int]($hardFailures -gt 0))
