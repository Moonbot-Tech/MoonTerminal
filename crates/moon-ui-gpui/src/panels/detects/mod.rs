//! Detachable detection-feed panel, ported from egui's `DetectRibbon`. It ingests group-core rows
//! for which `sound_alert || is_alert`. A row routed to a chart tab (`add_to_chart > 0`) is an
//! ordinary detect in every respect and joins the feed as well once `show_add_to_chart` is on;
//! while it is off, chart tabs show such rows alone, as they always did. Each `(core, market)` card
//! remains for its strategy `KeepAlert` seconds, or the Alerts panel duration when a figure
//! alert carries no strategy.
//! Left-click requests the market on Main without raising its window, while right-click requests a
//! custom comparison tab.
//!
//! **A second source, with no core behind it.** The crowd's rule (`crowd::service`) watches a
//! public statistics service and fires when one coin's rolling minute crosses both its lines. Such
//! a card has no core, no strategy and no server colour; what it does have besides the coin, the
//! money and the trades is BORROWED — the market of the first core in the group that trades the
//! coin lends it a chart, a venue and its price moves, and all of them are drawn. Both its clicks
//! open the coin the way every other bare ticker in the terminal opens: one core outright, several
//! through a picker. It is not scoped to a core, so no display preset can hide it: the crowd is not
//! one of this group's cores.
//!
//! The gear popup configures per-size dimensions, chart type, server rail, and field slots for each
//! group and persists them in `detects_view.toml`; see [`popup`]. Card layout and vector mini-charts
//! built from the snapshot captured at detection time live in [`cards`].

mod cards;
mod crowd;
mod popup;
mod rules;
mod selection;
mod snapshots;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

use crate::Backend;
use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_chart::paint::now_unix_ms;
use moon_core::config::{DETECT_RAIL_MAX, DetectViewCfg};
use moon_core::session::CoreId;
use moon_ui::{
    MoonPalette, MoonSliderEvent, MoonSliderState, Panel, PanelEvent, PanelState, h_flex, v_flex,
};
use rust_i18n::t;

use crate::workspace::scope_marker::ScopeMarker;

use crowd::DetectOrigin;
use rules::{
    crowd_card_yields, detect_card_ttl_ms, detect_expired, detection_core_visible,
    detection_route_visible, detects_sig, empty_feed_text,
};

/// Number of latest five-minute OHLC buckets retained for a card's candle chart, approximately two
/// hours. At a typical 75-130 px chart width, 24 buckets leave roughly 3-5 px per outlined candle;
/// denser candles would visually collapse into solid bars.
const DETECT_THUMB_BARS: usize = 24;

/// Frozen state for one detection-feed card, ported from `src/dock/detects.rs::RibbonItem`.
pub(crate) struct DetectItem {
    origin: DetectOrigin,
    /// The reporting core's name, empty for a crowd card, which names its source at render.
    core_name: String,
    /// Full market key passed to Main or comparison-tab open requests.
    market: String,
    /// Coin label derived from the market without its quote suffix (`ADAUSDT` to `ADA`).
    base: String,
    /// THE key for "is this the same coin on another exchange" — `MarketLabel::identity`, which is
    /// the core's own `market_currency_canonic` where the catalog has one.
    ///
    /// Frozen with the card like every other field, and NOT derived from [`Self::base`]: no rule
    /// over a name can tell Bybit's `1kBONKPERP` from Binance's spelling of the same coin, or tell
    /// either from `1000SATS`, whose thousand is part of its real ticker. Only the catalog knows,
    /// and this is the field it answers with — the same one the arbitrage column borrows quotes by.
    identity: String,
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
    /// Provider to borrow history from; crowd cards may have no matching market.
    snapshot_core: Option<CoreId>,
    /// Set during collection, consumed only after the final trim and expiry pass.
    snapshot_pending: bool,
    /// Unique incarnation across refreshes and even dismissal followed by the same detection.
    snapshot_generation: u64,
    /// Dropped with a superseded or pruned card; publication also validates its generation.
    snapshot_task: Option<Task<()>>,
}

impl DetectItem {
    /// The core that reported this card, or `None` for a crowd detection.
    fn core(&self) -> Option<CoreId> {
        match self.origin {
            DetectOrigin::Core(core) => Some(core),
            DetectOrigin::Crowd { .. } => None,
        }
    }

    /// What the crowd's minute was worth at the crossing, for a card the rule fired.
    fn crowd(&self) -> Option<(f64, u32)> {
        match self.origin {
            DetectOrigin::Crowd { profit, trades } => Some((profit, trades)),
            DetectOrigin::Core(_) => None,
        }
    }

    /// Whether any price history stood behind this card when it was frozen.
    ///
    /// The deltas and the chart come out of the SAME read: `DetectSnapshot` leaves both empty when
    /// the market has no retained history, and leaves the deltas at their `0.0` default while still
    /// filling in the venue. So emptiness here is the honest signal that a printed `0.00%` would be
    /// a flat day measured from nothing — a market name is not, and neither is having a core: a
    /// coin nobody has charted yet has both and no history at all.
    ///
    /// It is a floor, not a proof. A market holding a SINGLE five-minute bucket passes this and
    /// still reports both deltas as exactly zero, because the reader's own fallback compares that
    /// bucket with itself; the same fallback reports half an hour of movement under a 24-hour
    /// caption. Telling those apart needs the snapshot to say "no figure" instead of `0.0`, which
    /// is a change to what every card reads and not to what this one draws.
    fn has_price_history(&self) -> bool {
        !self.bars.is_empty() || !self.line.is_empty()
    }

    /// Stable element identity: what the card IS, never where it currently sits.
    ///
    /// The position shifts under a card whenever the queue drops its oldest or a replay re-sorts
    /// it, and a positional id would hand the element state — an open tooltip among it — to
    /// whichever detection inherits that slot.
    fn key(&self) -> String {
        match self.origin {
            DetectOrigin::Core(core) => format!("det-{core}-{}", self.market),
            DetectOrigin::Crowd { .. } => format!("det-crowd-{}", self.base),
        }
    }
}

pub struct DetectsPanel {
    backend: Entity<Backend>,
    group: String,
    items: VecDeque<DetectItem>,
    last_seq: HashMap<CoreId, u64>,
    /// How far this panel has read the crowd rule's own ring. See [`DetectsPanel::ingest_crowd`].
    crowd_cursor: u64,
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
    /// Monotonic snapshot incarnation shared by ordinary and crowd cards in this panel.
    snapshot_generation: u64,
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
            this.start_snapshots(cx);
            this.arm_prune_timer(cx);
            if changed {
                cx.notify();
            }
        })
        .detach();
        // The crowd's rule wakes ONLY this: its own channel, notified when a coin crosses, which
        // on an ordinary market is a few times an hour. Nothing about it goes through the backend,
        // whose notification would repaint seventeen views for a card in one panel.
        let crowd_detects = backend.read(cx).crowd().read(cx).detect_revision();
        cx.observe(&crowd_detects, |this, _revision, cx| {
            let now = now_unix_ms();
            let mut changed = this.ingest_crowd(now, cx);
            changed |= this.prune(now);
            this.start_snapshots(cx);
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
            cx.new(|_| MoonSliderState::new().max(max).min(min).step(step))
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
            // From zero rather than from the ring's head: the ring holds at most a minute's worth
            // of live cards, and a panel opened just after a crossing should show it exactly as it
            // shows a core detection that fired a moment before the panel existed.
            crowd_cursor: 0,
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
            snapshot_generation: 0,
        };
        let now = now_unix_ms();
        this.ingest(initial_backend.read(cx), now);
        this.ingest_crowd(now, cx);
        this.prune(now);
        this.start_snapshots(cx);
        this.arm_prune_timer(cx);
        this
    }

    /// Apply a changed display preference immediately and mark it for the shared deferred writer.
    /// An unchanged mutation emits neither a write request nor a notification (slider seeding).
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
            b.detects_view_dirty = true;
            bcx.notify();
            true
        });
        if changed {
            cx.notify();
        }
    }

    /// Collect eligible core rows, select the historical surviving queue, then build only its cards.
    /// History is scheduled separately after both sources have trimmed and expired cards are pruned.
    fn ingest(&mut self, b: &Backend, now_ms: f64) -> bool {
        let mut changed = false;
        let show_add_to_chart = b.detects_view.shows_add_to_chart(&self.group);
        let replayed = show_add_to_chart && !self.showed_add_to_chart;
        if show_add_to_chart != self.showed_add_to_chart {
            self.showed_add_to_chart = show_add_to_chart;
            changed = true;
            if show_add_to_chart {
                self.last_seq.clear();
            } else {
                self.items.retain(|it| it.add_to_chart == 0);
            }
        }
        let order = moon_core::session::core_order::CoreOrder::new(&b.config);
        let mut cores: Vec<_> = b
            .session
            .sessions()
            .iter()
            .filter(|s| s.group == self.group)
            .collect();
        order.sort_by(&mut cores, |session| session.id);
        let mut incoming = Vec::new();
        for session in cores {
            let id = session.id;
            let Some(d) = b.session.store().core(id) else {
                continue;
            };
            let color = crate::core_color::core_color(&b.config.servers, id)
                .unwrap_or(DEFAULT_SERVER_COLOR);
            let last = self.last_seq.get(&id).copied().unwrap_or(0);
            let mut fresh: Vec<&moon_core::feed::DetectRow> = Vec::new();
            let mut newest_of_market: HashSet<&str> = HashSet::new();
            for det in d.detects.iter().rev() {
                if det.seq <= last {
                    break;
                }
                // Deduplicate before eligibility, exactly as the old ingest did.
                if newest_of_market.insert(det.market.as_str()) {
                    fresh.push(&**det);
                }
            }
            if fresh.is_empty() {
                continue;
            }
            self.last_seq.insert(id, fresh[0].seq);
            for det in fresh.into_iter().rev() {
                if !det.sound_alert && !det.is_alert {
                    continue;
                }
                if det.add_to_chart > 0 && !show_add_to_chart {
                    continue;
                }
                let ttl =
                    detect_card_ttl_ms(det.keep_alert_secs, det.is_alert, b.alert_duration_s());
                if detect_expired(now_ms, det.time_ms, ttl) {
                    continue;
                }
                incoming.push(selection::Incoming {
                    core: id,
                    name: &session.name,
                    color,
                    row: det,
                    ttl_ms: ttl,
                });
            }
        }
        let selected = selection::select(&self.items, incoming, replayed);
        changed |= selected.len() != self.items.len()
            || selected.iter().any(|slot| slot.incoming.is_some());
        let mut held: Vec<_> = std::mem::take(&mut self.items)
            .into_iter()
            .map(Some)
            .collect();
        for slot in selected {
            let old = slot.held.and_then(|index| held[index].take());
            let Some(next) = slot.incoming else {
                self.items.push_back(old.expect("retained card"));
                continue;
            };
            let det = next.row;
            let label = b
                .session
                .market_source()
                .market_label(next.core, &det.market);
            let mut item = old.unwrap_or_else(|| DetectItem {
                origin: DetectOrigin::Core(next.core),
                core_name: next.name.to_string(),
                market: det.market.clone(),
                base: String::new(),
                identity: String::new(),
                color: next.color,
                kind: det.kind,
                is_short: det.is_short,
                strat_name: String::new(),
                is_alert: det.is_alert,
                add_to_chart: det.add_to_chart,
                born_ms: det.time_ms,
                ttl_ms: next.ttl_ms,
                bars: Vec::new(),
                line: Vec::new(),
                delta_24h: 0.0,
                delta_1h: 0.0,
                venue: None,
                exchange_kind: String::new(),
                snapshot_core: Some(next.core),
                snapshot_pending: false,
                snapshot_generation: 0,
                snapshot_task: None,
            });
            item.born_ms = det.time_ms;
            item.ttl_ms = next.ttl_ms;
            item.color = next.color;
            item.base = label.display_coin().to_string();
            item.identity = label.identity();
            item.kind = det.kind;
            item.is_short = det.is_short;
            item.strat_name = det.strat_name.clone();
            item.is_alert = det.is_alert;
            item.add_to_chart = det.add_to_chart;
            // Invalidate before scheduling: an older completion cannot publish during this refresh.
            item.snapshot_generation = 0;
            item.snapshot_task = None;
            item.snapshot_pending = true;
            self.items.push_back(item);
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
    /// flag, prunes expired cards against wall-clock time, notifies so their countdowns update,
    /// and rearms itself. Pruning to zero stops the timer but still issues its FINAL
    /// notification: that render is the one that replaces the last card with the empty state.
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
                    let pruned = this.prune(now_unix_ms());
                    // Refresh countdowns while cards remain, and paint the transition to empty
                    // exactly once. Gating this on a non-empty queue alone left the LAST expired
                    // card painted until some unrelated notification arrived — harmless while an
                    // empty feed drew nothing, but it is now the render that puts the
                    // empty-state sentence on screen.
                    if pruned || !this.items.is_empty() {
                        cx.notify();
                    }
                    this.arm_prune_timer(cx);
                })
                .is_ok()
            });
            if !alive {}
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
            .retain(|it| !(it.core() == Some(core) && it.market == market));
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
            .retain(|it| !(it.core() == Some(core) && it.market == market));
        self.arm_prune_timer(cx);
        cx.notify();
    }

    /// Returns this group's `detects_view.toml` display configuration or its default.
    fn view_cfg(&self, cx: &App) -> DetectViewCfg {
        self.backend.read(cx).detects_view.group(&self.group)
    }
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
    ///     Toolbar and filtered card grid without discarding out-of-scope retained cards, or the
    ///     toolbar above one centred sentence naming why the feed is empty.
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
        // The scope value is kept, not consumed straight into its ids: its membership counts are
        // what tells an empty feed whether a preset is hiding a full one.
        let (marker, visible_cores, available_cores, retained_reachable) = {
            let b = self.backend.read(cx);
            let scope =
                b.effective_workspace_scope(&self.group, crate::workspace::RetainedCoreScope::All);
            let available = scope.membership_total();
            let marker = ScopeMarker::new(
                b.display_preset(crate::workspace::DisplayOwner::Group(&self.group)),
                scope.membership_shown(),
                available,
            );
            // Count only the retained cards a preset change could actually bring BACK. A card
            // whose core has gone unavailable is unreachable, not hidden: deactivating a server
            // makes `SessionManager::reconcile` drop the core outright
            // (`moon-core/src/session/lifecycle.rs:304`), and nothing evicts the card it left
            // behind until its `KeepAlert` expires. Counting it as retained would tell a
            // multi-core group whose other cores are merely idle that the scope is hiding
            // detects, and send the user widening a preset that can never reveal them.
            // A crowd card is not counted either, for a different reason: no preset can hide it,
            // so it can never be one of the cards a preset change would bring back.
            let retained_reachable = self
                .items
                .iter()
                .filter(|it| {
                    it.core().is_some_and(|core| {
                        b.workspace_core_availability(&self.group, core)
                            .is_available()
                    })
                })
                .count();
            (marker, scope.ids().to_vec(), available, retained_reachable)
        };

        // Place the gear-triggered configuration toolbar and divider above the feed.
        let toolbar = popup::toolbar(self, &cfg, p, cx);
        let divider = div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border));

        // Render fixed-size cards in reverse insertion order in a wrapping grid. Newly inserted
        // markets appear first; a repeated core-market detection refreshes its existing position.
        let mut container = h_flex().flex_wrap().gap_1p5().content_start();
        let mut shown = 0usize;
        // Coins a core has already put on this screen. Collected from the cards that pass the
        // same two filters the loop below applies, so a core card the preset is hiding does not
        // silently take the crowd's card down with it — and the crowd's card is NOT dropped at
        // ingest, so it appears by itself the moment the core's own card expires.
        let cored: HashSet<&str> = self
            .items
            .iter()
            .filter(|it| {
                it.core().is_some()
                    && detection_core_visible(it.core(), &visible_cores)
                    && detection_route_visible(it.add_to_chart, cfg.show_add_to_chart)
            })
            .map(|it| it.identity.as_str())
            .collect();

        for it in self.items.iter().rev().filter(|item| {
            detection_core_visible(item.core(), &visible_cores)
                && detection_route_visible(item.add_to_chart, cfg.show_add_to_chart)
                && (item.crowd().is_none() || !crowd_card_yields(&item.identity, &cored))
        }) {
            let secs = ((it.ttl_ms - (now - it.born_ms)) / 1000.0).ceil().max(0.0) as u32;
            let card = cards::card(it, secs, &cfg, &theme, &badges, p, is_light, cx)
                .id(SharedString::from(it.key()))
                .cursor_pointer();
            let card = match it.core() {
                Some(core) => {
                    let market = it.market.clone();
                    let market_rmb = it.market.clone();
                    card.on_click(cx.listener(move |this, _, _, cx| {
                        this.open(core, market.clone(), cx);
                    }))
                    // Right-click requests the custom comparison-tab workflow with lock and broom
                    // mode.
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _, _, cx| {
                            this.open_compare(core, market_rmb.clone(), cx);
                            cx.stop_propagation();
                        }),
                    )
                }
                // Both buttons open the coin. Comparison is anchored on a core, and this card has
                // none — picking "whichever core happens to trade it" for a gesture the reader did
                // not aim at any core would be an invention, so the honest answer is the same
                // picker the left button raises.
                None => {
                    let coin = it.base.clone();
                    let coin_rmb = it.base.clone();
                    // `on_click`, like every core card: a picker raised on the press and left open
                    // under a held button is a different gesture from the one every other card here
                    // answers to.
                    card.on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        this.open_crowd(&coin, event.position(), window, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.open_crowd(&coin_rmb, event.position, window, cx);
                            cx.stop_propagation();
                        }),
                    )
                }
            };
            container = container.child(card);
            shown += 1;
        }

        // An empty feed states WHY it is empty instead of painting a blank pane under the gear. It
        // replaces the scroll box rather than sitting inside it — an empty scroll container would
        // still own the `flex_1` slot and leave the sentence pinned to the top-left corner.
        //
        // A COLUMN, not the Log panel's centred row, and the text carries its own definite width.
        // These sentences are whole clauses where `log.empty_filtered` is two words, so at a ~290px
        // side dock they have to WRAP — and a centred flex ROW cannot wrap them. GPUI measures text
        // with `wrap_width = known_dimensions.width.or(Definite(available))` (moon-gpui
        // `elements/text.rs:650`), so the min-content probe taffy runs to fix a row item's automatic
        // minimum width comes back with the whole one-line sentence: the item refuses to shrink,
        // overflows a narrow panel symmetrically under `justify_center`, and is clipped on BOTH
        // sides. `.text_center()` cannot save it — it centres lines inside a box already wider than
        // the panel. Giving the text `w_full` makes its width DEFINITE, which is the one input the
        // measure above needs; `max_w` then keeps a wide, undocked panel from stretching one clause
        // across the whole pane. Same shape as `analytics/render.rs`'s `quote_split_note`, and the
        // row-axis mirror of the rule pinned in `tests/theme_contract/shell.rs`.
        let body: AnyElement = if shown == 0 {
            v_flex()
                .flex_1()
                .w_full()
                .min_h(px(0.0))
                .items_center()
                .justify_center()
                .px_3()
                .py_2()
                .font_family(crate::design::ui_font())
                .text_size(crate::design::t_body(cx))
                .text_color(rgb(p.text_soft))
                .child(
                    v_flex()
                        .w_full()
                        .max_w(crate::design::font_w_px(cx, 560.0))
                        .items_center()
                        .gap_1p5()
                        .child(
                            div()
                                .text_size(crate::design::t_title(cx))
                                .text_color(rgb(p.text_muted))
                                .child("⚙"),
                        )
                        .child(div().w_full().text_center().child(empty_feed_text(
                            &marker,
                            retained_reachable,
                            available_cores,
                        )))
                        .when(available_cores > 0 && retained_reachable == 0, |block| {
                            block.child(
                                div()
                                    .w_full()
                                    .text_center()
                                    .text_size(crate::design::t_caption(cx))
                                    .text_color(rgb(p.text_muted))
                                    .child(t!("detects.empty_settings_hint").to_string()),
                            )
                        }),
                )
                .into_any_element()
        } else {
            div()
                .id("detects-scroll")
                .flex_1()
                .w_full()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .p_2()
                .child(container)
                .into_any_element()
        };

        v_flex()
            .id("detects")
            .relative()
            .size_full()
            .min_h(px(0.0))
            .track_focus(&self.focus)
            .bg(rgb(p.table_body))
            .child(toolbar)
            .child(divider)
            .child(body)
    }
}
