<#
.SYNOPSIS
  Measures CPU / GPU / RAM across the ENTIRE application process tree (Tauri or
  native Rust) for a fair renderer comparison. Optionally measures frames/latency via
  PresentMon.

.DESCRIPTION
  On Windows, Tauri uses a process tree: app.exe (Rust backend) plus several
  msedgewebview2.exe processes (renderer/GPU/utility). Chart rendering and presentation
  happen in the WebView2 processes, NOT in app.exe. Measuring app.exe alone is misleading.
  The script finds the ROOT by name and aggregates it with ALL descendants (native apps
  have no descendants, so one process is measured with the same methodology for symmetry).

  Metrics (sampled every -IntervalSec; the first -WarmupSec are discarded):
    - Process-tree CPU%, normalized by the number of logical cores (0..100 = entire machine).
    - Process-tree RAM (working set), in MB.
    - Process-tree GPU% from '\GPU Engine(*)\Utilization Percentage' counters for the 3D
      engine, summed for our PIDs (a quick metric; use PresentMon for rigorous measurements).
  Summary: mean / median / p95 / p99 / max. Writes a per-sample CSV and a summary.

.EXAMPLE
  # Native app (one process)
  ./tools/bench.ps1 -RootProcess moonterminal -DurationSec 300 -Label native

.EXAMPLE
  # Tauri (app + WebView2 descendants) with PresentMon frames from the WebView2 GPU process
  ./tools/bench.ps1 -RootProcess your-tauri-app -DurationSec 300 -Label tauri `
     -PresentMonPath C:\tools\PresentMon.exe -PresentProcess msedgewebview2.exe

.NOTES
  Run AS ADMINISTRATOR (GPU counters and PresentMon require elevated privileges). Close DevTools.
  Compare the two binaries under identical conditions: the same window size (physical pixels),
  the same monitor/DPI, a 60 FPS cap, the same scene/workload (see MOON_SYNTH in the README below),
  RELEASE builds, and the window in the foreground. Run N>=3 trials and compare the MEDIANS.
#>
[CmdletBinding()]
param(
  # Root process name WITHOUT .exe (for example, moonterminal). Aggregates it and its descendants.
  [Parameter(Mandatory)] [string] $RootProcess,
  [int]    $DurationSec   = 300,
  [double] $IntervalSec   = 1.0,
  [int]    $WarmupSec     = 30,
  [string] $Label         = 'run',
  [string] $OutDir        = (Join-Path $PSScriptRoot 'bench-out'),
  # PresentMon (frames/latency). Runs in parallel when a path is provided.
  [string] $PresentMonPath = '',
  # Process whose presentation events are captured: native = $RootProcess.exe; Tauri = msedgewebview2.exe.
  [string] $PresentProcess = ''
)

$ErrorActionPreference = 'Stop'
$rootName = $RootProcess -replace '\.exe$',''

# -- Utilities ----------------------------------------------------------------
function Get-Descendants([int]$rootPid, $procTable) {
  # Performs BFS by ParentProcessId. Returns a set of PIDs (root and all descendants).
  $set = New-Object 'System.Collections.Generic.HashSet[int]'
  $q = New-Object System.Collections.Queue
  [void]$set.Add($rootPid); $q.Enqueue($rootPid)
  while ($q.Count -gt 0) {
    $cur = $q.Dequeue()
    foreach ($child in $procTable[$cur]) {
      if ($set.Add($child)) { $q.Enqueue($child) }
    }
  }
  ,$set
}

function Resolve-Tree([string]$name) {
  # Builds the parent-to-children map once, then walks the tree from every root with this name.
  $all = Get-CimInstance Win32_Process -Property ProcessId,ParentProcessId
  $byParent = @{}
  foreach ($p in $all) {
    if (-not $byParent.ContainsKey($p.ParentProcessId)) { $byParent[$p.ParentProcessId] = New-Object System.Collections.Generic.List[int] }
    $byParent[$p.ParentProcessId].Add([int]$p.ProcessId)
  }
  $roots = Get-Process -Name $name -ErrorAction SilentlyContinue
  if (-not $roots) { return @() }
  $pids = New-Object 'System.Collections.Generic.HashSet[int]'
  foreach ($r in $roots) { foreach ($d in (Get-Descendants $r.Id $byParent)) { [void]$pids.Add($d) } }
  ,@($pids)
}

function Get-GpuPercent($pids) {
  # Sums 3D engine utilization percentages for our PIDs (quick metric; use PresentMon for rigor).
  try { $c = Get-Counter '\GPU Engine(*)\Utilization Percentage' -ErrorAction Stop }
  catch { return $null }
  $sum = 0.0; $want = @{}; foreach ($p in $pids) { $want[[int]$p] = $true }
  foreach ($s in $c.CounterSamples) {
    if ($s.InstanceName -match 'pid_(\d+).*engtype_3D') {
      if ($want.ContainsKey([int]$matches[1])) { $sum += $s.CookedValue }
    }
  }
  [math]::Round($sum, 2)
}

function Pct($arr, [double]$p) {
  if (-not $arr -or $arr.Count -eq 0) { return $null }
  $s = $arr | Sort-Object
  $idx = [math]::Min($s.Count - 1, [math]::Max(0, [int][math]::Ceiling($p / 100.0 * $s.Count) - 1))
  [math]::Round($s[$idx], 2)
}
function Stat($arr, [string]$name) {
  if (-not $arr -or $arr.Count -eq 0) { return [pscustomobject]@{ metric=$name; mean=$null; median=$null; p95=$null; p99=$null; max=$null; n=0 } }
  [pscustomobject]@{
    metric = $name
    mean   = [math]::Round(($arr | Measure-Object -Average).Average, 2)
    median = Pct $arr 50
    p95    = Pct $arr 95
    p99    = Pct $arr 99
    max    = [math]::Round(($arr | Measure-Object -Maximum).Maximum, 2)
    n      = $arr.Count
  }
}

# -- Setup --------------------------------------------------------------------
if (-not (Get-Process -Name $rootName -ErrorAction SilentlyContinue)) {
  throw "Процесс '$rootName' не запущен. Сначала запусти приложение (RELEASE-сборку), потом скрипт."
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$cores = (Get-CimInstance Win32_ComputerSystem).NumberOfLogicalProcessors
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$csv   = Join-Path $OutDir "$Label-$stamp-samples.csv"
$sumCsv= Join-Path $OutDir "$Label-$stamp-summary.csv"

Write-Host "[bench] root='$rootName'  cores=$cores  dur=${DurationSec}s  warmup=${WarmupSec}s  every=${IntervalSec}s" -ForegroundColor Cyan

# -- PresentMon (optional, parallel) ------------------------------------------
$pmProc = $null; $pmCsv = $null
if ($PresentMonPath -and (Test-Path $PresentMonPath)) {
  if (-not $PresentProcess) { $PresentProcess = "$rootName.exe" }
  $pmCsv = Join-Path $OutDir "$Label-$stamp-presentmon.csv"
  $pmArgs = @('--process_name', $PresentProcess, '--output_file', $pmCsv,
              '--timed', "$DurationSec", '--terminate_after_timed', '--stop_existing_session')
  Write-Host "[bench] PresentMon → $PresentProcess (CSV: $pmCsv)" -ForegroundColor DarkCyan
  try { $pmProc = Start-Process -FilePath $PresentMonPath -ArgumentList $pmArgs -PassThru -WindowStyle Hidden }
  catch { Write-Warning "PresentMon не запустился: $_"; $pmProc = $null }
}

# -- Sampling loop ------------------------------------------------------------
$prevCpu = @{}
$rows = New-Object System.Collections.Generic.List[object]
$cpuArr=@(); $gpuArr=@(); $ramArr=@()
$nTotal = [int][math]::Ceiling($DurationSec / $IntervalSec)
for ($i = 0; $i -lt $nTotal; $i++) {
  $t0 = Get-Date
  $pids = Resolve-Tree $rootName
  if (-not $pids -or $pids.Count -eq 0) { Write-Warning "дерево пустое (процесс закрылся?)"; break }
  $procs = Get-Process -Id $pids -ErrorAction SilentlyContinue
  $cpuDelta = 0.0; $ram = 0.0; $seen = @{}
  foreach ($p in $procs) {
    $seen[$p.Id] = $true
    $ram += $p.WorkingSet64
    if ($prevCpu.ContainsKey($p.Id)) { $cpuDelta += ($p.CPU - $prevCpu[$p.Id]) }  # CPU seconds during the interval
    $prevCpu[$p.Id] = $p.CPU                                                       # New PIDs: zero delta (init)
  }
  $gpu = Get-GpuPercent $pids
  $elapsed = ((Get-Date) - $t0).TotalSeconds
  $cpuPct = if ($elapsed -gt 0) { [math]::Round($cpuDelta / $IntervalSec / $cores * 100, 2) } else { 0 }
  $ramMB  = [math]::Round($ram / 1MB, 1)
  $warm   = ($i * $IntervalSec) -lt $WarmupSec

  $rows.Add([pscustomobject]@{
    t_sec = [math]::Round($i * $IntervalSec, 1); warmup = $warm
    cpu_pct = $cpuPct; gpu_pct = $gpu; ram_mb = $ramMB; n_proc = $procs.Count
  })
  if (-not $warm -and $i -gt 0) {  # At i=0 the CPU delta is zero (init), so skip it
    $cpuArr += $cpuPct; $ramArr += $ramMB; if ($null -ne $gpu) { $gpuArr += $gpu }
  }
  if (($i % 10) -eq 0) {
    $tag = if ($warm) { 'warmup' } else { 'measure' }
    Write-Host ("  t={0,5}s [{1}] cpu={2,5}%  gpu={3,5}%  ram={4,7}MB  proc={5}" -f $rows[-1].t_sec,$tag,$cpuPct,$gpu,$ramMB,$procs.Count)
  }
  $sleep = $IntervalSec - ((Get-Date) - $t0).TotalSeconds
  if ($sleep -gt 0) { Start-Sleep -Milliseconds ([int]($sleep * 1000)) }
}

$rows | Export-Csv -Path $csv -NoTypeInformation -Encoding utf8
Write-Host "[bench] посемпловый CSV: $csv" -ForegroundColor Green

# -- Summary ------------------------------------------------------------------
$summary = @(
  (Stat $cpuArr 'cpu_pct'),
  (Stat $gpuArr 'gpu_pct'),
  (Stat $ramArr 'ram_mb')
)
$summary | Export-Csv -Path $sumCsv -NoTypeInformation -Encoding utf8
Write-Host "`n=== СВОДКА ($Label, после warmup, n=$($cpuArr.Count)) ===" -ForegroundColor Yellow
$summary | Format-Table -AutoSize

# -- PresentMon frame analysis ------------------------------------------------
if ($pmProc) {
  try { $pmProc | Wait-Process -Timeout ($DurationSec + 30) -ErrorAction SilentlyContinue } catch {}
  if ($pmCsv -and (Test-Path $pmCsv)) {
    try {
      $pm = Import-Csv $pmCsv
      $col = ($pm[0].psobject.Properties.Name | Where-Object { $_ -match '^ms?Between?Presents$|MsBetweenPresents' } | Select-Object -First 1)
      if (-not $col) { $col = ($pm[0].psobject.Properties.Name | Where-Object { $_ -match 'BetweenPresents' } | Select-Object -First 1) }
      if ($col) {
        $ft = $pm | ForEach-Object { [double]$_.$col } | Where-Object { $_ -gt 0 }
        $dropCol = ($pm[0].psobject.Properties.Name | Where-Object { $_ -match 'Dropped' } | Select-Object -First 1)
        $drops = if ($dropCol) { ($pm | Where-Object { [int]$_.$dropCol -ne 0 }).Count } else { 'n/a' }
        $fps = [math]::Round(1000.0 / (($ft | Measure-Object -Average).Average), 1)
        Write-Host "=== PresentMon ($PresentProcess) ===" -ForegroundColor Yellow
        Write-Host ("  fps~{0}  frame_ms p50={1} p95={2} p99={3} max={4}  dropped={5}  frames={6}" -f `
          $fps, (Pct $ft 50), (Pct $ft 95), (Pct $ft 99), ([math]::Round(($ft|Measure-Object -Maximum).Maximum,2)), $drops, $ft.Count)
      } else { Write-Warning "PresentMon CSV: не нашёл колонку времени кадра — открой $pmCsv вручную." }
    } catch { Write-Warning "разбор PresentMon CSV не удался: $_  (CSV сохранён: $pmCsv)" }
  }
}

Write-Host "`n[bench] Готово. Запусти то же на ВТОРОМ бинаре с тем же -DurationSec и сравни МЕДИАНЫ." -ForegroundColor Cyan
