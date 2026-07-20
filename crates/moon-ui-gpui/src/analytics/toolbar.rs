//! Top chrome of the Analytics window: tab bar, filter combos (cores / side /
//! trade kind), the "from"–"to" date fields, the replica integrity note and the
//! period bar with presets. Controls only — the state lives in `mod.rs`.

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCalendar, MoonDropdown, MoonMenuSize,
    MoonPalette, MoonPopover, MoonPopoverPlacement, h_flex,
};
use rust_i18n::t;

use super::tuner;
use super::{AnalyticsView, Period, Tab};
use crate::design;
use crate::design::moon;
use crate::load_state::LoadState;
use moon_core::db::SideFilter;
use moon_core::db::integrity::Integrity;

impl AnalyticsView {
    /// Tab bar (same shape as the Settings tab bar).
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
                            // Each tab remembers its OWN time window: re-sync the
                            // period bar and the "from"/"to" fields to the active tab.
                            this.sync_period_pickers(window, cx);
                            // The new tab's time window differs from the one `data`
                            // was built for → reload, or the strategy list and the
                            // summary would show another tab's period. reload() also
                            // pulls the active tab's secondary data (tuner/profile).
                            let period_changed = matches!(t, Tab::Summary | Tab::Strategies)
                                && this.active_period() != this.data_period;
                            if period_changed {
                                this.reload(cx);
                            } else {
                                if t == Tab::Strategies
                                    && this.strat_mode == tuner::StratMode::Filters
                                    && this.tuner.needs_reload()
                                {
                                    this.reload_tuner(cx);
                                    this.reload_hist(cx);
                                }
                                if t == Tab::Strategies
                                    && this.strat_mode == tuner::StratMode::Time
                                    && (this.time_profiles.is_none() || this.time_dirty)
                                {
                                    this.reload_time(cx);
                                }
                                if t == Tab::Calendar && (this.cal_days.is_none() || this.cal_dirty)
                                {
                                    this.reload_calendar(cx);
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
    }

    /// Cores combo — multi-select (the shared widget, as in Orders/Report).
    fn core_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        crate::controls::core_combo(
            cx,
            "an-core",
            &self.cores,
            &self.sel_cores,
            t!("report.all_cores").to_string(),
            |n| t!("report.cores_n", n = n).to_string(),
            180.0,
            move |uid, app| {
                view.update(app, |t, c| t.toggle_core(uid, c));
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
            .label(format!("{cur} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(design::font_w(cx, 69.0))
            .menu_width(design::font_w(cx, 120.0))
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
            .label(format!("{cur} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(design::font_w(cx, 102.0))
            .menu_width(design::font_w(cx, 138.0))
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
            // The mirror of MoonCalendar's private layout lives in `calendar_outer_width`.
            .width(design::popover_outer_width(
                cx,
                design::calendar_outer_width(cx),
            ))
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

    /// Render period presets and the closed-trade or read-failure counter.
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
        let (counter, counter_failed) = match &self.data {
            LoadState::Loading { .. } => ("…".to_string(), false),
            LoadState::Ready(d) => (t!("analytics.trades_count", n = d.cur.n).to_string(), false),
            LoadState::NotReady => (String::new(), false),
            LoadState::Failed(_) => (t!("common.db_read_failed_short").to_string(), true),
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
