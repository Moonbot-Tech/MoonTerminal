//! Everything the Profit Monitor DRAWS below its controls: the responsive table, its sortable
//! heading and total footer, the split-currency and placeholder states, and the value formatting
//! they share.
//!
//! Split from `mod.rs`, which keeps the view state, the refresh machinery and the window chrome.
//! The rule for the boundary is simple: nothing here reads or writes `ProfitMonitorView` state —
//! every function takes what it needs and returns an element.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::db::ProfitUnit;
use moon_core::session::CoreId;
use moon_core::util::fmt::DeltaSign;
use moon_ui::{
    MoonAlert, MoonPalette, MoonScrollbarVisibility, MoonVirtualList, MoonVirtualListScrollHandle,
    h_flex, v_flex,
};
use rust_i18n::t;

use super::rows::MonitorRow;
use super::settings::MonitorPrefs;
use super::{
    AVERAGE_ORDER_COLUMN_WIDTH, EXCHANGE_LOGO_SIZE, MIN_NAME_COLUMN_WIDTH, MonitorLayout,
    MonitorSort, MonitorSortColumn, NAME_LOGO_GAP, PROFIT_COLUMN_WIDTH, PROFIT_LAST_TRADE_EXTRA,
    ProfitMonitorView, TABLE_COLUMN_GAP, TABLE_HORIZONTAL_PADDING, TRADES_COLUMN_WIDTH,
    WIN_RATE_COLUMN_WIDTH, sort_arrow,
};
use crate::design;
use crate::design::{moon, moon_alpha};
use crate::media::exchange_logos::exchange_logo;

/// Render a centered neutral message.
///
/// Args:
///     message: User-facing text.
///     palette: Active MoonUI palette.
///     cx: Render context.
///
/// Returns:
///     Full-height centered placeholder.
pub(super) fn centered_message(message: String, palette: MoonPalette, cx: &App) -> AnyElement {
    div()
        .flex_1()
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(moon(palette.text_muted))
        .text_size(design::t_body(cx))
        .child(message)
        .into_any_element()
}

/// Render a centered classified read failure.
///
/// Args:
///     title: Localized heading.
///     detail: Classified database detail.
///     cx: Render context.
///
/// Returns:
///     Centered MoonAlert.
pub(super) fn centered_alert(title: String, detail: String, cx: &App) -> AnyElement {
    div()
        .flex_1()
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .px(design::ui_px(cx, 20.0))
        .child(MoonAlert::error("profit-monitor-error", detail).title(title))
        .into_any_element()
}

/// Render exact split quote totals without a false combined scalar.
///
/// Args:
///     totals: Per-quote safe totals.
///     show_trades: Whether the current width retains the aggregate trade count.
///     palette: Active MoonUI palette.
///     cx: Render context.
///
/// Returns:
///     Explanation and quote chips, with the aggregate trade count when space permits.
pub(super) fn split_body(
    totals: &moon_core::db::QuoteBreakdown,
    show_trades: bool,
    palette: MoonPalette,
    cx: &App,
) -> AnyElement {
    let mut chips = h_flex()
        .flex_wrap()
        .justify_center()
        .gap(design::ui_px(cx, 6.0));
    for total in &totals.totals {
        let (amount, sign) = total.signed_display();
        chips = chips.child(
            div()
                .px(design::ui_px(cx, 9.0))
                .py(design::ui_px(cx, 5.0))
                .rounded(design::ui_px(cx, 5.0))
                .bg(moon(palette.table_head))
                .text_color(moon(sign.pick(
                    design::positive_color(palette),
                    design::danger_color(palette),
                    palette.text,
                )))
                .child(amount),
        );
    }
    v_flex()
        .flex_1()
        .w_full()
        .items_center()
        .justify_center()
        .gap(design::ui_px(cx, 12.0))
        .px(design::ui_px(cx, 20.0))
        .text_align(TextAlign::Center)
        .child(
            div()
                .text_color(moon(palette.text))
                .child(t!("profit_monitor.split_title").to_string()),
        )
        .child(
            div()
                .max_w(design::ui_px(cx, 560.0))
                .text_color(moon(palette.text_muted))
                .child(t!("profit_monitor.split_detail").to_string()),
        )
        .child(chips)
        .when(show_trades, |body| {
            body.child(
                div()
                    .text_color(moon(palette.text_soft))
                    .child(t!("profit_monitor.trades_total", n = totals.orders).to_string()),
            )
        })
        .into_any_element()
}

/// Render the responsive monitor table and exact total footer.
///
/// Args:
///     rows: Already grouped and sorted display rows.
///     unit: Comparable exact unit, or `None` for an empty result.
///     width: Current window width.
///     sort: Explicit user-selected ordering, if any.
///     prefs: Display preferences chosen in the ⚙ popup.
///     flash: Live arrival stamps, keyed by the core that closed the trade.
///     selection: Cores currently broadcast to the main window; empty means no filter.
///     scroll: Retained vertical-list position.
///     palette: Active MoonUI palette.
///     view: Owning monitor entity receiving sortable-header actions.
///     cx: Application context used for rendering.
///
/// Returns:
///     Fixed header/footer with a vertically scrolling row body.
#[allow(clippy::too_many_arguments)]
pub(super) fn table(
    rows: Vec<MonitorRow>,
    unit: Option<ProfitUnit>,
    width: f32,
    sort: Option<MonitorSort>,
    prefs: MonitorPrefs,
    flash: &crate::pulse::Arrivals<CoreId>,
    selection: &HashSet<CoreId>,
    scroll: &MoonVirtualListScrollHandle,
    palette: MoonPalette,
    view: Entity<ProfitMonitorView>,
    cx: &App,
) -> AnyElement {
    let layout = MonitorLayout::for_width(width, design::ui_value(cx, 1.0));
    let show_trades = layout.trades;
    let show_win = layout.win_rate;
    let show_average = layout.average_order;
    // Both halves have to agree: the preference asks for the suffix, the width decides whether the
    // column can hold it. Anything narrower would truncate a money value instead of dropping it.
    let show_last = prefs.last_trade && layout.last_trade;
    let profit_width = profit_column_width(show_last);
    let total = rows.iter().fold(MonitorRow::default(), |mut total, row| {
        total.profit += row.profit;
        total.trades += row.trades;
        total.wins += row.wins;
        total.positive_spent += row.positive_spent;
        total.positive_orders += row.positive_orders;
        // The footer's "last trade" is the newest one on screen, so Total answers the same question
        // its rows do rather than summing values from different instants. The core id breaks a tie:
        // folding in list order would otherwise let the footer's money change when the user clicks
        // a different sort column.
        if (row.last_close, row.last_core) > (total.last_close, total.last_core) {
            total.last_profit = row.last_profit;
            total.last_close = row.last_close;
            total.last_core = row.last_core;
        }
        total
    });
    // Resolved once per render rather than inside the row builder — that closure runs for every
    // visible row on every frame, and a lookup there would take the logo cache's global lock at
    // frame rate — and once per distinct EXCHANGE rather than once per row: two hundred cores on
    // one exchange are two hundred identical answers, and the resolver allocates a string for each.
    // One pass over the rows resolves both decorations. Doing either inside the virtual-list item
    // builder would repeat it for every visible row on every frame — and the highlight can drive
    // that at 10 Hz — while a merged Exchange row would pay one hash lookup per core it contains.
    let flashes: Vec<Option<Instant>> = rows
        .iter()
        .map(|row| row.cores.iter().filter_map(|core| flash.get(core)).max())
        .collect();
    // Resolved HERE, once per render, for the same reason the logos are: the item builder below
    // runs for every visible row on every frame, and it can be driven at 10 Hz by the arrival
    // highlight. Each entry then costs the builder two refcount bumps. The whole pass is skipped
    // while the feature is off.
    //
    // A merged Exchange row counts as selected only when the filter holds ALL of its cores: it is
    // one click target, so a half-selected row would offer a state its own click cannot produce.
    //
    // Weak, and cloned per row rather than the entity itself: `Entity::clone` takes the process
    // entity map's lock.
    let owner = view.downgrade();
    let selects: Vec<RowSelect> = if prefs.core_filter {
        rows.iter()
            .map(|row| RowSelect {
                selected: !selection.is_empty()
                    && !row.filter_cores.is_empty()
                    && row.filter_cores.iter().all(|core| selection.contains(core)),
                cores: row.filter_cores.clone(),
                owner: owner.clone(),
            })
            .collect()
    } else {
        Vec::new()
    };
    let logos: Vec<Option<Arc<RenderImage>>> = if prefs.exchange_icons {
        let mut resolved: HashMap<&str, Option<Arc<RenderImage>>> = HashMap::new();
        rows.iter()
            .map(|row| {
                let exchange = row.exchange.as_deref()?;
                resolved
                    .entry(exchange)
                    .or_insert_with(|| exchange_logo(exchange))
                    .clone()
            })
            .collect()
    } else {
        Vec::new()
    };
    let header = table_header(
        layout,
        profit_width,
        prefs.exchange_icons,
        sort,
        view.clone(),
        palette,
        cx,
    );
    let rows = Arc::new(rows);
    let row_count = rows.len();
    let list_rows = rows.clone();
    let row_height = design::fit_h_value(cx, 34.0, 13.0, 8.0);
    let body = MoonVirtualList::new(
        "profit-monitor-rows",
        row_count,
        row_height,
        move |index, _window, app| {
            let Some(row) = list_rows.get(index) else {
                return div().into_any_element();
            };
            let (profit, profit_sign) =
                format_profit(row.profit, row.last_profit.filter(|_| show_last), unit);
            let select = selects.get(index).cloned();
            let is_selected = select.as_ref().is_some_and(|select| select.selected);
            table_row(
                row.name.clone(),
                profit,
                profit_sign,
                format_trade_count(row.trades),
                Some(format_win_rate(row.win_rate())),
                Some(format_amount(row.average_order(), unit)),
                show_trades,
                show_win,
                show_average,
                RowChrome {
                    id: ("profit-monitor-row", row.primary_core).into(),
                    logo: logos.get(index).cloned().flatten(),
                    logo_gutter: prefs.exchange_icons,
                    flash: flashes.get(index).copied().flatten(),
                    profit_width,
                    select,
                },
                palette,
                app,
            )
            // Selection outranks the zebra stripe: it is the answer to "which cores is the main
            // window showing", and a stripe drawn over it would make every other selected row look
            // unselected. It is drawn in BLUE rather than in `table_selected`, which the arrival
            // tint already owns here — one colour for both would swallow the new-trade flash on
            // exactly the rows the user is watching. Same pairing the tuner's coin list uses.
            .when(is_selected, |element| {
                element.bg(moon_alpha(palette.blue, 0.18))
            })
            .when(!is_selected && index % 2 == 1, |element| {
                element.bg(moon_alpha(palette.table_head, 0.45))
            })
            .into_any_element()
        },
    )
    .track_scroll(scroll)
    .scrollbar_visibility(MoonScrollbarVisibility::Always)
    .surface(false)
    .border(false)
    .radius(0.0);
    let (total_profit, total_profit_sign) =
        format_profit(total.profit, total.last_profit.filter(|_| show_last), unit);
    let footer = table_row(
        t!("profit_monitor.total").to_string(),
        total_profit,
        total_profit_sign,
        format_trade_count(total.trades),
        Some(format_win_rate(total.win_rate())),
        Some(format_amount(total.average_order(), unit)),
        show_trades,
        show_win,
        show_average,
        RowChrome {
            // Its own prefix rather than a core id, which a real core could otherwise collide with.
            id: "profit-monitor-row-total".into(),
            // The total is not one exchange and never one core's arrival: no logo, no highlight,
            // and no click target — "filter the main window to every core at once" is what
            // clearing the filter already means. It keeps the gutter, or its label stops lining up
            // with the names above it.
            logo: None,
            logo_gutter: prefs.exchange_icons,
            flash: None,
            profit_width,
            select: None,
        },
        palette,
        cx,
    )
    .h(design::fit_h_px(cx, 42.0, 14.0, 10.0))
    .bg(moon(palette.table_head))
    .text_size(design::t_title(cx))
    .font_weight(FontWeight::SEMIBOLD)
    .border_t(px(2.0))
    .border_color(moon_alpha(palette.amber, 0.7));

    v_flex()
        .flex_1()
        .min_h_0()
        .w_full()
        .child(header)
        .child(div().flex_1().min_h_0().w_full().child(body))
        .child(footer)
        .into_any_element()
}

/// Render the fixed clickable header whose geometry mirrors the data rows.
///
/// Args:
///     layout: Responsive presentation including visible-column selection.
///     profit_width: Profit-column width the data rows are using.
///     logo_gutter: Whether the data rows reserve room for an exchange logo.
///     sort: Current explicit ordering.
///     view: Monitor entity receiving heading clicks.
///     palette: Active MoonUI palette.
///     cx: Render context used for scaled geometry.
///
/// Returns:
///     Fixed-height sortable table header.
#[allow(clippy::too_many_arguments)]
fn table_header(
    layout: MonitorLayout,
    profit_width: f32,
    logo_gutter: bool,
    sort: Option<MonitorSort>,
    view: Entity<ProfitMonitorView>,
    palette: MoonPalette,
    cx: &App,
) -> Div {
    let sortable = |id: &'static str,
                    title: String,
                    column: MonitorSortColumn,
                    width: Option<f32>,
                    right: bool| {
        let active = sort.is_some_and(|active| active.column == column);
        let target = view.clone();
        let mut cell = div()
            .id(id)
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .cursor_pointer()
            .hover(|style| style.text_color(moon(palette.amber)))
            .text_color(moon(if active {
                palette.amber
            } else {
                palette.text_soft
            }))
            // No tooltip: the heading repeats the text it would carry.
            .child(format!("{title}{}", sort_arrow(sort, column)))
            .on_click(move |_, _, app| {
                target.update(app, |this, cx| this.toggle_sort(column, cx));
            });
        if right {
            cell = cell.text_align(TextAlign::Right);
        }
        match width {
            Some(width) => cell.w(design::ui_px(cx, width)).flex_none(),
            None => cell
                .min_w(design::ui_px(cx, MIN_NAME_COLUMN_WIDTH))
                .flex_1(),
        }
    };

    h_flex()
        .w_full()
        .h(design::fit_h_px(cx, 34.0, 13.0, 8.0))
        .px(design::ui_px(cx, TABLE_HORIZONTAL_PADDING))
        .gap(design::ui_px(cx, TABLE_COLUMN_GAP))
        .bg(moon(palette.table_head))
        .border_b(px(1.0))
        .border_color(moon_alpha(palette.border, 0.7))
        .child(
            sortable(
                "profit-monitor-heading-name",
                t!("profit_monitor.column.name").to_string(),
                MonitorSortColumn::Name,
                None,
                false,
            )
            // Padding rather than a spacer sibling: a sibling would also collect the row's own
            // column gap and overshoot the logo by exactly that much. Without it the heading sits
            // left of every value beneath it once icons are on.
            .when(logo_gutter, |heading| {
                heading.pl(design::ui_px(cx, EXCHANGE_LOGO_SIZE + NAME_LOGO_GAP))
            }),
        )
        .child(sortable(
            "profit-monitor-heading-profit",
            t!("profit_monitor.column.profit").to_string(),
            MonitorSortColumn::Profit,
            Some(profit_width),
            true,
        ))
        .when(layout.trades, |header| {
            header.child(sortable(
                "profit-monitor-heading-trades",
                t!("profit_monitor.column.trades").to_string(),
                MonitorSortColumn::Trades,
                Some(TRADES_COLUMN_WIDTH),
                true,
            ))
        })
        .when(layout.win_rate, |header| {
            header.child(sortable(
                "profit-monitor-heading-win-rate",
                t!("profit_monitor.column.win_rate").to_string(),
                MonitorSortColumn::WinRate,
                Some(WIN_RATE_COLUMN_WIDTH),
                true,
            ))
        })
        .when(layout.average_order, |header| {
            header.child(sortable(
                "profit-monitor-heading-average-order",
                t!("profit_monitor.column.average_order").to_string(),
                MonitorSortColumn::AverageOrder,
                Some(AVERAGE_ORDER_COLUMN_WIDTH),
                true,
            ))
        })
}

/// Per-row decoration that is not one of the row's own numbers.
///
/// Bundled rather than passed as three more positional flags: every one of them is optional and
/// two of them are only ever set for data rows, so a struct is what keeps the Total call honest.
struct RowChrome {
    /// Stable element identity of the row.
    ///
    /// Derived from the row's core, never from its rendered text: the name changes with grouping
    /// mode, locale and a core being renamed, and two cores may carry the SAME display name — an
    /// id built from it collides between rows and moves under a click target. Built as a
    /// `(&'static str, u64)` pair, which costs no allocation in the item builder.
    ///
    /// An Exchange row is seeded by the lowest-uid core that traded in the period, so its identity
    /// can still move when that core goes quiet. That costs a dropped hover, never a dropped click:
    /// the handler is rebuilt with the row.
    id: ElementId,
    /// Exchange logo drawn before the name, when enabled and the brand is known.
    logo: Option<Arc<RenderImage>>,
    /// Whether to reserve the logo's width even without one.
    ///
    /// With icons on, a row whose brand is unknown, the Total footer and the sortable heading all
    /// have to start where the logo-bearing rows' text starts — otherwise the Name column no longer
    /// lines up with its own header.
    logo_gutter: bool,
    /// Instant this row's core closed its newest trade, while the highlight is still live.
    flash: Option<Instant>,
    /// Profit-column width selected by the current last-trade decision.
    profit_width: f32,
    /// What clicking this row does, or `None` when the preference is off.
    select: Option<RowSelect>,
}

/// One row's participation in the terminal-wide core filter.
///
/// Present only while the preference is on, which is what makes the row inert — no pointer
/// cursor, no handler — for someone who turned the feature off, rather than merely silent.
/// Cloning is two refcount bumps, which is what lets the item builder take one per visible row.
#[derive(Clone)]
struct RowSelect {
    /// Cores the row stands for, shared rather than copied per frame.
    cores: Rc<[CoreId]>,
    /// Whether the filter currently holds all of them.
    selected: bool,
    /// Monitor receiving the click; weak, because a click can outlive a closing window.
    owner: WeakEntity<ProfitMonitorView>,
}

/// Render one responsive table line.
///
/// Args:
///     name: Leading label.
///     profit: Profit text.
///     profit_sign: Sign represented by the already-rounded profit text.
///     trades: Trade-count text.
///     win_rate: Optional win-rate text.
///     average_order: Optional average-order text.
///     show_trades: Whether the current width retains trade count.
///     show_win: Whether the current width retains win rate.
///     show_average: Whether the current width retains average order.
///     chrome: Logo, arrival highlight, and profit-column width.
///     palette: Active MoonUI palette.
///     cx: Render context.
///
/// Returns:
///     One fixed-height table row.
#[allow(clippy::too_many_arguments)]
fn table_row(
    name: String,
    profit: String,
    profit_sign: DeltaSign,
    trades: String,
    win_rate: Option<String>,
    average_order: Option<String>,
    show_trades: bool,
    show_win: bool,
    show_average: bool,
    chrome: RowChrome,
    palette: MoonPalette,
    cx: &App,
) -> Stateful<Div> {
    let profit_color = profit_sign.pick(
        design::positive_color(palette),
        design::danger_color(palette),
        palette.text,
    );
    let logo_size = design::ui_px(cx, EXCHANGE_LOGO_SIZE);
    // The arrival tint is the News feed's, from the one shared definition: a full-bleed layer
    // declared BEFORE the cells so it sits under the text, and driven by the owner's own stamp
    // rather than by a GPUI animation, which would repaint the whole window at vblank for its
    // entire duration.
    let row = crate::pulse::with_arrival_tint(
        h_flex()
            .w_full()
            .relative()
            .h(design::fit_h_px(cx, 34.0, 13.0, 8.0))
            .px(design::ui_px(cx, TABLE_HORIZONTAL_PADDING))
            .gap(design::ui_px(cx, TABLE_COLUMN_GAP))
            .border_b(px(1.0))
            .border_color(moon_alpha(palette.border, 0.7)),
        palette.table_selected,
        chrome.flash,
    )
    // The WHOLE row is the click target, not the name cell: the numbers belong to the same core,
    // and a row where four of five cells silently do nothing reads as a broken control.
    .id(chrome.id)
    .when_some(chrome.select, |row, select| {
        let RowSelect { cores, owner, .. } = select;
        row.cursor_pointer()
            .on_click(move |event: &ClickEvent, _window, app| {
                // secondary() = Ctrl on Windows/Linux, ⌘ on macOS — the same multi-select modifier
                // the tuner's coin list and the strategies tree use.
                let additive = event.modifiers().secondary();
                // The window may already be gone; a click on a closing view is not an error.
                let _ = owner.update(app, |this, cx| this.filter_to_cores(&cores, additive, cx));
            })
    });
    row.child(
        h_flex()
            .flex_1()
            .min_w(design::ui_px(cx, MIN_NAME_COLUMN_WIDTH))
            .gap(design::ui_px(cx, NAME_LOGO_GAP))
            .overflow_hidden()
            .text_ellipsis()
            .whitespace_nowrap()
            .when_some(chrome.logo.clone(), |element, logo| {
                element.child(
                    img(logo)
                        .flex_none()
                        .w(logo_size)
                        .h(logo_size)
                        .rounded(design::ui_px(cx, 2.0)),
                )
            })
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    // No logo but the gutter is on: pad instead of adding an empty box, so a row
                    // whose brand is unknown still starts its name where its neighbours do without
                    // a second element in the layout tree. Same form the heading uses.
                    .when(chrome.logo.is_none() && chrome.logo_gutter, |text| {
                        text.pl(design::ui_px(cx, EXCHANGE_LOGO_SIZE + NAME_LOGO_GAP))
                    })
                    .child(name),
            ),
    )
    .child(numeric_cell(profit, chrome.profit_width, cx).text_color(moon(profit_color)))
    .when(show_trades, |element| {
        element.child(numeric_cell(trades, TRADES_COLUMN_WIDTH, cx))
    })
    .when(show_win, |element| {
        element.child(numeric_cell(
            win_rate.unwrap_or_default(),
            WIN_RATE_COLUMN_WIDTH,
            cx,
        ))
    })
    .when(show_average, |element| {
        element.child(numeric_cell(
            average_order.unwrap_or_default(),
            AVERAGE_ORDER_COLUMN_WIDTH,
            cx,
        ))
    })
}

/// Render one fixed-width numeric cell without allowing a value to create a second row.
///
/// Args:
///     text: Complete formatted value.
///     width: Design-reference column width.
///     cx: Application context used for UI scaling.
///
/// Returns:
///     Right-aligned, single-line, safely truncated cell.
fn numeric_cell(text: String, width: f32, cx: &App) -> Div {
    div()
        .w(design::ui_px(cx, width))
        .flex_none()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_align(TextAlign::Right)
        .child(text)
}

/// Return the profit column's design-reference width.
///
/// Args:
///     show_last: Whether the cell carries its `total(last)` suffix.
///
/// Returns:
///     Base width, plus the suffix allowance when the suffix is drawn.
pub(super) fn profit_column_width(show_last: bool) -> f32 {
    if show_last {
        PROFIT_COLUMN_WIDTH + PROFIT_LAST_TRADE_EXTRA
    } else {
        PROFIT_COLUMN_WIDTH
    }
}

/// Format profit with its exact comparable unit, optionally carrying the newest closed trade.
///
/// The suffix goes INSIDE the unit — `-57.11(-0.60) USDT`, not `-57.11 USDT (-0.60)` — so the two
/// amounts read as one measurement in one currency, which is what they are. Both are rounded to
/// the same unit decimals, so the bracket can never claim precision the total does not have.
///
/// The returned sign describes the TOTAL. The suffix is a different trade and may disagree; the
/// cell is coloured by the number it is about.
///
/// Args:
///     value: Projected profit.
///     last: Profit of the newest closed trade, when the suffix is enabled and one exists.
///     unit: Exact quote or percent unit.
///
/// Returns:
///     Signed compact text carrying its unit and the sign represented after display rounding.
pub(super) fn format_profit(
    value: f64,
    last: Option<f64>,
    unit: Option<ProfitUnit>,
) -> (String, DeltaSign) {
    let decimals = match unit {
        Some(ProfitUnit::Quote(currency)) => currency.display_decimals(),
        Some(ProfitUnit::Percent) | None => 2,
    };
    let (amount, sign) = moon_core::util::fmt::signed_amount(value, decimals);
    let amount = match last {
        Some(last) => {
            let (last, _) = moon_core::util::fmt::signed_amount(last, decimals);
            format!("{amount}({last})")
        }
        None => amount,
    };
    let text = match unit {
        Some(ProfitUnit::Quote(currency)) => format!("{amount} {}", currency.ticker()),
        Some(ProfitUnit::Percent) => format!("{amount}%"),
        None => amount,
    };
    (text, sign)
}

/// Format a monitor trade count with the terminal's shared thousands grouping.
///
/// Args:
///     value: Closed-trade count.
///
/// Returns:
///     ASCII digits separated into space-grouped thousands.
fn format_trade_count(value: i64) -> String {
    moon_core::util::fmt::group_thousands(&value.to_string())
}

/// Format win rate with the terminal's shared half-away-from-zero percentage rounding.
///
/// Args:
///     value: Win percentage in `0..=100`.
///
/// Returns:
///     Percentage with one decimal place.
fn format_win_rate(value: f64) -> String {
    moon_core::util::fmt::pct(value, 1)
        .map(|(text, _)| text)
        .unwrap_or_else(|| "0.0%".to_string())
}

/// Format average order spend in the query's comparable quote unit.
///
/// Args:
///     value: Average positive spend.
///     unit: Exact query unit.
///
/// Returns:
///     Compact unsigned order size with a quote ticker when known.
fn format_amount(value: f64, unit: Option<ProfitUnit>) -> String {
    match unit {
        Some(ProfitUnit::Quote(currency)) => format!(
            "{} {}",
            moon_core::util::fmt::compact(value, currency.display_decimals()),
            currency.ticker()
        ),
        Some(ProfitUnit::Percent) | None => moon_core::util::fmt::compact(value, 2),
    }
}
