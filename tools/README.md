# tools/

| What | Where |
|---|---|
| Tutorial page generator (`docs/tour/index.html`) | [`tour/`](tour/README.md) |
| Render benchmark (native terminal) | this file, below |
| Telegram Mini App preview and interaction check | [`miniapp_preview/`](#telegram-mini-app-preview), below |

## Telegram Mini App preview

`node tools/miniapp_preview/preview.mjs [--out <dir>] [--locale ru|en|es] [--only <screen>]`
renders every Mini App screen from the synthetic payloads in `miniapp_preview/fixtures/` (shaped
like `crates/moon-core/src/telegram/web/dto.rs`) at 421x900 and 390x844, light and dark, into
`<dir>` (default `tools/miniapp_preview/out/`, ignored), then checks the owner commands the page
sends and exits non-zero on a failure or a page error. No core or bot is needed. Install its two
packages once with `npm install --prefix tools/miniapp_preview` (`playwright-core`, `js-yaml`;
nothing is committed); it drives the system Chrome from the standard install path on Windows,
macOS or Linux, or `CHROME_PATH` when set.

---

# Benchmark: native terminal load (CPU / GPU / RAM / frames)

`bench.ps1` measures CPU, GPU, RAM and, optionally, PresentMon frames for one process tree
under a reproducible synthetic load. This repository builds the native terminal. The script
can also time some other root process you already have; it does not build one.

## What is included

- **`bench.ps1`** — measures CPU/GPU/RAM over the WHOLE process tree of `-RootProcess`.
  The native terminal is one process. A host that spawns children is summed with those
  children, plus frames/latency via PresentMon when you pass a path.
- **Synthetic feed** — the plaintext synthetic core emits one batch of AddToChart detects
  and then a deterministic tick and order-book stream. Implementation:
  [`crates/moon-core/src/feed/synth.rs`](../crates/moon-core/src/feed/synth.rs).
  There is no timed ramp.

## Why synthetic data rather than a live market

Market noise kills the comparison (activity differs every run). The synth feed drives a
**fixed rate** of ticks and order-book updates with a seed, so repeated runs match.
`MOON_CONFIG_PLAINTEXT` replaces the saved core list for that launch (`servers.enc` is not
read), so real cores are not connected and their traffic is not in the measurement. Other
settings still load.

## Env flags

| Flag | Def. | What |
|------|------|-----|
| `MOON_CONFIG_PLAINTEXT` | — | use the env core instead of `servers.enc` |
| `MOON_CONFIG_PLAINTEXT_SYNTHETIC` | — | that core runs `feed::synth` (no network, no API key) |
| `MOON_SYNTH_MARKETS` | = `MOON_STRESS_CHARTS` | how many synth markets (`SYNTH0`..) |
| `MOON_SYNTH_TPS` | 50 | ticks/sec per market |
| `MOON_SYNTH_BOOKHZ` | 20 | order-book updates/sec per market |
| `MOON_SYNTH_DEPTH` | 50 | levels per side of the order book |
| `MOON_SYNTH_SEED` | 1 | price-walker seed |
| `MOON_STRESS_WINDOWS` | 10 | AddToChart tabs emitted at startup (`add_to_chart` 1..=N) |
| `MOON_STRESS_CHARTS` | 5 | panels on each of those tabs |

`MOON_SYNTH`, `MOON_STRESS` and `MOON_STRESS_INTERVAL_MS` are not read. The tabs are not
opened on a timer: `synth::run` sends every detect in one `FeedMsg::Detects` before the
tick loop, and chart-tab ingest turns each `add_to_chart` number into one tab.

Default outcome: **10 AddToChart tabs × 5 panels = 50 panels**, created together at startup.

## Procedure (Windows, as administrator)

### 0. Level the conditions
Runs you compare — **the same size in physical pixels**, one monitor/DPI, 60 Hz, High
performance power, in the foreground. DevTools closed.

### 1. Build the native terminal in RELEASE
```powershell
cargo build --release          # → target/release/moonterminal.exe
```

### 2. Run the synthetic load
```powershell
$env:MOON_CONFIG_PLAINTEXT=1; $env:MOON_CONFIG_PLAINTEXT_SYNTHETIC=1
& .\target\release\moonterminal.exe
```
The AddToChart tabs come from that first detect batch. **Do not close them** — that is the
load being measured; it holds while the application is alive.

### 3. Measure (from another terminal window)
```powershell
# Native — one process. WarmupSec only discards the opening samples; there is no ramp.
./tools/bench.ps1 -RootProcess moonterminal -DurationSec 175 -WarmupSec 30 -Label native
```

`bench.ps1` can point `-RootProcess` at another executable and, for a WebView2 tree, add
`-PresentMonPath` and `-PresentProcess msedgewebview2.exe`. This repository has no
`src-tauri` tree and no `npm run tauri build`.

### 4. Compare
`tools/bench-out/*-summary.csv` — CPU/GPU/RAM medians. The PresentMon row — fps, frame_ms
p50/p95/**p99**, dropped. Do **N≥3** runs of each, compare medians. Tails (p99,
dropped) matter more than the average — a steady delay is less visible to the eye than one drop.

## Pitfalls (found the hard way)

- **The WHOLE process tree** of the root is counted (`-RootProcess` + children). For a
  WebView2 host that includes every `msedgewebview2.exe` — do NOT count only the parent exe.
  The native terminal has no children, so the same flag measures one process.
- **`bench.ps1` in UTF-8 without BOM** is not parsed by Windows PowerShell **5.1** (Cyrillic breaks
  the tokenizer). Run it through **PowerShell 7 (`pwsh`)** or from a copy of the file with a UTF-8 BOM.
- **GPU via `Get-Counter '\GPU Engine(*)'` slows badly with many charts** (the number of
  GPU instances explodes at the default 10 tabs → each sample gets slower, the loop can “hang”
  for minutes). For strict GPU/frames use **PresentMon** (`-PresentMonPath`), not
  the built-in counter. For the native terminal the present-process is the exe itself; for a
  WebView2 tree it is `msedgewebview2.exe`.
- **`WarmupSec` discards the opening samples** (`bench.ps1` defaults it to 30). It is not a
  ramp wait: nothing reads `MOON_STRESS_INTERVAL_MS`, and the default 10 tabs are emitted
  together.
- **RAM** in the summary is the process tree's working set (`WorkingSet64`). On this GPU
  terminal that absolute number is large; a big figure there is not by itself OOM.

## Reference numbers (native, 10 × 5 panels, n=60 once stable)

Taken on 24 logical cores, 61.6 GB RAM, on top of a live real session. The "ramp" below is
how that older run opened its charts; a current plaintext synthetic launch emits the default
10 tabs together and does not keep real cores connected:

| Metric | mean | median | p95 | p99/max |
|---|---|---|---|---|
| CPU % (whole machine) | 7.38 | **7.1** | 8.92 | 10.94 |
| GPU % (3D engine) | 12.34 | **11.73** | 16.89 | 17.43 |

Ramp RAM delta ≈ **+2.4 GB** (35.1 → 37.9 GB as 10 windows open). This is a landmark, not an
absolute — across runs, the CPU/GPU medians and PresentMon frame_ms are what to compare.
