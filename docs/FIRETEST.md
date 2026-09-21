# FireTest

Last updated: 2026-07-31.

FireTest is a built-in debug/test scenario runner for finding expensive UI mistakes in the hot chart path.

## Running on Windows

```powershell
$vcvars = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat'
cmd.exe /d /s /c "`"$vcvars`" && cargo build -p moon-ui-gpui --bin moonterminal --target x86_64-pc-windows-msvc"
Remove-Item -ErrorAction SilentlyContinue firetest.log, logs\render_diag.log
.\target\x86_64-pc-windows-msvc\debug\moonterminal.exe --debug-script chart-smoke
```

`chart-smoke` is one connected behavioural run. It waits for the application to start, opens the BTC chart, resolves the chart's real bounds, gives the live chart a short settle phase, warms a high-present baseline without a cursor, then for 5 seconds moves the system mouse over the chart with a frequent native mousemove storm. After that it turns on static text stress on the chart and repeats the mouse storm. Only after the hot chart path FireTest checks the runtime contract of command-delivery errors into the core, opens the Settings/Strategies/Assets tool windows and checks their dedup, checks the Root-owned overlay layer on a real window, then switches the interface language through the live apply path (`rust_i18n::set_locale` + `refresh_windows`) and checks that it lands without recreating tool windows, checks that scale `50% → 20% → Auto` reaches the active chart state, and then may run an opt-in test of placing and cancelling a real BTC order. On Windows the storm is done through a real `SetCursorPos` into the window's client area, on macOS — through CoreGraphics mouse move events. In both cases this is a real windowed input path, not a direct call of the chart API.

`order-cancel-lag` is a separate narrow scenario for investigating the order path only. It does `start → open_chart → wait_chart_probe → settle → order_cancel_lag → cooldown` and automatically turns on the real place/cancel test without mouse storm, static text and tool-window stages. Use it only deliberately: the scenario sends a trading command into the selected core.

On a macOS test machine the terminal/application may need the Accessibility or Input Monitoring right: that is macOS policy for programmatically sending mouse events.

## The committed-data stand (`--fixture`)

`chart-smoke` can be run not against the live market, but against a committed data set — then the run
is deterministic: the same candles, the same trades and the same figures on every launch.

```powershell
$env:MOON_FIRETEST_MARKET = "ACEUSDT"
.	arget_64-pc-windows-msvc\debug\moonterminal.exe --fixture --debug-script chart-smoke
```

The `--fixture [name]` flag is standalone and is not tied to FireTest: without `--debug-script` it simply
opens the application on the set, to look at it with your eyes. The set (`fixtures/chart-ace` by default)
is copied into a temporary directory, the data root is redirected there too, and a synthetic
core is raised with no network — the run does not open live databases. The data is shifted in time so that the day
of trades lands under the current moment, otherwise a chart following the live edge would show emptiness.

What is in the set: a day of ACEUSDT candles, 544 closed trades (anonymised) and one figure of each
drawing tool. A run without `--fixture` works exactly as before — the stand is activated only
by this flag.

- `MOON_FIXTURE_DIR` — where to look for sets, if they are not next to the exe and not in the repository root.
- `MOON_FIXTURE_FIGURES=0` — do not put figures on the stand. This is a switch for an A/B measurement: a run with
  figures and without them differs only by that, so the difference in counters is the cost of the figures themselves.
- `MOON_FIXTURE_LAYOUT` / `MOON_FIXTURE_SETTINGS` — a `layout.toml` / `settings.toml` copied into the stand
  before the configuration is read, so a run opens with a chosen layout or, for example, at
  `ui_scale = 1.5` to check input and overlays under UI zoom.

Details of the set's layout and regeneration are in the internal documentation.

## Settings

- `MOON_FIRETEST_MARKET` — the market, default `BTCUSDT`.
- `MOON_FIRETEST_MOUSE_HZ` — target frequency of the mousemove storm, default `5000`.
- `MOON_FIRETEST_STORM_MS` — storm duration, default `5000`.
- `MOON_FIRETEST_TEXT_LABELS` — number of retained text labels in static text stress, default `10000`.
- `MOON_FIRETEST_FLASH_MS` — duration of the `arrival_flash` stage window, default `5000` (the same as `idle_floor` measures — the windows are compared with each other). The first 1200 ms of the window are not sampled, so by default ~4 seconds count; if you want more points — raise this variable, do not expect more from the default.
- `MOON_ARRIVAL_FLASH=0` — globally turns off the arrival frame of a new chart (`0`/`false`/`no`/`off`). This is NOT a FireTest variable: the application itself reads it once at start and it also applies to a normal launch. The control side of the frame A/B measurement.
- `MOON_FIRETEST_ORDER_CANCEL=1` — turns on the real place/cancel order test on the open BTC chart. Off by default, so ordinary FireTest does not send trading commands.
- `MOON_FIRETEST_ORDER_SIZE` — explicit size of the test order in the base coin. If unset, `MOON_FIRETEST_ORDER_QUOTE_SIZE / order_price` is used, and if quote-size is also unset — the group's USD equivalent is converted into the base coin at the current base/USD rate. If there is no valid rate the scenario ends with an error, rather than sending the size as-is.
- `MOON_FIRETEST_ORDER_QUOTE_SIZE` — size of the test order in the quote currency (for BTCUSDT that is USDT). For example `500` with `MOON_FIRETEST_ORDER_PRICE_MULT=0.95` gives quantity `500 / (latest_price * 0.95)`.
- `MOON_FIRETEST_ORDER_PRICE_MULT` — multiplier on the last price for the test long-limit order, default `0.98`. The order is placed below the market so the test checks display/cancel, not accidental fill.
- `MOON_FIRETEST_ORDER_CANCEL_MAX_DISPLAY_MS` — allowed delay from applying a cancelled order in the store to the first chart present/draw with that order-line revision, default `750`.
Static text stress is part of standard `chart-smoke`: FireTest itself turns on
`10000` retained text labels after the first mouse storm. This does not mean
“draw every string over one viewport”: the layer bakes the whole set,
and present frames draw only the visible label-range. So the test checks exactly
retained buffer + culling, not a pointless GPU flood of thousands of unreadable
labels. New shared checks are added as stages of this same run. Narrow
diagnostic scenarios are allowed only when the shared run gets in the way of isolating
another problem, as `order-cancel-lag` for the delay of showing an order cancel.

## The idle floor

Between `settle_live_chart` and `baseline` comes stage `idle_floor`: a live chart is open, but
nobody is touching it and present-pressure is **off**. This is the only stage that
measures the application when nobody is forcing it to work. The live BTC feed legally drives
the chart's own pass, so what it catches is not “zero work”, but wake-ups of the GPUI layer without
input: a broadcast on every tick, a panel repainting on a revision that did not change,
a timer nobody needs.

It sits exactly HERE, before `static_text_gap` and the tool windows, on purpose: the text layer has no
off path, and tool windows are not closed, so an idle window after them would be measuring idle
together with 10000 retained labels and three open windows — and calling that the floor.

The stage writes the line `[firetest] idle_floor avg/max …`. **avg and max for every counter
are deliberate**: a single hot second — a arriving-news tint fading, a feed spike —
looks the same in a peak alone as a panel that spins forever, and they are cured
in opposite ways. Thresholds are calibrated from three live runs, not assigned: news and detached
are gated on the average only because their 115/s peak turned out to be a legitimate
tint fade.

**This calibration is stale and waiting for a revisit.** News tint no longer follows vblank: it
repaints the panel on `crate::pulse::PULSE_TICK` (10 Hz), so the 115/s peak went from legitimate
to a defect. The average-only gate remained from those days — it must be lifted by
a run where `news_render` is visible together with a non-zero `pulse_tick`.

## The arrival frame: the `arrival_flash` stage

Right after `idle_floor` and in the same cold mode comes `arrival_flash`: FireTest itself lights
the arrival frame on EVERY live chart and keeps it lit for the whole window (re-lights every
2400 ms, because own-pass extinguishes it after 2600 ms). Then it extinguishes it explicitly — `baseline` must not
inherit a dying frame.

The stage exists because the cost of the frame cannot be derived from reading the code and cannot be waited for:
a live detect is not scheduled, and a run that got not a single arrival reads
exactly as “the frame is free”. The frame lives in own-pass and costs **presents**, and present is window-wide:
every neighbouring canvas in the window re-runs its pass, including text shaping. Storm
stages cannot see this — they already force present.

The verdict writes the line `[firetest] arrival_flash idle->flash …`: `chart_present`, `chart_render`,
`shell_render` — per chart; `bg_draw`, `grid_draw`, `base_bake`, `combo_bake`, `userdata_draw`,
`chart_gpu_prepare` — per frame; CPU, `gpu_frame_ms` and `pulse_per_chart`. Each phase is divided by
its OWN chart count: the stand snapshot is taken at the end of `idle_floor`, and live detects open charts
after it — of ten recorded runs four grew right inside the flash window, one from 1 to 4.

**This stage has no ceiling, and that is a measured decision, not a deferred one.** Ten runs
(2026-08-08) gave: the flash costs ~+10 present/s per burning chart — exactly the 10 Hz at which
own-pass paces it; `chart_render` does not move (median +0.15/s), and work PER FRAME even
drops (`bg_draw` 0.74 → 0.59 per present — extra frames reuse the base cache). So
the flash costs frames, not work in a frame, and a ceiling here would gate the pacing constant, not
a regression. What would earn a ceiling is something else: a frequency other than 10 Hz per chart — and `pulse_per_chart`
already names that out loud.

What the stage DOES still fail is itself: with the frame on, `pulse_per_chart` must be
≥ 5/s (a lit frame bumps the counter 10 times a second per canvas), and with `MOON_ARRIVAL_FLASH=0` —
exactly zero. The first means “side A really was burning”, the second — “side B is truly
control”; without them a run with a broken hook would join the sample as “the frame costs nothing”.

`chart_arrival_pulse` is also printed in the `idle_floor` line — there it says whether a
real arrival happened during the cold window. Since 2026-08-08 it has a gate `7/s per chart ON THE AVERAGE`:
a stuck flash asks for present every 100 ms, i.e. holds 10/s forever, and one real
arrival is 2.6 s of a 5-second window, 5.2/s, and that is legal. On the average, not the peak,
exactly because duration, not frequency, distinguishes a stuck one from a spike: a stuck one's peak
is perfectly normal. In ten recorded runs a real arrival never landed in the idle window
once — the counter was 0/0 in all of them.

`idle_chart_render_avg_per_chart` is tightened there too: it was 150/s, became 30/s. The old 150 stood
because the frame was suspected of waking this counter, and the source of the wake was “not yet
found”. Now it is measured directly — stage `arrival_flash` does not move `chart_render` at all —
and the runs themselves gave 4.4–5.9/s per chart, so 150 sat 25 times above everything observed.

## Why the thresholds are normalised

The same binary on the same machine gives `chart_present` 119/s with one chart and 793/s with
five — how many charts a saved layout returns is not the code's decision. Therefore ceilings are counted
not in absolute values, but by the nature of the counter:

- **chart own-pass** (`bg_draw`, `grid_draw`, `combo_draw`, `userdata_draw`, `base_bake`,
  `combo_bake`, `orderbook_bake`, `chart_gpu_prepare`) — this is work PER FRAME, so it is measured
  as a ratio to `chart_present`: both as a delta over the baseline ratio, and as an absolute ceiling.
  Across 13 recorded runs the raw `bg_draw` delta wandered 0…109, the ratio — 0…0.13.
- **GPUI view renders** (`shell_render`, `orders_render`, `chart_render`) — NOT work per frame,
  they must not be divided by present (the ratio already wanders 0.04…0.80 in baseline). They have an absolute
  ceiling; for `chart_render` it is divided by the chart count, because the panel renders for every
  chart — but it is the LEVEL that is divided, not the delta.
- **A delta must not be divided by a stand unit at all**: baseline `c·b` minus storm `c·b+s` already
  cancels `c`, and dividing again simply makes a large stand more lenient.

All THREE storms — clean, static-text and `flash_storm` — go through the same three functions
(`check_storm_input` — input arrived and woke nobody, `check_storm` — work per frame,
`check_storm_load` — CPU/GPU/frame) with the same ceilings. They used to be counted together, and the cost of 10000
labels was attributed to cursor movement.

### `flash_storm` — the mouse OVER the flashing frame

A separate stage, because these two actions used to be measured separately: `arrival_flash` — the frame
without a cursor, `mouse_storm` — the cursor without a frame. In life they coincide (a detect opens a chart
while the user is already moving the mouse over the stack), and coinciding can cost more than the sum: both the cursor and the frame
ask own-pass for present and rebuild the readout. `flash_storm` runs the same storm, lighting
the frame before it starts and re-lighting it for the whole window, and is compared with the same `baseline`.

It sits BEFORE the text layer on purpose: the layer does not turn off, and a stage after it would be measuring three
variables instead of two.

The verdict writes `[firetest] flash_storm clean->flash …` — a comparison with the CLEAN storm, not with
baseline: the stage's question is “is the mouse over a flashing frame more expensive than the mouse without it”. Measured 2026-08-08
(2 runs): `present` 117 → 117/s, `chart_render` 6 → 6/s, `entity` and `input_notify` 0 → 0,
CPU 3.0 → 2.7%, work per frame +0.02 (`bg_draw` 0.23 → 0.25). So **the combination is not more expensive
than the sum**: the frame adds one instance to the readout batch in the frame and does not wake the entity path.

The bound of this measurement, which you need to know: the storms run under forced present, so the stage
answers “is the FRAME more expensive”, not “are there MORE FRAMES”. The second cannot grow under the mouse —
frames already hit the pacer, which the cursor saturates itself; how many frames the frame adds
WITHOUT a cursor is measured by the cold stage `arrival_flash` (+10/s per chart).

## What the test must catch

- at idle, with no input, the GPUI layer must not wake: Shell/Orders/Assets/News,
  `backend_notify`, the clock, the compactor and order-sync hold the measured ceiling;
- cursor-only mousemove must not wake the `ChartPanel` entity path;
- a UI command to a missing core must return a runtime error, not a successful no-op;
- the Settings/Strategies/Assets tool windows must open as real GPUI windows and a repeated open must focus the existing window, not create a second one;
- the Root-owned overlay layer must open a context menu, close it when a dialog opens, replace a unique dialog by id, show a notification and clear without hanging overlays;
- an interface-language change must reach the global rust-i18n locale live and MUST NOT recreate tool windows (redraw only);
- a scale choice from the toolbar-path must reach the active chart state: `50%`, then `20%`, then `Auto`;
- the opt-in place/cancel order test must measure the path `cancel_order` → inbound orders/server-log → `OrderLineStore` → chart userdata → GPU prepare → chart present/draw, and go red if the cancelled order reached the store but the chart keeps showing the old state for a long time;
- cursor-only mousemove must not do `cx.notify()` for chart input/canvas;
- static text stress over the chart must not break the mouse/input hot path and the GPU frame budget;
- Shell and Orders GPUI render hold an absolute ceiling (measured: 5.7/s avg, 11/s peak);
  `chart_render` is a GLOBAL counter, summed over every open `ChartPanel`, so its
  ceiling is divided by the chart count, otherwise it lies more the more charts are open;
- cursor-only mousemove must not raise the rate of expensive chart base draw/bake (`bg_draw`, `grid_draw`, `base_bake`, `combo_bake`, `orderbook_bake`) above baseline;
- `combo_draw_delta` remains a strict cross-platform signal: cursor-only mousemove must not add an expensive combo draw above baseline. If Metal/wgpu fail here, that is not a reason to weaken FireTest, but a signal to bring retained/base-cache parity up to the DX level.
- process CPU must not grow noticeably from mouse fuss alone;
- RAM must not grow;
- on Windows process GPU `%` is additionally written through PDH `GPU Engine`;
- on macOS system process GPU `%` is not faked: instead FireTest takes the real Metal `GPUStartTime/GPUEndTime` of a completed command buffer and checks `gpu_frame_ms`;
- the Linux mouse storm on X11 goes through XTest (`DISPLAY`/`XAUTHORITY` of the real test session). Wayland without XWayland/XTest remains a separate task: it needs a synthetic/platform test hook, `uinput` or a compositor-specific runner.

## Why there is a high-present baseline

A chart on live BTC can by itself bake base/combo often because of live-data and auto-Y. Therefore FireTest does not compare a mouse storm with a “quiet” idle. Before the storm it turns on the same frequent `gpu_canvas` present without a cursor and uses the maximum of the baseline samples as the rest. A red result means not “the market was active”, but “mousemove/readout added expensive work on top of an already hot chart-present mode”.

## The criterion

Each stage writes a log of the form:

```text
[firetest] stage=start
[firetest] stage=open_chart
[firetest] stage=wait_chart_probe
[firetest] stage=settle_live_chart
[firetest] stage=idle_floor
[firetest] stage=arrival_flash
[firetest] stage=baseline
[firetest] stage=mouse_storm
[firetest] stage=flash_storm
[firetest] stage=static_text_gap
[firetest] stage=static_text_warmup
[firetest] stage=static_text_storm
[firetest] stage=command_error_contract
[firetest] stage=tool_windows_open
[firetest] stage=tool_windows_verify_open
[firetest] stage=tool_windows_dedup
[firetest] stage=tool_windows_verify_dedup
[firetest] stage=root_overlay_contract
[firetest] stage=locale_switch
[firetest] stage=locale_switch_verify
[firetest] stage=price_scale_50
[firetest] stage=price_scale_20
[firetest] stage=price_scale_auto
[firetest] stage=price_scale_verify_auto
[firetest] stage=order_cancel_lag
[firetest] stage=cooldown
```

Success writes a `firetest.log` line:

```text
[firetest] result=PASS FIRETEST PASS ...
```

Failure writes `result=FAIL FIRETEST FAIL ... reasons=...` or
`result=FAIL FIRETEST FAIL reason=...` and exits the process with code `2`.

The test is deliberately red on regressions of the form “on mousemove someone again did a top-down render, notify, a heavy query, an allocating render path or an expensive GPU frame”. A screenshot is not a criterion of this test; FireTest checks behaviour and load.
