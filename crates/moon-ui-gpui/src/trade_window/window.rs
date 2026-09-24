//! Opening, placing and retiring a trade-detail window.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use gpui::*;
use moon_core::db::{ChartTradeRecord, TradeMeta};
use moon_ui::{MoonBackgroundPolicy, Root};

use super::{Host, TradeWindowState, TradeWindowView};
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
    let bg = backend.read(cx).config.chart_theme().bg;
    opts.window_clear_color = Some(gpui::rgb(
        ((bg[0] as u32) << 16) | ((bg[1] as u32) << 8) | bg[2] as u32,
    ));
    // Kept for the failure log below, which the move into the window builder would otherwise take.
    let (coin, record_id) = (record.coin.clone(), record.record_id);
    let owner = backend.clone();
    let seed = TradeSeed {
        record,
        meta,
        history,
        market,
        stamps,
    };
    let opened = cx.open_window(opts, move |window, cx| {
        crate::window::windowing::configure_chart_clear_color(window, cx);
        let host = Host::Window {
            window_id: window.window_handle().window_id(),
            taskbar_hide: crate::window::windowing::hide_window_from_taskbar_soon(window),
            cascade_px: step,
        };
        let view = cx.new(|vcx| new_view(&owner, seed, host, Some(window), vcx));
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

/// Everything a trade view is built from, resolved from the replica
/// (`open_record::resolve_trade_record`).
pub(crate) struct TradeSeed {
    /// The trade being shown.
    pub(crate) record: ChartTradeRecord,
    /// What the trade carried beside its prices — the detect line, the strategy, the exit reason.
    pub(crate) meta: TradeMeta,
    /// All closed neighbours selected from the same Report snapshot.
    pub(crate) history: Vec<ChartTradeRecord>,
    /// Exchange-native market the coin resolved to.
    pub(crate) market: String,
    /// Entry and exit times, already formatted in the caller's own clock.
    pub(crate) stamps: (String, String),
}

/// A trade view as a PANE of another view — the tuner's deal table — rather than a window.
///
/// The same view and the same chart a window gets, minus what only a window has: no geometry to
/// remember, no taskbar entry, no focus of its own and no keys (see [`Host::Embedded`]). Dropping
/// the entity releases it as closing a window does: the fetch is cancelled and the market refs
/// the chart took are given back.
///
/// Args:
///     backend: Shared application state.
///     seed: The resolved trade.
///     cx: Application context.
///
/// Returns:
///     The view, for the host to render and to hand modelled trades to.
#[expect(
    dead_code,
    reason = "the tuner's Entry/Exit axis, its consumer, lands separately"
)]
pub(crate) fn embedded_trade_view(
    backend: &Entity<Backend>,
    seed: TradeSeed,
    cx: &mut App,
) -> Entity<TradeWindowView> {
    cx.new(|vcx| new_view(backend, seed, Host::Embedded, None, vcx))
}

/// Build the view and its chart, for either host.
///
/// Args:
///     owner: Shared application state.
///     seed: The resolved trade.
///     host: A window of its own, or a pane.
///     window: The window of its own; a pane passes none — it takes nothing from the window it
///         is drawn in.
///     vcx: The new view's context.
fn new_view(
    owner: &Entity<Backend>,
    seed: TradeSeed,
    host: Host,
    window: Option<&mut Window>,
    vcx: &mut Context<TradeWindowView>,
) -> TradeWindowView {
    let TradeSeed {
        record,
        meta,
        history,
        market,
        stamps,
    } = seed;
    let core = record.core_uid;
    let theme = owner.read(vcx).config.chart_theme().clone();
    // ONE remembered scale for every trade window, not one per trade: same policy as the
    // rectangle above. `None` is Auto — a layout written before this field existed, and an
    // explicit Auto pick. Applied onto the new panel after construction, so a fresh window
    // opens on the last chosen zoom rather than on the panel's default Auto.
    let saved_scale = crate::controls::remembered_scale(owner.read(vcx).layout.trade_window_scale);
    // Resolved HERE, while the session is in hand: naming the strategy is a session lookup. Whether
    // it could be named travels with it — a core still connecting has no list yet, and the window
    // keeps asking until it does.
    let (resolved, named) = super::trade_labels(owner.read(vcx), core, &meta);
    let labels = std::rc::Rc::new(resolved);
    // The entry instant on the terminal's clock, for the strategy-version lookup: `fetch` resolves
    // the same pair again for the REST window, but that one is re-resolved on every Retry and the
    // version placement has no reason to follow it.
    let (buy_utc_ms, _) = super::utc_stamps_ms(
        &record,
        &owner
            .read(vcx)
            .report_axis(crate::chartdx::axes::display_zone()),
    );
    let epoch = moon_chart::paint::now_unix_ms();
    let panel_backend = owner.clone();
    let panel_market = market.clone();
    let panel = vcx.new(|pcx| {
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
    panel.update(vcx, |panel, pcx| {
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
    let embedded = host.embedded();
    // The remembered switches, read in one go: the constructor below borrows the context again.
    let (show_other_trades, fit_trade, hide_rail, show_corridor, load_ticks) = {
        let layout = &owner.read(vcx).layout;
        (
            layout.trade_window_other_trades.unwrap_or(true),
            layout.trade_window_fit.unwrap_or(false),
            match embedded {
                true => layout.analytics_trade_hide_rail.unwrap_or(true),
                false => layout.trade_window_hide_rail.unwrap_or(false),
            },
            layout.trade_window_moonshot_zone.unwrap_or(false),
            layout.trade_window_ticks.unwrap_or(true),
        )
    };
    let mut this = TradeWindowView {
        backend: owner.clone(),
        panel: panel.clone(),
        // The identity discriminates this VIEW's series from any other's, so two views on the
        // same trade cannot be told "nothing changed" by each other's revision. A pane has no
        // window of its own, so a process-wide count stands in for one.
        identity: mix_identity(
            record.core_uid,
            record.record_id,
            match &host {
                Host::Window { window_id, .. } => window_id.as_u64(),
                Host::Embedded => EMBEDDED_SEQ.fetch_add(1, Ordering::Relaxed) | (1 << 63),
            },
        ),
        record: record.clone(),
        history: std::rc::Rc::new(history),
        show_other_trades,
        settings_open: false,
        fit_trade,
        hide_rail,
        show_corridor,
        model_trades: Vec::new(),
        model_corridor: Vec::new(),
        load_ticks,
        fit_frame: None,
        applied_frame: None,
        candles_followed,
        core,
        market,
        stamps,
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
        host,
        focus: vcx.focus_handle(),
        modifier_watch: moon_ui::MoonHotkeyModifierWatch::default(),
    };
    if let (false, Some(window)) = (embedded, window) {
        // AFTER the panel exists, so the chart cannot take the keyboard back from the root on its
        // own construction. A pane leaves the keyboard to its host window.
        window.focus(&this.focus, vcx);
        // THE GEOMETRY MEMORY. One rectangle for every trade window, written into the layout
        // authority that the 100 ms coordination loop and `on_app_quit` both snapshot WHOLE — so
        // it survives a clean exit as well as a crash, with nothing to register in either. A
        // maximized or fullscreen window is remembered too: `window_geom_rect` keeps the restore
        // rectangle and carries the state beside it, exactly as the Assets window already
        // behaves.
        vcx.observe_window_bounds(window, |this: &mut TradeWindowView, window, cx| {
            let Host::Window { cascade_px, .. } = this.host else {
                return;
            };
            let geom = crate::window::windowing::window_geom_rect(window, cx);
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
        // The independent-window taskbar policy is not durable on its own: the shell republishes
        // the item after a show and after an un-minimize, so the burst is re-armed on every
        // activation and the previous token cancelled first.
        vcx.observe_window_activation(window, |this: &mut TradeWindowView, window, _cx| {
            if let Host::Window { taskbar_hide, .. } = &mut this.host {
                taskbar_hide.cancel();
                *taskbar_hide = crate::window::windowing::hide_window_from_taskbar_soon(window);
            }
            if !window.is_window_active() {
                // The modifier state a returning window is re-told is not a press.
                this.modifier_watch.forget();
            }
        })
        .detach();
    }
    // What the view watches the application for: the strategy list of a core that was still
    // connecting when it opened — a revision compare per notification, see
    // `retry_strategy_name`, and nothing at all once the name is in...
    // ...and the trade-window candle setting, which any trade view's popup moves: a compare per
    // notification, a re-pin only when it changed (graphics and captions reach the panel on their
    // own, since it holds no override for them).
    vcx.observe(owner, |this: &mut TradeWindowView, _backend, cx| {
        this.retry_strategy_name(cx);
        this.follow_candle_default(cx);
    })
    .detach();
    // ...and the archived order traces it asked the resolver for, on the resolver's own wake: a
    // signature compare per notification, a rebuild only when a line of THIS view's changed.
    let traces_revision = owner.read(vcx).traces_revision();
    vcx.observe(&traces_revision, |this: &mut TradeWindowView, _rev, cx| {
        this.sync_traces(false, cx);
    })
    .detach();
    // Captions edited from this view's own chart menu, relayed up by the panel for its OWNER to
    // store — the same observer the detached chart window runs, for the same reason: the panel
    // applies, the owner persists.
    vcx.observe(&panel, |this: &mut TradeWindowView, _panel, cx| {
        this.drain_panel_labels(cx);
    })
    .detach();
    vcx.on_release(|this, app| {
        // Stop the fetch rather than merely ignoring its answer: this is the only one of the three
        // guards that reaches the worker.
        this.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        // A frozen viewer must not leave the application subscribed to a live market it opened
        // only to look at the past.
        this.panel.update(app, |panel, pcx| {
            panel.attach_trade_replay(None, pcx);
            panel.release_market_refs(pcx);
        });
        let Host::Window {
            window_id,
            taskbar_hide,
            ..
        } = &mut this.host
        else {
            return;
        };
        taskbar_hide.cancel();
        let window_id = *window_id;
        this.backend.update(app, |b, bcx| {
            // Unregister exactly this window: another may have taken the same trade key after
            // this one was retired by the cap.
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
    // Build the store from the rows alone, now that the neighbours are asked for too: the exit
    // lines the rows give and the fitted frame exist before any archive answers. A row with a
    // `ReportUID` was built once already inside `request_traces`, before the neighbours; a row
    // without one is built here for the first time, and again only when a shown neighbour's
    // answer moves the signature.
    this.sync_traces(true, vcx);
    this.fetch(vcx);
    this
}

/// A count standing in for a window id in a pane's series identity — see [`mix_identity`].
static EMBEDDED_SEQ: AtomicU64 = AtomicU64::new(1);

/// Mix a stable, per-WINDOW discriminator for the frozen series it draws.
///
/// The trade alone is not enough: the cap can retire a window and the user can reopen the same
/// trade, and a reused discriminator would let the new engine be told "nothing changed" about a
/// series it has never seen.
///
/// Args:
///     core_uid: Core that recorded the trade.
///     record_id: Durable record id.
///     window: The window this series belongs to, or a pane's stand-in for one.
///
/// Returns:
///     A non-zero discriminator.
fn mix_identity(core_uid: u64, record_id: i64, window: u64) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for value in [core_uid, record_id as u64, window] {
        hash ^= value;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash.max(1)
}
