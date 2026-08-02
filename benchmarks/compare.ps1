[CmdletBinding()]
param(
    [ValidateSet('Both', 'Rustshot', 'Lightshot')]
    [string] $Applications = 'Both',

    [ValidateSet('Random', 'RustshotFirst', 'LightshotFirst')]
    [string] $TargetOrder = 'Random',

    [ValidateRange(1, 1000)]
    [int] $Iterations = 30,

    [ValidateRange(0, 100)]
    [int] $WarmupIterations = 3,

    [string] $RustshotPath,
    [string] $LightshotPath,
    [string] $RustshotHotkey,
    [string] $LightshotHotkey,
    [string] $RustshotCopyHotkey = 'C',
    [string] $OutputDirectory,

    [ValidateRange(1, 60)]
    [int] $SettleSeconds = 3,

    [ValidateRange(100, 60000)]
    [int] $TimeoutMilliseconds = 5000,

    [ValidateRange(100, 60000)]
    [int] $ResourceSampleMilliseconds = 2000,

    [ValidateRange(0, 2000)]
    [int] $OverlayReadyMilliseconds = 100,

    [ValidateRange(20, 2000)]
    [int] $DragMilliseconds = 200,

    [ValidateRange(0, 2000)]
    [int] $SelectionReadyMilliseconds = 300,

    [switch] $SkipBuild,
    [switch] $RestartRunningApps,
    [switch] $ValidateOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$benchmarkRoot = $PSScriptRoot
$repositoryRoot = Split-Path -Parent $benchmarkRoot
$nativeSource = Join-Path $benchmarkRoot 'Benchmark.Native.cs'

function Write-Step([string] $Message) {
    Write-Host "`n==> $Message" -ForegroundColor Cyan
}

function Resolve-FullPath([string] $Path, [string] $BasePath) {
    if ([string]::IsNullOrWhiteSpace($Path)) {
        return $null
    }
    if (![System.IO.Path]::IsPathRooted($Path)) {
        $Path = Join-Path $BasePath $Path
    }
    return [System.IO.Path]::GetFullPath($Path)
}

function Resolve-CargoExecutable {
    $installed = Get-Command cargo -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($installed) {
        return $installed.Source
    }

    $localCargo = Join-Path $repositoryRoot '.tooling\cargo\bin\cargo.exe'
    $localRustup = Join-Path $repositoryRoot '.tooling\rustup'
    if (Test-Path -LiteralPath $localCargo -PathType Leaf) {
        $env:CARGO_HOME = Join-Path $repositoryRoot '.tooling\cargo'
        if (Test-Path -LiteralPath $localRustup -PathType Container) {
            $env:RUSTUP_HOME = $localRustup
        }
        return $localCargo
    }

    throw 'Cargo was not found on PATH or in .tooling\cargo\bin. Install Rust 1.88+ or pass -SkipBuild for an existing release executable.'
}

function Find-LightshotExecutable {
    $running = @(Get-Process -Name Lightshot -ErrorAction SilentlyContinue)
    foreach ($process in $running) {
        try {
            if ($process.Path -and (Test-Path -LiteralPath $process.Path -PathType Leaf)) {
                return $process.Path
            }
        }
        catch {
            # Access to process paths can be denied across privilege boundaries.
        }
    }

    $candidates = @(
        (Join-Path $env:ProgramFiles 'Skillbrains\lightshot\Lightshot.exe'),
        (Join-Path ${env:ProgramFiles(x86)} 'Skillbrains\lightshot\Lightshot.exe'),
        (Join-Path $env:LOCALAPPDATA 'Skillbrains\lightshot\Lightshot.exe')
    )
    if (${env:ProgramFiles(x86)}) {
        $versioned = Join-Path ${env:ProgramFiles(x86)} 'Skillbrains\lightshot\*\Lightshot.exe'
        $candidates += @(Get-ChildItem -Path $versioned -File -ErrorAction SilentlyContinue |
                Sort-Object -Property FullName -Descending |
                ForEach-Object FullName)
    }
    return $candidates | Where-Object { $_ -and (Test-Path -LiteralPath $_ -PathType Leaf) } |
        Select-Object -First 1
}

function Find-RustshotRegionHotkey {
    $configCandidates = @(
        (Join-Path $env:APPDATA 'CodyKoInABox\Rustshot\config\config.toml'),
        (Join-Path $env:APPDATA 'Rustshot\config.toml'),
        (Join-Path $env:LOCALAPPDATA 'Rustshot\config.toml')
    )
    foreach ($path in $configCandidates) {
        if (!(Test-Path -LiteralPath $path -PathType Leaf)) {
            continue
        }
        $contents = Get-Content -LiteralPath $path -Raw
        $match = [regex]::Match($contents, '(?m)^\s*region_shortcut\s*=\s*["'']([^"'']+)["'']')
        if ($match.Success) {
            return $match.Groups[1].Value
        }
    }
    return 'Ctrl+Shift+F10'
}

function Find-LightshotRegionHotkey {
    $settings = Get-ItemProperty -Path 'Registry::HKEY_CURRENT_USER\Software\Skillbrains\lightshot' `
        -ErrorAction SilentlyContinue
    if (!$settings) {
        return 'PrintScreen'
    }
    $enabled = $settings.PSObject.Properties['Hotkey_main_enabled']
    if ($enabled -and ![bool]$enabled.Value) {
        throw 'Lightshot region capture is disabled in its hotkey settings.'
    }
    $modifiers = $settings.PSObject.Properties['Hotkey_main_mod']
    $virtualKey = $settings.PSObject.Properties['Hotkey_main_vk']
    if ($modifiers -and $virtualKey) {
        return [RustshotBench.NativeBench]::FormatWindowsHotkey(
            [int]$modifiers.Value,
            [int]$virtualKey.Value)
    }
    return 'PrintScreen'
}

function Get-TargetProcesses {
    param([string[]] $Names)

    $processes = foreach ($name in $Names) {
        Get-Process -Name $name -ErrorAction SilentlyContinue
    }
    return @($processes | Sort-Object -Property Id -Unique)
}

function Get-ProcessPathSafe([System.Diagnostics.Process] $Process) {
    try {
        return $Process.Path
    }
    catch {
        return $null
    }
}

function Stop-OwnedProcess([System.Diagnostics.Process] $Process) {
    if (!$Process -or $Process.HasExited) {
        return
    }
    Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue
    try {
        $Process.WaitForExit(5000) | Out-Null
    }
    catch {
        # The process may already have exited and disposed its handle.
    }
}

function Start-TargetProcess([pscustomobject] $Target) {
    $launched = Start-Process -FilePath $Target.Path -PassThru -WindowStyle Hidden
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    do {
        if (!$launched.HasExited) {
            return $launched
        }
        $candidate = Get-Process -Name $Target.ProcessName -ErrorAction SilentlyContinue |
            Where-Object {
                $candidatePath = Get-ProcessPathSafe $_
                $candidatePath -and ([System.IO.Path]::GetFullPath($candidatePath) -eq $Target.Path)
            } |
            Select-Object -First 1
        if ($candidate) {
            return $candidate
        }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $deadline)

    throw "$($Target.Name) did not leave a running process after launch"
}

function Close-CaptureOverlay {
    param(
        [long] $Handle,
        [int] $Timeout = 1500
    )

    if (!$Handle) {
        return
    }
    [RustshotBench.NativeBench]::SendEscape()
    if (![RustshotBench.NativeBench]::WaitForWindowGone($Handle, $Timeout)) {
        [RustshotBench.NativeBench]::SendEscape()
        [RustshotBench.NativeBench]::WaitForWindowGone($Handle, $Timeout) | Out-Null
    }
}

function Reset-CaptureState {
    param([System.Diagnostics.Process] $Process)

    $overlay = [RustshotBench.NativeBench]::FindAnyOverlay($Process.Id, 400, 300)
    if ($overlay) {
        Close-CaptureOverlay -Handle $overlay.Handle
    }
}

function Measure-ColdReadiness {
    param(
        [pscustomobject] $Target,
        [RustshotBench.PatternHost] $Pattern
    )

    $clock = [System.Diagnostics.Stopwatch]::StartNew()
    $process = Start-TargetProcess $Target
    $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMilliseconds * 3)
    $overlay = $null
    try {
        $bounds = $Pattern.Bounds
        do {
            if ($process.HasExited) {
                throw "$($Target.Name) exited before accepting its capture hotkey"
            }
            $Pattern.Activate()
            [RustshotBench.NativeBench]::MoveCursor($bounds.Left + 20, $bounds.Top + 20)
            [RustshotBench.NativeBench]::SendShortcut($Target.Hotkey)
            $probeDeadline = [DateTime]::UtcNow.AddMilliseconds(80)
            do {
                $overlay = [RustshotBench.NativeBench]::FindAnyOverlay($process.Id, 400, 300)
                if ($overlay) {
                    break
                }
                Start-Sleep -Milliseconds 2
            } while ([DateTime]::UtcNow -lt $probeDeadline)
        } while (!$overlay -and [DateTime]::UtcNow -lt $deadline)

        if (!$overlay) {
            throw "timed out waiting for $($Target.Name) to accept '$($Target.Hotkey)' after launch"
        }
        $clock.Stop()
        return [pscustomobject]@{
            Process = $process
            ColdReadinessMs = [Math]::Round($clock.Elapsed.TotalMilliseconds, 3)
            Overlay = $overlay
        }
    }
    catch {
        Stop-OwnedProcess $process
        throw
    }
}

function New-TrialRow {
    param(
        [pscustomobject] $Target,
        [int] $Iteration,
        [bool] $IsWarmup
    )

    return [ordered]@{
        application = $Target.Name
        iteration = $Iteration
        warmup = $IsWarmup
        activation_ms = $null
        copy_ms = $null
        copy_action = $Target.CopyAction
        workflow_ms = $null
        image_width = $null
        image_height = $null
        expected_width = 480
        expected_height = 320
        dimension_pass = $false
        pixel_mean_error = $null
        pixel_pass = $false
        alignment_x = $null
        alignment_y = $null
        sha256 = $null
        overlay_class = $null
        overlay_title = $null
        overlay_left = $null
        overlay_top = $null
        overlay_width = $null
        overlay_height = $null
        success = $false
        error = $null
    }
}

function Invoke-Trial {
    param(
        [pscustomobject] $Target,
        [System.Diagnostics.Process] $Process,
        [RustshotBench.PatternHost] $Pattern,
        [int] $Iteration,
        [bool] $IsWarmup
    )

    $row = New-TrialRow -Target $Target -Iteration $Iteration -IsWarmup $IsWarmup
    $overlayHandle = 0
    try {
        if ($Process.HasExited) {
            throw "$($Target.Name) exited during the benchmark"
        }
        Reset-CaptureState -Process $Process
        $Pattern.Activate()
        $bounds = $Pattern.Bounds
        [RustshotBench.NativeBench]::MoveCursor($bounds.Left + 20, $bounds.Top + 20)
        Start-Sleep -Milliseconds 75

        $marker = "rustshot-benchmark-$($Target.Name)-$Iteration-$([Guid]::NewGuid())"
        [RustshotBench.NativeBench]::SetClipboardMarker($marker)
        $previousSequence = [RustshotBench.NativeBench]::ClipboardSequenceNumber

        $activation = [RustshotBench.NativeBench]::MeasureActivation(
            $Process.Id,
            $Target.Hotkey,
            $TimeoutMilliseconds,
            400,
            300)
        if (!$activation.Success) {
            throw "activation failed: $($activation.Error)"
        }
        $overlayHandle = $activation.Window.Handle
        [RustshotBench.NativeBench]::ActivateWindow($overlayHandle)
        $row.activation_ms = [Math]::Round($activation.ElapsedMs, 3)
        $row.overlay_class = $activation.Window.ClassName
        $row.overlay_title = $activation.Window.Title
        $row.overlay_left = $activation.Window.Left
        $row.overlay_top = $activation.Window.Top
        $row.overlay_width = $activation.Window.Width
        $row.overlay_height = $activation.Window.Height

        if ($OverlayReadyMilliseconds) {
            Start-Sleep -Milliseconds $OverlayReadyMilliseconds
        }
        $selectionX = 120
        $selectionY = 100
        $selectionWidth = 480
        $selectionHeight = 320
        $startX = $bounds.Left + $selectionX
        $startY = $bounds.Top + $selectionY
        $endX = $startX + $selectionWidth
        $endY = $startY + $selectionHeight

        [RustshotBench.NativeBench]::SendMouseDrag(
            $startX,
            $startY,
            $endX,
            $endY,
            $DragMilliseconds) | Out-Null
        if ($SelectionReadyMilliseconds) {
            Start-Sleep -Milliseconds $SelectionReadyMilliseconds
        }

        $copyStart = [RustshotBench.NativeBench]::Timestamp
        if ($Target.CopyMode -eq 'LightshotToolbar') {
            # With the fixed selection safely away from screen edges, Lightshot
            # places its toolbar below the selection. The copy button is the
            # fifth 29px button, centered 75px left of the selection's edge.
            [RustshotBench.NativeBench]::SendMouseClick($endX - 75, $endY + 21)
        }
        else {
            [RustshotBench.NativeBench]::SendShortcut($Target.CopyHotkey)
        }
        # Returning to the PowerShell message pump before polling avoids
        # starving applications whose foreground input queue is attached to it.
        Start-Sleep -Milliseconds 20
        $copy = [RustshotBench.NativeBench]::WaitForClipboardImage(
            $previousSequence,
            $TimeoutMilliseconds,
            $selectionX,
            $selectionY,
            $copyStart)
        if (!$copy.Success) {
            throw "copy failed: $($copy.Error)"
        }

        $row.copy_ms = [Math]::Round($copy.ElapsedMs, 3)
        $row.workflow_ms = [Math]::Round(
            [RustshotBench.NativeBench]::MillisecondsBetween(
                $activation.StartTimestamp,
                $copy.EndTimestamp),
            3)
        $row.image_width = $copy.Width
        $row.image_height = $copy.Height
        $row.dimension_pass = ([Math]::Abs($copy.Width - $selectionWidth) -le 2) -and
            ([Math]::Abs($copy.Height - $selectionHeight) -le 2)
        $row.pixel_mean_error = [Math]::Round($copy.PixelMeanError, 3)
        $row.pixel_pass = $copy.PixelMeanError -le 4.0
        $row.alignment_x = $copy.AlignmentX
        $row.alignment_y = $copy.AlignmentY
        $row.sha256 = $copy.Sha256
        $row.success = $row.dimension_pass -and $row.pixel_pass
        if (!$row.success) {
            $row.error = 'clipboard image did not pass dimension and pixel validation'
        }
    }
    catch {
        $row.error = $_.Exception.Message
    }
    finally {
        if ($overlayHandle) {
            Close-CaptureOverlay -Handle $overlayHandle
        }
        else {
            Reset-CaptureState -Process $Process
        }
        Start-Sleep -Milliseconds 75
    }
    return [pscustomobject]$row
}

function Get-Percentile {
    param(
        [object[]] $Values,
        [ValidateRange(0, 100)] [double] $Percentile
    )

    $numbers = @($Values | Where-Object { $null -ne $_ } | ForEach-Object { [double]$_ } | Sort-Object)
    if (!$numbers.Count) {
        return $null
    }
    $index = [Math]::Ceiling(($Percentile / 100.0) * $numbers.Count) - 1
    $index = [Math]::Max(0, [Math]::Min($numbers.Count - 1, $index))
    return [Math]::Round($numbers[$index], 3)
}

function Get-Median([object[]] $Values) {
    $numbers = @($Values | Where-Object { $null -ne $_ } | ForEach-Object { [double]$_ } | Sort-Object)
    if (!$numbers.Count) {
        return $null
    }
    $middle = [Math]::Floor($numbers.Count / 2)
    if (($numbers.Count % 2) -eq 1) {
        return [Math]::Round($numbers[$middle], 3)
    }
    return [Math]::Round(($numbers[$middle - 1] + $numbers[$middle]) / 2.0, 3)
}

function New-ApplicationSummary {
    param(
        [pscustomobject] $Target,
        [object[]] $TrialRows,
        [pscustomobject] $ProcessRow
    )

    $measured = @($TrialRows | Where-Object { !$_.warmup })
    $successful = @($measured | Where-Object success)
    $activation = @($measured | Where-Object { $null -ne $_.activation_ms } | ForEach-Object activation_ms)
    $copy = @($measured | Where-Object { $null -ne $_.copy_ms } | ForEach-Object copy_ms)
    $workflow = @($measured | Where-Object { $null -ne $_.workflow_ms } | ForEach-Object workflow_ms)

    return [ordered]@{
        application = $Target.Name
        executable = $Target.Path
        version = $Target.Version
        binary_bytes = $Target.BinaryBytes
        cold_readiness_ms = $ProcessRow.cold_readiness_ms
        private_bytes = $ProcessRow.private_bytes
        working_set_bytes = $ProcessRow.working_set_bytes
        idle_cpu_percent = $ProcessRow.idle_cpu_percent
        handles = $ProcessRow.handles
        gdi_objects = $ProcessRow.gdi_objects
        user_objects = $ProcessRow.user_objects
        iterations = $measured.Count
        successful_iterations = $successful.Count
        success_rate_percent = if ($measured.Count) {
            [Math]::Round(100.0 * $successful.Count / $measured.Count, 2)
        }
        else { 0 }
        activation_ms = [ordered]@{
            median = Get-Median $activation
            p95 = Get-Percentile $activation 95
        }
        copy_ms = [ordered]@{
            median = Get-Median $copy
            p95 = Get-Percentile $copy 95
        }
        workflow_ms = [ordered]@{
            median = Get-Median $workflow
            p95 = Get-Percentile $workflow 95
        }
    }
}

if ($env:OS -ne 'Windows_NT') {
    throw 'This benchmark requires an interactive Windows desktop session.'
}
if ([Threading.Thread]::CurrentThread.GetApartmentState() -ne [Threading.ApartmentState]::STA) {
    throw 'Run the benchmark in an STA host, for example: powershell.exe -STA -File .\benchmarks\compare.ps1'
}
if (!(Test-Path -LiteralPath $nativeSource -PathType Leaf)) {
    throw "Missing native helper source: $nativeSource"
}

Write-Step 'Compiling the Win32 benchmark helper'
Add-Type -TypeDefinition (Get-Content -LiteralPath $nativeSource -Raw) `
    -ReferencedAssemblies @('System.Windows.Forms.dll', 'System.Drawing.dll') `
    -Language CSharp
[RustshotBench.NativeBench]::EnableDpiAwareness()

if ([string]::IsNullOrWhiteSpace($RustshotHotkey)) {
    $RustshotHotkey = Find-RustshotRegionHotkey
}
if ([string]::IsNullOrWhiteSpace($LightshotHotkey)) {
    $LightshotHotkey = Find-LightshotRegionHotkey
}

$rustshotPathWasProvided = ![string]::IsNullOrWhiteSpace($RustshotPath)
$shouldBuildRustshot = !$SkipBuild -and !$ValidateOnly -and
    $Applications -ne 'Lightshot' -and !$rustshotPathWasProvided
$benchmarkCargoTarget = Join-Path $repositoryRoot 'target\benchmark-build'
if (!$rustshotPathWasProvided) {
    $RustshotPath = if ($shouldBuildRustshot) {
        Join-Path $benchmarkCargoTarget 'release\rustshot.exe'
    }
    else {
        Join-Path $repositoryRoot 'target\release\rustshot.exe'
    }
}
$RustshotPath = Resolve-FullPath $RustshotPath $repositoryRoot

if ($shouldBuildRustshot) {
    Write-Step 'Building Rustshot in release mode'
    $cargoExecutable = Resolve-CargoExecutable
    & $cargoExecutable build `
        --release `
        --locked `
        --target-dir $benchmarkCargoTarget `
        --manifest-path (Join-Path $repositoryRoot 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE"
    }
}

if ([string]::IsNullOrWhiteSpace($LightshotPath) -and $Applications -ne 'Rustshot') {
    $LightshotPath = Find-LightshotExecutable
}
if ($LightshotPath) {
    $LightshotPath = Resolve-FullPath $LightshotPath $repositoryRoot
}

$targets = @()
if ($Applications -ne 'Lightshot') {
    if (!(Test-Path -LiteralPath $RustshotPath -PathType Leaf)) {
        throw "Rustshot executable not found: $RustshotPath. Build it or pass -RustshotPath."
    }
    [RustshotBench.NativeBench]::ValidateShortcut($RustshotHotkey) | Out-Null
    [RustshotBench.NativeBench]::ValidateShortcut($RustshotCopyHotkey) | Out-Null
    $file = Get-Item -LiteralPath $RustshotPath
    $targets += [pscustomobject]@{
        Name = 'Rustshot'
        ProcessName = [System.IO.Path]::GetFileNameWithoutExtension($RustshotPath)
        Path = $RustshotPath
        Hotkey = $RustshotHotkey
        CopyHotkey = $RustshotCopyHotkey
        CopyMode = 'Hotkey'
        CopyAction = "Hotkey $RustshotCopyHotkey"
        Version = $file.VersionInfo.ProductVersion
        BinaryBytes = $file.Length
    }
}
if ($Applications -ne 'Rustshot') {
    if (!$LightshotPath -or !(Test-Path -LiteralPath $LightshotPath -PathType Leaf)) {
        throw 'Lightshot was not found. Install it or pass -LightshotPath C:\path\to\Lightshot.exe.'
    }
    [RustshotBench.NativeBench]::ValidateShortcut($LightshotHotkey) | Out-Null
    $file = Get-Item -LiteralPath $LightshotPath
    $targets += [pscustomobject]@{
        Name = 'Lightshot'
        ProcessName = [System.IO.Path]::GetFileNameWithoutExtension($LightshotPath)
        Path = $LightshotPath
        Hotkey = $LightshotHotkey
        CopyHotkey = $null
        CopyMode = 'LightshotToolbar'
        CopyAction = 'Toolbar button'
        Version = $file.VersionInfo.ProductVersion
        BinaryBytes = $file.Length
    }
}

if ($targets.Count -eq 2) {
    $lightshotFirst = $TargetOrder -eq 'LightshotFirst' -or
        ($TargetOrder -eq 'Random' -and (Get-Random -Minimum 0 -Maximum 2) -eq 1)
    if ($lightshotFirst) {
        $targets = @($targets | Sort-Object { if ($_.Name -eq 'Lightshot') { 0 } else { 1 } })
    }
    else {
        $targets = @($targets | Sort-Object { if ($_.Name -eq 'Rustshot') { 0 } else { 1 } })
    }
}

Write-Host ($targets | Format-Table Name, Version, Hotkey, CopyAction, Path -AutoSize | Out-String)

if (!$ValidateOnly) {
    if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
        $OutputDirectory = Join-Path $benchmarkRoot (Join-Path 'results' (Get-Date -Format 'yyyyMMdd-HHmmss'))
    }
    $OutputDirectory = Resolve-FullPath $OutputDirectory $repositoryRoot
    New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
}

$pattern = [RustshotBench.PatternHost]::new(800, 600)
$pattern.Start()
try {
    $bounds = $pattern.Bounds
    if ($bounds.Width -lt 720 -or $bounds.Height -lt 500) {
        throw 'The benchmark pattern did not receive the required 800x600 client area.'
    }
    if ($ValidateOnly) {
        Write-Host 'Validation passed: helper compiled, hotkeys parsed, executables resolved, and pattern window rendered.' -ForegroundColor Green
        return
    }
}
finally {
    if ($ValidateOnly) {
        $pattern.Dispose()
    }
}

$targetNames = @($targets | ForEach-Object ProcessName)
$preexisting = @(Get-TargetProcesses $targetNames)
$restartPaths = @()
if ($preexisting.Count) {
    if (!$RestartRunningApps) {
        $pattern.Dispose()
        $description = ($preexisting | ForEach-Object { "$($_.ProcessName) (PID $($_.Id))" }) -join ', '
        throw "Close the running capture applications before benchmarking: $description. Or pass -RestartRunningApps to stop and restore them automatically."
    }
    Write-Step 'Temporarily stopping running capture applications'
    foreach ($process in $preexisting) {
        $path = Get-ProcessPathSafe $process
        if ($path) {
            $restartPaths += $path
        }
        Stop-Process -Id $process.Id -Force
    }
    Start-Sleep -Milliseconds 500
}

$trialRows = [System.Collections.Generic.List[object]]::new()
$processRows = [System.Collections.Generic.List[object]]::new()
$runErrors = [System.Collections.Generic.List[string]]::new()

try {
    foreach ($target in $targets) {
        Write-Step "Benchmarking $($target.Name)"
        $process = $null
        try {
            $cold = Measure-ColdReadiness -Target $target -Pattern $pattern
            $process = $cold.Process
            Write-Host ("Cold readiness: {0:N1} ms" -f $cold.ColdReadinessMs)
            Close-CaptureOverlay -Handle $cold.Overlay.Handle
            Start-Sleep -Seconds $SettleSeconds

            $resources = [RustshotBench.NativeBench]::SampleResources(
                $process,
                $ResourceSampleMilliseconds)
            $processRow = [pscustomobject][ordered]@{
                application = $target.Name
                process_id = $process.Id
                cold_readiness_ms = $cold.ColdReadinessMs
                private_bytes = $resources.PrivateBytes
                working_set_bytes = $resources.WorkingSetBytes
                idle_cpu_percent = [Math]::Round($resources.CpuPercent, 4)
                handles = $resources.HandleCount
                gdi_objects = $resources.GdiObjects
                user_objects = $resources.UserObjects
                sample_milliseconds = $resources.SampleMilliseconds
            }
            $processRows.Add($processRow)

            $totalIterations = $WarmupIterations + $Iterations
            for ($index = 1; $index -le $totalIterations; $index++) {
                $isWarmup = $index -le $WarmupIterations
                $measuredIndex = if ($isWarmup) { $index } else { $index - $WarmupIterations }
                $label = if ($isWarmup) { "warmup $measuredIndex/$WarmupIterations" } else { "trial $measuredIndex/$Iterations" }
                Write-Progress -Activity "Benchmarking $($target.Name)" -Status $label -PercentComplete (100 * $index / $totalIterations)
                $row = Invoke-Trial `
                    -Target $target `
                    -Process $process `
                    -Pattern $pattern `
                    -Iteration $measuredIndex `
                    -IsWarmup $isWarmup
                $trialRows.Add($row)
                if (!$row.success -and !$isWarmup) {
                    Write-Warning "$($target.Name) trial $measuredIndex failed: $($row.error)"
                }
            }
            Write-Progress -Activity "Benchmarking $($target.Name)" -Completed
        }
        catch {
            $message = "$($target.Name): $($_.Exception.Message)"
            $runErrors.Add($message)
            Write-Warning $message
        }
        finally {
            Stop-OwnedProcess $process
            Start-Sleep -Milliseconds 500
        }
    }
}
finally {
    $pattern.Dispose()
    foreach ($path in @($restartPaths | Sort-Object -Unique)) {
        if (Test-Path -LiteralPath $path -PathType Leaf) {
            Start-Process -FilePath $path -WindowStyle Hidden | Out-Null
        }
    }
}

$trialsPath = Join-Path $OutputDirectory 'trials.csv'
$processPath = Join-Path $OutputDirectory 'process.csv'
$summaryPath = Join-Path $OutputDirectory 'summary.json'
$trialRows | Export-Csv -LiteralPath $trialsPath -NoTypeInformation -Encoding UTF8
$processRows | Export-Csv -LiteralPath $processPath -NoTypeInformation -Encoding UTF8

$summaries = foreach ($target in $targets) {
    $targetTrials = @($trialRows | Where-Object application -eq $target.Name)
    $targetProcess = $processRows | Where-Object application -eq $target.Name | Select-Object -First 1
    if ($targetProcess) {
        New-ApplicationSummary -Target $target -TrialRows $targetTrials -ProcessRow $targetProcess
    }
}

$environment = [ordered]@{
    timestamp = (Get-Date).ToString('o')
    computer = $env:COMPUTERNAME
    os = [Environment]::OSVersion.VersionString
    powershell = $PSVersionTable.PSVersion.ToString()
    logical_processors = [Environment]::ProcessorCount
    iterations = $Iterations
    warmup_iterations = $WarmupIterations
    settle_seconds = $SettleSeconds
    resource_sample_milliseconds = $ResourceSampleMilliseconds
    drag_milliseconds = $DragMilliseconds
    overlay_ready_milliseconds = $OverlayReadyMilliseconds
    selection_ready_milliseconds = $SelectionReadyMilliseconds
    target_order = @($targets | ForEach-Object Name)
}
$report = [ordered]@{
    environment = $environment
    applications = @($summaries)
    errors = @($runErrors)
}
$report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summaryPath -Encoding UTF8

Write-Step 'Benchmark summary'
$summaries | ForEach-Object {
    [pscustomobject]@{
        Application = $_.application
        Success = ('{0:N1}%' -f $_.success_rate_percent)
        'Activation median' = if ($null -ne $_.activation_ms.median) { '{0:N2} ms' -f $_.activation_ms.median } else { '-' }
        'Activation p95' = if ($null -ne $_.activation_ms.p95) { '{0:N2} ms' -f $_.activation_ms.p95 } else { '-' }
        'Workflow median' = if ($null -ne $_.workflow_ms.median) { '{0:N2} ms' -f $_.workflow_ms.median } else { '-' }
        'Private memory' = ('{0:N1} MiB' -f ($_.private_bytes / 1MB))
    }
} | Format-Table -AutoSize

Write-Host "Results written to: $OutputDirectory" -ForegroundColor Green
Write-Host "  $summaryPath"
Write-Host "  $trialsPath"
Write-Host "  $processPath"
$measuredFailures = @($trialRows | Where-Object { !$_.warmup -and !$_.success })
if ($runErrors.Count -or $measuredFailures.Count) {
    throw "The benchmark completed with $($runErrors.Count) application error(s) and $($measuredFailures.Count) failed measured trial(s). Inspect summary.json and trials.csv."
}
