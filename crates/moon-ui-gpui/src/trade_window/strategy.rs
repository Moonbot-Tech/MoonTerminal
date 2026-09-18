//! The trade's strategy, stated at the head of the figures rail.
//!
//! Two things a reader breaking down an old trade asks and the chart cannot answer: WHICH strategy
//! made it — with a way to get to it — and whether that strategy's settings are still the ones it
//! ran on. The first is the same reveal the Orders table and the Analytics tuner make. The second
//! is the version stamp: the `valid_from` the Versions pane labels its rows with, printed through
//! the same formatter, so the reader finds the row by eye — and a click lands on it.
//!
//! Everything here beyond the live store is one background SQLite read at open
//! (`strategies.sqlite`: the head row with its `deleted` flag, and the version in effect at the
//! entry instant). It lands once, repaints the rail once, and is never re-read: the window shows a
//! closed trade, and nothing about it changes while the window is open.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::session::CoreId;
use moon_core::strat_db::stats::{HeadStatus, VersionAt};
use moon_ui::{
    MoonButton, MoonButtonIconSlot, MoonButtonVariant, MoonPalette, MoonSize, h_flex, v_flex,
};
use rust_i18n::t;

use super::TradeWindowView;
use crate::design;
use crate::design::moon;

/// What `strategies.sqlite` says about the trade's strategy, read once in the background.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct StrategyLookup {
    /// The saved head row, or `None` when the terminal never saved this strategy at all.
    pub head: Option<HeadStatus>,
    /// The saved version in effect at the trade's entry.
    pub version: VersionAt,
}

impl StrategyLookup {
    /// Perform the read. Call from a background executor: this opens the SQLite file.
    ///
    /// Args:
    ///     core: Core that recorded the trade.
    ///     strategy_id: Delphi-signed strategy id from the report replica.
    ///     buy_utc_ms: Entry instant in Unix UTC milliseconds, the clock `valid_from` is on.
    ///
    /// Returns:
    ///     The lookup; an absent database reads as "never saved, no history".
    pub fn read(core: CoreId, strategy_id: i64, buy_utc_ms: i64) -> Self {
        Self {
            head: moon_core::strat_db::stats::head_status(core, strategy_id),
            version: moon_core::strat_db::stats::version_at(core, strategy_id, buy_utc_ms),
        }
    }
}

/// Where the strategy is, as far as the rail can tell — decides the status line and the buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Presence {
    /// The core lists it: named from the live store, reveal goes to the live row.
    Live,
    /// The core no longer lists it, but the terminal kept it: reveal goes to the Deleted branch.
    Deleted,
    /// The core is not connected, so nothing can be revealed; the name may still be known locally.
    Offline,
    /// Neither the core nor the local database has it: the number is all that is left.
    Gone,
    /// The lookup has not landed yet, or the live list has not: no verdict, name if known.
    Pending,
}

impl Presence {
    /// Whether the Strategies window can be asked to show this strategy at all.
    pub fn can_reveal(self) -> bool {
        matches!(self, Self::Live | Self::Deleted | Self::Pending)
    }
}

/// Resolve the presence from the three facts the window holds.
///
/// Args:
///     live_named: The live store named the strategy (the caption is not standing in a number).
///     core_connected: The core is connected and answering (`ConnStatus::Ready`) right now; a
///         configured core that is connecting, staged or dropped counts as not connected.
///     lookup: The background read, once it has landed.
///
/// Returns:
///     The presence the rail should state.
pub(super) fn presence(
    live_named: bool,
    core_connected: bool,
    lookup: Option<&StrategyLookup>,
) -> Presence {
    if live_named {
        return Presence::Live;
    }
    let Some(lookup) = lookup else {
        return Presence::Pending;
    };
    match (&lookup.head, core_connected) {
        (Some(head), _) if head.deleted => Presence::Deleted,
        // Saved as live, yet the connected core's list does not hold it: either the list is still
        // filling or the deletion has not been recorded yet. Both resolve on their own; a reveal
        // meanwhile still has somewhere to go.
        (Some(_), true) => Presence::Pending,
        (Some(_), false) => Presence::Offline,
        (None, true) => Presence::Gone,
        (None, false) => Presence::Offline,
    }
}

/// The status line under the name, if the presence warrants one.
fn status_key(presence: Presence) -> Option<&'static str> {
    match presence {
        Presence::Deleted => Some("trade_window.strategy.in_deleted"),
        Presence::Gone => Some("trade_window.strategy.deleted"),
        Presence::Offline => Some("trade_window.strategy.core_offline"),
        Presence::Live | Presence::Pending => None,
    }
}

/// The version line: its text, its tooltip, and the version a click should open.
///
/// A current version opens nothing in particular — that click is the same as the strategy
/// button's, live mode is exactly where the reader lands anyway. A `Known` past version carries
/// its `valid_from`. `BeforeHistory` says so and opens nothing; `NoHistory` has no line at all.
///
/// Args:
///     version: The saved-version placement of the entry instant.
///     zone: Display zone the Versions pane formats its stamps in.
///     now_ms: Current UTC Unix milliseconds, for the pane's own today/other-day rule.
///
/// Returns:
///     `(text, tooltip key, version to open)`, or `None` for no line.
pub(super) fn version_line(
    version: VersionAt,
    zone: chrono_tz::Tz,
    now_ms: i64,
) -> Option<(String, &'static str, Option<i64>)> {
    match version {
        VersionAt::Known { current: true, .. } => Some((
            t!("trade_window.strategy.version_current").to_string(),
            "trade_window.strategy.version_tip",
            None,
        )),
        VersionAt::Known { valid_from, .. } => {
            // THE SAME formatter the Versions pane labels its rows with: the whole point of the
            // line is that the reader can match it against that pane by eye.
            let stamp =
                moon_core::util::display_time::format_chart_clock(valid_from, zone, false, now_ms);
            Some((
                t!("trade_window.strategy.version", stamp = stamp).to_string(),
                "trade_window.strategy.version_tip",
                Some(valid_from),
            ))
        }
        VersionAt::BeforeHistory => Some((
            t!("trade_window.strategy.version_unknown").to_string(),
            "trade_window.strategy.version_unknown_tip",
            None,
        )),
        VersionAt::NoHistory => None,
    }
}

/// Everything the block prints, resolved from the view's state before any element is built.
pub(super) struct StrategyBlock {
    pub strategy_id: i64,
    pub name: String,
    pub presence: Presence,
    pub version: Option<VersionAt>,
}

impl TradeWindowView {
    /// Resolve the strategy block, or `None` for a trade that carries no strategy.
    ///
    /// Args:
    ///     cx: Application context, for the live store.
    ///
    /// Returns:
    ///     The block's facts, ready to render.
    pub(super) fn strategy_block(&self, cx: &App) -> Option<StrategyBlock> {
        let strategy_id = self.meta.strategy_id?;
        let backend = self.backend.read(cx);
        // `Ready`, not "in the store": a configured core stays in the store through every
        // disconnect, and only its status says whether it can answer a reveal.
        let core_connected = backend
            .session
            .store()
            .core(self.core)
            .is_some_and(|core| core.status == moon_core::feed::ConnStatus::Ready);
        let live_name = (!self.strategy_pending)
            .then(|| super::strategy_name(backend, self.core, strategy_id))
            .flatten();
        let presence = presence(
            live_name.is_some(),
            core_connected,
            self.strategy_lookup.as_ref(),
        );
        // The live name first; the saved head second; the signed number last — the same number
        // the Report's own cell shows, so a caption standing in still matches the table.
        let name = live_name
            .or_else(|| {
                self.strategy_lookup
                    .as_ref()
                    .and_then(|l| l.head.as_ref())
                    .map(|h| h.head.name.clone())
            })
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| strategy_id.to_string());
        Some(StrategyBlock {
            strategy_id,
            name,
            presence,
            version: self.strategy_lookup.as_ref().map(|l| l.version),
        })
    }

    /// Ask the Strategies window to show this trade's strategy, optionally on one saved version.
    ///
    /// Unscoped (`workspace_group = None`) like the Analytics tuner: this window belongs to no
    /// workspace group, so there is no rail selection for a retained callback to violate.
    ///
    /// Args:
    ///     view: This window's view.
    ///     version: `valid_from` of the saved version to open, or `None` for live mode.
    ///     window: This window, as the reveal's owner.
    ///     cx: Application context.
    pub(super) fn open_strategy(
        view: &Entity<Self>,
        version: Option<i64>,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Copied out before the reveal takes `cx` mutably.
        let (backend, core, strategy_id) = {
            let this = view.read(cx);
            (this.backend.clone(), this.core, this.meta.strategy_id)
        };
        let Some(strategy_id) = strategy_id else {
            return;
        };
        let owner_display = window.display(cx).map(|display| display.id());
        crate::strategies::open_goto_version(
            backend,
            core,
            strategy_id as u64,
            version,
            None,
            Some(window.window_handle()),
            owner_display,
            cx,
        );
    }
}

/// Build the rail's strategy block.
///
/// Args:
///     block: The resolved facts.
///     view: This window's view, for the click handlers.
///     p: Active palette.
///     cx: Render context, for scaled type.
///
/// Returns:
///     One label-over-content block shaped like the rail's other cells.
pub(super) fn render_block(
    block: &StrategyBlock,
    view: &Entity<TradeWindowView>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let caption = |text: String| {
        div()
            .text_size(design::t_caption(cx))
            .text_color(moon(p.text_muted))
            .child(text)
    };
    let can_reveal = block.presence.can_reveal();
    let goto_button = |id: &'static str, version: Option<i64>, tip: String| {
        let view = view.clone();
        MoonButton::new(id)
            .width(design::micro_control_h_value(cx))
            .variant(MoonButtonVariant::Soft)
            .size(MoonSize::Xs)
            .leading_icon(MoonButtonIconSlot::new("icons/bot.svg").color(p.text_soft))
            .tooltip(tip)
            .on_click(move |_, window, app| {
                app.stop_propagation();
                TradeWindowView::open_strategy(&view, version, window, app);
            })
            .render()
    };
    let name_row = h_flex()
        .w_full()
        .min_w_0()
        .items_center()
        .gap(design::ui_px(cx, design::CHROME_GAP))
        .child(
            div()
                .id("tw-strategy-name")
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(design::t_body(cx))
                .font_family(design::mono())
                .text_color(moon(p.text))
                .tooltip(crate::panels::common::text_tooltip(block.name.clone()))
                .child(block.name.clone()),
        )
        .when(can_reveal, |el| {
            el.child(goto_button(
                "tw-strategy-goto",
                None,
                t!("coin_menu.strategy_goto").to_string(),
            ))
        });
    let status = status_key(block.presence).map(|key| caption(t!(key).to_string()));
    let version_row = block.version.and_then(|version| {
        let now_ms = moon_core::util::now_unix_ms_i64();
        let zone = crate::chartdx::axes::display_zone();
        let (text, tip_key, open) = version_line(version, zone, now_ms)?;
        let clickable = can_reveal && open.is_some();
        let tip = t!(tip_key).to_string();
        Some(
            h_flex()
                .w_full()
                .min_w_0()
                .items_center()
                .gap(design::ui_px(cx, design::CHROME_GAP))
                .child(
                    div()
                        .id("tw-strategy-version")
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_soft))
                        .tooltip(crate::panels::common::text_tooltip(tip.clone()))
                        .child(text),
                )
                .when(clickable, |el| {
                    el.child(goto_button("tw-strategy-version-goto", open, tip))
                }),
        )
    });
    // No `w_full()`: in the wide rail the column stretches this block like every other cell; in
    // the narrow, wrapping strip a full-width child would take a line of its own and push every
    // other cell onto the rows below.
    v_flex()
        .id(SharedString::from(format!(
            "tw-strategy-{}",
            block.strategy_id
        )))
        .min_w_0()
        .gap(design::ui_px(cx, 1.0))
        .child(caption(t!("trade_window.figure.strategy").to_string()))
        .child(name_row)
        .children(status)
        .children(version_row)
        .into_any_element()
}
