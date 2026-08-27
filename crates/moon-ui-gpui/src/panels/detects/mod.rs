//! Detachable detection-feed panel, ported from egui's `DetectRibbon`. It ingests group-core rows
//! for which `sound_alert || is_alert`. A row routed to a chart tab (`add_to_chart > 0`) is an
//! ordinary detect in every respect and joins the feed as well once `show_add_to_chart` is on;
//! while it is off, chart tabs show such rows alone, as they always did. Each `(core, market)` card
//! remains for `max(keep_alert_secs, 1)` seconds.
//! Left-click requests the market on Main without raising its window, while right-click requests a
//! custom comparison tab.
//!
//! The gear popup configures per-size dimensions, chart type, server rail, and field slots for each
//! group and persists them in `detects_view.toml`; see [`popup`]. Card layout and vector mini-charts
//! built from the snapshot captured at detection time live in [`cards`].

mod cards;
mod popup;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

use crate::Backend;
use gpui::*;
use moon_chart::paint::now_unix_ms;
use moon_core::config::{DETECT_RAIL_MAX, DetectViewCfg};
use moon_core::session::CoreId;
use moon_ui::{
    MoonPalette, MoonSliderEvent, MoonSliderState, Panel, PanelEvent, PanelState, h_flex, v_flex,
};

/// Number of latest five-minute OHLC buckets retained for a card's candle chart, approximately two
/// hours. At a typical 75-130 px chart width, 24 buckets leave roughly 3-5 px per outlined candle;
/// denser candles would visually collapse into solid bars.
const DETECT_THUMB_BARS: usize = 24;

/// Frozen state for one detection-feed card, ported from `src/dock/detects.rs::RibbonItem`.
pub(crate) struct DetectItem {
    core: CoreId,
    core_name: String,
    /// Full market key passed to Main or comparison-tab open requests.
    market: String,
    /// Coin label derived from the market without its quote suffix (`ADAUSDT` to `ADA`).
    base: String,
    color: [u8; 3],
    /// Source-strategy kind ordinal from `DetectRow.kind`, used for the detection-type badge.
    kind: u8,
    /// Source-strategy direction from `DetectRow.is_short`, used for the badge outline.
    is_short: bool,
    /// `DetectRow.strat_name`: the strategy that fired this detect, empty when none did. Frozen
    /// with the card rather than resolved while rendering, like every other field here: the
    /// strategy can be renamed or deleted during the card's KeepAlert, and the card states what
    /// fired it, not what that strategy is called now.
    strat_name: String,
    /// `DetectRow.is_alert`: whether a drawn chart object fired this, not a strategy. The only
    /// case that legitimately has no [`Self::strat_name`], so the strategy field names it instead
    /// of leaving a hole.
    is_alert: bool,
    /// `DetectRow.add_to_chart`: the chart tab this detect also opens, `0` for none. Retained so
    /// that turning `show_add_to_chart` off hides these cards at once instead of leaving them for
    /// the rest of their `KeepAlert`.
    add_to_chart: u32,
    born_ms: f64,
    ttl_ms: f64,
    /// Five-minute `(open, high, low, close)` snapshot frozen when the detection is ingested.
    bars: Vec<(f32, f32, f32, f32)>,
    /// Close prices over the 24-hour snapshot window for line mode, oldest to newest.
    line: Vec<f32>,
    /// 24-hour and one-hour percentage price changes at detection time.
    delta_24h: f32,
    delta_1h: f32,
    /// Venue captured at detection time, captioned through the shared directory.
    venue: Option<moon_core::venue::CoreVenue>,
    /// Connection type, such as spot, futures, or DEX, captured at detection time.
    ///
    /// A separate axis from [`Self::venue`]: this is the capability mask the connection reported
    /// and can name two markets at once (`Спот/Фьючи`), while the venue is the one market the core
    /// actually trades.
    exchange_kind: String,
}

pub struct DetectsPanel {
    backend: Entity<Backend>,
    group: String,
    items: VecDeque<DetectItem>,
    last_seq: HashMap<CoreId, u64>,
    last_sig: (u64, bool),
    prune_timer_armed: bool,
    focus: FocusHandle,
    /// Whether the gear configuration popup is open and which card-size tab it edits. Opening the
    /// popup selects the tab corresponding to the feed's active size.
    popup_open: bool,
    popup_tab: u8,
    /// Popup sliders for card width, height, server rail, and gradient. Opening or switching a tab
    /// seeds them from that size's configuration; `Change` events write back to the same size.
    w_slider: Entity<MoonSliderState>,
    h_slider: Entity<MoonSliderState>,
    rail_slider: Entity<MoonSliderState>,
    grad_slider: Entity<MoonSliderState>,
    /// Value of `show_add_to_chart` at the previous ingest, so that the pass which sees it turn on
    /// can replay each core's ring instead of waiting for the next unrelated detect.
    showed_add_to_chart: bool,
}

const MAX_DETECT_BTNS: usize = 48;
const DEFAULT_SERVER_COLOR: [u8; 3] = [0xff, 0xb3, 0x47];

impl DetectsPanel {
    /// Build a group collector whose retained cards are scoped only at presentation time. The one
    /// thing ingestion itself decides is whether chart-routed detects are wanted at all, and it
    /// replays the ring when that answer changes, so no card is lost to the cursor.
    ///
    /// Args:
    ///     backend: Shared terminal state and workspace authority.
    ///     group: Window group whose detection feeds are continuously ingested.
    ///     cx: Panel context used to register data, workspace, and settings observers.
    ///
    /// Returns:
    ///     Initialized detection panel with group-wide cursors and retained cards.
    pub fn new(backend: Entity<Backend>, group: String, cx: &mut Context<Self>) -> Self {
        let initial_sig = detects_sig(backend.read(cx), &group);
        cx.observe(&backend, |this, backend, cx| {
            let now = now_unix_ms();
            let sig = detects_sig(backend.read(cx), &this.group);
            let mut changed = false;
            if sig != this.last_sig {
                this.last_sig = sig;
                changed |= this.ingest(backend.read(cx), now);
            }
            changed |= this.prune(now);
            this.arm_prune_timer(cx);
            if changed {
                cx.notify();
            }
        })
        .detach();
        let workspace_revision = backend.read(cx).workspace_revision();
        cx.observe(&workspace_revision, |_this, _revision, cx| {
            // Detection cursors and retained cards remain group-wide. A scope change only alters
            // presentation, so repaint without ingesting or discarding anything.
            cx.notify();
        })
        .detach();
        let initial_backend = backend.clone();
        let mk_slider = |cx: &mut Context<Self>, min: f32, max: f32, step: f32| {
            cx.new(|_| MoonSliderState::new().min(min).max(max).step(step))
        };
        let w_slider = mk_slider(cx, 20.0, 320.0, 2.0);
        let h_slider = mk_slider(cx, 20.0, 320.0, 2.0);
        let rail_slider = mk_slider(cx, 0.0, f32::from(DETECT_RAIL_MAX), 1.0);
        // The gradient slider spans the maximum card width. Configuration clamps its effective
        // value to the selected card width, and the caption displays that clamped value.
        let grad_slider = mk_slider(cx, 0.0, 320.0, 2.0);
        // Write slider changes to the size tab currently being edited. `write_view` ignores the
        // unchanged values emitted while sliders are seeded.
        for (sl, apply) in [
            (
                &w_slider,
                (|c: &mut moon_core::config::DetectSizeCfg, v: f32| c.w = v.round() as u16)
                    as fn(&mut moon_core::config::DetectSizeCfg, f32),
            ),
            (&h_slider, |c, v| c.h = v.round() as u16),
            (&rail_slider, |c, v| c.rail_w = v.round() as u8),
            (&grad_slider, |c, v| c.rail_grad = v.round() as u16),
        ] {
            cx.subscribe(sl, move |this, _, ev: &MoonSliderEvent, cx| {
                if let MoonSliderEvent::Change(v) = ev {
                    let v = v.end();
                    let tab = this.popup_tab;
                    this.write_view(cx, |cfg| {
                        apply(cfg.size_cfg_mut(tab), v);
                    });
                }
            })
            .detach();
        }
        let mut this = Self {
            backend,
            group,
            items: VecDeque::new(),
            last_seq: HashMap::new(),
            last_sig: initial_sig,
            prune_timer_armed: false,
            focus: cx.focus_handle(),
            popup_open: false,
            popup_tab: 0,
            w_slider,
            h_slider,
            rail_slider,
            grad_slider,
            // Seeded from the signature's own copy of the setting, so the field means what it says
            // from the first pass on. That pass needs no replay either way: its cursors are empty,
            // so it walks the rings regardless.
            showed_add_to_chart: initial_sig.1,
        };
        let now = now_unix_ms();
        this.ingest(initial_backend.read(cx), now);
        this.prune(now);
        this.arm_prune_timer(cx);
        this
    }

    /// Applies a mutation to this group's display configuration and persists a changed value to
    /// `detects_view.toml`. An unchanged mutation performs no write or notification, suppressing
    /// slider-seeding echoes.
    fn write_view(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut DetectViewCfg)) {
        let group = self.group.clone();
        let changed = self.backend.update(cx, |b, bcx| {
            let mut cfg = b.detects_view.group(&group);
            let before = cfg;
            f(&mut cfg);
            if cfg == before {
                return false;
            }
            b.detects_view.set_group(&group, cfg);
            b.detects_view.save();
            bcx.notify();
            true
        });
        if changed {
            cx.notify();
        }
    }

    /// Ingests group-core detections newer than each core's sequence cursor. Rows are eligible when
    /// they request a sound or represent an alert firing. A row routed to AddToChart joins them
    /// only while this group enables `show_add_to_chart`; the setting is read here so that a
    /// disabled feed pays for no snapshot at all. Returns whether the visible card collection or a
    /// retained card changed.
    fn ingest(&mut self, b: &Backend, now_ms: f64) -> bool {
        let mut changed = false;
        let show_add_to_chart = b.detects_view.shows_add_to_chart(&self.group);
        let replayed = show_add_to_chart && !self.showed_add_to_chart;
        if show_add_to_chart != self.showed_add_to_chart {
            self.showed_add_to_chart = show_add_to_chart;
            changed = true;
            if show_add_to_chart {
                // Cursors have walked past the rows dropped while the setting was off, so replay
                // each ring — otherwise the feed would stay empty until some unrelated detect
                // fired. Expired rows and rows this panel already holds are both skipped below, so
                // the replay costs only the cards it actually adds. It does re-add ordinary cards
                // dismissed by a click while they are still within their KeepAlert, exactly as
                // rebuilding this panel has always done. Retained cards are NOT dropped first: a
                // long-lived card whose row has since left the 2000-row ring could not be rebuilt.
                self.last_seq.clear();
            } else {
                // Drop what just became invisible instead of letting it hold a slot in the 48-card
                // queue and keep the prune timer awake for the rest of its KeepAlert.
                self.items.retain(|it| it.add_to_chart == 0);
            }
        }
        // Read each core's server color from configuration. The coin label comes from the core's
        // catalog when the card is built, not from the market name.
        // Canonical order: fresh events are appended core by core and rendered back in
        // reverse insertion order — so this order is what decides how detects of the same instant
        // read on screen. The one exception is a replay (below), which re-fills the queue out of
        // order and re-sorts it stably to put that right; nothing on the normal path re-sorts.
        let order = crate::core_order::CoreOrder::new(&b.config);
        let mut cores: Vec<(CoreId, String, [u8; 3])> = b
            .session
            .sessions()
            .iter()
            .filter(|s| s.group == self.group)
            .map(|s| {
                let color = b
                    .config
                    .servers
                    .iter()
                    .find(|sv| sv.id == s.id)
                    .map(|sv| sv.color)
                    .unwrap_or(DEFAULT_SERVER_COLOR);
                (s.id, s.name.clone(), color)
            })
            .collect();
        order.sort_by(&mut cores, |(id, _, _)| *id);
        for (id, name, color) in cores {
            let Some(d) = b.session.store().core(id) else {
                continue;
            };
            let last = self.last_seq.get(&id).copied().unwrap_or(0);
            let mut fresh: Vec<&moon_core::feed::DetectRow> = Vec::new();
            let mut newest_of_market: HashSet<&str> = HashSet::new();
            for det in d.detects.iter().rev() {
                if det.seq <= last {
                    break;
                }
                // Newest first, so the first row of a market is the one that decides its card —
                // an older row would only be overwritten in place. Keeping the rest would buy each
                // of them a market snapshot for nothing, which is what a replay is full of.
                if newest_of_market.insert(det.market.as_str()) {
                    fresh.push(det);
                }
            }
            if fresh.is_empty() {
                continue;
            }
            self.last_seq.insert(id, fresh[0].seq);
            for det in fresh.iter().rev() {
                // Show sound-enabled detections and alert firings, including alerts without a
                // strategy. An AddToChart row passes the same gate as any other detect, but only
                // when the group asked for those cards; otherwise chart tabs consume it alone.
                if !det.sound_alert && !det.is_alert {
                    continue;
                }
                if det.add_to_chart > 0 && !show_add_to_chart {
                    continue;
                }
                let ttl = (det.keep_alert_secs.max(1) as f64) * 1000.0;
                // Drop a row whose card would be pruned on this very pass. A core's ring holds
                // thousands of rows and is walked whole whenever cursors are empty — a panel just
                // built, or the setting above just turned on — and each row accepted below pays
                // for a market snapshot.
                if detect_expired(now_ms, det.time_ms, ttl) {
                    continue;
                }
                // A row this panel already holds at the same instant is the same row seen twice —
                // a replay. Leave that card untouched: its chart is frozen at detection time, and
                // re-taking the snapshot would both cost a market read and hand the card a picture
                // newer than the countdown printed on it.
                if self
                    .items
                    .iter()
                    .any(|it| it.core == id && it.market == det.market && it.born_ms == det.time_ms)
                {
                    continue;
                }
                // Freeze five-minute chart history, 24-hour line data, deltas, and exchange
                // metadata. The market source assembles retained snapshots, local cache, and trade
                // ring data without making an exchange API request here.
                let snap =
                    b.session
                        .market_source()
                        .detect_snapshot(id, &det.market, DETECT_THUMB_BARS);
                if let Some(it) = self
                    .items
                    .iter_mut()
                    .find(|it| it.core == id && it.market == det.market)
                {
                    it.born_ms = det.time_ms;
                    it.ttl_ms = ttl;
                    it.color = color;
                    // Re-resolve the label too: a card first built before its core sent a market
                    // list would otherwise wear the name-derived spelling for its whole TTL.
                    it.base = b
                        .session
                        .market_source()
                        .market_label(id, &det.market)
                        .display_coin()
                        .to_string();
                    it.kind = det.kind;
                    it.is_short = det.is_short;
                    it.strat_name = det.strat_name.clone();
                    it.is_alert = det.is_alert;
                    it.add_to_chart = det.add_to_chart;
                    // Refresh the snapshot and TTL in place when the same core and market fire again.
                    it.bars = snap.bars;
                    it.line = snap.line;
                    it.delta_24h = snap.delta_24h;
                    it.delta_1h = snap.delta_1h;
                    it.venue = snap.venue;
                    it.exchange_kind = snap.exchange_kind;
                    changed = true;
                } else {
                    self.items.push_back(DetectItem {
                        core: id,
                        core_name: name.clone(),
                        market: det.market.clone(),
                        // Resolved when the card is built, not while rendering: a card is
                        // re-rendered constantly, and the core's catalog is the only thing that
                        // can name a Hyperliquid spot index.
                        base: b
                            .session
                            .market_source()
                            .market_label(id, &det.market)
                            .display_coin()
                            .to_string(),
                        color,
                        kind: det.kind,
                        is_short: det.is_short,
                        strat_name: det.strat_name.clone(),
                        is_alert: det.is_alert,
                        add_to_chart: det.add_to_chart,
                        born_ms: det.time_ms,
                        ttl_ms: ttl,
                        bars: snap.bars,
                        line: snap.line,
                        delta_24h: snap.delta_24h,
                        delta_1h: snap.delta_1h,
                        venue: snap.venue,
                        exchange_kind: snap.exchange_kind,
                    });
                    changed = true;
                }
            }
        }
        if replayed {
            // A replay appends rows the feed skipped earlier, and those can be older than cards it
            // already holds — which would draw them ahead of fresher ones and make the trim below
            // evict the wrong end. Sorting is STABLE, so detects of the same instant keep the
            // ingest order this feed reads by; only the replayed rows move.
            self.items
                .make_contiguous()
                .sort_by(|a, b| a.born_ms.total_cmp(&b.born_ms));
        }
        while self.items.len() > MAX_DETECT_BTNS {
            self.items.pop_front();
            changed = true;
        }
        changed
    }

    fn prune(&mut self, now_ms: f64) -> bool {
        let before = self.items.len();
        self.items
            .retain(|it| !detect_expired(now_ms, it.born_ms, it.ttl_ms));
        self.items.len() != before
    }

    /// Arms a one-second refresh timer while the queue is nonempty. On wake it clears the armed
    /// flag, prunes expired cards against wall-clock time, notifies if cards remain so their
    /// countdowns update, and rearms itself. If pruning empties the queue, the timer stops and this
    /// callback does not issue a final notification.
    fn arm_prune_timer(&mut self, cx: &mut Context<Self>) {
        if self.prune_timer_armed || self.items.is_empty() {
            return;
        }
        self.prune_timer_armed = true;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(Duration::from_millis(1000)).await;
            let alive = cx.update(|cx| {
                this.update(cx, |this, cx| {
                    this.prune_timer_armed = false;
                    this.prune(now_unix_ms());
                    // Refresh countdowns and reflect removals while at least one card remains.
                    if !this.items.is_empty() {
                        cx.notify();
                    }
                    this.arm_prune_timer(cx);
                })
                .is_ok()
            });
            if !alive {
                return;
            }
        })
        .detach();
    }

    /// Remove an authorized card and request its market on the group's Main chart.
    ///
    /// Args:
    ///     core: Core captured by the rendered detection card.
    ///     market: Exact captured market.
    ///     cx: Panel context used to revalidate authority and update the queue.
    ///
    /// Returns:
    ///     Nothing; a stale callback leaves both the card and chart request unchanged.
    fn open(&mut self, core: CoreId, market: String, cx: &mut Context<Self>) {
        let group = self.group.clone();
        let authorized = self.backend.update(cx, |b, bcx| {
            let authorized =
                b.open_on_main_if_authorized(Some(&group), (core, market.clone()), false);
            if authorized {
                bcx.notify();
            }
            authorized
        });
        if !authorized {
            return;
        }
        self.items
            .retain(|it| !(it.core == core && it.market == market));
        self.arm_prune_timer(cx);
        cx.notify();
    }

    /// Remove an authorized card and request a custom comparison tab through `Backend`. The group's
    /// `ChartTabs` handler focuses an existing tab with the same coin label or creates a horizontal
    /// tab anchored on this market, adds its exact market from other group cores with at most one
    /// core per market-data provider, pins every chart, and enables anchor lock and broom mode.
    ///
    /// Args:
    ///     core: Core captured by the rendered detection card.
    ///     market: Exact captured market used as the comparison anchor.
    ///     cx: Panel context used to revalidate authority and update the queue.
    ///
    /// Returns:
    ///     Nothing; a stale callback leaves both the card and comparison request unchanged.
    fn open_compare(&mut self, core: CoreId, market: String, cx: &mut Context<Self>) {
        let group = self.group.clone();
        let authorized = self.backend.update(cx, |b, bcx| {
            let authorized = b.open_compare_if_authorized(Some(&group), (core, market.clone()));
            if authorized {
                bcx.notify();
            }
            authorized
        });
        if !authorized {
            return;
        }
        self.items
            .retain(|it| !(it.core == core && it.market == market));
        self.arm_prune_timer(cx);
        cx.notify();
    }

    /// Returns this group's `detects_view.toml` display configuration or its default.
    fn view_cfg(&self, cx: &App) -> DetectViewCfg {
        self.backend.read(cx).detects_view.group(&self.group)
    }
}

/// Signature that makes the panel re-ingest: a hash of every group core's detect revision, PLUS the
/// AddToChart setting as its own value. The setting belongs here because flipping it changes which
/// rows the feed accepts, and this is what wakes EVERY panel of the group — the one whose checkbox
/// was clicked notifies itself, but a second, detached panel learns of it only through this.
///
/// The setting is a separate field rather than a seed folded into the hash: at `31 * flag + rev`,
/// a core whose revision advanced by exactly 31 in the same flush would cancel the flip out, and
/// the panel would never see it.
fn detects_sig(b: &Backend, group: &str) -> (u64, bool) {
    let store = b.session.store();
    let revs = b
        .session
        .sessions()
        .iter()
        .filter(|s| s.group == group)
        .filter_map(|s| store.core(s.id))
        .fold(0u64, |a, c| a.wrapping_mul(31).wrapping_add(c.detects_rev));
    (revs, b.detects_view.shows_add_to_chart(group))
}

impl EventEmitter<PanelEvent> for DetectsPanel {}
impl Focusable for DetectsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for DetectsPanel {
    fn panel_name(&self) -> &'static str {
        "Detects"
    }
    /// Visible tab caption. `panel_name` is the stable persistence key and stays untouched.
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        crate::persistence::panel_meta::tab_label(self.panel_name())
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::persistence::panel_meta::panel_title(self.panel_name())
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::persistence::dock_persist::panel_state_with_group("Detects", &self.group)
    }
}

impl Render for DetectsPanel {
    /// Render retained detection cards visible in the current effective workspace scope.
    ///
    /// Args:
    ///     _window: Owning window; unused because card interactions use the application context.
    ///     cx: Panel context providing workspace scope, theme, and configuration.
    ///
    /// Returns:
    ///     Toolbar and filtered card grid without discarding out-of-scope retained cards.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::bump(&crate::diag::DETECTS_RENDER);
        let _render_us = crate::diag::scope(&crate::diag::DETECTS_RENDER_US);
        let p = MoonPalette::active(cx);
        let is_light = p.is_light();
        let cfg = self.view_cfg(cx);
        // Badge codes and kind colors plus direction outlines come from the active configuration
        // and theme. Cloning at most a few dozen entries is cheap for this infrequently rendered
        // feed.
        let badges = self.backend.read(cx).config.badges.clone();
        // The chart theme supplies colors for card candle and line vectors.
        let theme = self.backend.read(cx).config.chart_theme().clone();
        let now = now_unix_ms();
        let visible_cores = self
            .backend
            .read(cx)
            .effective_workspace_scope(&self.group, crate::workspace::RetainedCoreScope::All)
            .ids()
            .to_vec();

        // Place the gear-triggered configuration toolbar and divider above the feed.
        let toolbar = popup::toolbar(self, &cfg, p, cx);
        let divider = div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border));

        // Render fixed-size cards in reverse insertion order in a wrapping grid. Newly inserted
        // markets appear first; a repeated core-market detection refreshes its existing position.
        let mut container = h_flex().flex_wrap().gap_1p5().content_start();
        for it in self.items.iter().rev().filter(|item| {
            detection_core_visible(item.core, &visible_cores)
                && detection_route_visible(item.add_to_chart, cfg.show_add_to_chart)
        }) {
            let secs = ((it.ttl_ms - (now - it.born_ms)) / 1000.0).ceil().max(0.0) as u32;
            let (core, market) = (it.core, it.market.clone());
            let market_rmb = it.market.clone();
            let card = cards::card(it, secs, &cfg, &theme, &badges, p, is_light, cx)
                // Keyed by what the card IS, not by where it currently sits: ingest keeps one card
                // per core and market, while the position shifts under it whenever the queue drops
                // its oldest card or a replay re-sorts it. A positional id hands the element state
                // — a card's open tooltip among it — to whichever detect inherits that slot.
                .id(SharedString::from(format!("det-{}-{}", it.core, it.market)))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open(core, market.clone(), cx);
                }))
                // Right-click requests the custom comparison-tab workflow with lock and broom mode.
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _, _, cx| {
                        this.open_compare(core, market_rmb.clone(), cx);
                        cx.stop_propagation();
                    }),
                );
            container = container.child(card);
        }

        let scroll = div()
            .id("detects-scroll")
            .flex_1()
            .w_full()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .p_2()
            .child(container);

        v_flex()
            .id("detects")
            .relative()
            .size_full()
            .min_h(px(0.0))
            .track_focus(&self.focus)
            .bg(rgb(p.table_body))
            .child(toolbar)
            .child(divider)
            .child(scroll)
    }
}

/// Return whether a retained detection card belongs to the current presentation scope.
///
/// Args:
///     core: Core attached to one retained card.
///     visible: Effective workspace core ids.
///
/// Returns:
///     `true` only when presentation may render the retained card.
fn detection_core_visible(core: CoreId, visible: &[CoreId]) -> bool {
    visible.contains(&core)
}

/// Return whether a detection has outlived its `KeepAlert` window.
///
/// One rule for both readers: `prune` drops the cards that reach it, and ingestion refuses to build
/// a card — or pay for its market snapshot — for a row that would be dropped on the same pass.
///
/// Args:
///     now_ms: Wall-clock time of this pass, in Unix milliseconds.
///     born_ms: When the core reported the detection.
///     ttl_ms: `KeepAlert` for that detection, in milliseconds.
///
/// Returns:
///     `true` when the detection may no longer occupy the feed.
fn detect_expired(now_ms: f64, born_ms: f64, ttl_ms: f64) -> bool {
    now_ms - born_ms >= ttl_ms
}

/// Return whether a retained card survives the AddToChart setting.
///
/// Ingestion already applies the same rule, so this only matters for cards taken in while the
/// setting was on: turning it off must clear them immediately rather than leave them for the rest
/// of their `KeepAlert`.
///
/// Args:
///     add_to_chart: The card's `AddToChart` tab number, `0` when the detect opens no tab.
///     show_add_to_chart: Whether this group displays chart-routed detects in the feed.
///
/// Returns:
///     `true` only when presentation may render the retained card.
fn detection_route_visible(add_to_chart: u32, show_add_to_chart: bool) -> bool {
    add_to_chart == 0 || show_add_to_chart
}
