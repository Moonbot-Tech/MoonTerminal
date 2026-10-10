//! The transient month-by-days surface, sharing the monitor's serialized refresh gate.

use super::*;
use crate::load_state::LoadState;
use days_model::{DayReport, DayRow, current_month, period_label, step_month};
use moon_core::db::{QuoteBreakdown, ReportFilter, RowScope};
use moon_core::util::fmt;
use moon_ui::{MoonButton, MoonDisclosure, MoonDisclosureDirection};

impl ProfitMonitorView {
    /// Toggle the third body view; each opening resets its independent month and disclosure.
    pub(super) fn toggle_days(&mut self, cx: &mut Context<Self>) {
        self.busy_retries.reset();
        self.days_open = !self.days_open;
        if self.days_open {
            self.days_month = current_month(now_utc(), self.zone);
            self.days_extra = false;
        }
        self.reload(false, cx);
        self.invalidate_content(cx);
        cx.notify();
    }

    /// Navigate one month within the current-month ceiling and replace the prior result.
    pub(super) fn shift_days(&mut self, forward: bool, cx: &mut Context<Self>) {
        if let Some(month) = step_month(
            self.days_month,
            forward,
            current_month(now_utc(), self.zone),
        ) {
            self.days_month = month;
            self.busy_retries.reset();
            self.reload(false, cx);
        }
    }

    /// Load days on open, month change and the existing monitor refresh, with no repaint timer.
    pub(super) fn reload_days(&mut self, after_report: bool, cx: &mut Context<Self>) {
        // A clock-zone change at a month boundary can move the ceiling back one civil month.
        self.days_month = self.days_month.min(current_month(now_utc(), self.zone));
        if self.db_active {
            if !after_report {
                self.seq = self.seq.wrapping_add(1);
                self.days_data = LoadState::default();
                self.invalidate_content(cx);
            }
            self.refresh
                .request_refresh(std::time::Instant::now(), false, RefreshUrgency::Writer);
            self.schedule_refresh(cx);
            cx.notify();
            return;
        }
        begin_days_read(&mut self.days_data, after_report);
        self.refresh_error = None;
        self.seq = self.seq.wrapping_add(1);
        let request = self.seq;
        let generation = self.current_generation();
        self.refresh
            .refresh_started(generation, std::time::Instant::now());
        let query = self.query();
        let filter = ReportFilter {
            core_uids: query.cores,
            emulator: Some(false),
            rows: RowScope::Closed,
            valuation: self.valuation,
            ..Default::default()
        };
        let month = self.days_month;
        let zone = self.zone;
        let now = now_utc();
        self.db_active = true;
        self.invalidate_content(cx);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { days_model::read_days(filter, month, now, zone) })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.db_active = false;
                    if this.seq == request {
                        let error = result.as_ref().err().cloned();
                        this.days_data.apply(result);
                        this.refresh_error.clone_from(&error);
                        this.invalidate_content(cx);
                        if report_result_is_stale(generation, this.current_generation(), false) {
                            this.refresh.request_refresh(
                                std::time::Instant::now(),
                                false,
                                RefreshUrgency::Writer,
                            );
                        }
                        this.settle_busy_retry(error.as_ref(), cx);
                    }
                    this.schedule_refresh(cx);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Draw the approved same-window report and its loading, empty or retryable failure state.
    pub(super) fn days_body(
        &self,
        width: f32,
        palette: MoonPalette,
        view: Entity<Self>,
        cx: &App,
    ) -> AnyElement {
        let refreshed = self
            .days_data
            .data()
            .map_or_else(now_utc, |report| report.refreshed);
        let mut ids = self.live.core_order.clone();
        ids.extend(
            self.seen_data_cores
                .iter()
                .copied()
                .filter(|core| !self.live.configured_core_ids.contains(core)),
        );
        ids.sort_unstable();
        ids.dedup();
        let previous = view.clone();
        let next = view.clone();
        let header = day_header(
            DayHeader {
                month: self.days_month,
                now: refreshed,
                zone: self.zone,
                count: ids.len(),
                width,
                palette,
            },
            cx,
            (
                move |_: &mut Window, app: &mut App| {
                    previous.update(app, |this, cx| this.shift_days(false, cx))
                },
                move |_: &mut Window, app: &mut App| {
                    next.update(app, |this, cx| this.shift_days(true, cx))
                },
            ),
        );
        let body = match &self.days_data {
            LoadState::Loading { stale: None } => {
                centered_message(t!("profit_monitor.days.loading").to_string(), palette, cx)
            }
            LoadState::NotReady => {
                centered_message(t!("profit_monitor.not_ready").to_string(), palette, cx)
            }
            LoadState::Failed(_) => {
                let retry = view.clone();
                v_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 10.0))
                    .p(design::ui_px(cx, 16.0))
                    .font_family(design::ui_font())
                    .child(
                        div()
                            .text_color(moon(palette.red))
                            .child(t!("profit_monitor.days.failed").to_string()),
                    )
                    .child(
                        MoonButton::new("profit-monitor-days-retry")
                            .label(t!("profit_monitor.days.retry").to_string())
                            .size(MoonButtonSize::density(cx))
                            .variant(MoonButtonVariant::Soft)
                            .on_click(move |_, _, app| {
                                retry.update(app, |this, cx| {
                                    this.busy_retries.reset();
                                    this.reload(false, cx);
                                })
                            })
                            .render(),
                    )
                    .into_any_element()
            }
            LoadState::Ready(report)
            | LoadState::Loading {
                stale: Some(report),
            } if report.rows.is_empty() => {
                centered_message(t!("profit_monitor.days.empty").to_string(), palette, cx)
            }
            LoadState::Ready(report)
            | LoadState::Loading {
                stale: Some(report),
            } => {
                let owner = view.clone();
                let label = t!("profit_monitor.days.details").to_string();
                let toggle = move |_: &ClickEvent, _: &mut Window, app: &mut App| {
                    owner.update(app, |this, cx| {
                        this.days_extra = !this.days_extra;
                        this.invalidate_content(cx);
                        cx.notify();
                    })
                };
                let caret_owner = view.clone();
                v_flex()
                    .w_full()
                    .gap(design::ui_px(cx, 6.0))
                    .child(day_table(
                        report,
                        self.days_extra,
                        width,
                        self.zone,
                        palette,
                        cx,
                    ))
                    .when(self.days_extra, |el| {
                        el.child(currency_facts(report, palette, cx))
                    })
                    .child(
                        h_flex()
                            .gap(design::ui_px(cx, 4.0))
                            .child(
                                MoonDisclosure::button(
                                    "profit-monitor-days-disclosure",
                                    self.days_extra,
                                )
                                .direction(MoonDisclosureDirection::DownUp)
                                .tooltip(label.clone())
                                .on_toggle(
                                    move |expanded, _, app| {
                                        caret_owner.update(app, |this, cx| {
                                            this.days_extra = *expanded;
                                            this.invalidate_content(cx);
                                            cx.notify();
                                        })
                                    },
                                ),
                            )
                            .child(
                                MoonButton::new("profit-monitor-days-details-label")
                                    .label(label)
                                    .variant(MoonButtonVariant::Ghost)
                                    .size(MoonButtonSize::density(cx))
                                    .on_click(toggle)
                                    .render(),
                            ),
                    )
                    .into_any_element()
            }
        };
        v_flex()
            .size_full()
            .min_h_0()
            .child(header)
            .child(
                div()
                    .id("profit-monitor-days-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(design::ui_px(cx, 12.0))
                    .child(body),
            )
            .into_any_element()
    }
}

/// Inputs for the backend-independent report header.
pub(super) struct DayHeader {
    pub month: chrono::NaiveDate,
    pub now: DateTime<Utc>,
    pub zone: Tz,
    pub count: usize,
    pub width: f32,
    pub palette: MoonPalette,
}

/// Render a centred one-line period group and a muted scope count; narrow windows omit labels.
pub(super) fn day_header(
    header: DayHeader,
    cx: &App,
    navigation: (
        impl Fn(&mut Window, &mut App) + 'static,
        impl Fn(&mut Window, &mut App) + 'static,
    ),
) -> AnyElement {
    let DayHeader {
        month,
        now,
        zone,
        count,
        width,
        palette,
    } = header;
    let (previous, next) = navigation;
    let title = t!("profit_monitor.days.title").to_string();
    let label = period_label(month, now, zone);
    let count_label = core_count_noun(count);
    let navigation_width = design::ui_value(cx, 26.0);
    let gap = design::ui_value(cx, 6.0);
    let side_width = design::mono_caption_text_width(cx, &count.to_string(), 400.0)
        + design::ui_caption_text_width(cx, &count_label, 400.0)
        + design::ui_value(cx, 4.0);
    let period_width = design::mono_caption_text_width(cx, &label, 600.0);
    let title_width = design::ui_caption_text_width(cx, &title, 600.0);
    let available = width - design::ui_value(cx, 24.0);
    // Reserve equal side slots from actual text, rather than dropping labels at a zoom-sensitive tier.
    let full_group = 2.0 * navigation_width + 3.0 * gap + period_width + title_width;
    let show_title = full_group + 2.0 * side_width <= available;
    let group_width = if show_title {
        full_group
    } else {
        2.0 * navigation_width + 2.0 * gap + period_width
    };
    let show_count = group_width + 2.0 * side_width <= available;
    let group = h_flex()
        .flex_none()
        .gap(design::ui_px(cx, 6.0))
        .debug_selector(|| "days-header-group".into())
        .child(
            MoonButton::new("days-previous-month")
                .icon("icons/chevron-left.svg")
                .tooltip(t!("profit_monitor.days.previous").to_string())
                .variant(MoonButtonVariant::Soft)
                .size(MoonButtonSize::density(cx))
                .padding_x(4.0)
                .width(navigation_width)
                .on_click(move |_, window, app| previous(window, app))
                .render(),
        )
        .when(show_title, |el| {
            el.child(
                div()
                    .flex_none()
                    .font_family(design::ui_font())
                    .debug_selector(|| "days-header-title".into())
                    .child(title),
            )
        })
        .child(
            div()
                .flex_none()
                .font_family(design::mono())
                .debug_selector(|| "days-period".into())
                .child(label),
        )
        .child(
            MoonButton::new("days-next-month")
                .icon("icons/chevron-right.svg")
                .tooltip(t!("profit_monitor.days.next").to_string())
                .variant(MoonButtonVariant::Soft)
                .size(MoonButtonSize::density(cx))
                .padding_x(4.0)
                .width(navigation_width)
                .disabled(month >= current_month(now_utc(), zone))
                .on_click(move |_, window, app| next(window, app))
                .render(),
        );
    h_flex()
        .w_full()
        .flex_none()
        .py(design::ui_px(cx, 8.0))
        .px(design::ui_px(cx, 12.0))
        .debug_selector(|| "days-header".into())
        .text_size(design::t_caption(cx))
        .font_weight(FontWeight::SEMIBOLD)
        .child(div().flex_1().min_w_0())
        .child(group)
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .justify_end()
                .when(show_count, |el| {
                    el.child(
                        h_flex()
                            .id("profit-monitor-days-core-count")
                            .debug_selector(|| "days-core-count".into())
                            .flex_none()
                            .gap(design::ui_px(cx, 4.0))
                            .font_family(design::ui_font())
                            .font_weight(FontWeight::NORMAL)
                            .text_color(moon(palette.text_muted))
                            .tooltip(crate::panels::common::text_tooltip(
                                t!("profit_monitor.days.cores_tip").to_string(),
                            ))
                            .child(div().font_family(design::mono()).child(count.to_string()))
                            .child(count_label),
                    )
                }),
        )
        .into_any_element()
}

/// Format the bot's signed USDT result with its rounded semantic sign, withholding incomplete values.
pub(super) fn day_profit(quotes: &QuoteBreakdown) -> (String, Option<fmt::DeltaSign>) {
    quotes
        .unified_usdt()
        .and_then(|value| fmt::signed_fixed(value.profit, 2))
        .map_or_else(
            || (t!("telegram.report_unvalued").to_string(), None),
            |(text, sign)| (format!("{text}$"), Some(sign)),
        )
}

/// Format exactly the bot's average-order amount, leaving unsupported scopes explicitly unvalued.
pub(super) fn day_average(quotes: &QuoteBreakdown) -> String {
    quotes.average_order_return().map_or_else(
        || t!("telegram.report_unvalued").to_string(),
        |value| {
            format!(
                "{} {}",
                fmt::group_decimal(&if value.avg_order < 1.0 {
                    fmt::adaptive(value.avg_order)
                } else {
                    fmt::compact(value.avg_order, 2)
                }),
                value.currency.ticker()
            )
        },
    )
}

/// Render measured columns and a bold snapshot total; insufficient widths use vertical rows.
pub(super) fn day_table(
    report: &DayReport,
    extra: bool,
    width: f32,
    zone: Tz,
    palette: MoonPalette,
    cx: &App,
) -> AnyElement {
    let headings = [
        t!("profit_monitor.days.date").to_string(),
        t!("profit_monitor.days.result").to_string(),
        t!("profit_monitor.days.trades").to_string(),
        t!("profit_monitor.days.average").to_string(),
    ];
    let mut values: Vec<[String; 4]> = report
        .rows
        .iter()
        .map(|row| day_cells(row.date.to_string(), &row.quotes))
        .collect();
    values.push(total_cells(&report.total));
    let columns = if extra { 4 } else { 3 };
    let mut widths = [0.0_f32; 4];
    for col in 0..columns {
        widths[col] = design::ui_caption_text_width(cx, &headings[col], 600.0);
        for row in &values {
            widths[col] = widths[col].max(design::mono_caption_text_width(cx, &row[col], 600.0));
        }
        widths[col] += design::ui_value(cx, 4.0);
    }
    let gap = design::ui_value(cx, 8.0);
    let available = width - design::ui_value(cx, 46.0);
    let natural = widths[..columns].iter().sum::<f32>() + gap * (columns - 1) as f32;
    let stacked = natural > available;
    if !stacked {
        widths[1] += available - natural;
    }
    let mut table = v_flex()
        .w_full()
        .border_1()
        .border_color(moon(palette.border))
        .rounded(design::ui_px(cx, 4.0));
    let layout = DayColumns {
        widths,
        count: columns,
        stacked,
    };
    table = table.child(day_table_row(
        &headings, layout, "head", true, None, palette, cx,
    ));
    let today = now_utc().with_timezone(&zone).date_naive();
    for (index, row) in report.rows.iter().enumerate() {
        let sign = day_profit(&row.quotes).1;
        table = table.child(
            day_table_row(
                &values[index],
                layout,
                &index.to_string(),
                false,
                sign,
                palette,
                cx,
            )
            .when(row.date == today, |el| {
                el.bg(moon_alpha(palette.amber, 0.08))
            }),
        );
    }
    table
        .child(
            day_table_row(
                values.last().expect("total cell exists"),
                layout,
                "total",
                false,
                day_profit(&report.total).1,
                palette,
                cx,
            )
            .font_weight(FontWeight::BOLD)
            .bg(moon(palette.shell_high)),
        )
        .into_any_element()
}

/// Build the table's date, profit, count and average cells from one shared breakdown.
fn day_cells(date: String, quotes: &QuoteBreakdown) -> [String; 4] {
    [
        date,
        day_profit(quotes).0,
        quotes.orders.to_string(),
        day_average(quotes),
    ]
}

/// Build the actual total cells with the bot's localized, approved total caption.
fn total_cells(quotes: &QuoteBreakdown) -> [String; 4] {
    day_cells(t!("telegram.report_total").to_string(), quotes)
}

/// Keep the current month visible during writer catch-up, but clear it for a different query.
/// Completed failures still go through `LoadState::apply`, which always drops retained figures.
fn begin_days_read(state: &mut LoadState<DayReport>, background: bool) {
    if background {
        state.begin();
    } else {
        *state = LoadState::default();
    }
}

/// Localize a complete count noun so mono numbers never require empty-template interpolation.
fn core_count_noun(count: usize) -> String {
    t!(days_model::core_count_key(
        count,
        rust_i18n::locale().as_ref()
    ))
    .to_string()
}

/// Measured widths and the narrow-host degradation shared by every row.
#[derive(Clone, Copy)]
struct DayColumns {
    widths: [f32; 4],
    count: usize,
    stacked: bool,
}

/// Lay out a row without shrinking its measured text; a narrow host stacks the cells vertically.
fn day_table_row(
    values: &[String; 4],
    layout: DayColumns,
    id: &str,
    heading: bool,
    sign: Option<fmt::DeltaSign>,
    palette: MoonPalette,
    cx: &App,
) -> Div {
    let DayColumns {
        widths,
        count: columns,
        stacked,
    } = layout;
    let mut row = div()
        .flex()
        .when(stacked, |el| el.flex_col())
        .w_full()
        .px(design::ui_px(cx, 10.0))
        .py(design::ui_px(cx, 4.0))
        .gap(design::ui_px(cx, 8.0))
        .border_t_1()
        .border_color(moon_alpha(palette.border, 0.4))
        .text_size(design::t_caption(cx))
        .font_family(if heading {
            design::ui_font()
        } else {
            design::mono()
        });
    for col in 0..columns {
        let selector = format!("days-cell-{id}-{col}");
        let color = if col == 1 && !heading {
            sign.map_or(palette.text_muted, |sign| {
                sign.pick(palette.green, palette.red, palette.text)
            })
        } else if heading || col == 3 {
            palette.text_muted
        } else {
            palette.text
        };
        row = row.child(
            div()
                .flex_none()
                .when(!stacked, |el| el.w(px(widths[col])))
                .when(col > 0 && !stacked, |el| el.text_right())
                .when(id == "total" && col == 0, |el| {
                    el.font_family(design::ui_font())
                })
                .debug_selector(move || selector)
                .text_color(moon(color))
                .child(values[col].clone()),
        );
    }
    row
}

/// Expose the bot's native currency amounts and partial average coverage, without new metrics.
fn currency_facts(report: &DayReport, palette: MoonPalette, cx: &App) -> AnyElement {
    let mut facts = v_flex()
        .w_full()
        .gap(design::ui_px(cx, 4.0))
        .font_family(design::ui_font())
        .text_size(design::t_caption(cx))
        .text_color(moon(palette.text_muted));
    for DayRow { date, quotes } in &report.rows {
        let native = quotes
            .totals
            .iter()
            .map(|part| {
                format!(
                    "{} {}",
                    fmt::signed_fixed(part.profit, part.currency.display_decimals()).map_or_else(
                        || t!("telegram.report_unvalued").to_string(),
                        |value| value.0
                    ),
                    part.currency.ticker()
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        facts = facts.child(
            v_flex()
                .w_full()
                .child(
                    h_flex()
                        .gap(design::ui_px(cx, 6.0))
                        .child(div().font_family(design::mono()).child(date.to_string()))
                        .child(t!("profit_monitor.days.currency").to_string()),
                )
                .child(div().w_full().font_family(design::mono()).child(native)),
        );
        let (counted, excluded) = quotes.average_order_return().map_or_else(
            || {
                let counted = quotes
                    .entry_spend
                    .counted_orders
                    .clamp(0, quotes.orders.max(0));
                (counted, quotes.orders.saturating_sub(counted))
            },
            |value| (value.counted, value.excluded),
        );
        if excluded > 0 {
            facts = facts.child(
                t!(
                    "telegram.report_average_coverage",
                    counted = counted,
                    excluded = excluded
                )
                .to_string(),
            );
        }
    }
    facts.into_any_element()
}

#[cfg(test)]
mod tests;
