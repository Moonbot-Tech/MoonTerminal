//! The core rows' shared model pieces: the feed flags and their count, the field writers the
//! core card calls (`set_core_active`, `set_for_station`, `apply_pasted_key`), the live status
//! dot, the first-run attention hint, and the add/delete/rebuild actions behind the Cores page.

use gpui::*;
use moon_ui::{MoonPalette, MoonTooltipView};
use rust_i18n::t;

use super::{SettingsView, build_conn, sync_groups_from_servers};
use crate::design;
use moon_core::config::{FeedFlags, Secret, ServerConfig, TotalMode, WorkspaceMembership};
use moon_core::feed::ConnStatus;
use moon_core::session::CoreId;

/// The eight client-side filters on incoming core data, each with a label key, getter, and
/// setter. The ninth item of the menu, `FeedFlags::log_delivery`, is a request to the core and is
/// built apart from these. The constant stores static locale keys rather than rendered labels.
pub(super) const FEED_FLAGS: [(&str, fn(&FeedFlags) -> bool, fn(&mut FeedFlags, bool)); 8] = [
    ("conn.tip.orders", |f| f.orders, |f, v| f.orders = v),
    ("conn.tip.detects", |f| f.detects, |f, v| f.detects = v),
    ("conn.tip.reports", |f| f.reports, |f, v| f.reports = v),
    ("conn.tip.balance", |f| f.balance, |f, v| f.balance = v),
    ("conn.tip.strat", |f| f.strategies, |f, v| f.strategies = v),
    ("conn.tip.log", |f| f.log, |f, v| f.log = v),
    ("conn.tip.alerts", |f| f.alerts, |f, v| f.alerts = v),
    ("conn.tip.arb", |f| f.arb, |f, v| f.arb = v),
];

/// How many of the nine data kinds a core sends: the eight client-side filters plus
/// `log_delivery`. The `n/9` the table trigger and the Cores list tag show.
pub(super) fn feed_count(feed: &FeedFlags) -> u8 {
    let on = FEED_FLAGS.iter().filter(|(_, g, _)| g(feed)).count() + usize::from(feed.log_delivery);
    on as u8
}

/// Every one of the nine data kinds: eight filters, then `log_delivery`.
pub(super) const FEED_TOTAL: u8 = FEED_FLAGS.len() as u8 + 1;

/// Read data kind `ix` (0..8 the filters, 8 `log_delivery`).
pub(super) fn feed_get(feed: &FeedFlags, ix: usize) -> bool {
    match FEED_FLAGS.get(ix) {
        Some((_, get, _)) => get(feed),
        None => feed.log_delivery,
    }
}

/// Set data kind `ix` (0..8 the filters, 8 `log_delivery`).
pub(super) fn feed_set(feed: &mut FeedFlags, ix: usize, on: bool) {
    match FEED_FLAGS.get(ix) {
        Some((_, _, set)) => set(feed, on),
        None => feed.log_delivery = on,
    }
}

/// Whether the station dials this core's typed address too. One writer for the table's cloud
/// glyph and the card's "и для станции" box, so both move the same field.
pub(super) fn set_for_station(s: &mut ServerConfig, on: bool) -> bool {
    let changed = s.endpoint_to_station != on;
    s.endpoint_to_station = on;
    changed
}

/// Switch a core on or off in the draft: the table's "Акт" box and the card's switch.
pub(super) fn set_core_active(s: &mut ServerConfig, on: bool) -> bool {
    let changed = s.active != on;
    s.active = on;
    changed
}

/// Put a pasted key into a draft row: the key, the transport mode it seeds while the row has none,
/// and a voided manual-strategy id (a new key can name another Moonbot, where ids differ).
pub(super) fn apply_pasted_key(s: &mut ServerConfig, text: &str) {
    s.transport = moon_core::config::seeded_transport(s.transport, text);
    s.key = Secret::new(text.to_string());
    if let Some(manual) = s.manual_strategy.as_mut() {
        manual.id = 0;
    }
}

/// Fixed pitch of one core, group, or exchange row in the virtualized Connections list.
///
/// Every entry the list draws -- pending/group/exchange headers included -- takes this same
/// height, because `MoonVirtualList` is `uniform_list`-backed and needs one row height for every
/// item. Provably clipping-free: `fit_height(h, lh, pad) = ui(h).max(line(lh) + ui(pad)*2)`
/// (MoonUI `theme.rs`); the tallest row content is a Small `MoonInput`, whose own fit is
/// `ui(22).max(line(13) + ui(4.5)*2)`. Both terms in this row height strictly dominate the
/// corresponding input-height terms at every font scale because `ui()` and `line()` are monotone,
/// so a Small input never clips.
///
/// No `conn_row_h_px` companion (the house `_value`/`_px` pair, e.g.
/// `analytics/profit_monitor/line.rs::row_h_value`/`row_h_px`): `MoonVirtualList` already wraps
/// every item in a `div().h(px(item_height))` of its own (`virtual_list.rs::render_range`), so
/// nothing here needs a `Pixels` copy of the same number -- one would be dead code.
///
/// Args:
///     cx: Application context used to resolve scaled dimensions.
///
/// Returns:
///     The uniform virtual-list row height in unscaled design units.
pub(in crate::settings) fn conn_row_h_value(cx: &App) -> f32 {
    design::fit_h_value(cx, 30.0, 13.0, 8.5)
}

/// What a status dot's tooltip says when no connection verdict is available for it.
#[derive(Clone, Copy)]
enum StatusFallback {
    /// A plain `conn.status.*` key that needs no interpolation.
    Plain(&'static str),
    /// `conn.status.failed`, whose message slot has nothing to put in it.
    FailedWithoutDetail,
}

impl StatusFallback {
    /// Resolve this fallback state to its localized tooltip text.
    ///
    /// Returns:
    ///     The translated status message, with a placeholder for an absent failure detail.
    fn render(self) -> String {
        match self {
            Self::Plain(key) => t!(key).to_string(),
            Self::FailedWithoutDetail => t!("conn.status.failed", err = "-").to_string(),
        }
    }
}

/// Render a connection-status dot with a localized tooltip, including failure details.
///
/// The dot's COLOUR still comes from the coarse status, because a glance down the column has to
/// stay readable. Its tooltip comes from the verdict: this is where a user pastes the key, so it is
/// also where "why did that not work, and what do I do" belongs.
///
/// The VERDICT is resolved inside the tooltip closure, not here: only two of the six arms can carry
/// one, only a hover ever reads it, and building it eagerly cost a `diagnose()` per core per frame.
///
/// Args:
///     row_key: The owning `ConnRow`'s per-session identity, used for the dot's element id.
///     core_id: Core whose live record the tooltip re-reads at hover time.
///     active: Whether the configured core is enabled.
///     status: Latest lifecycle state, when the runtime has reported one.
///     view: Weak handle used by the tooltip to reach the backend.
///     p: Active palette supplying status colours.
///
/// Returns:
///     A status dot with a tooltip appropriate to the available connection evidence.
pub(super) fn status_dot(
    row_key: u64,
    core_id: CoreId,
    active: bool,
    status: Option<&ConnStatus>,
    view: WeakEntity<SettingsView>,
    p: MoonPalette,
) -> impl IntoElement {
    let (color, fallback, wants_verdict) = match status {
        _ if !active => (
            p.text_soft,
            StatusFallback::Plain("conn.status.inactive"),
            false,
        ),
        Some(ConnStatus::Ready) => (p.green, StatusFallback::Plain("conn.status.ready"), false),
        Some(ConnStatus::Failed(_)) => (p.red, StatusFallback::FailedWithoutDetail, true),
        Some(ConnStatus::Connecting | ConnStatus::Stage(_)) => (
            p.amber,
            StatusFallback::Plain("conn.status.connecting"),
            true,
        ),
        Some(ConnStatus::Disconnected) => (
            p.text_soft,
            StatusFallback::Plain("conn.status.disconnected"),
            false,
        ),
        None => (
            p.text_soft,
            StatusFallback::Plain("conn.status.none"),
            false,
        ),
    };
    div()
        .id(("conn-st", row_key))
        .w(px(10.0))
        .h(px(10.0))
        .rounded_full()
        .bg(rgb(color))
        .tooltip(move |_window, cx| {
            let verdict = wants_verdict
                .then(|| {
                    let view = view.upgrade()?;
                    let backend = view.read(cx).backend.clone();
                    let b = backend.read(cx);
                    let store = b.session.store();
                    let core = store.core(core_id)?;
                    let diag = moon_core::feed::diagnose(
                        &core.status,
                        core.fault.as_ref(),
                        &core.startup,
                    )?;
                    let mode_suggestion =
                        crate::conn_diag::fleet_mode_suggestion(core_id, &b.config.servers, |id| {
                            store
                                .core(id)
                                .is_some_and(|c| c.status == ConnStatus::Ready)
                        });
                    Some(crate::panels::problem_diagnostic_text(
                        &diag,
                        core.fault.as_ref(),
                        &core.startup,
                        mode_suggestion,
                    ))
                })
                .flatten();
            let tip = verdict.unwrap_or_else(|| fallback.render());
            cx.new(|_| MoonTooltipView::new(tip).max_width(320.0))
                .into()
        })
}

impl SettingsView {
    /// Start (or restart) the first-run hint's repaint chain.
    ///
    /// Called once when the window opens and again after a row is added, because adding a row MOVES
    /// the hint: with no rows there is no key field to point at, so the add button carries it, and
    /// the moment a row exists the empty key field is the thing the user actually needs.
    ///
    /// Args:
    ///     cx: The Settings view context.
    ///
    /// Returns:
    ///     Nothing; the chain repaints this view until the hint expires.
    pub(in crate::settings) fn arm_conn_hint(&mut self, cx: &mut Context<Self>) {
        if self.backend.read(cx).config.core_ever_configured() {
            return;
        }
        self.conn_hint_at = Some(std::time::Instant::now());
        crate::pulse::arm(
            self,
            cx,
            |this| &mut this.conn_hint_armed,
            |this| {
                this.conn_hint_at
                    .is_some_and(|at| at.elapsed() < crate::pulse::ATTENTION)
            },
        );
    }

    /// The live first-run hint, or `None` when there is nothing to point at.
    ///
    /// Two conditions, deliberately: the timer bounds how long it breathes, and the SAVED config
    /// decides whether it is still relevant. That second half is what makes a successful Save tear
    /// the ring down on the next frame instead of at the end of the timer, on every control that
    /// asks -- so the teardown lives in ONE place rather than at each call site.
    ///
    /// Args:
    ///     cx: Any app context that can read the backend.
    ///
    /// Returns:
    ///     The arming instant while the hint is live.
    pub(super) fn conn_hint(&self, cx: &App) -> Option<std::time::Instant> {
        self.conn_hint_at
            .filter(|_| !self.backend.read(cx).config.core_ever_configured())
    }

    /// Add a draft server with `id = max + 1` to the given group, then rebuild editor state.
    ///
    /// Args:
    ///     group: Group assigned to the new draft server.
    ///     window: Settings window used to build the new editor states.
    ///     cx: Settings context used to update the draft and request a repaint.
    ///
    /// Returns:
    ///     Nothing; the method adds the row, resets row-owned transient state, and repaints.
    pub(super) fn add_server(
        &mut self,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let default_color = design::u32_to_rgb(MoonPalette::active(cx).accent);
        self.backend.update(cx, |b, bcx| {
            if let Some(p) = b.preview.as_mut() {
                let next = p.servers.iter().map(|s| s.id).max().unwrap_or(0) + 1;
                p.servers.push(ServerConfig {
                    id: next,
                    uid: 0,
                    name: format!("server {next}"),
                    active: true,
                    feed: FeedFlags::default(),
                    key: Secret::new(""),
                    endpoint_override: String::new(),
                    endpoint_to_station: false,
                    group,
                    market: "BTCUSDT".into(),
                    color: default_color,
                    synthetic: false,
                    chart_bundle: String::new(),
                    default_alert_strategy: 0,
                    own_trade_config: false,
                    strat_slots: None,
                    manual_strategy: None,
                    trade: None,
                    // No key yet, so no mode to seed: pasting one fills this in.
                    transport: None,
                    // A new core ships shown everywhere.
                    workspace_membership: WorkspaceMembership::default(),
                    total_mode: TotalMode::default(),
                });
                sync_groups_from_servers(&p.servers, &mut p.groups);
                bcx.notify();
            }
        });
        // The new core is the last row: its card opens at once.
        let last = self
            .backend
            .read(cx)
            .preview
            .as_ref()
            .map(|p| p.servers.len() - 1);
        self.rebuild_conn(last, None, window, cx);
        // The hint MOVES with this row: it pointed at this button, and the thing the newcomer needs
        // next is the empty key field that just appeared. Re-arming restarts the clock so the ring
        // is at full strength on the control that now matters.
        self.arm_conn_hint(cx);
        cx.notify();
    }

    /// Delete draft server `i`, synchronize groups, then rebuild editor state.
    ///
    /// Args:
    ///     i: Draft index of the server to remove.
    ///     window: Settings window used to rebuild the remaining editor states.
    ///     cx: Settings context used to update the draft and request a repaint.
    ///
    /// Returns:
    ///     Nothing; the method removes the row when it exists and clears row-owned transient state.
    pub(super) fn delete_server(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.backend.update(cx, |b, bcx| {
            if let Some(p) = b.preview.as_mut()
                && i < p.servers.len()
            {
                p.servers.remove(i);
                sync_groups_from_servers(&p.servers, &mut p.groups);
                bcx.notify();
            }
        });
        self.rebuild_conn(self.conn_selected_index(), Some(i), window, cx);
        cx.notify();
    }

    /// Draft index of the selected core.
    pub(in crate::settings) fn conn_selected_index(&self) -> Option<usize> {
        let key = self.conn_selected?;
        self.conn.iter().position(|r| r.row_key == key)
    }

    /// Rebuild every core row's editor state after the draft's core list changed.
    ///
    /// Every row gets a new key, so in this order and before anything (a paste affix, an open
    /// menu) can name a row: open menus and the focused row are dropped, then the selection is
    /// moved onto the new keys — the same core when it survived, the one that took its place when
    /// it was deleted, else the first.
    ///
    /// Args:
    ///     selected_index: Draft index to select, as it was before the change.
    ///     removed: Draft index the change deleted.
    ///     window: Settings window the editors are built in.
    ///     cx: Settings context.
    pub(in crate::settings) fn rebuild_conn(
        &mut self,
        selected_index: Option<usize>,
        removed: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picking = None;
        self.conn = build_conn(&self.backend, window, cx);
        let keys: Vec<u64> = self.conn.iter().map(|r| r.row_key).collect();
        self.conn_selected = super::entries::reselect(selected_index, removed, &keys);
    }
}
