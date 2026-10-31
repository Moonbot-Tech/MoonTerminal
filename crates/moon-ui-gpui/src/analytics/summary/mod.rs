//! "Summary" tab of the "Analytics" window: KPI cards compared against the
//! previous period, charts (cumulative profit + daily bars), the top 5
//! best/worst trades and automatic "insights". Layout follows the
//! analytics-mock artifact.

use gpui::*;
use moon_ui::MoonPalette;
use rust_i18n::t;

use super::AnalyticsView;
mod charts;
pub(super) use charts::PopupHover;
/// Snapshot-owned chart derivations prepared before Summary rendering.
pub(in crate::analytics) mod derived;
/// Pure view-model of the rebuilt Summary blocks; rendered from the later batches on.
pub(in crate::analytics) mod model;
/// Display-only Summary choices: chart lines, trade side, ranking filter and fold.
pub(in crate::analytics) mod state;
use crate::design;
use crate::load_state::Note;
use moon_core::util::fmt::{self, compact};
pub(in crate::analytics) use state::SummaryUi;
/// The per-core profit ranking card.
pub(in crate::analytics) mod cores;
/// The KPI band of the rebuilt Summary; assembled into the tab in batch B9.
pub(in crate::analytics) mod kpi;
/// Debug selectors shared by the Summary blocks and their layout probe.
pub(in crate::analytics) mod layout;
/// The combined profit chart card.
pub(in crate::analytics) mod profit_chart;
/// The right column: insights and the best/worst trades table.
pub(in crate::analytics) mod side;
/// Loading, empty, error and catch-up faces and the page frame.
pub(in crate::analytics) mod states;

/// Format a compact signed value such as `+341.2`, `-40.1`, or `0` — WITHOUT any unit.
///
/// For dimensionless figures like profit factor. Rounding happens before sign selection, so a
/// value that rounds to zero has no minus sign. Returns an em dash when the input or rounded
/// result is non-finite.
pub(in crate::analytics) fn fmt_signed_plain(v: f64) -> String {
    let Some(v) = fmt::round_to(v, 2) else {
        return "—".to_string();
    };
    if v > 0.0 {
        format!("+{}", compact(v, 2))
    } else {
        compact(v, 2)
    }
}

/// Format a compact signed PROFIT, carrying the active metric's unit: "%" in percent mode
/// (the report `Profit` column), nothing in quote-money mode. Every profit figure on the window goes
/// through here, so the unit follows the toolbar switch everywhere at once. The em dash for a
/// non-finite value is never given a unit.
///
/// Args:
///     v: Profit value in the active comparable unit.
///
/// Returns:
///     Signed compact value with `%` only in Percent mode, or an em dash.
pub(super) fn fmt_signed(v: f64) -> String {
    let s = fmt_signed_plain(v);
    if s == "—" {
        s
    } else {
        format!("{}{}", s, super::pnl_suffix())
    }
}

/// Signed profit carrying the FULL unit for prose (the insights sentences): "+15.34%" in percent
/// mode, "+15.34 USDC" in a USDC scope. [`fmt_signed`] leaves a money figure unitless because its
/// surrounding label supplies the exact quote ticker; a free-standing sentence has no such label,
/// so the ticker rides along here. A non-finite value stays a bare em dash.
///
/// Args:
///     v: Profit value in the active comparable unit.
///
/// Returns:
///     Signed compact value with exact ticker or `%`, or an em dash.
pub(super) fn fmt_signed_unit(v: f64) -> String {
    let s = fmt_signed(v);
    if s == "—" || super::pnl_is_pct() {
        s
    } else {
        let unit = super::pnl_unit_label();
        if unit.is_empty() {
            s
        } else {
            format!("{s} {unit}")
        }
    }
}

/// Return the terminal colour for the same rounded sign used by [`fmt_signed`].
///
/// Positive values are green, negative values are orange, and zero or a non-finite rounding result
/// is muted. This keeps the colour consistent with the displayed text or em dash.
pub(super) fn sign_color(p: MoonPalette, v: f64) -> u32 {
    match fmt::round_to(v, 2) {
        Some(v) if v > 0.0 => p.green,
        Some(v) if v < 0.0 => p.orange,
        _ => p.text_muted,
    }
}

impl AnalyticsView {
    /// Render the Summary face its load state calls for: the static skeleton, the empty period,
    /// the read failure, the shared note, or the data page.
    ///
    /// Builds models and wires callbacks only; it schedules nothing (no notify, spawn or timer).
    ///
    /// Args:
    ///     derived: Prepared chart data or the exact placeholder from the readiness fold.
    ///     p: Active MoonUI palette.
    ///     chrome_width: Window's responsive width; picks the wide, two- or one-column layout.
    ///     cx: Analytics view context.
    ///
    /// Returns:
    ///     The Summary face inside the page scroll.
    pub(super) fn summary_tab(
        &self,
        derived: Result<std::rc::Rc<derived::SummaryDerived>, Note>,
        p: MoonPalette,
        chrome_width: f32,
        cx: &Context<Self>,
    ) -> AnyElement {
        let scale = design::ui_value(cx, 1.0);
        let cols = kpi::kpi_cols(chrome_width - 20.0 * scale, scale);
        let marker = || {
            self.summary_scope_marker().map(|m| {
                // `line`, not `facts`: the caption is its own flex child.
                let line = m.line();
                let tip = m.tooltip(std::slice::from_ref(&line));
                (line, tip)
            })
        };
        let face = states::face_of(&derived);
        let body: AnyElement = match (face, derived) {
            (states::Face::Data, Ok(derived)) => {
                self.summary_data_page(&derived, cols, marker(), p, cx)
            }
            (states::Face::Skeleton, _) => states::skeleton_el(cols, p, cx),
            (states::Face::Empty, _) => states::empty_el(
                states::period_days(self.active_period().range(self.bound_zone())),
                self.summary_filters_line(),
                // An empty result under a scope the viewing preset or the Auto rail narrowed must
                // still say so, or every core hidden reads as "nothing happened".
                self.summary_scope_marker()
                    .filter(|m| m.hides_anything())
                    .map(|m| {
                        let line = m.line();
                        let tip = m.tooltip(std::slice::from_ref(&line));
                        (line, tip)
                    }),
                &self.state_acts(cx),
                p,
                cx,
            ),
            (
                states::Face::Error,
                Err(Note::Failed {
                    kind,
                    path,
                    operation,
                    code,
                    ..
                }),
            ) => states::error_el(kind, path, operation, code, &self.state_acts(cx), p, cx),
            (_, Err(note)) => super::note_el("an-summary-note", note, 18.0, p, cx),
            (_, Ok(_)) => unreachable!("face_of maps Ok to Data"),
        };
        let page = if face == states::Face::Data {
            body
        } else {
            div()
                .w_full()
                .p(design::ui_px(cx, 10.0))
                .child(body)
                .into_any_element()
        };
        // The whole page scrolls as one.
        div()
            .id("an-sum-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(page)
            .into_any_element()
    }

    /// The data page: banner, KPI band, chart beside (or above) the side column, cores card.
    fn summary_data_page(
        &self,
        derived: &std::rc::Rc<derived::SummaryDerived>,
        cols: kpi::KpiCols,
        marker: Option<(String, String)>,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let data = derived.data().clone();
        debug_assert!(
            self.data
                .data()
                .is_some_and(|current| std::sync::Arc::ptr_eq(current, &data))
        );
        let zone = self.bound_zone();
        let ui = &self.summary_ui;
        let banner = super::refresh::summary_catchup_banner(
            self.summary_catching_up
                && self.cached_refreshing[super::Tab::Summary.snapshot_index()].is_none(),
            self.data.data().is_some(),
        )
        .then(|| {
            states::catchup_banner_el(
                self.data_as_of
                    .map_or_else(|| "—".to_string(), |t| states::as_of_text(t, zone)),
            )
        });
        let rows = model::core_rows(&data.core_days, ui.core_filter, ui.unfolded);
        let conc = model::Concentration::of(&data.core_days);
        let blocks = states::DataBlocks {
            banner,
            kpi: kpi::kpi_band_el(
                &model::KpiBand::new(&data, derived.pts.clone(), self.compare_prev),
                cols,
                p,
                cx,
            ),
            chart: profit_chart::profit_chart_el(
                &profit_chart::ProfitChartModel::new(derived, ui, zone, marker, p),
                self.hover_profit_bucket,
                &self.profit_chart_acts(cx),
                p,
                cx,
            ),
            insights: side::insights_el(
                &model::insights(&data, zone),
                side::period_caption(&data),
                p,
                cx,
            ),
            trades: side::trades_el(
                side::side_rows(&data, ui.trades),
                ui.trades,
                zone,
                &self.trades_acts(cx),
                p,
                cx,
            ),
            cores: cores::cores_el(
                &data.core_days,
                &rows,
                &model::DivergingScale::of(&data.core_days),
                conc.as_ref(),
                ui,
                &derived.colors,
                &self.cores_acts(cx),
                p,
                cx,
            ),
        };
        states::data_page_el(cols, blocks, cx).into_any_element()
    }

    /// The filters that narrowed an empty read, quoted for the empty face; empty when none did.
    fn summary_filters_line(&self) -> String {
        let (open, close) = if matches!(rust_i18n::locale().as_ref(), "ru" | "uk") {
            ("«", "»")
        } else {
            ("“", "”")
        };
        let mut parts = Vec::new();
        if self.side != moon_core::db::SideFilter::All {
            parts.push(
                t!(
                    "analytics.filter.side_v",
                    v = crate::panels::side_label(self.side)
                )
                .to_string(),
            );
        }
        if let Some(emu) = self.emu {
            let v = if emu {
                t!("report.kind.emu")
            } else {
                t!("report.kind.real")
            };
            parts.push(t!("analytics.filter.kind_v", v = v).to_string());
        }
        if !self.strategy_mask.is_empty() {
            parts.push(self.strategy_mask.clone());
        }
        parts
            .iter()
            .map(|p| format!("{open}{p}{close}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Callbacks of the empty and error faces.
    fn state_acts(&self, cx: &Context<Self>) -> states::StateActs {
        let view = cx.entity().downgrade();
        let (v1, v2) = (view.clone(), view.clone());
        states::StateActs {
            on_show_year: std::rc::Rc::new(move |window, app| {
                let _ = view.update(app, |this, cx| {
                    this.set_period(super::Period::Year, window, cx);
                });
            }),
            on_reset: std::rc::Rc::new(move |window, app| {
                let _ = v1.update(app, |this, cx| this.reset_summary_filters(window, cx));
            }),
            on_retry: std::rc::Rc::new(move |_, app| {
                let _ = v2.update(app, |this, cx| {
                    this.request_report_refresh(super::RefreshUrgency::User, true, cx);
                });
            }),
        }
    }

    /// Callbacks of the trades card: the best/worst switch.
    fn trades_acts(&self, cx: &Context<Self>) -> side::TradesActs {
        let view = cx.entity().downgrade();
        side::TradesActs {
            on_side: std::rc::Rc::new(move |side, _, app| {
                let _ = view.update(app, |this, cx| {
                    if this.summary_ui.trades != side {
                        this.summary_ui.trades = side;
                        cx.notify();
                    }
                });
            }),
        }
    }

    /// Callbacks of the cores card: pick a core, filter the list, fold or unfold it.
    fn cores_acts(&self, cx: &Context<Self>) -> cores::CoresActs {
        let view = cx.entity().downgrade();
        let (v1, v2) = (view.clone(), view.clone());
        cores::CoresActs {
            on_pick: std::rc::Rc::new(move |uid, _, app| {
                let _ = view.update(app, |this, cx| {
                    this.summary_ui.toggle_pick(uid);
                    cx.notify();
                });
            }),
            on_filter: std::rc::Rc::new(move |filter, _, app| {
                let _ = v1.update(app, |this, cx| {
                    if this.summary_ui.core_filter != filter {
                        this.summary_ui.core_filter = filter;
                        cx.notify();
                    }
                });
            }),
            on_fold: std::rc::Rc::new(move |unfolded, _, app| {
                let _ = v2.update(app, |this, cx| {
                    if this.summary_ui.unfolded != unfolded {
                        this.summary_ui.unfolded = unfolded;
                        // A display lens that persists: the fold is chosen once and expected
                        // back after a restart.
                        this.backend.update(cx, |b, _| {
                            b.layout.analytics_cores_show_all = unfolded;
                            b.layout_dirty = true;
                        });
                        cx.notify();
                    }
                });
            }),
        }
    }
}

impl AnalyticsView {
    /// Callbacks of the profit card, routed to the view's line mode and hover fences.
    fn profit_chart_acts(&self, cx: &Context<Self>) -> profit_chart::ChartActs {
        let view = cx.entity().downgrade();
        let (v1, v2, v3) = (view.clone(), view.clone(), view.clone());
        profit_chart::ChartActs {
            on_lines: std::rc::Rc::new(move |lines, _, app| {
                let _ = view.update(app, |this, cx| {
                    if this.summary_ui.lines != lines {
                        this.summary_ui.lines = lines;
                        cx.notify();
                    }
                });
            }),
            on_area: std::rc::Rc::new(move |hovered, _, app| {
                let _ = v1.update(app, |this, cx| {
                    if hovered {
                        this.summary_popup_hover.profit_pointer_entered();
                    } else if let Some(key) = this.summary_popup_hover.profit_pointer_left() {
                        charts::chart_hover(this, key, false, false, cx);
                    }
                });
            }),
            on_bucket: std::rc::Rc::new(move |bucket, _, app| {
                let _ = v2.update(app, |this, cx| match bucket {
                    Some(bi) => {
                        if this.summary_popup_hover.profit_move_should_enter(bi) {
                            charts::chart_hover(
                                this,
                                charts::PopupKey::Profit(bi),
                                true,
                                false,
                                cx,
                            );
                        }
                    }
                    None => {
                        if let Some(key) = this.summary_popup_hover.profit_pointer_left() {
                            charts::chart_hover(this, key, false, false, cx);
                        }
                    }
                });
            }),
            on_popup: std::rc::Rc::new(move |bi, hovered, _, app| {
                let _ = v3.update(app, |this, cx| {
                    charts::chart_hover(this, charts::PopupKey::Profit(bi), hovered, true, cx);
                });
            }),
        }
    }
}

/// Format a top-table timestamp as `DD.MM.YY HH:MM` in the zone it is handed.
///
/// Args:
///     secs: Absolute UTC Unix seconds.
///     zone: Zone to render in — the report axis's for a replicated value, never the display
///         zone independently.
///
/// Returns:
///     Civil date-time label, or the shared formatter's fallback text.
pub(super) fn fmt_dm_hm(secs: i64, zone: chrono_tz::Tz) -> String {
    let s = moon_core::util::display_time::format_minute(secs, zone);
    // The shared formatter returns "YYYY-MM-DD HH:MM"; take "DD.MM.YY HH:MM".
    if s.len() >= 16 {
        format!("{}.{}.{} {}", &s[8..10], &s[5..7], &s[2..4], &s[11..16])
    } else {
        s
    }
}

/// Display name of a strategy: `strategyid = 0` means manual orders (no
/// strategy), so the bare "0" is replaced with a human-readable label.
///
/// The short form for callers that hold no identity to fall back on — the Tuner's three. A
/// caller that DOES know the row's strategy id takes [`strat_display_ex`] instead, which is the
/// only way a row with no resolved name gets a label rather than a raw 19-digit hash.
pub(super) fn strat_display(name: &str) -> String {
    // The "0" rule stays HERE, not in `strat_display_ex`. This form's callers hand it text that
    // may be an id, so "0" means the manual-orders identity; `strat_display_ex`'s callers say
    // whether their text is an id, and there a strategy a user NAMED "0" must keep that name.
    if name == "0" {
        return t!("analytics.manual_orders").to_string();
    }
    name.to_string()
}

/// A strategy cell: what to print, whether it is the app speaking rather than the user, and the
/// full identity behind a shortened one.
pub(in crate::analytics) struct StratLabel {
    /// Text rendered in the Summary cell.
    pub text: String,
    /// The label is OURS, not a name the user gave — render it muted so the two never look alike.
    pub muted: bool,
    /// Present only when [`Self::text`] shortened an identity, so a tooltip can show all of it.
    pub full_id: Option<String>,
}

/// Turn one strategy identity into the cell the Summary renders.
///
/// The unresolved case is REPORTED by the database layer (`GroupStat::name_is_id`,
/// `TopTrade::strategy_is_id`), never guessed from the text: a strategy a user named "12345" is
/// indistinguishable from an id fallback by comparison, and guessing would relabel it.
///
/// Args:
///     name: Display text as the database layer produced it.
///     id: The row's own `strategyid` text, used only when `is_id` says `name` IS that text.
///     is_id: Whether `name` is the id fallback rather than a name.
///     alive: Head status with `GroupStat::alive`'s encoding: `None` when status is unavailable,
///         `0` when the database lacks the pair, and `1` or `2` when it has the pair. Only `0`
///         produces the deleted-strategy label.
///
/// Returns:
///     The cell's text, whether it is muted, and the full identity when the text shortened one.
fn strat_display_ex(name: &str, id: &str, is_id: bool, alive: Option<i64>) -> StratLabel {
    // A RESOLVED name is printed as it is, before any rule keyed on text: a strategy a user
    // named "0" is not the manual-orders identity, and relabelling it would be exactly the
    // guessing this signature exists to remove.
    if !is_id {
        return StratLabel {
            text: name.to_string(),
            muted: false,
            full_id: None,
        };
    }
    // `strategyid = 0` is manual orders — a real identity with its own label that no strategy
    // database will ever name, so it reaches here as an unresolved id and stops here.
    if name == "0" {
        return StratLabel {
            text: t!("analytics.manual_orders").to_string(),
            muted: false,
            full_id: None,
        };
    }
    // `alive == Some(0)` is the database saying it does not have this pair — deleted. Anything
    // else (a live head whose name is blank, or no strategy database at all) is NOT a deletion,
    // and saying so would be a claim nobody checked.
    let key = if alive == Some(0) {
        "analytics.strategy_deleted"
    } else {
        "analytics.strategy_unnamed"
    };
    StratLabel {
        text: t!(key, id = short_id(id)).to_string(),
        muted: true,
        full_id: Some(id.to_string()),
    }
}

/// Tail of a strategy id, for a label that must fit a table cell.
///
/// Moonbot writes `strategyid` as a 19-digit signed hash, which is both unreadable and wider
/// than the column. The tail is what a human compares against the tooltip's full id, and it is
/// DECIMAL because every other place this app prints a strategy id is (`analytics.purge.*`).
///
/// Args:
///     id: Full id text as the report row stored it.
///
/// Returns:
///     The id itself when it is already short, else an ellipsis and its last six characters.
fn short_id(id: &str) -> String {
    let n = id.chars().count();
    if n <= 8 {
        return id.to_string();
    }
    format!("…{}", id.chars().skip(n - 6).collect::<String>())
}

#[cfg(test)]
mod tests;
