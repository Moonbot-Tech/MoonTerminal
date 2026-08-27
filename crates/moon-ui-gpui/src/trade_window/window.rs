//! Opening, placing and retiring a trade-detail window.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gpui::*;
use moon_core::db::{ChartTradeRecord, TradeMeta};
use moon_core::session::CoreId;
use moon_ui::{MoonBackgroundPolicy, Root};

use super::{TradeWindowState, TradeWindowView};
use crate::Backend;
use crate::panels::chart::ChartPanel;

/// Initial window size, in logical pixels.
const WIN_W: f32 = 1100.0;
const WIN_H: f32 = 720.0;

/// Smallest size at which the chart and the figures rail both stay readable.
const MIN_W: f32 = 720.0;
const MIN_H: f32 = 420.0;

/// Where the first trade window lands before any cascade.
const FIRST_ORIGIN: (f32, f32) = (160.0, 120.0);

/// How far each further window is offset from the previous one.
const CASCADE_STEP: f32 = 34.0;

/// Smallest visible area, in square logical pixels, that still makes a restored window reachable.
///
/// A remembered geometry outlives the monitors it was saved on — a laptop undocked, a display
/// rearranged, a resolution changed — and this window hides its taskbar button, so one restored
/// onto a screen that no longer exists is invisible AND unfocusable, which makes even its Escape
/// key unreachable. Below this much overlap with some attached display, the saved rectangle is
/// dropped and the default placement is used instead.
///
/// The number is "enough of the title bar to grab": roughly 200 logical pixels of width across the
/// ~30-pixel header. It is deliberately small — a window the user deliberately parked mostly
/// off-screen is a placement they chose, and this must not move it back on every reopen.
const MIN_VISIBLE_PX: u64 = 200 * 30;

/// How many trade windows may be open at once.
///
/// The goal asks for a second trade beside the first; it does not ask for a wall of them, and each
/// one holds a chart engine with its own GPU resources. Two is the stated requirement, enforced
/// rather than hoped for: a third open retires the oldest.
const MAX_WINDOWS: usize = 2;

/// Open — or focus — the trade-detail window for one closed trade.
///
/// Re-clicking a trade whose window is already open FOCUSES it rather than opening a duplicate:
/// two identical windows would be two identical fetches and two identical pictures.
///
/// Args:
///     backend: Shared application state.
///     record: The clicked trade, already resolved from the durable replica.
///     meta: What that trade carried beside its prices — the detect line, the strategy, the exit
///         reason — read in the same background pass as the record itself.
///     core: Core that recorded it.
///     market: Exchange-native market the coin resolved to.
///     stamps: Entry and exit times, already formatted in the Report's own clock.
///     cx: Application context.
pub(crate) fn open_trade_window(
    backend: &Entity<Backend>,
    record: ChartTradeRecord,
    meta: TradeMeta,
    core: CoreId,
    market: String,
    stamps: (String, String),
    cx: &mut App,
) {
    let key = (record.core_uid, record.record_id);
    let open: Vec<((u64, i64), WindowHandle<Root>)> = backend.read(cx).trade_windows.clone();
    if let Some((_, handle)) = open.iter().find(|(k, _)| *k == key) {
        let _ = handle.update(cx, |_, window, _| window.activate_window());
        return;
    }
    // Retire the oldest BEFORE opening, so the cap is never momentarily exceeded and the cascade
    // below counts the windows that will actually coexist.
    if open.len() >= MAX_WINDOWS {
        let excess = open.len() + 1 - MAX_WINDOWS;
        for (_, handle) in open.iter().take(excess) {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }
    let step = (backend.read(cx).trade_windows.len() as f32) * CASCADE_STEP;
    // ONE remembered rectangle for every trade window, not one per trade: the user adjusts this
    // window once and expects that shape back, and a per-trade key would mean the first open of
    // every new coin ignored every adjustment ever made.
    //
    // A rectangle that no attached display still covers is DROPPED rather than restored: this
    // window hides its taskbar button, so one opened onto a monitor that is gone can be neither
    // seen nor focused, which takes its Escape key away too.
    //
    // The test runs on the rectangle this window will ACTUALLY occupy — the remembered one plus
    // this open's cascade — not on the remembered one alone. A rectangle sitting just barely on the
    // edge of a screen passes on its own and is then pushed off it by the offset, which is exactly
    // the second window of the pair and exactly the case the check exists for.
    // The remembered STATE is read before the reachability filter below, which throws the whole
    // rectangle away when it lands off every monitor. A maximized window covers a screen by
    // construction, so its state survives even when its restore coordinates do not.
    let saved_state = backend
        .read(cx)
        .layout
        .trade_window
        .map_or((false, false), |geom| (geom.maximized, geom.fullscreen));
    let saved = backend.read(cx).layout.trade_window.filter(|geom| {
        let step = step as i32;
        let candidate = moon_core::config::layout::GeomRect {
            x: geom.x.saturating_add(step),
            y: geom.y.saturating_add(step),
            ..*geom
        };
        candidate.is_reachable_on(&crate::window::windowing::display_rects(cx), MIN_VISIBLE_PX)
    });
    // The display is resolved before the origin is adjusted, because the adjustment is expressed
    // relative to the display finally chosen. The saved identity outranks the saved coordinates,
    // which is what makes a restore survive the monitors being rearranged.
    let display_id = crate::window::windowing::saved_or_owner_display_id(
        saved.and_then(|geom| geom.display_uuid),
        saved.map(|geom| point(px(geom.x as f32), px(geom.y as f32))),
        None,
        None,
        cx,
    );
    let (origin, window_size) = match saved {
        // A SAVED origin is already absolute, so it takes the cascade and nothing else. Putting it
        // through `cascade_origin_on` would add the display's own origin on top and throw the
        // window off the very monitor it was remembered on.
        Some(geom) => (
            point(px(geom.x as f32 + step), px(geom.y as f32 + step)),
            size(px(geom.w as f32), px(geom.h as f32)),
        ),
        // The DEFAULT placement is a display-relative point, and Windows reads window coordinates
        // as GLOBAL: left as-is against a non-primary display it falls outside it, and the platform
        // layer silently replaces the whole rectangle with default bounds. Unchanged from before
        // this window remembered anything.
        None => (
            crate::window::windowing::cascade_origin_on(
                point(px(FIRST_ORIGIN.0 + step), px(FIRST_ORIGIN.1 + step)),
                display_id,
                cx,
            ),
            size(px(WIN_W), px(WIN_H)),
        ),
    };
    let theme = backend.read(cx).config.chart_theme().clone();
    let title = format!("MoonTerminal - {} - {}", record.coin, stamps.1);
    let mut opts = crate::window::windowing::trade_window_options(
        title,
        // The cascade offset is inert while maximized: every trade window then covers the same
        // screen, which is exactly what maximizing asked for, and the offset returns with the
        // restore rectangle underneath it.
        crate::window::windowing::window_bounds_for(
            saved_state.0,
            saved_state.1,
            Bounds {
                origin,
                size: window_size,
            },
        ),
        display_id,
        size(px(MIN_W), px(MIN_H)),
    );
    // The window body is transparent so the chart's own GPU pass shows through; the clear colour
    // is what supplies the background beneath it.
    let bg = theme.bg;
    opts.window_clear_color = Some(gpui::rgb(
        ((bg[0] as u32) << 16) | ((bg[1] as u32) << 8) | bg[2] as u32,
    ));
    // Resolved HERE, while the session is in hand: naming the strategy is a session lookup. Whether
    // it could be named travels with it — a core still connecting has no list yet, and the window
    // keeps asking until it does.
    let (resolved, named) = super::trade_labels(backend.read(cx), core, &meta);
    let labels = std::rc::Rc::new(resolved);
    let epoch = moon_chart::paint::now_unix_ms();
    // Kept for the failure log below, which the move into the window builder would otherwise take.
    let (coin, record_id) = (record.coin.clone(), record.record_id);
    let owner = backend.clone();
    let opened = cx.open_window(opts, move |window, cx| {
        crate::window::windowing::configure_chart_clear_color(window, cx);
        let panel_backend = owner.clone();
        let panel_market = market.clone();
        let panel = cx.new(|pcx| {
            // A HISTORICAL viewer, not a live chart: no order book and no trading controls. The
            // constructor is what decides that, so it cannot be undone by anything the panel is
            // told later.
            ChartPanel::new_historical(panel_backend, Some((core, panel_market)), epoch, theme, pcx)
        });
        // THE ONE-MINUTE PIN, and it must be in place BEFORE the first fetch answers.
        //
        // The replay is always fetched at one minute, but the rows are DRAWN at whatever timeframe
        // the panel asks for, and a fresh panel asks for the global `layout.candle_view` — five
        // minutes by default, or whatever the user's main chart is on. The read then resamples the
        // minute rows into that coarser bucket
        // (`moon-core/src/market/trade_replay/mod.rs`: the caller's `tf_ms` wins, and a wider one
        // resamples), while the caption underneath claims minutes. That is the whole of the user's
        // "нужны минутные свечи, а не 5минутные".
        //
        // Only `tf_min` is forced. Starting from the EFFECTIVE settings rather than
        // `CandleViewCfg::default()` keeps the user's candle mode, outline width, in-zone colours
        // and MoonShot corridor exactly as they are on their own charts.
        panel.update(cx, |panel, pcx| {
            let mut view = panel.effective_candle_view(pcx);
            view.tf_min = 1;
            // The SECOND thing the effective settings can carry that makes this window useless:
            // candle mode Off is a pure TICK chart, and a candle replay has no ticks. Inherited
            // unchanged it would draw an empty pane under a caption naming candles — the same
            // dishonesty the timeframe pin exists to remove, one step further along. A user who
            // turned candles off on their live chart still asked to SEE this trade, so the window
            // falls back to the shipped drawing mode rather than to nothing.
            if view.mode == moon_core::market::candles::CANDLE_MODE_OFF {
                view.mode = moon_core::market::CandleViewCfg::default().mode;
            }
            panel.set_candle_view(Some(view), pcx);
            // THE TRADE'S OWN CAPTIONS, published before the first fetch answers like the
            // timeframe pin above: they come from the replica, not from the network, so the window
            // states what this trade WAS even while the picture behind it is still loading — and
            // never has to swap one set of captions for another once it lands.
            panel.attach_trade_labels(Some(labels.clone()), pcx);
        });
        // Cloned BEFORE the view takes the panel: the observer below needs the handle, and an
        // `Entity` handle is a refcount, not a copy of the panel.
        let panel_handle = panel.clone();
        let view = cx.new(|vcx| {
            let mut this = TradeWindowView {
                backend: owner.clone(),
                panel,
                // The identity discriminates this WINDOW's series from any other's, so two windows
                // on the same trade cannot be told "nothing changed" by each other's revision.
                identity: mix_identity(
                    record.core_uid,
                    record.record_id,
                    window.window_handle().window_id(),
                ),
                record: record.clone(),
                core,
                market: market.clone(),
                stamps: stamps.clone(),
                state: TradeWindowState::Loading,
                meta,
                strategy_pending: !named,
                // Nothing searched yet; the first notification does the walk.
                strategies_rev: None,
                sequence: 0,
                cancel: Arc::new(AtomicBool::new(false)),
                window_id: window.window_handle().window_id(),
                taskbar_hide: crate::window::windowing::hide_window_from_taskbar_soon(window),
                focus: vcx.focus_handle(),
                cascade_px: step,
            };
            // AFTER the panel exists, so the chart cannot take the keyboard back from the root on
            // its own construction. `vcx` rather than the enclosing context: that one is already
            // borrowed for this very `cx.new` call.
            window.focus(&this.focus, vcx);
            // The independent-window taskbar policy is not durable on its own: the shell
            // republishes the item after a show and after an un-minimize, so the burst is re-armed
            // on every activation and the previous token cancelled first.
            // THE GEOMETRY MEMORY. One rectangle for every trade window, written into the layout
            // authority that the 100 ms coordination loop and `on_app_quit` both snapshot WHOLE —
            // so it survives a clean exit as well as a crash, with nothing to register in either.
            // A maximized or fullscreen window is remembered too: `window_geom_rect` keeps the
            // restore rectangle and carries the state beside it, exactly as the Assets window
            // already behaves.
            vcx.observe_window_bounds(window, |this: &mut TradeWindowView, window, cx| {
                let geom = crate::window::windowing::window_geom_rect(window, cx);
                let cascade_px = this.cascade_px;
                this.backend.update(cx, |b, _| {
                    let geom = super::remembered_geometry(b.layout.trade_window, geom, cascade_px);
                    // Only a real change dirties the layout: this observer fires throughout a drag,
                    // and marking it every time would schedule a file write per frame of it.
                    if b.layout.trade_window != Some(geom) {
                        b.layout.trade_window = Some(geom);
                        b.layout_dirty = true;
                    }
                });
            })
            .detach();
            // The ONE thing this window watches the application for: the strategy list of a core
            // that was still connecting when the window opened. It costs a revision compare per
            // notification — see `retry_strategy_name` — and nothing at all once the name is in.
            vcx.observe(&owner, |this: &mut TradeWindowView, _backend, cx| {
                this.retry_strategy_name(cx);
            })
            .detach();
            // Captions edited from this window's own chart menu, relayed up by the panel for its
            // OWNER to store — the same observer the detached chart window runs, for the same
            // reason: the panel applies, the owner persists.
            vcx.observe(&panel_handle, |this: &mut TradeWindowView, _panel, cx| {
                this.drain_panel_labels(cx);
            })
            .detach();
            vcx.observe_window_activation(window, |this: &mut TradeWindowView, window, _cx| {
                this.taskbar_hide.cancel();
                this.taskbar_hide = crate::window::windowing::hide_window_from_taskbar_soon(window);
            })
            .detach();
            vcx.on_release(|this, app| {
                this.taskbar_hide.cancel();
                // Stop the fetch rather than merely ignoring its answer: this is the only one of
                // the three guards that reaches the worker.
                this.cancel
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                // A frozen viewer must not leave the application subscribed to a live market it
                // opened only to look at the past.
                this.panel.update(app, |panel, pcx| {
                    panel.attach_trade_replay(None, pcx);
                    panel.release_market_refs(pcx);
                });
                let window_id = this.window_id;
                this.backend.update(app, |b, bcx| {
                    // Unregister exactly this window: another may have taken the same trade key
                    // after this one was retired by the cap.
                    let mine = b
                        .trade_windows
                        .iter()
                        .any(|(_, h)| h.window_id() == window_id);
                    if !mine {
                        return;
                    }
                    b.trade_windows.retain(|(_, h)| h.window_id() != window_id);
                    bcx.notify();
                });
            })
            .detach();
            this.fetch(vcx);
            this
        });
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::NoFill))
    });
    match opened {
        Ok(handle) => {
            backend.update(cx, |b, _| b.trade_windows.push((key, handle)));
            crate::window::windowing::activate_new_window(handle.into(), cx);
        }
        Err(error) => {
            log::warn!("[x] failed to open trade window for {coin} record={record_id}: {error}")
        }
    }
}

/// Mix a stable, per-WINDOW discriminator for the frozen series it draws.
///
/// The trade alone is not enough: the cap can retire a window and the user can reopen the same
/// trade, and a reused discriminator would let the new engine be told "nothing changed" about a
/// series it has never seen.
///
/// Args:
///     core_uid: Core that recorded the trade.
///     record_id: Durable record id.
///     window_id: The window this series belongs to.
///
/// Returns:
///     A non-zero discriminator.
fn mix_identity(core_uid: u64, record_id: i64, window_id: WindowId) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for value in [core_uid, record_id as u64, window_id.as_u64()] {
        hash ^= value;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash.max(1)
}
