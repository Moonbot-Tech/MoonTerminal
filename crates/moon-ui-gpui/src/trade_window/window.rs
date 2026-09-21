//! Opening, placing and retiring a trade-detail window.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gpui::*;
use moon_core::db::{ChartTradeRecord, TradeMeta};
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

/// How many trade windows may be open at once.
///
/// The goal asks for a second trade beside the first; it does not ask for a wall of them, and each
/// one holds a chart engine with its own GPU resources. Two is the stated requirement, enforced
/// rather than hoped for: a third open retires the oldest.
const MAX_WINDOWS: usize = 2;

/// Open — or focus — the trade-detail window for one closed trade.
///
/// Re-clicking a trade focuses its existing window and refreshes its Report-period neighbours.
/// Its replay and viewport stay intact rather than fetching an identical picture again.
/// New windows check the cascaded rectangle for reachability and fit it into the display work area.
///
/// Args:
///     backend: Shared application state.
///     record: The clicked trade, already resolved from the durable replica.
///     meta: What that trade carried beside its prices — the detect line, the strategy, the exit
///         reason — read in the same background pass as the record itself.
///     history: All closed neighbours selected from the same Report snapshot.
///     market: Exchange-native market the coin resolved to.
///     stamps: Entry and exit times, already formatted in the Report's own clock.
///     cx: Application context.
pub(crate) fn open_trade_window(
    backend: &Entity<Backend>,
    record: ChartTradeRecord,
    meta: TradeMeta,
    history: Vec<ChartTradeRecord>,
    market: String,
    stamps: (String, String),
    cx: &mut App,
) {
    let core = record.core_uid;
    let key = (record.core_uid, record.record_id);
    let open: Vec<((u64, i64), WindowHandle<Root>)> = backend.read(cx).trade_windows.clone();
    if let Some((_, handle)) = open.iter().find(|(k, _)| *k == key) {
        if let Ok(Some(view)) = handle.update(cx, |root, window, _| {
            window.activate_window();
            root.view().clone().downcast::<TradeWindowView>().ok()
        }) {
            view.update(cx, |view, cx| view.replace_history(history, cx));
        }
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
    // Keep state and display identity even when the cascaded restore rectangle is unreachable.
    let saved = backend.read(cx).layout.trade_window;
    let saved_state = saved.map_or((false, false), |geom| (geom.maximized, geom.fullscreen));
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
    // A new or unreachable trade window retains its existing display-relative cascade.
    let fallback = Bounds {
        origin: crate::window::windowing::cascade_origin_on(
            point(px(FIRST_ORIGIN.0 + step), px(FIRST_ORIGIN.1 + step)),
            display_id,
            cx,
        ),
        size: size(px(WIN_W), px(WIN_H)),
    };
    // Saved coordinates are already in the display's coordinate space: only add the cascade,
    // then test that final candidate so the second window cannot be pushed off-screen unchecked.
    let candidate = saved.map_or(fallback, |geom| Bounds {
        origin: point(px(geom.x as f32 + step), px(geom.y as f32 + step)),
        size: size(px(geom.w as f32), px(geom.h as f32)),
    });
    let bounds = crate::window::windowing::reachable_window_bounds(
        candidate,
        display_id,
        Some(fallback),
        cx,
    );
    let theme = backend.read(cx).config.chart_theme().clone();
    // ONE remembered scale for every trade window, not one per trade: same policy as the
    // rectangle above. `None` is Auto — a layout written before this field existed, and an
    // explicit Auto pick. Applied onto the new panel after construction, so a fresh window
    // opens on the last chosen zoom rather than on the panel's default Auto.
    let saved_scale = crate::controls::remembered_scale(backend.read(cx).layout.trade_window_scale);
    let title = format!("MoonTerminal - {} - {}", record.coin, stamps.1);
    let mut opts = crate::window::windowing::trade_window_options(
        title,
        // The cascade offset is inert while maximized: every trade window then covers the same
        // screen, which is exactly what maximizing asked for, and the offset returns with the
        // restore rectangle underneath it.
        crate::window::windowing::window_bounds_for(saved_state.0, saved_state.1, bounds),
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
    // The entry instant on the terminal's clock, for the strategy-version lookup: `fetch` resolves
    // the same pair again for the REST window, but that one is re-resolved on every Retry and the
    // version placement has no reason to follow it.
    let (buy_utc_ms, _) = super::utc_stamps_ms(
        &record,
        &backend
            .read(cx)
            .report_axis(crate::chartdx::axes::display_zone()),
    );
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
        // "I need one-minute candles, not five-minute candles."
        //
        // Only `tf_min` is forced. Starting from the EFFECTIVE settings rather than
        // `CandleViewCfg::default()` keeps the user's candle mode, outline width, in-zone colours
        // and MoonShot corridor exactly as they are on their own charts.
        // Captured immediately BEFORE the candle-stage force below, so `TradeWindowView::apply`
        // can hand it back to `restore_candle_mode` once a tick outcome lands and that force's
        // reason is gone. A window that never gets past the candle stage never reaches the
        // restore path either, and keeps the forced mode for its whole life by construction —
        // which is exactly right, since a candle replay genuinely has no ticks to fall back to.
        let mut user_candle_mode = 0;
        // What the pin was taken from: with no override yet, the effective view IS the
        // trade-window kind's stored set, and `follow_candle_default` re-pins when it moves.
        let mut candles_followed = moon_core::market::CandleViewCfg::default();
        panel.update(cx, |panel, pcx| {
            let user = panel.effective_candle_view(pcx);
            user_candle_mode = user.mode;
            candles_followed = user;
            // THE INITIAL CANDLE-MODE FORCE, and the timeframe pin with it — both in
            // `settings::pinned_candle_view`, the one rule this window, its popup and its reset
            // draw candles through. The mode force holds until a tick outcome arrives: a fresh
            // request yields candles before its optional tick upgrade, while a settled tick-cache
            // hit can yield ticks as its FIRST outcome. Candle mode Off is a pure TICK chart, and
            // a candle replay has no ticks. Inherited unchanged it would draw an empty pane under
            // a caption naming candles — the same dishonesty the timeframe pin exists to remove,
            // one step further along. A user who turned candles off on their live chart still
            // asked to SEE this trade, so the window falls back to the shipped drawing mode rather
            // than to nothing, until `apply` restores the real choice once a tick series is on
            // offer.
            panel.set_candle_view(Some(super::settings::pinned_candle_view(user, false)), pcx);
            // Frame the trade NOW, before the REST fetch answers. The pane used to stay live until
            // `publish`, so a window opened with toolbar Live on sat on `now` for the whole load
            // and the closed trade was off the left edge. The 1-minute pin above is the bar width
            // this first frame uses; a later tick upgrade keeps it (`first_publish` is false).
            let (buy_utc_ms, close_utc_ms) = super::utc_stamps_ms(
                &record,
                &owner
                    .read(pcx)
                    .report_axis(crate::chartdx::axes::display_zone()),
            );
            if let Some((start_ms, end_ms)) =
                super::frame::trade_frame(buy_utc_ms, close_utc_ms, 60_000)
            {
                panel.show_time_range(start_ms, end_ms, 0.0);
            }
            // THE TRADE'S OWN CAPTIONS, published before the first fetch answers like the
            // timeframe pin above: they come from the replica, not from the network, so the window
            // states what this trade WAS even while the picture behind it is still loading — and
            // never has to swap one set of captions for another once it lands.
            panel.attach_trade_labels(Some(labels.clone()), pcx);
            if let Some(pct) = saved_scale {
                panel.force_scale(Some(pct), pcx);
            }
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
                history: std::rc::Rc::new(history),
                show_other_trades: owner
                    .read(vcx)
                    .layout
                    .trade_window_other_trades
                    .unwrap_or(true),
                settings_open: false,
                fit_trade: owner.read(vcx).layout.trade_window_fit.unwrap_or(false),
                hide_rail: owner
                    .read(vcx)
                    .layout
                    .trade_window_hide_rail
                    .unwrap_or(false),
                load_ticks: owner.read(vcx).layout.trade_window_ticks.unwrap_or(true),
                fit_frame: None,
                applied_frame: None,
                candles_followed,
                core,
                market: market.clone(),
                stamps: stamps.clone(),
                state: TradeWindowState::Loading,
                framed_this_sequence: false,
                user_candle_mode,
                meta,
                strategy_pending: !named,
                // Nothing searched yet; the first notification does the walk.
                strategies_rev: None,
                strategy_lookup: None,
                // Asked for below, once the observers are in place to hear the answer.
                traces: super::traces::TraceState::Pending,
                traces_sig: 0,
                neighbours_drawn: 0,
                frozen_rev: 0,
                sequence: 0,
                cancel: Arc::new(AtomicBool::new(false)),
                window_id: window.window_handle().window_id(),
                taskbar_hide: crate::window::windowing::hide_window_from_taskbar_soon(window),
                focus: vcx.focus_handle(),
                modifier_watch: moon_ui::MoonHotkeyModifierWatch::default(),
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
            // What this window watches the application for: the strategy list of a core that was
            // still connecting when the window opened — a revision compare per notification, see
            // `retry_strategy_name`, and nothing at all once the name is in...
            // ...and the trade-window candle setting, which any trade window's popup moves: a
            // compare per notification, a re-pin only when it changed (graphics and captions
            // reach the panel on their own, since it holds no override for them).
            vcx.observe(&owner, |this: &mut TradeWindowView, _backend, cx| {
                this.retry_strategy_name(cx);
                this.follow_candle_default(cx);
            })
            .detach();
            // ...and the archived order traces it asked the resolver for, on the resolver's own
            // wake: a signature compare per notification, a rebuild only when a line of THIS
            // window's changed.
            let traces_revision = owner.read(vcx).traces_revision();
            vcx.observe(&traces_revision, |this: &mut TradeWindowView, _rev, cx| {
                this.sync_traces(false, cx);
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
                if !window.is_window_active() {
                    // The modifier state a returning window is re-told is not a press.
                    this.modifier_watch.forget();
                }
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
            this.spawn_strategy_lookup(buy_utc_ms, vcx);
            this.request_traces(vcx);
            this.request_neighbour_traces(vcx);
            // Build the store from the rows alone, now that the neighbours are asked for too:
            // the exit lines the rows give and the fitted frame exist before any archive answers.
            // A row with a `ReportUID` was built once already inside `request_traces`, before
            // the neighbours; a row without one is built here for the first time, and again only
            // when a shown neighbour's answer moves the signature.
            this.sync_traces(true, vcx);
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
