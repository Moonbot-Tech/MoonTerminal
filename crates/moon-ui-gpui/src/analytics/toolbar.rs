//! Top chrome of the Analytics window: tab bar, filter combos (cores / side /
//! trade kind), the "from"–"to" date fields, the replica integrity note and the
//! period bar with presets. Controls only — the state lives in `mod.rs`.

use gpui::*;
use moon_ui::{
    MoonAlert, MoonButton, MoonButtonSize, MoonButtonVariant, MoonCalendar, MoonDropdown,
    MoonMenuSize, MoonPalette, MoonPopover, MoonPopoverPlacement, h_flex,
};
use rust_i18n::t;

use super::{AnalyticsView, Period, Tab};
use crate::design;
use crate::design::moon;
use crate::load_state::LoadState;
use moon_core::db::analytics::UndatedCloses;
use moon_core::db::integrity::Integrity;
use moon_core::db::{ProfitMetric, ReadFail, SideFilter};

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

/// Decide what the strip shows, given only the read outcome and whether the user opened it.
///
/// A free function rather than a method so the decision can be exercised directly: it is the
/// only thing standing between "money is missing from these figures" and silence.
///
/// A read FAILURE outranks everything, collapsing included — a query that did not run cannot
/// be summarised as a count, and hiding it behind a one-liner would let a broken replica read
/// as a small tidy footnote.
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
        return UndatedBanner::Collapsed(t!("analytics.undated_collapsed", n = u.n).to_string());
    }
    UndatedBanner::Full(
        t!("analytics.undated_title").to_string(),
        t!(
            "analytics.undated_detail",
            n = u.n,
            pnl = format!("{:+.2}", u.profit)
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
        let mut row = h_flex()
            .flex_none()
            .w_full()
            .h(design::fit_h_px(cx, 34.0, 13.0, 10.5))
            .gap(design::ui_px(cx, 6.0))
            .px(design::ui_px(cx, 8.0))
            .items_center()
            .bg(moon(p.shell_high))
            .border_b_1()
            .border_color(moon(p.border));
        for t in Tab::ALL {
            let on = self.tab == t;
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
                    .width(112.0)
                    .selected(on)
                    .label(t.title())
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
        // Filters — pinned to the right (same controls as in Orders/Report).
        row.child(div().flex_1())
            .child(self.core_combo(cx))
            .child(self.side_combo(cx))
            .child(self.kind_combo(cx))
            .child(self.metric_combo(cx))
    }

    /// Profit metric combo (USDT / Profit %): switches every figure and the tuner sweep
    /// between absolute money and the report's `Profit` column (profit ÷ spent).
    fn metric_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let cur = match self.metric {
            ProfitMetric::Usdt => t!("analytics.metric.usdt"),
            ProfitMetric::Percent => t!("analytics.metric.pct"),
        };
        let view = cx.entity();
        let items = crate::panels::radio_items(
            [
                (
                    ProfitMetric::Usdt,
                    "am-usd".into(),
                    t!("analytics.metric.usdt").to_string().into(),
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
            .trigger_width_scaled(78.0)
            .menu_width_scaled(120.0)
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
        // Raw DB result (names from `reports.sqlite`, possibly including cores whose server was
        // deleted) — ranked here, on render, against the current config.
        let (cores, exchange_names) = {
            let backend = self.backend.read(cx);
            (
                crate::core_order::CoreOrder::new(&backend.config).from_db(self.cores.clone()),
                backend.session.market_source().core_exchange_names(),
            )
        };
        let toggle_view = view.clone();
        crate::controls::core_combo(
            "an-core",
            &cores,
            &exchange_names,
            &self.sel_cores,
            t!("report.all_cores").to_string(),
            |n| t!("report.cores_n", n = n).to_string(),
            180.0,
            move |uid, app| {
                toggle_view.update(app, |t, c| t.toggle_core(uid, c));
            },
            move |exchange_cores, app| {
                view.update(app, |t, c| {
                    t.toggle_exchange_cores(exchange_cores, c);
                });
            },
        )
    }

    /// Side combo (All/Long/Short) — as in the Report.
    fn side_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let cur = match self.side {
            SideFilter::All => t!("report.filter.all").to_string(),
            SideFilter::Long => t!("report.side.long").to_string(),
            SideFilter::Short => t!("report.side.short").to_string(),
        };
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
            .trigger_width_scaled(69.0)
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
            .trigger_width_scaled(102.0)
            .menu_width_scaled(138.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }

    /// One bound of a custom period: a "from/to dd.mm.yy" button plus a popover
    /// holding the moonui calendar (the ready MoonCalendar; MoonDatePicker won't
    /// shrink to Micro chip height — Sizable is not in the moon_ui facade).
    fn date_field(&self, is_to: bool, _p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let (cal, open) = if is_to {
            (&self.cal_to, self.cal_to_open)
        } else {
            (&self.cal_from, self.cal_from_open)
        };
        let date_txt = cal
            .read(cx)
            .date()
            .format("%d.%m.%y")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "—".to_string());
        let lbl = if is_to {
            t!("analytics.period.to_lbl")
        } else {
            t!("analytics.period.from_lbl")
        };
        let set = cal.read(cx).date().is_some();
        let custom_on = matches!(self.active_period(), Period::Custom(..)) && set;
        let view = cx.entity();
        MoonPopover::new(if is_to { "an-date-to" } else { "an-date-from" })
            .placement(MoonPopoverPlacement::BottomStart)
            .fit_content()
            .open(open)
            .on_open_change(move |o, _, app| {
                view.update(app, |t, cx| {
                    if is_to {
                        t.cal_to_open = o;
                    } else {
                        t.cal_from_open = o;
                    }
                    cx.notify();
                });
            })
            .trigger(
                MoonButton::new(if is_to {
                    "an-date-to-btn"
                } else {
                    "an-date-from-btn"
                })
                .variant(if custom_on {
                    MoonButtonVariant::Amber
                } else {
                    MoonButtonVariant::Soft
                })
                .size(MoonButtonSize::Micro)
                .selected(custom_on)
                .label(format!("{lbl} {date_txt}"))
                .render(),
            )
            .content(MoonCalendar::new(cal))
    }

    /// Return the background integrity warning as `(title, detail)` when needed.
    ///
    /// Polls rather than subscribing, with at most one delayed retry timer armed
    /// while the check is running. Successful and absent-replica verdicts render
    /// no warning.
    pub(super) fn integrity_note(&mut self, cx: &mut Context<Self>) -> Option<(String, String)> {
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
            return None;
        };
        match verdict {
            Integrity::Damaged(lines) => Some((
                t!("analytics.integrity_damaged").to_string(),
                lines.first().cloned().unwrap_or_default(),
            )),
            Integrity::CheckFailed(msg) => {
                Some((t!("analytics.integrity_unchecked").to_string(), msg.clone()))
            }
            Integrity::Ok | Integrity::NotPresent => None,
        }
    }

    /// The strip under the period bar: the "closed trades the core never dated" notice, or
    /// nothing at all.
    ///
    /// Silent unless there is something to say — a database with no undated trades gets no
    /// empty band under its period bar. The count and the money are already scoped by the
    /// window's own filters, so the sentence describes exactly the rows the figures beside it
    /// were computed from, minus the ones that could not be.
    pub(super) fn notice_strip(
        &self,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let undated = undated_banner_state(
            self.undated_error.as_ref(),
            self.undated,
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
    pub(super) fn period_bar(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let mut seg = h_flex().gap(design::ui_px(cx, 4.0)).items_center();
        // Highlight follows the ACTIVE tab's period (Summary/Tuning are independent).
        let active = self.active_period();
        for per in Period::ALL {
            let on = active == per;
            seg = seg.child(
                MoonButton::new(per.id())
                    .variant(if on {
                        MoonButtonVariant::Amber
                    } else {
                        MoonButtonVariant::Soft
                    })
                    .size(MoonButtonSize::Micro)
                    .selected(on)
                    .label(per.title())
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.set_period(per, window, cx)),
                    )
                    .render(),
            );
        }
        // Custom range: two "from"/"to" popover fields backed by MoonCalendar.
        seg = seg
            .child(self.date_field(false, p, cx))
            .child(self.date_field(true, p, cx));
        // Keep read failure distinct from both an empty count and loading.
        let (counter, counter_failed) = match self.tab {
            Tab::Strategies => match &self.strategy_data {
                LoadState::Loading { .. } => ("…".to_string(), false),
                LoadState::Ready(data) => (
                    t!("analytics.trades_count", n = data.trades).to_string(),
                    false,
                ),
                LoadState::NotReady => (String::new(), false),
                LoadState::Failed(_) => (t!("common.db_read_failed_short").to_string(), true),
            },
            _ => match &self.data {
                LoadState::Loading { .. } => ("…".to_string(), false),
                LoadState::Ready(data) => (
                    t!("analytics.trades_count", n = data.cur.n).to_string(),
                    false,
                ),
                LoadState::NotReady => (String::new(), false),
                LoadState::Failed(_) => (t!("common.db_read_failed_short").to_string(), true),
            },
        };
        h_flex()
            .flex_none()
            .w_full()
            .px(design::ui_px(cx, 10.0))
            .py(design::ui_px(cx, 8.0))
            .gap(design::ui_px(cx, 8.0))
            .items_center()
            .child(seg)
            .child(div().flex_1())
            .child(
                div()
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
