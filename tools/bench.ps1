<#
.SYNOPSIS
  Замер CPU / GPU / RAM по ВСЕМУ дереву процессов приложения (Tauri или нативный
  Rust) для честного сравнения рендеров. Опционально — кадры/латентность через
  PresentMon.

.DESCRIPTION
  Tauri на Windows — это дерево процессов: app.exe (Rust-бэк) + несколько
  msedgewebview2.exe (renderer/GPU/utility). Рендер чарта и present живут в
  WebView2-процессах, НЕ в app.exe. Замер только app.exe врёт. Скрипт находит
  КОРЕНЬ по имени и суммирует его + ВСЕХ потомков (для натива потомков нет —
  считается один процесс, та же методика → симметрично).

  Метрики (семпл раз в -IntervalSec, первые -WarmupSec отбрасываются):
    - CPU% дерева, нормированный на число логических ядер (0..100 = вся машина).
    - RAM (working set) дерева, МБ.
    - GPU% дерева — счётчики '\GPU Engine(*)\Utilization Percentage', движок 3D,
      суммарно по нашим PID (быстрая метрика; для строгой бери PresentMon).
  Сводка: mean / median / p95 / p99 / max. Пишет посемпловый CSV + сводку.

.EXAMPLE
  # Натив (один процесс)
  ./tools/bench.ps1 -RootProcess moon-terminal -DurationSec 300 -Label native

.EXAMPLE
  # Tauri (app + WebView2-потомки) + кадры через PresentMon по GPU-процессу WebView2
  ./tools/bench.ps1 -RootProcess moon-terminal -DurationSec 300 -Label tauri `
     -PresentMonPath C:\tools\PresentMon.exe -PresentProcess msedgewebview2.exe

.NOTES
  Запускать ОТ АДМИНА (GPU-счётчики и PresentMon требуют прав). Закрыть DevTools.
  Сравнивай два бинаря в одинаковых условиях: одинаковый размер окна (физ. пиксели),
  один монитор/DPI, 60fps-cap, та же сцена/нагрузка (см. MOON_SYNTH в README ниже),
  RELEASE-сборки, окно на переднем плане. Делай N>=3 прогонов, сравнивай МЕДИАНЫ.
#>
[CmdletBinding()]
param(
  # Имя корневого процесса БЕЗ .exe (например moon-terminal). Суммируем его + потомков.
  [Parameter(Mandatory)] [string] $RootProcess,
  [int]    $DurationSec   = 300,
  [double] $IntervalSec   = 1.0,
  [int]    $WarmupSec     = 30,
  [string] $Label         = 'run',
  [string] $OutDir        = (Join-Path $PSScriptRoot 'bench-out'),
  # PresentMon (кадры/латентность). Если путь задан — запускаем параллельно.
  [string] $PresentMonPath = '',
  # Какой процесс ловит present: натив = $RootProcess.exe; Tauri = msedgewebview2.exe.
  [string] $PresentProcess = ''
)

$ErrorActionPreference = 'Stop'
$rootName = $RootProcess -replace '\.exe$',''

# ── Утилиты ──────────────────────────────────────────────────────────────────
function Get-Descendants([int]$rootPid, $procTable) {
  # BFS по ParentProcessId. Возвращает множество PID (корень + все потомки).
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
  # Карта parent->children один раз, затем дерево от каждого корня с таким именем.
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
  # Сумма Utilization % движка 3D по нашим PID (быстрая метрика; строгая — PresentMon).
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

# ── Подготовка ───────────────────────────────────────────────────────────────
if (-not (Get-Process -Name $rootName -ErrorAction SilentlyContinue)) {
  throw "Процесс '$rootName' не запущен. Сначала запусти приложение (RELEASE-сборку), потом скрипт."
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$cores = (Get-CimInstance Win32_ComputerSystem).NumberOfLogicalProcessors
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$csv   = Join-Path $OutDir "$Label-$stamp-samples.csv"
$sumCsv= Join-Path $OutDir "$Label-$stamp-summary.csv"

Write-Host "[bench] root='$rootName'  cores=$cores  dur=${DurationSec}s  warmup=${WarmupSec}s  every=${IntervalSec}s" -ForegroundColor Cyan

# ── PresentMon (опционально, параллельно) ────────────────────────────────────
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

# ── Цикл семплов ─────────────────────────────────────────────────────────────
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
    if ($prevCpu.ContainsKey($p.Id)) { $cpuDelta += ($p.CPU - $prevCpu[$p.Id]) }  # сек CPU за интервал
    $prevCpu[$p.Id] = $p.CPU                                                       # новые PID: дельта 0 (init)
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
  if (-not $warm -and $i -gt 0) {  # i=0 дельта-CPU нулевая (init) — пропускаем
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

# ── Сводка ───────────────────────────────────────────────────────────────────
$summary = @(
  (Stat $cpuArr 'cpu_pct'),
  (Stat $gpuArr 'gpu_pct'),
  (Stat $ramArr 'ram_mb')
)
$summary | Export-Csv -Path $sumCsv -NoTypeInformation -Encoding utf8
Write-Host "`n=== СВОДКА ($Label, после warmup, n=$($cpuArr.Count)) ===" -ForegroundColor Yellow
$summary | Format-Table -AutoSize

# ── Разбор PresentMon (кадры) ────────────────────────────────────────────────
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
