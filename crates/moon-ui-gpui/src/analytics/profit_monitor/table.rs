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
    ColumnFloor, ColumnMetrics, ProfitColumn, ProfitLen, format_amount, format_profit,
    format_trade_count, format_win_rate, plan_profit_column, unit_ticker,
};
use super::line::{
    RowChrome, RowRole, RowSelect, row_h_px, row_h_value, row_id, section_header, table_row,
};
use super::rows::MonitorRow;
use super::sections::MonitorEntry;
use super::settings::MonitorPrefs;
use super::{
    AVERAGE_ORDER_COLUMN_WIDTH, EXCHANGE_LOGO_SIZE, MonitorLayout, MonitorSort, MonitorSortColumn,
    NAME_LOGO_GAP, PROFIT_MIN_COLUMN_WIDTH, ProfitMonitorView, TABLE_COLUMN_GAP,
    TABLE_HORIZONTAL_PADDING, TRADES_COLUMN_WIDTH, WIN_RATE_COLUMN_WIDTH, name_min_width,
    run_slots, sort_arrow,
};

use crate::Backend;
use crate::controls::core_run::{RunKey, RunScope, RunSlots, reserved_cell, run_cell};
use crate::design;
use crate::design::{moon, moon_alpha};
use crate::media::exchange_logos::exchange_logo;
use crate::workspace::scope_marker::ScopeMarker;
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
///     scope_marker: This surface's marker, built from the same live context the query used. This
///         arm carried no marker at all before §4.3 — its totals are scoped exactly like the table's
///         now, and it needs one just as much.
///     palette: Active MoonUI palette.
///     cx: Render context.
///
/// Returns:
///     Explanation and quote chips, with the aggregate trade count when space permits and a soft
///     scope caption when the active preset hides at least one configured core.
pub(super) fn split_body(
    totals: &moon_core::db::QuoteBreakdown,
    show_trades: bool,
    scope_marker: ScopeMarker,
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
        .when(scope_marker.hides_anything(), |body| {
            // The same facts the Ready footer states, AND the same recovery hint: the marker's
            // contract is that a surface which states its scope also says how to widen it, and a
            // Split body states scoped money exactly as a Ready one does.
            let facts = scope_marker.facts();
            body.child(
                div()
                    .id("pm-split-scope-marker")
                    .text_color(moon(palette.text_soft))
                    .child(facts.join(" "))
                    .tooltip(crate::panels::common::text_tooltip(
                        scope_marker.tooltip(&facts),
                    )),
            )
        })
        .into_any_element()
}

/// Return the profit heading, which names the unit exactly when the cells stopped printing it.
///
/// Args:
///     unit: Comparable exact unit, or `None` for an empty result.
///     ticker_shown: Whether the cells below still carry their ticker.
///
/// Returns:
///     Heading text for the profit column.
fn profit_heading(unit: Option<ProfitUnit>, ticker_shown: bool) -> String {
    match unit_ticker(unit).filter(|_| !ticker_shown) {
        Some(ticker) => t!("profit_monitor.column.profit_with_unit", unit = ticker).to_string(),
        None => t!("profit_monitor.column.profit").to_string(),
    }
}

/// Return the sort arrow every heading budget reserves.
///
/// Reserved whether or not this column carries the arrow right now: sorting by the column would
/// otherwise widen its heading, and the whole table would step sideways on a heading click. Taken
/// from `sort_arrow`, which draws it, so the reserved glyph cannot drift from the drawn one; both
/// arrows are one glyph after one space, so either serves as the measured stand-in.
///
/// Returns:
///     The descending arrow, leading space included.
fn sort_arrow_reserve() -> &'static str {
    sort_arrow(
        Some(MonitorSort {
            column: MonitorSortColumn::Profit,
            descending: true,
        }),
        MonitorSortColumn::Profit,
    )
}

/// Return the design-reference width one heading occupies, sort arrow included.
///
/// MEASURED rather than counted in monospace cells: the headings are localized and the sort arrow
/// is not a Latin glyph, so either can resolve through a fallback face whose advance is not the
/// digit advance the values are counted in.
///
/// Args:
///     title: Heading text.
///     scale: Active UI geometry scale the fixed widths are stated against.
///     cx: Application context used to measure text.
///
/// Returns:
///     Design-reference width the heading needs.
fn heading_width(title: &str, scale: f32, cx: &App) -> f32 {
    let text = format!("{title}{}", sort_arrow_reserve());
    design::mono_body_text_width(cx, &text, FontWeight::NORMAL.0) / scale
}

/// Return the room the profit column may take before the name column drops below its minimum.
///
/// Args:
///     layout: Responsive selection of the fixed columns to its right.
///     slots: Run-control slots reserved on the left of every line.
///     width: Current window width in rendered pixels.
///     scale: Active UI geometry scale the fixed widths are stated against.
///
/// Returns:
///     Design-reference width, never below the floor the window promises the column.
fn available_width(layout: MonitorLayout, slots: RunSlots, width: f32, scale: f32) -> f32 {
    let mut used = 2.0 * TABLE_HORIZONTAL_PADDING + TABLE_COLUMN_GAP + name_min_width(slots);
    if slots.any() {
        used += slots.width() + TABLE_COLUMN_GAP;
    }
    for (shown, column) in [
        (layout.trades, TRADES_COLUMN_WIDTH),
        (layout.win_rate, WIN_RATE_COLUMN_WIDTH),
        (layout.average_order, AVERAGE_ORDER_COLUMN_WIDTH),
    ] {
        if shown {
            used += column + TABLE_COLUMN_GAP;
        }
    }
    // The floor is a backstop, not a reservation: `MIN_WINDOW_WIDTH` is sized so a legal window
    // always leaves more than this, and everything above the column's measured need already goes to
    // the name. It only bites if a window somehow gets narrower than the OS minimum, and then a
    // readable amount outranks a longer name.
    (width / scale - used).max(PROFIT_MIN_COLUMN_WIDTH)
}

/// Everything one profit-column measurement needs from the view.
///
/// A value rather than ten positional arguments, half of which travel together while the rest are
/// derived from `prefs`: the caller states what is on screen, and the measurement answers with a
/// column.
pub(super) struct ColumnRequest<'a> {
    /// Display lines the table is about to draw.
    pub(super) entries: &'a [MonitorEntry],
    /// The window fold drawn in the footer, at its own larger type step.
    pub(super) total: &'a MonitorRow,
    /// Comparable exact unit shared by every value in the column.
    pub(super) unit: Option<ProfitUnit>,
    /// Display preferences, which decide the suffix and the run-control slots.
    pub(super) prefs: MonitorPrefs,
    /// Responsive selection of the other columns.
    pub(super) layout: MonitorLayout,
    /// Current window width in rendered pixels.
    pub(super) width: f32,
    /// Active UI geometry scale, resolved once by the caller that also sized `layout`.
    pub(super) scale: f32,
    /// How far the column was already pushed, and what it was pushed under.
    pub(super) floor: ColumnFloor,
}

/// Size the profit column from the values this snapshot actually holds.
///
/// Measured, not assumed: a column sized for a worst case nobody is showing spends the name
/// column's room on digits that are never drawn, and the name is what truncates. Every line the
/// table will draw is measured — the footer on its own larger type step — and the widest form that
/// fits the remaining room wins.
///
/// Args:
///     request: The lines to measure, the room to measure them against, and the period floor.
///     cx: Application context used to measure glyph advances.
///
/// Returns:
///     The chosen column, and the floor to carry into the next measurement.
pub(super) fn profit_column(request: ColumnRequest<'_>, cx: &App) -> (ProfitColumn, ColumnFloor) {
    let ColumnRequest {
        entries,
        total,
        unit,
        prefs,
        layout,
        width,
        scale,
        floor,
    } = request;
    let want_suffix = prefs.last_trade;
    let slots = run_slots(prefs);
    // ONE advance, measured once and multiplied: the whole window is monospaced, so every glyph in
    // this column is the same width. Measured rather than assumed because the Font slider moves the
    // text without moving the design units the columns are stated in. The heavier of the two row
    // weights wins, because a section subtotal draws its amount semibold in the same column.
    let row_char = design::mono_body_text_width(cx, "0", FontWeight::NORMAL.0).max(
        design::mono_body_text_width(cx, "0", FontWeight::SEMIBOLD.0),
    ) / scale;
    let total_char = design::mono_title_text_width(cx, "0", FontWeight::SEMIBOLD.0) / scale;
    let mut rows = ProfitLen::default();
    for entry in entries {
        let row = match entry {
            MonitorEntry::Row { row, .. } | MonitorEntry::Subtotal { row, .. } => row,
            MonitorEntry::Header(_) => continue,
        };
        rows.absorb(ProfitLen::measure(
            row.profit,
            row.last_profit.filter(|_| want_suffix),
            unit,
        ));
    }
    let ticker = unit_ticker(unit);
    let heading = heading_width(&profit_heading(unit, true), scale, cx);
    let metrics = ColumnMetrics {
        row_char,
        total_char,
        heading,
        // Only the ticker-less rungs are drawn under it, so a unit with no ticker neither builds
        // nor measures a second heading it can never reach.
        heading_with_unit: match ticker {
            Some(_) => heading_width(&profit_heading(unit, false), scale, cx),
            None => heading,
        },
        ticker: ticker.map_or(0, |ticker| ticker.chars().count()),
        available: available_width(layout, slots, width, scale),
    };
    let column = plan_profit_column(
        rows,
        ProfitLen::measure(
            total.profit,
            total.last_profit.filter(|_| want_suffix),
            unit,
        ),
        want_suffix,
        &metrics,
        floor.carried(unit, &metrics),
    );
    (column, ColumnFloor::taken(unit, &metrics, column))
}

/// Render the responsive monitor table and exact total footer.
///
/// Args:
///     entries: Already sectioned, sorted display entries — captions, rows and subtotals.
///     total: The window's own fold, counting every core exactly once.
///     unit: Comparable exact unit, or `None` for an empty result.
///     column: Profit column already sized from this snapshot by [`profit_column`].
///     layout: Responsive presentation selected from the current width.
///     sort: Explicit user-selected ordering, if any.
///     prefs: Display preferences chosen in the ⚙ popup.
///     flash: Live arrival stamps, keyed by the core that closed the trade.
///     selection: Cores currently broadcast to the main window; empty means no filter.
///     scroll: Retained vertical-list position.
///     scope_marker: This surface's marker, built from the same live context the scoped query used.
///         Its facts are spliced into the grand-total footer's own label as visible text, and the
///         same facts back the footer's tooltip — a tooltip alone is not a scope statement.
///     action_cores: Cores the header's own FLEET run cell may command, independent of the active
///         preset's display narrowing — see `LiveContext::action_core_ids`. Never the union of the
///         (possibly display-scoped) per-row `run_scopes` below, or a hidden core's row taking its
///         own button with it would also narrow this table-wide cell, which commands rather than
///         merely reports.
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
    column: ProfitColumn,
    layout: MonitorLayout,
    sort: Option<MonitorSort>,
    prefs: MonitorPrefs,
    flash: &crate::pulse::Arrivals<CoreId>,
    selection: &HashSet<CoreId>,
    scroll: &MoonVirtualListScrollHandle,
    scope_marker: ScopeMarker,
    action_cores: &[CoreId],
    palette: MoonPalette,
    view: Entity<ProfitMonitorView>,
    backend: Entity<Backend>,
    cx: &App,
) -> AnyElement {
    let show_trades = layout.trades;
    let show_win = layout.win_rate;
    let show_average = layout.average_order;
    let sectioned = entries
        .iter()
        .any(|entry| matches!(entry, MonitorEntry::Header(_)));
    // Both halves already agreed when the column was sized: the preference asks for the suffix and
    // the measurement decides whether it fits. Anything narrower would truncate a money value
    // instead of dropping it.
    let form = column.form;
    let profit_width = column.width;
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
                let cores = entry.scope_cores()?;
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
    // Resolved once per render like the logos and the click payloads, and for the same reason: the
    // item builder runs for every visible line on every frame. A line's scope is its FILTER
    // payload, not the cores that traded — an Exchange row commands every configured core of that
    // exchange, and a Core row commands exactly its own, which is also what makes the restart
    // button available to a row and not to a caption. Skipped entirely while the column is off.
    let slots = run_slots(prefs);
    let run_scopes: Vec<Option<RunScope>> = if slots.any() {
        entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let cores = entry.scope_cores()?;
                let (key, offers) = match entry {
                    // A core saved into several groups is drawn once per group, so only its first
                    // line may key on the core itself — exactly what `row_id` does for the row.
                    MonitorEntry::Row {
                        row, occurrence: 0, ..
                    } => (RunKey::Core(row.primary_core), slots),
                    MonitorEntry::Row { .. } => (RunKey::Repeat(index), slots),
                    // A caption carries the CHOSEN controls, or none of them: the group preference
                    // says where the controls the user picked also appear, and that includes the
                    // status dot — a caption commanding nothing must not keep a column of its own.
                    MonitorEntry::Header(head) => (
                        RunKey::Section(head.section),
                        if prefs.group_controls {
                            slots
                        } else {
                            RunSlots::default()
                        },
                    ),
                    MonitorEntry::Subtotal { .. } => return None,
                };
                (!cores.is_empty()).then_some(RunScope {
                    key,
                    cores,
                    reserve: slots,
                    offers,
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
    // The heading's own run cell commands EVERY core its ACTION authority names, never only the
    // ones the display scope happened to leave in `entries` — see `action_cores`'s own doc. Built
    // here, where the entries are still the plain vector, and passed in already rendered: the
    // heading itself owns no backend and must not learn to.
    let fleet = fleet_scope(action_cores, slots, prefs.header_controls)
        .and_then(|scope| run_cell(&scope, &backend, palette, cx))
        .or_else(|| reserved_cell(slots, cx));
    let header = table_header(
        layout,
        profit_heading(unit, form.ticker),
        profit_width,
        prefs.exchange_icons,
        slots,
        fleet,
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
            let run = run_scopes
                .get(index)
                .and_then(|scope| scope.as_ref())
                .and_then(|scope| run_cell(scope, &backend, palette, app))
                .or_else(|| reserved_cell(slots, app));
            let select = selects.get(index).cloned().flatten();
            let is_selected = select.as_ref().is_some_and(|select| select.selected);
            let (row, name, stripe, id) = match entry {
                MonitorEntry::Header(head) => {
                    return section_header(
                        head,
                        prefs.exchange_icons,
                        run,
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
            let (profit, profit_sign) = format_profit(row.profit, row.last_profit, unit, form);
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
                    run,
                    run_slots: slots,
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
        format_profit(total.profit, total.last_profit, unit, form);
    // Visible statement, not only a tooltip: a hidden marker never told anyone their money was
    // scoped. Clips LAST, after the money, by riding the footer's own existing name cell — the
    // same tail order the Report's footer already uses for its own facts.
    let scope_facts = scope_marker.facts();
    let grand_total_label = if scope_facts.is_empty() {
        t!("profit_monitor.grand_total").to_string()
    } else {
        format!(
            "{} {}",
            t!("profit_monitor.grand_total"),
            scope_facts.join(" ")
        )
    };
    let footer = table_row(
        grand_total_label,
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
            // The total commands nothing. Every core at once is what the HEADING offers, under its
            // own preference and without a restart; repeating it on a fold that is only a sum would
            // put two "all cores" controls in one window. It keeps the reserved width, or its label
            // stops lining up with the names above it, exactly like the logo gutter beside it.
            run: reserved_cell(slots, cx),
            run_slots: slots,
        },
        palette,
        cx,
    )
    .h(design::fit_h_px(cx, 42.0, 14.0, 10.0))
    .bg(moon(palette.table_head))
    .text_size(design::t_title(cx))
    .font_weight(FontWeight::SEMIBOLD)
    .border_t(px(2.0))
    .border_color(moon_alpha(palette.amber, 0.7))
    .when(scope_marker.hides_anything(), |element| {
        element.tooltip(crate::panels::common::text_tooltip(
            scope_marker.tooltip(&scope_facts),
        ))
    });

    v_flex()
        .flex_1()
        .min_h_0()
        .w_full()
        .child(header)
        .child(div().flex_1().min_h_0().w_full().child(body))
        .child(footer)
        .into_any_element()
}

/// Build the scope of the heading's own run cell: every core its ACTION authority names.
///
/// The one cell in the window that acts on the whole table, which is why it takes its OWN input
/// rather than the per-line pass's: unioning the (possibly display-scoped) row scopes would let the
/// active preset narrow this table-wide COMMAND cell the moment it hides a core that traded — see
/// `LiveContext::action_core_ids`'s own doc for why that is a distinct failure from a hidden row
/// losing its own button, which is fine. `action_cores` already comes deduplicated, one entry per
/// configured server.
///
/// Restart is deliberately withheld — "restart the fleet" is not an action this window offers, and
/// the scope's own single-core guard would still hand it over on a table holding exactly one core.
/// The status slot draws its folded dot, which reports rather than commands.
///
/// Args:
///     action_cores: Every core the header cell may command, resolved once per body build.
///     slots: Slots the table reserves, which decide what the cell may fill.
///     enabled: The `header_controls` preference; the cell exists only when it is on.
///
/// Returns:
///     The heading's scope, or `None` when the table commands nothing — the preference is off, no
///     control slot is reserved, or no core is in scope.
fn fleet_scope(action_cores: &[CoreId], slots: RunSlots, enabled: bool) -> Option<RunScope> {
    if !enabled || !(slots.trading || slots.auto) || action_cores.is_empty() {
        return None;
    }
    Some(RunScope {
        key: RunKey::Fleet,
        cores: action_cores.into(),
        reserve: slots,
        // Every reserved slot, the status dot included: this cell exists only because the heading
        // preference asked for it. The restart button is withheld by `allows_restart` alone.
        offers: slots,
    })
}

/// Render the fixed clickable header whose geometry mirrors the data rows.
///
/// Args:
///     layout: Responsive presentation including visible-column selection.
///     profit_title: Profit heading, which names the unit when the cells stopped printing it.
///     profit_width: Profit-column width the data rows are using.
///     logo_gutter: Whether the data rows reserve room for an exchange logo.
///     run_slots: Slots every line of the table reserves.
///     run_cell: The heading's own run cell, already rendered — the table-wide controls, or the
///         reserved width with nothing in it.
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
    profit_title: String,
    profit_width: f32,
    logo_gutter: bool,
    run_slots: RunSlots,
    run_cell: Option<AnyElement>,
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
                .min_w(design::ui_px(cx, name_min_width(run_slots)))
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
        // The run column has no sortable heading: it is a control, not a value — and here it is
        // literally one, commanding every core in the table. Either way it keeps its width, so the
        // Name heading starts where the names below it do.
        .children(run_cell)
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
            profit_title,
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
