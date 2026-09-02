//! Top chrome of the Analytics window: tab bar, filter combos (cores / side /
//! trade kind), the "from"–"to" date fields, the replica integrity note and the
//! period bar with presets. Controls only — the state lives in `mod.rs`.

use std::collections::HashSet;

use gpui::*;
use moon_ui::{
    MoonAlert, MoonButton, MoonButtonIconSlot, MoonButtonSize, MoonButtonVariant,
    MoonDateTimePicker, MoonDropdown, MoonInput, MoonMenuItem, MoonMenuSize, MoonPalette,
    MoonSegmentItem, MoonSegmentedControl, h_flex,
};
use rust_i18n::t;

use super::refresh::RefreshUrgency;
use super::{AnalyticsView, Period, ProfitLoadState, Tab};
use crate::design;
use crate::design::moon;
use moon_core::config::CoreGroup;
use moon_core::db::analytics::UndatedCloses;
use moon_core::db::integrity::Integrity;
use moon_core::db::report_recovery::RecoveryNotice;
use moon_core::db::{ProfitMetric, ReadFail, SideFilter};

use crate::workspace::query_core_ids;

/// One entry of the money-scale selector.
///
/// The query stores two independent flags — the profit metric and the USDT preference — because
/// they mean different things to it. A user picks ONE scale, so the menu is a single radio group
/// and this type is where the two representations meet, rather than at each call site.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MetricChoice {
    /// Report each scope in whatever quote its own trades used.
    Native,
    /// Convert every scope to USDT wherever the rows can be valued.
    Usdt,
    /// Return on spent capital, which has no currency at all.
    Percent,
}

impl MetricChoice {
    /// Fold the stored flags into the selected entry.
    pub(super) fn of(metric: ProfitMetric, prefer_usdt: bool) -> Self {
        match (metric, prefer_usdt) {
            (ProfitMetric::Percent, _) => Self::Percent,
            (ProfitMetric::Quote, true) => Self::Usdt,
            (ProfitMetric::Quote, false) => Self::Native,
        }
    }

    /// Expand the selected entry back into the flags the query carries.
    pub(super) fn flags(self) -> (ProfitMetric, bool) {
        match self {
            Self::Native => (ProfitMetric::Quote, false),
            Self::Usdt => (ProfitMetric::Quote, true),
            // The USDT preference is meaningless without money, and keeping it set would make
            // returning to quote money silently land on a different scale than the user left.
            Self::Percent => (ProfitMetric::Percent, false),
        }
    }
}

/// Unscaled width of the Analytics side-selector trigger.
const SIDE_TRIGGER_W: f32 = 69.0;
/// Unscaled width of the Analytics trade-kind-selector trigger.
const KIND_TRIGGER_W: f32 = 102.0;
/// Unscaled width of the Analytics profit-metric-selector trigger.
const METRIC_TRIGGER_W: f32 = 116.0;
/// Unscaled horizontal spacing between neighboring toolbar controls.
const TOOLBAR_GAP: f32 = 6.0;

/// Unscaled width of the strategy-name mask field.
///
/// The same 150 the Report gives its own mask box, so a user who knows one field's reach
/// reads the other the same way.
const MASK_FIELD_W: f32 = 150.0;

/// Base floor for a period preset's fitted cell width.
const PRESET_CELL_MIN_W: f32 = 44.0;

/// Base ceiling for a period preset's fitted cell width.
///
/// A long localized label cannot stretch the strip across the window; MoonUI truncates the visible
/// label, and the item's tooltip still carries the whole of it.
const PRESET_CELL_MAX_W: f32 = 104.0;

/// Reserved width for the trade counter that closes the period bar on its right edge.
///
/// The counter's exact text varies with the trade count and the active locale, so this is a
/// conservative floor rather than a measurement: six digits plus the localized noun is wider than
/// any realistic count, and undercounting here would let the collapse decision keep the preset row
/// inline for a beat after the counter has already started to clip.
const PERIOD_COUNTER_RESERVED_W: f32 = 90.0;

#[cfg(test)]
mod tests;

/// Whether the Analytics period-preset strip stays an inline row at `available` width, or must
/// collapse into a single dropdown trigger instead.
///
/// The presets are the ONLY thing that yields on this bar — the custom-range group and the trade
/// counter never shrink, wrap, or clip, so `available` already excludes their own reserved width.
/// Kept a pure function of two plain widths, with no `cx`/`App` dependency at all, mirroring the
/// cx-free half of `design::ticker_visible`'s window-level threshold and
/// `controls/toolbar.rs::label_ladder`'s decision split — so it is unit-testable with no window and
/// no theme. Inclusive: the row stays a row at the exact pixel it still fits, the same convention
/// every other collapse threshold in this window uses.
///
/// Args:
///     available: Width left for the presets after every other atom on the bar has taken its own.
///     presets_row_width: Total width the segmented preset row would occupy inline.
///
/// Returns:
///     `true` when the row fits inline, `false` when it must collapse to a dropdown trigger.
pub(super) fn presets_row_fits(available: f32, presets_row_width: f32) -> bool {
    available >= presets_row_width
}

/// What the "undated trades" strip should be right now.
///
/// Three states rather than an `Option` plus a bool: collapsed, the strip does not disappear
/// — it SHRINKS to a line carrying the way back. A banner the user can hide with no way to
/// unhide it is a setting they cannot find again.
#[derive(Debug, PartialEq)]
pub(super) enum UndatedBanner {
    /// Nothing to say — no undated trades, or the count is not known yet.
    None,
    /// The full warning: heading + the sentence about the money it excludes.
    Full(String, String),
    /// The default: one muted line naming the count, plus a way to open it.
    Collapsed(String),
    /// The undated-close query failed, so absence cannot be claimed.
    Failed(String, String),
}

/// Presentation provenance for the caption beside the Analytics core selector.
///
/// Membership alone is insufficient: a manually rebuilt selection must remain an ordinary
/// numeric multi-select even when it happens to equal a saved group. The retained name records
/// the user's explicit group action, while [`Self::visible_name`] revalidates its live meaning.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct CoreSelectionCaption {
    /// Last explicitly applied saved group, or none after an effective manual edit.
    applied_group: Option<String>,
}

impl CoreSelectionCaption {
    /// Replace the retained saved-group provenance.
    ///
    /// Args:
    ///     group: Exact live group name after a saved-group action, or `None` when the resulting
    ///         selection is not that group.
    ///
    /// Returns:
    ///     Whether the presentation state changed.
    pub(super) fn set_applied_group(&mut self, group: Option<String>) -> bool {
        if self.applied_group == group {
            return false;
        }
        self.applied_group = group;
        true
    }

    /// Forget group provenance after an effective manual selection edit.
    ///
    /// Returns:
    ///     Whether a saved-group caption was cleared.
    pub(super) fn manual_selection_changed(&mut self) -> bool {
        self.applied_group.take().is_some()
    }

    /// Resolve the meaningful live name shown beside the numeric selector trigger.
    ///
    /// Args:
    ///     groups: Current saved-group definitions from configuration.
    ///     cores: Cores currently presented by Analytics, with display names.
    ///     selected: Effective explicit selection; empty represents All.
    ///
    /// Returns:
    ///     The explicitly applied group's current name when it still exactly matches, otherwise
    ///     the sole selected core name, or `None` for an ordinary multi-select/All state.
    pub(super) fn visible_name<'a>(
        &self,
        groups: &'a [CoreGroup],
        cores: &'a [(u64, String)],
        selected: &HashSet<u64>,
    ) -> Option<&'a str> {
        if let Some(applied_name) = self.applied_group.as_deref() {
            let selectable = cores.iter().map(|(core, _)| *core).collect();
            if let Some(group) = groups.iter().find(|group| group.name == applied_name)
                && crate::controls::group_is_applied(&group.cores, &selectable, selected)
            {
                return Some(group.name.as_str());
            }
        }
        sole_core_name(cores, selected)
    }
}

/// The single selected core's display name, or `None` when the tab bar must not name one.
///
/// Answers with a name exactly when the Analytics trigger shows the count `1`, including an
/// explicitly selected core in a single-core installation. The exclusive All state is empty and
/// answers `None`; naming a core beside a trigger reading "All cores" would assert a filter the
/// user did not select.
///
/// "Exactly one" is counted over the cores that still EXIST, not over the raw selection, because
/// that is what the trigger counts: a selection deliberately keeps the id of a deleted core so it
/// cannot silently broaden the query, and `{live, deleted}` shows as `1` while other live cores
/// remain available. Counting raw ids would leave that case reading "1" with no name beside it.
///
/// A selection of nothing but stale ids resolves to no core and answers `None` — a deleted core
/// must not hand its name to whichever row happens to sit first.
///
/// Args:
///     cores: Available core ids and names, as the analytics read returned them.
///     selected: Currently selected core ids.
///
/// Returns:
///     The sole selected core's name, or `None`.
pub(super) fn sole_core_name<'a>(
    cores: &'a [(u64, String)],
    selected: &HashSet<u64>,
) -> Option<&'a str> {
    if selected.is_empty() {
        return None;
    }
    let mut live = cores.iter().filter(|(id, _)| selected.contains(id));
    let (_, name) = live.next()?;
    live.next().is_none().then_some(name.as_str())
}

/// Convert the effective Analytics selection into database filter ids.
///
/// Args:
///     selected: Retained explicit core ids; an empty set represents the unpinned All row.
///     workspace: Concrete Auto-workspace ids when a selected rail core pins Analytics.
///     hidden: Configured cores the Classic viewing preset hides, or `None` when it hides none.
///     universe: Every core this window's replica read can name, for expanding an implicit "All"
///         selection before subtracting `hidden` from it.
///
/// Returns:
///     Workspace ids when pinned (`hidden` never applies to a pinned Auto rail — the group already
///     bounds it), every retained explicit id when unpinned and nothing is hidden, the unfiltered
///     retained selection while the implicit All row cannot yet be expanded (`universe` still
///     empty — a fresh window's first query, before its replica read has returned), or an empty
///     unfiltered list for All once the universe is known. An empty pinned Auto group, and a
///     non-`None` `hidden` whose subtraction leaves no core selected, both route through
///     [`query_core_ids`] as a PRESENT-but-empty scope: see `moon_core::config::NO_MATCH_CORE_UID`
///     for why an empty `Vec` there would mean "unfiltered" and reproduce the original bug.
pub(super) fn analytics_core_filter_ids(
    selected: &HashSet<u64>,
    workspace: Option<&[u64]>,
    hidden: Option<&[u64]>,
    universe: &[(u64, String)],
) -> Vec<u64> {
    match workspace {
        Some([]) => return query_core_ids(Vec::new(), true),
        Some(cores) => return cores.to_vec(),
        None => {}
    }
    let Some(hidden) = hidden else {
        return selected.iter().copied().collect();
    };
    // A fresh window's replica universe starts empty and only arrives with the FIRST query's own
    // result, after this same call already built that query. An empty `selected` (the implicit
    // "All" row) can only be expanded against a non-empty `universe`; expanding it against an
    // empty one would collapse to the no-match sentinel before a single core is known, turning a
    // brand-new Classic-scoped window falsely empty instead of leaving it unfiltered for one frame
    // like every other bootstrap read in this view. A non-empty EXPLICIT `selected` never depends
    // on `universe` at all and is filtered against `hidden` unconditionally below, exactly as it
    // always was.
    if selected.is_empty() && universe.is_empty() {
        return selected.iter().copied().collect();
    }
    let base: Vec<u64> = if selected.is_empty() {
        universe.iter().map(|(id, _)| *id).collect()
    } else {
        selected.iter().copied().collect()
    };
    let filtered: Vec<u64> = base.into_iter().filter(|id| !hidden.contains(id)).collect();
    query_core_ids(filtered, true)
}

/// Decide what the strip shows, given only the read outcome and whether the user opened it.
///
/// A free function rather than a method so the decision can be exercised directly: it is the
/// only thing standing between "money is missing from these figures" and silence.
///
/// A read FAILURE outranks everything, collapsing included — a query that did not run cannot
/// be summarised as a count, and hiding it behind a one-liner would let a broken replica read
/// as a small tidy footnote.
///
/// Args:
///     error: Classified failure of the undated-row read.
///     undated: Successfully loaded safe per-quote totals.
///     expanded: Whether the user opened the detailed strip.
///
/// Returns:
///     Exact hidden, collapsed, expanded, or failure presentation state.
pub(super) fn undated_banner_state(
    error: Option<&ReadFail>,
    undated: Option<UndatedCloses>,
    expanded: bool,
) -> UndatedBanner {
    if let Some(error) = error {
        return UndatedBanner::Failed(t!("common.db_read_failed").to_string(), error.to_string());
    }
    let Some(u) = undated else {
        return UndatedBanner::None;
    };
    if u.is_empty() {
        return UndatedBanner::None;
    }
    if !expanded {
        return UndatedBanner::Collapsed(
            t!("analytics.undated_collapsed", n = u.totals.orders).to_string(),
        );
    }
    let mut amounts = u
        .totals
        .totals
        .iter()
        .copied()
        .map(|total| total.signed_display().0)
        .collect::<Vec<_>>();
    if u.totals.unknown_orders > 0 {
        amounts.push(
            t!(
                "analytics.quote_unknown_orders",
                n = u.totals.unknown_orders
            )
            .to_string(),
        );
    }
    UndatedBanner::Full(
        t!("analytics.undated_title").to_string(),
        t!(
            "analytics.undated_detail",
            n = u.totals.orders,
            totals = amounts.join(" · ")
        )
        .to_string(),
    )
}

impl AnalyticsView {
    /// Build the top-level tab bar and retain page changes for later window recreation.
    ///
    /// Args:
    ///     p: Active palette for selected and unselected buttons.
    ///     cx: Analytics context used to wire click handlers.
    ///
    /// Returns:
    ///     The rendered tab-strip element.
    pub(super) fn tabs_bar(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let filter_pin = self.core_filter_pin();
        let workspace_pinned = filter_pin.is_some();
        let presented_selection: HashSet<u64> = match filter_pin {
            Some(scope) => scope.selected_core.into_iter().collect(),
            None => self.sel_cores.clone(),
        };
        let presented_cores: Vec<(u64, String)> = match filter_pin {
            Some(scope) => self
                .cores
                .iter()
                .filter(|(core, _)| scope.core_ids.contains(core))
                .cloned()
                .collect(),
            // Unpinned: the universe narrowed by whatever the Classic viewing preset hides — the
            // pinned arm above is untouched, since Auto membership is a separate, group-scoped
            // question `analytics_workspace_scope` already answers.
            None => {
                let hidden = self.hidden_core_ids();
                self.cores
                    .iter()
                    .filter(|(core, _)| hidden.is_none_or(|h| !h.contains(core)))
                    .cloned()
                    .collect()
            }
        };
        let core_caption = if workspace_pinned {
            sole_core_name(&presented_cores, &presented_selection)
                .map(crate::display_text::flatten_lines)
        } else {
            let backend = self.backend.read(cx);
            self.core_caption
                .visible_name(
                    &backend.config.core_groups,
                    &presented_cores,
                    &presented_selection,
                )
                .map(crate::display_text::flatten_lines)
        };
        let mut row = h_flex()
            .flex_none()
            .w_full()
            .min_h(design::fit_h_px(cx, 34.0, 13.0, 10.5))
            .flex_wrap()
            .gap_x(design::ui_px(cx, TOOLBAR_GAP))
            .gap_y(design::ui_px(cx, 4.0))
            .pt(design::ui_px(cx, 5.0))
            .pb(design::ui_px(cx, 4.0))
            .px(design::ui_px(cx, 8.0))
            .items_center()
            .bg(moon(p.shell_high))
            .border_b_1()
            .border_color(moon(p.border));
        for t in Tab::ALL {
            let on = self.tab == t;
            let title = t.title();
            // The Calendar is the one tab that browses time by its own navigation and hides the
            // period bar, so a rule separates it from the two that share the window's filters —
            // `Tab::ALL` puts it last precisely so this stands between the two kinds.
            if t == Tab::Calendar {
                row = row.child(design::chrome_divider(cx, p));
            }
            // MoonButton's custom size has no horizontal padding, so give each localized title
            // measured breathing room while retaining a useful click target for short labels.
            let tab_width = (design::ui_text_width(cx, &title, 10.5, 400.0, true)
                + design::ui_value(cx, 20.0))
            .max(design::ui_value(cx, 72.0));
            row = row.child(
                MoonButton::new(t.id())
                    .variant(if on {
                        MoonButtonVariant::Blue
                    } else {
                        MoonButtonVariant::Ghost
                    })
                    .size(MoonButtonSize::Custom {
                        height: 24.0,
                        radius: design::R_BUTTON_BASE,
                        font_size: 10.5,
                        line_height: 13.0,
                        gap: 5.0,
                    })
                    .width(tab_width)
                    .selected(on)
                    .label(title)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.tab != t {
                            this.tab = t;
                            this.backend.update(cx, |b, _| {
                                b.ui_session.analytics.tab = t;
                            });
                            // Each tab remembers its OWN time window: re-sync the
                            // period bar and the "from"/"to" fields to the active tab.
                            this.sync_period_pickers(window, cx);
                            // The new tab's time window differs from the one `data`
                            // was built for → reload, or the strategy list and the
                            // summary would show another tab's period. reload() also
                            // pulls the active tab's secondary data (tuner/profile).
                            let period_changed = match t {
                                Tab::Summary => this.active_period() != this.data_period,
                                Tab::Strategies => {
                                    this.active_period() != this.strategy_data_period
                                }
                                Tab::Calendar => false,
                            };
                            let base_dirty = match t {
                                Tab::Summary => this.data_dirty,
                                Tab::Strategies => this.strategy_dirty,
                                Tab::Calendar => false,
                            };
                            if period_changed {
                                this.reload(cx);
                            } else if matches!(t, Tab::Summary | Tab::Strategies) && base_dirty {
                                // A hidden base view can lag a generation while Calendar alone
                                // refreshes. Catch it up on entry without destructive scope
                                // invalidation, which would erase tuner drafts.
                                this.request_report_refresh(RefreshUrgency::User, true, cx);
                            } else {
                                // Tab-entry catch-up uses the report gate so it cannot overlap
                                // an automatic full-period scan already in flight.
                                if t == Tab::Strategies {
                                    this.request_axis_if_stale(this.strat_mode, cx);
                                }
                                if t == Tab::Calendar
                                    && (this.cal_days.data().is_none() || this.cal_dirty)
                                {
                                    this.request_report_refresh(RefreshUrgency::User, true, cx);
                                }
                            }
                            cx.notify();
                        }
                    }))
                    .render(),
            );
        }
        // Keep the selector widths and their internal gaps together. One additional gap belongs to
        // the caption, so the whole semantic group moves to the next line before any control is
        // clipped. MoonUI scales Action dropdown widths from their 10.5px reference font.
        let action_trigger_scale = design::font_value(cx, 10.5) / 10.5;
        let clear_core_filter_w = if workspace_pinned || self.sel_cores.is_empty() {
            0.0
        } else {
            design::glyph_btn_w(cx) + design::ui_value(cx, TOOLBAR_GAP)
        };
        let filters_min_w = clear_core_filter_w
            + (crate::controls::CORE_COMBO_TRIGGER_W
                + SIDE_TRIGGER_W
                + KIND_TRIGGER_W
                + METRIC_TRIGGER_W)
                * action_trigger_scale
            + design::ui_value(cx, TOOLBAR_GAP * 4.0);
        let clear_core_filter = (!workspace_pinned && !self.sel_cores.is_empty()).then(|| {
            MoonButton::new("an-core-clear")
                .width(design::glyph_btn_w(cx))
                .variant(MoonButtonVariant::Ghost)
                .size(MoonButtonSize::Action)
                .leading_icon(MoonButtonIconSlot::new("icons/close.svg"))
                .tooltip(t!("analytics.core_selection.clear").to_string())
                .on_click(cx.listener(|this, _, _, cx| this.toggle_core(None, cx)))
                .render()
        });
        let selectors = h_flex()
            .flex_none()
            .gap(design::ui_px(cx, TOOLBAR_GAP))
            .children(clear_core_filter)
            .child(self.core_combo(cx))
            .child(self.side_combo(cx))
            .child(self.kind_combo(cx))
            .child(self.metric_combo(cx));
        // Right alignment keeps the selected core beside its selector. `flex_1 + min_w_0 +
        // truncate` lets the caption yield all optional width before the atomic selector group
        // wraps. `flatten_lines` folds a hard break that the one-line caption would otherwise clip.
        let filters = h_flex()
            .flex_1()
            .min_w(px(filters_min_w))
            .items_center()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .pr(design::ui_px(cx, TOOLBAR_GAP))
                    .text_right()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .children(core_caption),
            )
            .child(selectors);
        // The strategy-name mask is its OWN atomic group, a peer of `filters` rather than a child
        // of it: the selector group does not wrap internally, so folding the field in there would
        // raise that atom's floor by the field's whole width and clip it in a narrow dock instead
        // of moving it to the next line. The divider travels INSIDE the group for the same reason
        // the period bar keeps its own: a free rule can wrap onto a line by itself.
        //
        // It lives here and not in `period_bar` because `render` hides that bar entirely on the
        // Calendar tab, and the Calendar is one of the surfaces the mask has to narrow.
        let mask = design::chrome_section(cx)
            .child(design::chrome_divider(cx, p))
            .child(
                div()
                    .id("an-strategy-mask-tip")
                    .flex_none()
                    .w(design::font_w_px(cx, MASK_FIELD_W))
                    .tooltip(crate::panels::common::text_tooltip(
                        t!("analytics.filter.strategy_mask_tip").to_string(),
                    ))
                    .child(
                        MoonInput::new("an-strategy-mask")
                            .state(&self.strategy_mask_input)
                            .small()
                            .cleanable(true),
                    ),
            );
        row = row.child(mask);
        row.child(filters)
    }

    /// Profit metric combo: the scale every figure and the tuner sweep are computed in.
    ///
    /// Three choices, because two of them answer different questions about the SAME money. "Own
    /// quote" lets the unit follow whatever the period holds — which is why a BTC-quoted core
    /// reads in BTC for one range and in USDT for a wider one. "USDT" pins the scale so the two
    /// ranges are finally comparable. "Profit %" leaves money behind entirely.
    ///
    /// Args:
    ///     cx: Analytics view context used for menu actions.
    ///
    /// Returns:
    ///     Metric dropdown element labeled with the exact current unit when known.
    fn metric_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let active_unit = match self.tab {
            Tab::Summary => self.data.unit(),
            Tab::Strategies => self.strategy_data.unit(),
            Tab::Calendar => self.cal_days.unit(),
        };
        // The trigger shows the unit the data ACTUALLY came back in, so a USDT choice that could
        // not be valued reads as the native quote rather than claiming a conversion that did not
        // happen.
        let cur = match self.metric {
            ProfitMetric::Quote => match active_unit {
                Some(moon_core::db::ProfitUnit::Quote(currency)) => currency.ticker().to_string(),
                Some(moon_core::db::ProfitUnit::Percent) | None => {
                    t!("analytics.metric.quote").to_string()
                }
            },
            ProfitMetric::Percent => t!("analytics.metric.pct").to_string(),
        };
        let view = cx.entity();
        let choice = MetricChoice::of(self.metric, self.prefer_usdt);
        let items = crate::panels::radio_items(
            [
                (
                    MetricChoice::Native,
                    "am-quote".into(),
                    t!("analytics.metric.quote").to_string().into(),
                ),
                (
                    MetricChoice::Usdt,
                    "am-usdt".into(),
                    t!("analytics.metric.usdt").to_string().into(),
                ),
                (
                    MetricChoice::Percent,
                    "am-pct".into(),
                    t!("analytics.metric.pct").to_string().into(),
                ),
            ],
            choice,
            crate::panels::RadioMark::Highlight,
            move |app, m| {
                view.update(app, |t, c| t.set_metric_choice(m, c));
            },
        );
        MoonDropdown::new("an-metric")
            .label(cur)
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width_scaled(METRIC_TRIGGER_W)
            .fit_menu_width(120.0, 240.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }

    /// Render the shared exchange-grouped core multi-selector with batch toggles.
    ///
    /// Args:
    ///     cx: Analytics context used to read current cores and wire selection callbacks.
    ///
    /// Returns:
    ///     The configured fixed-trigger dropdown.
    fn core_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let filter_pin = self.core_filter_pin();
        let workspace_pinned = filter_pin.is_some();
        let selected: HashSet<u64> = match filter_pin {
            Some(scope) => scope.selected_core.into_iter().collect(),
            None => self.sel_cores.clone(),
        };
        // Raw DB result (names from `reports.sqlite`, possibly including cores whose server was
        // deleted) — ranked here, on render, against the current config.
        let (cores, venues) = {
            let backend = self.backend.read(cx);
            let db_cores = match filter_pin {
                Some(scope) => self
                    .cores
                    .iter()
                    .filter(|(core, _)| scope.core_ids.contains(core))
                    .cloned()
                    .collect(),
                // Unpinned: narrowed by whatever the Classic viewing preset hides, same as
                // `tabs_bar`'s `presented_cores`; the pinned arm above is untouched.
                None => {
                    let hidden = self.hidden_core_ids();
                    self.cores
                        .iter()
                        .filter(|(core, _)| hidden.is_none_or(|h| !h.contains(core)))
                        .cloned()
                        .collect()
                }
            };
            (
                crate::core_order::CoreOrder::new(&backend.config).from_db(db_cores),
                backend.session.core_venues(),
            )
        };
        // The unpinned selector always uses the generic All label; Overview is a rail state, not
        // a distinct selector row.
        let all_label = t!("report.all_cores").to_string();
        let toggle_view = view.clone();
        let extras =
            crate::controls::core_combo_extras(!workspace_pinned, &view, &self.backend, cx);
        crate::controls::core_combo(
            "an-core",
            &cores,
            &venues,
            &selected,
            crate::controls::CoreAllRowMode::ImplicitOnly,
            all_label,
            |n| t!("report.cores_n", n = n).to_string(),
            180.0,
            extras,
            move |uid, app| {
                toggle_view.update(app, |t, c| t.toggle_core(uid, c));
            },
            move |exchange_cores, app| {
                view.update(app, |t, c| {
                    t.toggle_exchange_cores(exchange_cores, c);
                });
            },
        )
        .disabled(workspace_pinned)
    }

    /// Side combo (All/Long/Short). Analytics keeps it a field of its own; the Report folds the
    /// same choice into its merged scope field.
    fn side_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let cur = crate::panels::side_label(self.side);
        let view = cx.entity();
        let items = crate::panels::radio_items(
            [
                (
                    SideFilter::All,
                    "as-all".into(),
                    t!("report.filter.all").to_string().into(),
                ),
                (
                    SideFilter::Long,
                    "as-long".into(),
                    t!("report.side.long").to_string().into(),
                ),
                (
                    SideFilter::Short,
                    "as-short".into(),
                    t!("report.side.short").to_string().into(),
                ),
            ],
            self.side,
            crate::panels::RadioMark::Highlight,
            move |app, side| {
                view.update(app, |t, c| t.set_side(side, c));
            },
        );
        MoonDropdown::new("an-side")
            .label(cur)
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width_scaled(SIDE_TRIGGER_W)
            .menu_width_scaled(120.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }

    /// Order kind combo (All / Real / Emulated) — as in the Report.
    fn kind_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let cur = match self.emu {
            None => t!("report.kind.all"),
            Some(false) => t!("report.kind.real"),
            Some(true) => t!("report.kind.emu"),
        };
        let view = cx.entity();
        let items = crate::panels::radio_items(
            [
                (
                    None,
                    "ak-all".into(),
                    t!("report.kind.all").to_string().into(),
                ),
                (
                    Some(false),
                    "ak-real".into(),
                    t!("report.kind.real").to_string().into(),
                ),
                (
                    Some(true),
                    "ak-emu".into(),
                    t!("report.kind.emu").to_string().into(),
                ),
            ],
            self.emu,
            crate::panels::RadioMark::Check,
            move |app, k| {
                view.update(app, |t, c| t.set_emu(k, c));
            },
        );
        MoonDropdown::new("an-kind")
            .label(cur)
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width_scaled(KIND_TRIGGER_W)
            .menu_width_scaled(138.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }

    /// One bound of a custom period: a "с"/"по" caption plus the moonui date+time field, which
    /// carries the calendar and the clock drums in ONE popup.
    ///
    /// The caption stays outside the field because the picker replaces its label with the picked
    /// value; without it a filled field would no longer say which edge it is.
    fn date_field(&self, is_to: bool, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let picker = if is_to { &self.cal_to } else { &self.cal_from };
        let lbl = if is_to {
            t!("analytics.period.to_lbl")
        } else {
            t!("analytics.period.from_lbl")
        };
        h_flex()
            .gap_1()
            .items_center()
            // The picker's field clips its label but wraps first, and a wrapped value hides the
            // time half on an invisible second line. Text style inherits, so this reaches it.
            .whitespace_nowrap()
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(moon(p.text_soft))
                    .child(lbl.to_string()),
            )
            .child(
                MoonDateTimePicker::new(if is_to { "an-date-to" } else { "an-date-from" }, picker)
                    .placeholder(t!("analytics.period.any_date").to_string())
                    .cleanable(true)
                    .width(crate::controls::date_range::field_width(cx))
                    .render(),
            )
    }

    /// Return the startup-recovery or background-integrity note as `(title, detail)`.
    ///
    /// Blocked/failed recovery is more actionable than the expected damage verdict and therefore
    /// has priority. A successful recovery remains visible for this process without claiming to
    /// know when every core has completed its independent catch-up.
    ///
    /// Args:
    ///     cx: View context used to schedule the bounded integrity re-poll.
    ///
    /// Returns:
    ///     Localized title/detail pair, or `None` when no notice is required.
    pub(super) fn integrity_note(&mut self, cx: &mut Context<Self>) -> Option<(String, String)> {
        let recovery_note = moon_core::db::report_recovery::status();
        match recovery_note {
            Some(RecoveryNotice::Blocked {
                detail: _,
                snapshot_dir: Some(snapshot),
            }) => {
                return Some((
                    t!("analytics.recovery_blocked").to_string(),
                    t!(
                        "analytics.recovery_blocked_snapshot",
                        path = snapshot.display().to_string()
                    )
                    .to_string(),
                ));
            }
            Some(RecoveryNotice::Blocked {
                detail: _,
                snapshot_dir: None,
            }) => {
                return Some((
                    t!("analytics.recovery_blocked").to_string(),
                    t!("analytics.recovery_blocked_detail").to_string(),
                ));
            }
            Some(RecoveryNotice::Failed { detail: _ }) => {
                return Some((
                    t!("analytics.recovery_failed").to_string(),
                    t!("analytics.recovery_failed_detail").to_string(),
                ));
            }
            Some(RecoveryNotice::Recovered { .. }) | None => {}
        }
        let recovered = match recovery_note {
            Some(RecoveryNotice::Recovered { snapshot_dir }) => Some((
                t!("analytics.recovery_done").to_string(),
                t!(
                    "analytics.recovery_done_detail",
                    path = snapshot_dir.display().to_string()
                )
                .to_string(),
            )),
            _ => None,
        };

        let Some(verdict) = moon_core::db::integrity::status() else {
            // Still running. Re-poll once per armed timer; the check cannot
            // publish before its own startup delay, so the first wait matches
            // that rather than hammering a repaint every few seconds.
            if !self.integrity_poll_armed {
                self.integrity_poll_armed = true;
                let wait = moon_core::db::integrity::poll_hint();
                cx.spawn(async move |this, cx| {
                    let executor = cx.update(|cx| cx.background_executor().clone());
                    executor.timer(wait).await;
                    let _ = cx.update(|cx| {
                        let _ = this.update(cx, |this, cx| {
                            this.integrity_poll_armed = false;
                            cx.notify();
                        });
                    });
                })
                .detach();
            }
            return recovered;
        };
        match verdict {
            Integrity::Damaged(lines) => Some((
                t!("analytics.integrity_damaged").to_string(),
                lines.first().cloned().unwrap_or_default(),
            )),
            Integrity::CheckFailed(msg) => {
                Some((t!("analytics.integrity_unchecked").to_string(), msg.clone()))
            }
            Integrity::Ok | Integrity::NotPresent => recovered,
        }
    }

    /// The strip under the period bar: the "closed trades the core never dated" notice, or
    /// nothing at all.
    ///
    /// Silent unless there is something to say — a database with no undated trades gets no
    /// empty band under its period bar. The count and the money are already scoped by the
    /// window's own filters, so the sentence describes exactly the rows the figures beside it
    /// were computed from, minus the ones that could not be.
    ///
    /// Args:
    ///     p: Active MoonUI palette.
    ///     cx: Analytics view context.
    ///
    /// Returns:
    ///     Notice strip only when a count or read failure exists.
    pub(super) fn notice_strip(
        &self,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let undated = undated_banner_state(
            self.undated_error.as_ref(),
            self.undated.clone(),
            self.undated_expanded,
        );
        let row = h_flex()
            .w_full()
            .px(design::ui_px(cx, 10.0))
            .pb(design::ui_px(cx, 6.0))
            .gap(design::ui_px(cx, 8.0))
            .items_start();
        let row = match undated {
            UndatedBanner::None => return None,
            UndatedBanner::Full(title, detail) => row
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(MoonAlert::warning("an-undated-banner", detail).title(title)),
                )
                .child(
                    MoonButton::new("an-undated-hide")
                        .variant(MoonButtonVariant::Ghost)
                        .size(MoonButtonSize::Micro)
                        .label(t!("analytics.undated_hide").to_string())
                        .on_click(cx.listener(|this, _, _, cx| this.undated_hide(cx)))
                        .render(),
                ),
            UndatedBanner::Collapsed(line) => row
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_muted))
                        .child(line),
                )
                .child(
                    MoonButton::new("an-undated-show")
                        .variant(MoonButtonVariant::Ghost)
                        .size(MoonButtonSize::Micro)
                        .label(t!("analytics.undated_show").to_string())
                        .on_click(cx.listener(|this, _, _, cx| this.undated_show(cx)))
                        .render(),
                ),
            UndatedBanner::Failed(title, detail) => row.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(MoonAlert::error("an-undated-error", detail).title(title)),
            ),
        };
        Some(row)
    }

    /// Collapse the notice back to its one-line form.
    pub(super) fn undated_hide(&mut self, cx: &mut Context<Self>) {
        self.set_undated_expanded(false, cx);
    }

    /// Open the full notice.
    pub(super) fn undated_show(&mut self, cx: &mut Context<Self>) {
        self.set_undated_expanded(true, cx);
    }

    /// Record the notice's open state for as long as this process runs.
    ///
    /// Deliberately NOT written to `layout.toml`: see
    /// [`super::AnalyticsSessionState::undated_expanded`]. Storing it would let a user who
    /// once tidied the warning away never be told again that money is missing from every
    /// figure on the window.
    fn set_undated_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        self.undated_expanded = expanded;
        self.backend.update(cx, |b, _| {
            b.ui_session.analytics.undated_expanded = expanded;
        });
        cx.notify();
    }

    /// Fit every preset's segment item once, at the current locale and display zone.
    ///
    /// `MoonSegmentItem::fit_width` is a pure `cx`-only computation — no window, no paint, no
    /// `cx.notify()` — so calling it here and reusing its output both for the collapse-decision
    /// sum ([`Self::period_bar`]) and for the actual inline render ([`Self::presets_row`]) costs
    /// one fit per preset per frame instead of two, and removes the drift between a hand-rolled
    /// width mirror and the real `MoonSegmentItem` layout.
    ///
    /// Args:
    ///     cx: Application context supplying theme-aware scale and text measurements.
    ///
    /// Returns:
    ///     One fitted, tooltipped item per [`Period::ALL`] entry, in that order.
    fn fitted_preset_items(&self, cx: &App) -> Vec<MoonSegmentItem> {
        Period::ALL
            .into_iter()
            .map(|per| {
                // The tooltip carries the untruncated title, since `fit_width` elides the label
                // and an elided preset is otherwise unreadable with no way to recover it.
                let title = per.title(self.display_zone);
                MoonSegmentItem::new("", title.clone())
                    .fit_width(cx, PRESET_CELL_MIN_W, PRESET_CELL_MAX_W)
                    .tooltip(title)
            })
            .collect()
    }

    /// Build the inline segmented preset row from items [`Self::fitted_preset_items`] already fit.
    ///
    /// Args:
    ///     active: Currently active preset, or a custom range that matches none of them.
    ///     items: This frame's fitted preset items, in [`Period::ALL`] order.
    ///     cx: Analytics view context used to wire click handlers.
    ///
    /// Returns:
    ///     The rendered segmented control.
    fn presets_row(
        &self,
        active: Period,
        items: Vec<MoonSegmentItem>,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        // The segmented control's handler is a plain indexed `Fn`, not a listener type, so the
        // view is reached the documented other way — `cx.entity()` plus `update`.
        let view = cx.entity();
        MoonSegmentedControl::new("an-period-presets")
            .items(
                Period::ALL
                    .into_iter()
                    .zip(items)
                    .map(|(per, item)| item.selected(active == per)),
            )
            .on_click(move |ix, _, window, app| {
                let Some(per) = Period::ALL.get(ix).copied() else {
                    return;
                };
                view.update(app, |this, cx| this.set_period(per, window, cx));
            })
            .render()
    }

    /// Build the collapsed period-preset dropdown — the narrow-window fallback for
    /// [`Self::presets_row`], reusing `MoonDropdown` exactly like this bar's own side/kind/metric
    /// combos rather than a hand-rolled trigger. Its label always names the active preset, so a
    /// collapsed bar never hides which period is selected.
    ///
    /// Args:
    ///     active: Currently active preset, or a custom range that matches none of them.
    ///     cx: Analytics view context used to wire click handlers.
    ///
    /// Returns:
    ///     The rendered dropdown trigger and menu.
    fn presets_dropdown(&self, active: Period, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let items: Vec<MoonMenuItem> = Period::ALL
            .into_iter()
            .map(|per| {
                let view = view.clone();
                let title = per.title(self.display_zone);
                MoonMenuItem::with_key(per.id(), title)
                    .selected(active == per)
                    .on_click(move |_, window, app| {
                        view.update(app, |this, cx| this.set_period(per, window, cx));
                    })
            })
            .collect();
        MoonDropdown::new("an-period-presets-dd")
            .label(active.title(self.display_zone))
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .fit_trigger_width(PRESET_CELL_MIN_W, PRESET_CELL_MAX_W)
            .fit_menu_width(120.0, 220.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }

    /// Render the active tab's period presets, custom date range, and scoped trade count.
    ///
    /// Summary and Tuning retain independent periods, so selection and count both read from the
    /// currently visible tab.
    ///
    /// When the preset row fits `chrome_width` it stays a row — a user with room keeps the
    /// one-click strip. When it does not, the presets collapse into a single dropdown trigger
    /// instead of wrapping onto a second line or clipping, conditional on space rather than
    /// unconditional. The custom-range group never yields — only the presets do, mirroring
    /// `design::ticker_visible`'s "the ticker collapses, the clock does not" priority.
    ///
    /// `chrome_width` is a WINDOW-level value read once at `render()`'s entry
    /// (`crate::window::windowing::responsive_width`), and every width this function compares
    /// against it is a pure font-metric estimate ([`Self::fitted_preset_items`]'s summed
    /// `resolved_width`, the custom-group and counter budgets below) — never an actual measured
    /// layout. That is deliberate: this window
    /// has none of the dock panels' repaint throttles (no `flush_backend_notify`, no `Shell`
    /// observe gate, no per-panel `RenderGate`), so a decision that measured a real layout and then
    /// `cx.notify()`d to re-render at the corrected size would be an unbounded repaint loop with
    /// nothing here to stop it. Estimating instead means this render pass settles the collapse
    /// decision in one pass, with no follow-up notify of its own.
    ///
    /// Args:
    ///     p: Active MoonUI palette.
    ///     chrome_width: Current window width, read once at the render root.
    ///     cx: Analytics view context.
    ///
    /// Returns:
    ///     Period controls and exact scoped trade count.
    pub(super) fn period_bar(
        &self,
        p: MoonPalette,
        chrome_width: f32,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        // Highlight follows the ACTIVE tab's period (Summary/Tuning are independent).
        let active = self.active_period();
        // Keep the divider inside the custom-range group so it cannot wrap onto a line by itself.
        let custom_label = t!("analytics.period.custom_lbl").to_string();
        let custom = design::chrome_section(cx)
            .child(design::chrome_divider(cx, p))
            .child(
                div()
                    .flex_none()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .child(custom_label.clone()),
            )
            .child(self.date_field(false, p, cx))
            .child(self.date_field(true, p, cx));
        // Everything the presets are NOT free to take: the row's own padding, the gap before each
        // of the two neighbouring atoms, the custom-range group (divider + label + two date
        // fields, each with its own localized caption and inner gap, mirroring `chrome_section`'s
        // own internal gap), and the counter's reserved floor.
        let field_w = crate::controls::date_range::field_width(cx);
        let from_lbl = t!("analytics.period.from_lbl").to_string();
        let to_lbl = t!("analytics.period.to_lbl").to_string();
        // Each `date_field` draws its caption at `design::t_body(cx)`, so measure at the same
        // unscaled base rather than a second guessed size.
        let date_captions_w =
            design::ui_text_width(cx, &from_lbl, design::base_text(cx), 400.0, true)
                + design::ui_text_width(cx, &to_lbl, design::base_text(cx), 400.0, true);
        // `date_field`'s own `h_flex().gap_1()` between its caption and picker — GPUI's
        // `rems(0.25)`, at the window's rem size, which this app never overrides from GPUI's
        // default `px(16.)`. One gap per field, not scaled by the Font slider.
        let date_field_gaps_w = f32::from(rems(0.25).to_pixels(px(16.0))) * 2.0;
        let custom_group_w = 1.0
            + design::ui_text_width(cx, &custom_label, 10.5, 400.0, true)
            + field_w * 2.0
            + date_captions_w
            + date_field_gaps_w
            + design::ui_value(cx, design::CHROME_GAP) * 3.0;
        let fixed_w = design::ui_value(cx, 10.0) * 2.0
            + custom_group_w
            + design::ui_value(cx, PERIOD_COUNTER_RESERVED_W)
            + design::ui_value(cx, design::CHROME_GAP) * 2.0;
        let available_for_presets = (chrome_width - fixed_w).max(0.0);
        let fitted_presets = self.fitted_preset_items(cx);
        let presets_row_w: f32 = fitted_presets
            .iter()
            .map(MoonSegmentItem::resolved_width)
            .sum();
        let presets = if presets_row_fits(available_for_presets, presets_row_w) {
            self.presets_row(active, fitted_presets, cx)
                .into_any_element()
        } else {
            self.presets_dropdown(active, cx).into_any_element()
        };
        // Split scopes carry the real count outside the deliberately empty scalar payload.
        let split_orders = match self.tab {
            Tab::Summary => self.data.split(),
            Tab::Strategies => self.strategy_data.split(),
            Tab::Calendar => self.cal_days.split(),
        }
        .map(|totals| totals.orders);
        // Keep read failure distinct from both an empty count and loading.
        let (counter, counter_failed) = if let Some(orders) = split_orders {
            (t!("analytics.trades_count", n = orders).to_string(), false)
        } else {
            match self.tab {
                Tab::Strategies => match &self.strategy_data {
                    ProfitLoadState::Loading => ("…".to_string(), false),
                    ProfitLoadState::Ready { data, .. } => (
                        t!("analytics.trades_count", n = data.trades).to_string(),
                        false,
                    ),
                    ProfitLoadState::Split(totals) => (
                        t!("analytics.trades_count", n = totals.orders).to_string(),
                        false,
                    ),
                    ProfitLoadState::NotReady => (String::new(), false),
                    ProfitLoadState::Failed(_) => {
                        (t!("common.db_read_failed_short").to_string(), true)
                    }
                },
                _ => match &self.data {
                    ProfitLoadState::Loading => ("…".to_string(), false),
                    ProfitLoadState::Ready { data, .. } => (
                        t!("analytics.trades_count", n = data.cur.n).to_string(),
                        false,
                    ),
                    ProfitLoadState::Split(totals) => (
                        t!("analytics.trades_count", n = totals.orders).to_string(),
                        false,
                    ),
                    ProfitLoadState::NotReady => (String::new(), false),
                    ProfitLoadState::Failed(_) => {
                        (t!("common.db_read_failed_short").to_string(), true)
                    }
                },
            }
        };
        h_flex()
            .flex_none()
            .w_full()
            .min_h(design::fit_h_px(cx, 34.0, 13.0, 10.5))
            // Deliberately NOT `.flex_wrap()`: a narrow window collapses the presets into
            // `presets_dropdown` instead, so this row never needs a second line — the settled
            // requirement is a dropdown "only for those who lack the room", never a wrap.
            .px(design::ui_px(cx, 10.0))
            .py(design::ui_px(cx, 8.0))
            .gap_x(design::ui_px(cx, design::CHROME_GAP))
            .items_center()
            // The bottom rule separates the controls from the muted notice below.
            .border_b_1()
            .border_color(moon(p.border))
            .child(presets)
            .child(custom)
            .child(
                div()
                    .ml_auto()
                    .min_w_0()
                    .truncate()
                    .text_size(design::t_body(cx))
                    .text_color(moon(if counter_failed {
                        p.orange
                    } else {
                        p.text_muted
                    }))
                    .child(counter),
            )
    }
}
