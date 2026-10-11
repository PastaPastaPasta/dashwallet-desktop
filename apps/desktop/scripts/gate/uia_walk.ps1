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
# UIA control types (ControlType.ProgrammaticName without its prefix) that a user operates.
$interactiveTypes = @(
    'Button', 'CheckBox', 'ComboBox', 'Edit', 'Hyperlink', 'ListItem', 'MenuItem',
    'RadioButton', 'Slider', 'Spinner', 'SplitButton', 'TabItem', 'TreeItem'
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

# Every property read is a cross-process call: read each once, and leave the supported patterns to
# Dump (Wait-For polls the whole tree every 250 ms).
function Info($e) {
    $c = $e.Current
    $r = $c.BoundingRectangle
    return [ordered]@{
        type      = $c.ControlType.ProgrammaticName -replace '^ControlType\.', ''
        localized = $c.LocalizedControlType
        name      = $c.Name
        id        = $c.AutomationId
        offscreen = $c.IsOffscreen
        # On screen and with a box; Walk also requires it of every ancestor. WebView2 exposes its
        # hidden browser views (an App bar, caption buttons, a tab strip) under boxless parents.
        # AT-SPI's 'showing' state is the Linux walk's equivalent.
        shown     = -not $c.IsOffscreen -and -not $r.IsEmpty
        enabled   = $c.IsEnabled
        focusable = $c.IsKeyboardFocusable
        password  = $c.IsPassword
        # An element with no box reports an empty (infinite) rectangle, which [int] cannot convert.
        rect      = if ($r.IsEmpty) { 'empty' } else { "$([int]$r.X),$([int]$r.Y) $([int]$r.Width)x$([int]$r.Height)" }
        element   = $e
    }
}

# Depth-first walk of the raw view: [(depth, info)]. An element is shown only if its parent is.
function Walk($root) {
    $walker = [System.Windows.Automation.TreeWalker]::RawViewWalker
    $out = New-Object System.Collections.ArrayList
    $stack = New-Object System.Collections.Stack
    $stack.Push(@($root, 0, $true))
    while ($stack.Count -gt 0 -and $out.Count -lt 5000) {
        $entry = $stack.Pop()
        $e = $entry[0]; $depth = $entry[1]
        # An element that vanished mid-walk is recorded as such; its children are still walked.
        try { $info = Info $e } catch { $info = [ordered]@{ type = 'Unreadable'; name = "$($_.Exception.Message)"; id = ''; rect = ''; shown = $false; element = $e } }
        $info.shown = $info.shown -and $entry[2]
        [void]$out.Add(@($depth, $info))
        $kids = New-Object System.Collections.ArrayList
        try {
            $child = $walker.GetFirstChild($e)
            while ($null -ne $child) { [void]$kids.Add($child); $child = $walker.GetNextSibling($child) }
        } catch { }
        for ($i = $kids.Count - 1; $i -ge 0; $i--) { $stack.Push(@($kids[$i], $depth + 1, $info.shown)) }
    }
    return , $out
}

function Patterns($e) {
    try { return @($e.GetSupportedPatterns() | ForEach-Object { $_.ProgrammaticName -replace 'Identifiers\.Pattern$', '' }) }
    catch { return @('?') }
}

function Dump([string] $Screen) {
    $win = Find-Window
    if ($null -eq $win) { Check 'hard' "${Screen}: the window is in the UIA tree" $false | Out-Null; return }
    $nodes = Walk $win
    $lines = foreach ($n in $nodes) {
        $i = $n[1]
        ('  ' * $n[0]) + "[$($i.type)/$($i.localized)] '$($i.name)' id=$($i.id) $($i.rect)$(if ($i.offscreen) { ' offscreen' })$(if ($i.password) { ' password' }) {$((Patterns $i.element) -join ',')}"
    }
    $lines | Set-Content -Encoding UTF8 (Join-Path $OutDir "uia-$Screen.txt")
    $items = @($nodes | Where-Object { ($_[1].type -in $interactiveTypes) -and $_[1].shown } | ForEach-Object { $_[1] })
    $hidden = @($nodes | Where-Object { ($_[1].type -in $interactiveTypes) -and -not $_[1].shown }).Count
    $unnamed = @($items | Where-Object { -not $_.name.Trim() })
    $summary[$Screen] = [ordered]@{
        nodes            = $nodes.Count
        interactive      = $items.Count
        unnamed          = $unnamed.Count
        hiddenSkipped    = $hidden
        unnamedDetail    = @($unnamed | ForEach-Object { "$($_.type) $($_.rect) id=$($_.id)" })
        interactiveNames = @($items | ForEach-Object { "$($_.type): $($_.name)" } | Sort-Object -Unique | Select-Object -First 80)
    }
    Write-Host "== dumped $($nodes.Count) nodes to uia-$Screen.txt ($($items.Count) interactive, $($unnamed.Count) unnamed, $hidden hidden skipped)"
}

# The first shown element matching the predicate, polling until the timeout.
# Scriptblocks see variables by dynamic scope: callers pass closures (GetNewClosure), and the
# parameter names differ, so a predicate never resolves a name to this function's own variables.
function Wait-For([scriptblock] $Match, [int] $Seconds = 15) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    do {
        $win = Find-Window
        if ($null -ne $win) {
            foreach ($n in (Walk $win)) { $i = $n[1]; if ($i.shown -and (& $Match $i)) { return $i } }
        }
        Start-Sleep -Milliseconds 250
    } while ((Get-Date) -lt $deadline)
    return $null
}

# Invokes the first shown element that matches and supports Invoke (a list item that wraps a
# button shares its name but has no Invoke pattern).
function Press([scriptblock] $Want, [string] $Label) {
    $invokable = $AE::IsInvokePatternAvailableProperty
    $hit = Wait-For ({ param($i) (& $Want $i) -and $(try { $i.element.GetCurrentPropertyValue($invokable) } catch { $false }) }.GetNewClosure())
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

        # Rows are buttons named by their content ("Received 1:40 AM Pending +0.20 tDASH"); the time
        # tells them from the hero's amount button.
        Press { param($i) $i.name -match '\d{1,2}:\d{2}\s[AP]M' -and $i.name -match 'DASH' } 'first transaction row' | Out-Null
        # PowerShell's -and and -or bind equally (left to right): keep the parentheses.
        $dialog = Wait-For { param($i) ($i.localized -match 'dialog') -or ($i.type -eq 'Window' -and $i.name -eq 'Transaction details') }
        if (Check 'hard' 'the detail sheet is a dialog' ($null -ne $dialog)) {
            Check 'soft' 'the dialog is named' ([bool]$dialog.name) $dialog.name | Out-Null
        }
        Start-Sleep -Seconds 1
        Dump 'txDetail'
        # The sheet's Close, not the title bar's (AutomationId 'Close'), which would quit the app.
        Press { param($i) $i.name -eq 'Close' -and $i.id -ne 'Close' } 'Close' | Out-Null
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
                [System.Windows.Forms.SendKeys]::SendWait(($Passphrase -replace '([+^%~(){}\[\]])', '{$1}'))
                Check 'hard' 'type the passphrase with the keyboard (SendKeys)' $true | Out-Null
            }
            Start-Sleep -Milliseconds 500
            Press { param($i) $i.name -eq 'Unlock' } 'Unlock' | Out-Null
            # Opened, not merely 'the field is gone' (one empty poll would do): the toolbar's Lock
            # is enabled only for an unlocked wallet, and the passphrase field has left.
            $open = Wait-For { param($i) $i.type -eq 'Button' -and $i.name -ceq 'Lock wallet' -and $i.enabled } 10
            $still = if ($null -ne $open) { Wait-For { param($i) $i.type -eq 'Edit' -and $i.name -ceq 'Passphrase' } 0 }
            Check 'hard' 'unlocking through UIA opens the wallet (Lock enabled, no passphrase field)' ($null -ne $open -and $null -eq $still) | Out-Null
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
