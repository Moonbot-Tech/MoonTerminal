//! Everything the Profit Monitor DRAWS below its controls: the responsive table, its sortable
//! heading and total footer, the split-currency and placeholder states, and the value formatting
//! they share.
//!
//! Split from `mod.rs`, which keeps the view state, the refresh machinery and the window chrome.
//! The rule for the boundary is simple: nothing here reads or writes `ProfitMonitorView` state —
//! every function takes what it needs and returns an element.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::db::ProfitUnit;
use moon_core::session::CoreId;
use moon_ui::{
    MoonAlert, MoonPalette, MoonScrollbarVisibility, MoonVirtualList, MoonVirtualListScrollHandle,
    h_flex, v_flex,
};
use rust_i18n::t;

use super::format::{
    format_amount, format_profit, format_trade_count, format_win_rate, profit_column_width,
};
use super::line::{
    RowChrome, RowRole, RowSelect, row_h_px, row_h_value, row_id, section_header, table_row,
};
use super::rows::MonitorRow;
use super::sections::MonitorEntry;
use super::settings::MonitorPrefs;
use super::{
    AVERAGE_ORDER_COLUMN_WIDTH, EXCHANGE_LOGO_SIZE, MIN_NAME_COLUMN_WIDTH, MonitorLayout,
    MonitorSort, MonitorSortColumn, NAME_LOGO_GAP, ProfitMonitorView, TABLE_COLUMN_GAP,
    TABLE_HORIZONTAL_PADDING, TRADES_COLUMN_WIDTH, WIN_RATE_COLUMN_WIDTH, sort_arrow,
};
use crate::design;
use crate::design::{moon, moon_alpha};
use crate::media::exchange_logos::exchange_logo;
use moon_core::venue::Brand;

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
///     entries: Already sectioned, sorted display entries — captions, rows and subtotals.
///     total: The window's own fold, counting every core exactly once.
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
    entries: Vec<MonitorEntry>,
    total: MonitorRow,
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
    let sectioned = entries
        .iter()
        .any(|entry| matches!(entry, MonitorEntry::Header(_)));
    // Both halves have to agree: the preference asks for the suffix, the width decides whether the
    // column can hold it. Anything narrower would truncate a money value instead of dropping it.
    let show_last = prefs.last_trade && layout.last_trade;
    let profit_width = profit_column_width(show_last);
    // Resolved once per render rather than inside the row builder — that closure runs for every
    // visible row on every frame, and a lookup there would take the logo cache's global lock at
    // frame rate — and once per distinct EXCHANGE rather than once per row: two hundred cores on
    // one exchange are two hundred identical answers, and the resolver allocates a string for each.
    // One pass over the entries resolves both decorations. Doing either inside the virtual-list
    // item builder would repeat it for every visible row on every frame — and the highlight can
    // drive that at 10 Hz — while a merged Exchange row would pay one hash lookup per core it
    // contains. A caption or a subtotal carries neither.
    let flashes: Vec<Option<Instant>> = entries
        .iter()
        .map(|entry| match entry {
            MonitorEntry::Row { row, .. } => {
                row.cores.iter().filter_map(|core| flash.get(core)).max()
            }
            MonitorEntry::Header(_) | MonitorEntry::Subtotal { .. } => None,
        })
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
    //
    // A group caption is a click target too, standing for every configured member — the same
    // promise an exchange row makes. A subtotal is not: it is a fold, and "filter to this fold"
    // would mean whatever its section happens to hold right now.
    let owner = view.downgrade();
    let selects: Vec<Option<RowSelect>> = if prefs.core_filter {
        entries
            .iter()
            .map(|entry| {
                let cores = match entry {
                    MonitorEntry::Row { row, .. } => row.filter_cores.clone(),
                    MonitorEntry::Header(head) => head.cores.clone(),
                    MonitorEntry::Subtotal { .. } => return None,
                };
                Some(RowSelect {
                    selected: !selection.is_empty()
                        && !cores.is_empty()
                        && cores.iter().all(|core| selection.contains(core)),
                    cores,
                    owner: owner.clone(),
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let logos: Vec<Option<Arc<RenderImage>>> = if prefs.exchange_icons {
        let mut resolved: HashMap<Brand, Option<Arc<RenderImage>>> = HashMap::new();
        entries
            .iter()
            .map(|entry| {
                let MonitorEntry::Row { row, .. } = entry else {
                    return None;
                };
                let brand = row.venue.as_ref()?.brand()?;
                resolved
                    .entry(brand)
                    .or_insert_with(|| exchange_logo(brand))
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
    let entries = Arc::new(entries);
    let row_count = entries.len();
    let list_entries = entries.clone();
    let row_height = row_h_value(cx);
    let body = MoonVirtualList::new(
        "profit-monitor-rows",
        row_count,
        row_height,
        move |index, _window, app| {
            let Some(entry) = list_entries.get(index) else {
                return div().into_any_element();
            };
            let select = selects.get(index).cloned().flatten();
            let is_selected = select.as_ref().is_some_and(|select| select.selected);
            let (row, name, stripe, id) = match entry {
                MonitorEntry::Header(head) => {
                    return section_header(
                        head,
                        prefs.exchange_icons,
                        select.filter(|select| !select.cores.is_empty()),
                        is_selected,
                        palette,
                        app,
                    );
                }
                MonitorEntry::Row {
                    row,
                    stripe,
                    occurrence,
                } => (
                    row,
                    row.name.clone(),
                    *stripe,
                    row_id(*occurrence, index, row),
                ),
                MonitorEntry::Subtotal {
                    label,
                    row,
                    section,
                } => (
                    row,
                    label.clone(),
                    false,
                    ("profit-monitor-subtotal", *section as u64).into(),
                ),
            };
            let subtotal = matches!(entry, MonitorEntry::Subtotal { .. });
            let subtotal_tooltip = subtotal.then(|| name.clone());
            let (profit, profit_sign) =
                format_profit(row.profit, row.last_profit.filter(|_| show_last), unit);
            table_row(
                name,
                profit,
                profit_sign,
                format_trade_count(row.trades),
                // Empty, not `0.0%` and not `0.00 USDT`, when the ratio has no denominator: the
                // one definition lives on the row itself.
                row.win_rate().map(format_win_rate),
                row.average_order().map(|value| format_amount(value, unit)),
                show_trades,
                show_win,
                show_average,
                RowChrome {
                    id,
                    logo: logos.get(index).cloned().flatten(),
                    logo_gutter: prefs.exchange_icons,
                    role: if subtotal {
                        RowRole::SectionSubtotal
                    } else if sectioned {
                        RowRole::SectionMember
                    } else {
                        RowRole::Plain
                    },
                    flash: flashes.get(index).copied().flatten(),
                    profit_width,
                    select,
                },
                palette,
                app,
            )
            .when_some(subtotal_tooltip, |element, label| {
                element.tooltip(crate::panels::common::text_tooltip(label))
            })
            // Weaker than the grand-total footer: the muted surface, body text size and thin closing
            // rule keep this summary inside its group instead of presenting a competing answer.
            .when(subtotal, |element| {
                element
                    .bg(moon_alpha(palette.table_head, 0.65))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(moon(palette.text_soft))
            })
            // Selection outranks the zebra stripe: it is the answer to "which cores is the main
            // window showing", and a stripe drawn over it would make every other selected row look
            // unselected. It is drawn in BLUE rather than in `table_selected`, which the arrival
            // tint already owns here — one colour for both would swallow the new-trade flash on
            // exactly the rows the user is watching. Same pairing the tuner's coin list uses.
            .when(is_selected, |element| {
                element.bg(moon_alpha(palette.blue, 0.18))
            })
            .when(!is_selected && stripe, |element| {
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
        t!("profit_monitor.grand_total").to_string(),
        total_profit,
        total_profit_sign,
        format_trade_count(total.trades),
        total.win_rate().map(format_win_rate),
        total
            .average_order()
            .map(|value| format_amount(value, unit)),
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
            role: RowRole::Plain,
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
        .h(row_h_px(cx))
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
