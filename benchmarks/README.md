# Rustshot vs. Lightshot benchmark

This is an interactive Windows black-box benchmark. It drives the installed applications through
the same visible region-capture workflow instead of comparing Rustshot internals with unavailable
Lightshot internals.

The benchmark creates an 800x600 deterministic color pattern, opens each application's capture
overlay, selects the same 480x320 physical-pixel rectangle, copies it, and validates sampled pixels
and dimensions. It also samples startup readiness, idle resources, executable size, and reliability.

## Quick start

Run this from the repository root in an unlocked, interactive desktop session:

```powershell
powershell.exe -NoProfile -STA -ExecutionPolicy Bypass `
  -File .\benchmarks\compare.ps1 `
  -RestartRunningApps
```

`-RestartRunningApps` temporarily stops running Rustshot and Lightshot processes, benchmarks clean
instances sequentially, and relaunches the original executables when the run ends. Quit the two apps
yourself and omit this switch if you do not want the script to manage them.

The default run builds Rustshot in release mode under the isolated `target/benchmark-build`
directory, performs 3 warmups and 30 measured iterations per application, and randomly chooses
which application runs first. The isolated build avoids replacing a Rustshot executable that may
already be running. Do not use the mouse or keyboard, cover the pattern window, lock the desktop, or
change display settings until it finishes.

Results are written under `benchmarks/results/<timestamp>/`:

- `summary.json` contains environment details and median/p95 application summaries.
- `trials.csv` contains every warmup and measured iteration, including failures and pixel checks.
- `process.csv` contains cold readiness and the idle resource sample.

## Useful commands

Validate paths, hotkey discovery, C# compilation, and pattern rendering without driving either app:

```powershell
powershell.exe -NoProfile -STA -ExecutionPolicy Bypass `
  -File .\benchmarks\compare.ps1 -ValidateOnly -SkipBuild
```

Run a quick two-iteration smoke benchmark:

```powershell
powershell.exe -NoProfile -STA -ExecutionPolicy Bypass `
  -File .\benchmarks\compare.ps1 `
  -RestartRunningApps -SkipBuild -WarmupIterations 1 -Iterations 2
```

Benchmark one application:

```powershell
.\benchmarks\compare.ps1 -Applications Rustshot -RestartRunningApps
.\benchmarks\compare.ps1 -Applications Lightshot -RestartRunningApps -SkipBuild
```

Override discovery for a portable or customized installation:

```powershell
.\benchmarks\compare.ps1 `
  -RustshotPath .\target\release\rustshot.exe `
  -LightshotPath 'C:\Tools\Lightshot\Lightshot.exe' `
  -RustshotHotkey 'Alt+BracketRight' `
  -LightshotHotkey 'Shift+Backspace' `
  -RestartRunningApps
```

An explicit `-RustshotPath` is treated as an existing executable and is not rebuilt; build it first
or omit the path to use the benchmark's isolated release build.

Pass `-OutputDirectory <path>` to choose the result location and `-TargetOrder RustshotFirst` or
`-TargetOrder LightshotFirst` to make run order deterministic. Run `Get-Help
.\benchmarks\compare.ps1 -Detailed` or inspect the parameter block for timing controls.

## What the metrics mean

- **Cold readiness**: process start until an injected region hotkey produces a capture overlay. The
  driver probes the hotkey during startup, so treat this as user-visible readiness rather than pure
  process initialization time.
- **Activation**: hotkey injection until the application's large overlay window is visible. This is
  the cleanest comparable capture-latency number.
- **Copy**: copy action until a new clipboard bitmap is readable. Rustshot receives its documented
  `C` command. Lightshot 5.5 uses its visible Copy toolbar button because its `Ctrl+C` handler does
  not accept synthetic input reliably.
- **Workflow**: hotkey through validated clipboard output. It includes identical fixed settling and
  mouse-drag timing, so compare applications within one run rather than treating it as human speed.
- **Success rate**: iterations whose clipboard bitmap was produced, within two pixels of 480x320,
  and within a mean RGB error of 4 against the deterministic source pattern.
- **Idle resources**: private bytes, working set, normalized CPU percentage, handles, and GDI/USER
  objects after the configured settling interval.

Warmups remain in `trials.csv` with `warmup=True`, but summaries exclude them. Percentile results use
the nearest-rank definition.

## Hotkeys and safety

Rustshot's region hotkey is read from its normal `config.toml`; the built-in default is used when no
saved config exists. Lightshot's enabled region hotkey is read from its per-user Skillbrains registry
settings. Explicit command-line hotkeys take precedence.

The script refuses to start when a selected application is already running unless
`-RestartRunningApps` is supplied. Only processes matching the selected executable names are stopped,
and executables that were running beforehand are relaunched in a `finally` block. Partial CSV/JSON
files are still useful when an individual iteration fails; the `error` column explains the failure.

For credible comparisons, keep the display topology, DPI scaling, power mode, foreground workload,
and benchmark parameters fixed. Run several complete benchmark sessions and compare their medians,
not one isolated trial.
