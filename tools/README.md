# tools/

| What | Where |
|---|---|
| Tutorial page generator (`docs/tour/index.html`) | [`tour/`](tour/README.md) |
| Render benchmark (native Rust vs Tauri) | this file, below |

---

# Benchmark: native Rust vs Tauri (CPU / GPU / RAM / frames)

Tools for an honest comparison of two renderers under **the same reproducible load**.

## What is included

- **`bench.ps1`** — measures CPU/GPU/RAM over the WHOLE process tree (native = one process;
  Tauri = `app.exe` + `msedgewebview2.exe` children), + frames/latency via PresentMon.
- **Synth/stress mode** (env flags) — both binaries ramp the load themselves: every N seconds
  they open a new chart WINDOW with M synth panels, up to K windows. The data is a **deterministic
  synthetic feed** (the same in both). Native implementation: [`crates/moon-core/src/feed/synth.rs`](../crates/moon-core/src/feed/synth.rs).

## Why synthetic data rather than a live market

Market noise kills the comparison (activity differs every run). The synth feed drives a
**fixed rate** of ticks/order book with a seed → the load is identical across runs AND across
binaries. Real servers still connect as usual (their background also lands in the measurement) —
synthetic data is only on the test panels. For a **clean** render comparison keep real cores
off, or compare the ramp delta (RAM/CPU before and after the windows open).

## Env flags (the same for both binaries)

| Flag | Def. | What |
|------|------|-----|
| `MOON_SYNTH` | — | turn on the synth core (markets `SYNTH0..`), next to the real ones |
| `MOON_SYNTH_MARKETS` | =charts | how many synth markets to feed |
| `MOON_SYNTH_TPS` | 50 | ticks/sec per market |
| `MOON_SYNTH_BOOKHZ` | 20 | order-book updates/sec per market |
| `MOON_SYNTH_DEPTH` | 50 | levels per side of the order book |
| `MOON_SYNTH_SEED` | 1 | price-walker seed (reproducibility) |
| `MOON_STRESS` | — | turn on the window stress ramp |
| `MOON_STRESS_INTERVAL_MS` | 10000 | period for a new window to appear |
| `MOON_STRESS_WINDOWS` | 10 | max windows |
| `MOON_STRESS_CHARTS` | 5 | panels in each window |

Default outcome: **10 windows × 5 panels = 50 panels** at once, growing by 5 every
10 s. CPU/GPU/RAM grow in steps — you see how the renderer scales with the number of windows/panels.

## Procedure (Windows, as administrator)

### 0. Level the conditions
Both windows — **the same size in physical pixels**, one monitor/DPI, 60 Hz, High
performance power, in the foreground. DevTools closed.

### 1. Build both in RELEASE
```powershell
# Native
cargo build --release          # → target/release/moonterminal.exe
# Tauri
npm run tauri build            # → src-tauri/target/release/<exe>
```

### 2. Run with the synthetic/stress load
```powershell
$env:MOON_SYNTH=1; $env:MOON_STRESS=1
& .\target\release\moonterminal.exe
```
Let the windows open: 10×10s ≈ **100 s** until the ramp is complete.
**Do not close or touch the windows** — that is the load being measured; it holds while
the application is alive.

### 3. Measure (from another terminal window)
```powershell
# Native — one process. WarmupSec ≥ windows×interval, so the measurement runs AFTER the ramp.
./tools/bench.ps1 -RootProcess moonterminal -DurationSec 175 -WarmupSec 115 -Label native

# Tauri — process tree + frames via PresentMon (the GPU counter for Tauri is unreliable, see below)
./tools/bench.ps1 -RootProcess <tauri-exe> -DurationSec 175 -WarmupSec 115 -Label tauri `
   -PresentMonPath C:\tools\PresentMon.exe -PresentProcess msedgewebview2.exe
```

### 4. Compare
`tools/bench-out/*-summary.csv` — CPU/GPU/RAM medians. The PresentMon row — fps, frame_ms
p50/p95/**p99**, dropped. Do **N≥3** runs of each, compare medians. Tails (p99,
dropped) matter more than the average — a steady delay is less visible to the eye than one drop.

## Pitfalls (found the hard way)

- **The WHOLE process tree** of the root is counted (`-RootProcess` + children). For Tauri this
  automatically includes every `msedgewebview2.exe` — do NOT count only `app.exe`.
- **`bench.ps1` in UTF-8 without BOM** is not parsed by Windows PowerShell **5.1** (Cyrillic breaks
  the tokenizer). Run it through **PowerShell 7 (`pwsh`)** or from a copy of the file with a UTF-8 BOM.
- **GPU via `Get-Counter '\GPU Engine(*)'` slows badly with many windows** (the number of
  GPU instances explodes at 10 windows → each sample gets slower, the loop can “hang”
  for minutes). For strict GPU/frames use **PresentMon** (`-PresentMonPath`), not
  the built-in counter. For native the present-process = the exe itself; for Tauri = `msedgewebview2.exe`.
- **`WarmupSec` ≥ `windows × interval_ms/1000`** (for the default — ≥100s, take 115), otherwise the
  ramp phase lands in the measurement and the medians drift.
- **RAM working set** on a GPU application (Vulkan/ReBAR) includes the VRAM mapping — the absolute
  number is large; look at the ramp **delta**, not the absolute. This is not OOM.

## Reference numbers (native, 10 windows × 5 panels, n=60 once stable)

Taken on 24 logical cores, 61.6 GB RAM, on top of a live real session:

| Metric | mean | median | p95 | p99/max |
|---|---|---|---|---|
| CPU % (whole machine) | 7.38 | **7.1** | 8.92 | 10.94 |
| GPU % (3D engine) | 12.34 | **11.73** | 16.89 | 17.43 |

Ramp RAM delta ≈ **+2.4 GB** (35.1 → 37.9 GB as 10 windows open). This is a landmark, not an
absolute — for comparison with Tauri the CPU/GPU medians and PresentMon frame_ms matter.
