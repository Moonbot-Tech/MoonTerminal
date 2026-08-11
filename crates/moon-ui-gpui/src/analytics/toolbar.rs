//! Top chrome of the Analytics window: tab bar, filter combos (cores / side /
//! trade kind), the "from"–"to" date fields, the replica integrity note and the
//! period bar with presets. Controls only — the state lives in `mod.rs`.

use std::collections::HashSet;

use gpui::*;
use moon_ui::{
    MoonAlert, MoonButton, MoonButtonIconSlot, MoonButtonSize, MoonButtonVariant,
    MoonDateTimePicker, MoonDropdown, MoonMenuSize, MoonPalette, MoonSegmentItem,
    MoonSegmentedControl, h_flex,
};
use rust_i18n::t;

use super::{AnalyticsView, Period, ProfitLoadState, Tab};
use crate::design;
use crate::design::moon;
use moon_core::db::analytics::UndatedCloses;
use moon_core::db::integrity::Integrity;
use moon_core::db::report_recovery::RecoveryNotice;
use moon_core::db::{ProfitMetric, ReadFail, SideFilter};

/// Unscaled width of the Analytics side-selector trigger.
const SIDE_TRIGGER_W: f32 = 69.0;
/// Unscaled width of the Analytics trade-kind-selector trigger.
const KIND_TRIGGER_W: f32 = 102.0;
/// Unscaled width of the Analytics profit-metric-selector trigger.
const METRIC_TRIGGER_W: f32 = 116.0;
/// Unscaled horizontal spacing between neighboring toolbar controls.
const TOOLBAR_GAP: f32 = 6.0;

/// Base floor for a period preset's fitted cell width.
const PRESET_CELL_MIN_W: f32 = 44.0;

/// Base ceiling for a period preset's fitted cell width.
///
/// A long localized label cannot stretch the strip across the window; MoonUI truncates the visible
/// label, and the item's tooltip still carries the whole of it.
const PRESET_CELL_MAX_W: f32 = 104.0;

#[cfg(test)]
mod tests;

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
///     selected: Retained explicit core ids; only an empty set represents the Classic All row.
///     workspace: Concrete Auto-workspace ids, when a live singleton owner pins Analytics.
///
/// Returns:
///     Workspace ids when pinned, every retained explicit id in Classic, or an empty unfiltered
///     list only for Classic All. An empty Auto group uses core id zero, which cannot be assigned to
///     a reconciled server, so it stays an explicit no-match query instead of broadening globally.
pub(super) fn analytics_core_filter_ids(
    selected: &HashSet<u64>,
    workspace: Option<&[u64]>,
) -> Vec<u64> {
    match workspace {
        Some([]) => vec![0],
        Some(cores) => cores.to_vec(),
        None => selected.iter().copied().collect(),
    }
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
        let workspace_pinned = self.workspace_scope.is_some();
        let presented_selection: HashSet<u64> = match &self.workspace_scope {
            Some(scope) => scope.selected_core.into_iter().collect(),
            None => self.sel_cores.clone(),
        };
        let presented_cores: Vec<(u64, String)> = match &self.workspace_scope {
            Some(scope) => self
                .cores
                .iter()
                .filter(|(core, _)| scope.core_ids.contains(core))
                .cloned()
                .collect(),
            None => self.cores.clone(),
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
                                this.request_report_refresh(true, cx);
                            } else {
                                // Tab-entry catch-up uses the report gate so it cannot overlap
                                // an automatic full-period scan already in flight.
                                if t == Tab::Strategies {
                                    this.request_axis_if_stale(this.strat_mode, cx);
                                }
                                if t == Tab::Calendar
                                    && (this.cal_days.data().is_none() || this.cal_dirty)
                                {
                                    this.request_report_refresh(true, cx);
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
                    .children(
                        sole_core_name(&presented_cores, &presented_selection)
                            .map(crate::display_text::flatten_lines),
                    ),
            )
            .child(selectors);
        row.child(filters)
    }

    /// Profit metric combo (quote money / Profit %): switches every figure and the tuner sweep
    /// between absolute money and the report's `Profit` column (profit ÷ spent).
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
        let items = crate::panels::radio_items(
            [
                (
                    ProfitMetric::Quote,
                    "am-quote".into(),
                    t!("analytics.metric.quote").to_string().into(),
                ),
                (
                    ProfitMetric::Percent,
                    "am-pct".into(),
                    t!("analytics.metric.pct").to_string().into(),
                ),
            ],
            self.metric,
            crate::panels::RadioMark::Highlight,
            move |app, m| {
                view.update(app, |t, c| t.set_metric(m, c));
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
        let workspace_pinned = self.workspace_scope.is_some();
        let selected: HashSet<u64> = match &self.workspace_scope {
            Some(scope) => scope.selected_core.into_iter().collect(),
            None => self.sel_cores.clone(),
        };
        // Raw DB result (names from `reports.sqlite`, possibly including cores whose server was
        // deleted) — ranked here, on render, against the current config.
        let (cores, venues) = {
            let backend = self.backend.read(cx);
            let db_cores = match &self.workspace_scope {
                Some(scope) => self
                    .cores
                    .iter()
                    .filter(|(core, _)| scope.core_ids.contains(core))
                    .cloned()
                    .collect(),
                None => self.cores.clone(),
            };
            (
                crate::core_order::CoreOrder::new(&backend.config).from_db(db_cores),
                backend.session.core_venues(),
            )
        };
        let all_label = if self
            .workspace_scope
            .as_ref()
            .is_some_and(|scope| scope.selected_core.is_none())
        {
            t!("workspace.overview").to_string()
        } else {
            t!("report.all_cores").to_string()
        };
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

    /// Render the active tab's period presets, custom date range, and scoped trade count.
    ///
    /// Summary and Tuning retain independent periods, so selection and count both read from the
    /// currently visible tab.
    ///
    /// Presets and the custom range remain atomic groups so a narrow host wraps between controls
    /// instead of clipping one away.
    ///
    /// Args:
    ///     p: Active MoonUI palette.
    ///     cx: Analytics view context.
    ///
    /// Returns:
    ///     Period controls and exact scoped trade count.
    pub(super) fn period_bar(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        // Highlight follows the ACTIVE tab's period (Summary/Tuning are independent).
        let active = self.active_period();
        // The segmented control's handler is a plain indexed `Fn`, not a listener type, so the
        // view is reached the documented other way — `cx.entity()` plus `update`.
        let view = cx.entity();
        let presets = MoonSegmentedControl::new("an-period-presets")
            .items(Period::ALL.map(|per| {
                // The tooltip carries the untruncated title, since `fit_width` elides the label
                // and an elided preset is otherwise unreadable with no way to recover it.
                let title = per.title(self.display_zone);
                MoonSegmentItem::new("", title.clone())
                    .fit_width(cx, PRESET_CELL_MIN_W, PRESET_CELL_MAX_W)
                    .tooltip(title)
                    .selected(active == per)
            }))
            .on_click(move |ix, _, window, app| {
                let Some(per) = Period::ALL.get(ix).copied() else {
                    return;
                };
                view.update(app, |this, cx| this.set_period(per, window, cx));
            })
            .render();
        // Keep the divider inside the custom-range group so it cannot wrap onto a line by itself.
        let custom = design::chrome_section(cx)
            .child(design::chrome_divider(cx, p))
            .child(
                div()
                    .flex_none()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .child(t!("analytics.period.custom_lbl").to_string()),
            )
            .child(self.date_field(false, p, cx))
            .child(self.date_field(true, p, cx));
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
            // A minimum rather than fixed height leaves room for a wrapped second line.
            .min_h(design::fit_h_px(cx, 34.0, 13.0, 10.5))
            .flex_wrap()
            .px(design::ui_px(cx, 10.0))
            .py(design::ui_px(cx, 8.0))
            .gap_x(design::ui_px(cx, design::CHROME_GAP))
            .gap_y(design::ui_px(cx, 4.0))
            .items_center()
            // The bottom rule separates the controls from the muted notice below.
            .border_b_1()
            .border_color(moon(p.border))
            .child(presets)
            .child(custom)
            .child(
                // A margin avoids adding a spacer that could take its own line when the row wraps.
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
