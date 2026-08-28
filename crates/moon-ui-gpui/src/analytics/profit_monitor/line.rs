//! One drawn line of the Profit Monitor table: a data row, a group caption, or a summary footer.
//!
//! Split from `table.rs`, which assembles the header, the virtualized body and the footer out of
//! these. Everything here is about ONE line — its shared frame and height, its element identity,
//! its click gesture and its cells.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::session::CoreId;
use moon_core::util::fmt::DeltaSign;
use moon_ui::{MoonPalette, h_flex};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use super::rows::MonitorRow;
use super::sections::SectionHead;
use super::{
    AVERAGE_ORDER_COLUMN_WIDTH, EXCHANGE_LOGO_SIZE, NAME_LOGO_GAP, ProfitMonitorView,
    TABLE_COLUMN_GAP, TABLE_HORIZONTAL_PADDING, TRADES_COLUMN_WIDTH, WIN_RATE_COLUMN_WIDTH,
    name_min_width,
};
use crate::controls::core_run::RunSlots;
use crate::design;
use crate::design::{moon, moon_alpha};

/// Space between a group's boundary rail and its heading.
const SECTION_LABEL_GAP: f32 = 8.0;
/// Additional inset that makes member names subordinate to their group heading.
const SECTION_MEMBER_INDENT: f32 = 14.0;

/// Visual role of a numeric table row within the group hierarchy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RowRole {
    /// A row in the unsectioned flat table.
    Plain,
    /// A core row nested under a visible group heading.
    SectionMember,
    /// The summary row that closes a multi-row group.
    SectionSubtotal,
}

/// Attach the core-filter gesture one clickable line carries.
///
/// Shared by the data rows and the group captions so the two cannot answer a click differently —
/// same pointer cursor, same additive modifier, same tolerance for a window that closed first.
///
/// Args:
///     element: The line receiving the gesture.
///     select: Cores it stands for and the monitor to publish through.
///
/// Returns:
///     The line with its click handler.
pub(super) fn attach_select(element: Stateful<Div>, select: RowSelect) -> Stateful<Div> {
    let RowSelect { cores, owner, .. } = select;
    element
        .cursor_pointer()
        .on_click(move |event: &ClickEvent, _window, app| {
            // secondary() = Ctrl on Windows/Linux, ⌘ on macOS — the same multi-select modifier the
            // tuner's coin list and the strategies tree use.
            let additive = event.modifiers().secondary();
            // The window may already be gone; a click on a closing view is not an error.
            let _ = owner.update(app, |this, cx| this.filter_to_cores(&cores, additive, cx));
        })
}

/// Return the height every line of the table takes, in scaled pixels.
///
/// One definition, because the virtual list is built on ONE fixed row height: the heading, the data
/// rows and the group captions must all agree with the value handed to `MoonVirtualList`, or every
/// line below a disagreeing one is drawn off its own slot.
///
/// Args:
///     cx: Application context supplying the UI scale.
///
/// Returns:
///     The shared row height.
pub(super) fn row_h_px(cx: &App) -> Pixels {
    design::fit_h_px(cx, 34.0, 13.0, 8.0)
}

/// Return the same height as the raw value the virtual list is measured in.
///
/// Args:
///     cx: Application context supplying the UI scale.
///
/// Returns:
///     The shared row height.
pub(super) fn row_h_value(cx: &App) -> f32 {
    design::fit_h_value(cx, 34.0, 13.0, 8.0)
}

/// Return one data row's element identity.
///
/// The core id, which survives re-sorting and regrouping — never the rendered text, which changes
/// with grouping mode, locale and a rename, and which two cores can share. A core saved into
/// several groups is drawn once per group on purpose, and gpui keys interactive state on this
/// identity, so every repeat takes the entry position instead: two live rows under one identity
/// would trade hover and press state every frame. Only the repeats pay the moving id, and the row a
/// user reaches from an unsectioned table keeps the stable one.
///
/// Args:
///     occurrence: How many times this core was already drawn above.
///     index: Position of this line in the table.
///     row: The row being drawn.
///
/// Returns:
///     An identity unique among the lines drawn in one frame.
pub(super) fn row_id(occurrence: usize, index: usize, row: &MonitorRow) -> ElementId {
    if occurrence == 0 {
        ("profit-monitor-row", row.primary_core).into()
    } else {
        ("profit-monitor-row-repeat", index as u64).into()
    }
}

/// Render one saved-group caption above the rows that belong to it.
///
/// A caption is a full-width line rather than a row with empty columns: it names a section, and
/// blank cells under Profit and Trades would read as a group that traded nothing. It keeps the data
/// rows' height because the virtual list is built on ONE row height — a taller caption would
/// misplace every row below it.
///
/// A quiet boundary rail and one-step-larger label make the caption the start of a nested block
/// without competing with the fixed grand-total footer.
///
/// Args:
///     head: The caption, the cores it stands for, and its stable position.
///     logo_gutter: Whether the rows below reserve room for an exchange logo.
///     select: Click payload, already `None` when the feature is off or no member is configured.
///     selected: Whether the filter currently holds every one of those cores.
///     palette: Active MoonUI palette.
///     cx: Render context.
///
/// Returns:
///     One fixed-height caption line.
pub(super) fn section_header(
    head: &SectionHead,
    logo_gutter: bool,
    run: Option<AnyElement>,
    select: Option<RowSelect>,
    selected: bool,
    palette: MoonPalette,
    cx: &App,
) -> AnyElement {
    let label_gap = SECTION_LABEL_GAP
        + if logo_gutter {
            EXCHANGE_LOGO_SIZE + NAME_LOGO_GAP
        } else {
            0.0
        };
    h_flex()
        .id(("profit-monitor-section", head.section as u64))
        .w_full()
        .h(row_h_px(cx))
        .px(design::ui_px(cx, TABLE_HORIZONTAL_PADDING))
        .gap(design::ui_px(cx, TABLE_COLUMN_GAP))
        .border_b(px(1.0))
        .border_color(moon_alpha(palette.border, 0.7))
        .bg(moon_alpha(palette.table_head, 0.8))
        // The SAME blue the rows use, for the same reason: `table_selected` belongs to the arrival
        // tint here, and one colour for both would make a selected section swallow the new-trade
        // flash of the rows it introduces.
        .when(selected, |caption| {
            caption.bg(moon_alpha(palette.blue, 0.18))
        })
        .text_size(design::t_body_lg(cx))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(moon(palette.text))
        .when_some(select, attach_select)
        .tooltip(crate::panels::common::text_tooltip(head.name.clone()))
        .children(run)
        .child(
            h_flex()
                .flex_1()
                .h_full()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .border_l(px(2.0))
                .border_color(moon_alpha(palette.border_soft, 0.9))
                // Padding rather than a spacer sibling keeps the caption inside the flexible Name
                // column and cannot move any numeric column at the minimum window width.
                .pl(design::ui_px(cx, label_gap))
                .child(head.name.clone()),
        )
        .into_any_element()
}

/// Per-row decoration that is not one of the row's own numbers.
///
/// Bundled rather than passed as positional flags so plain, grouped, subtotal, and grand-total
/// call sites cannot silently disagree about their hierarchy chrome.
pub(super) struct RowChrome {
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
    pub(super) id: ElementId,
    /// Exchange logo drawn before the name, when enabled and the brand is known.
    pub(super) logo: Option<Arc<RenderImage>>,
    /// Whether to reserve the logo's width even without one.
    ///
    /// With icons on, a row whose brand is unknown, the grand-total footer and the sortable heading
    /// all have to start where the logo-bearing rows' text starts, or the Name column no longer
    /// lines up with its own header.
    pub(super) logo_gutter: bool,
    /// Whether this row is flat data, a nested group member, or the group's closing subtotal.
    pub(super) role: RowRole,
    /// Instant this row's core closed its newest trade, while the highlight is still live.
    pub(super) flash: Option<Instant>,
    /// Profit-column width selected by the current last-trade decision.
    pub(super) profit_width: f32,
    /// What clicking this row does, or `None` when the preference is off.
    pub(super) select: Option<RowSelect>,
    /// The leading run cell, already built, or the empty reservation standing in for it.
    ///
    /// Built by the caller because it needs `Backend`, which nothing else in this module reads;
    /// `None` only when the run column is switched off entirely, and then no child is added at all
    /// — a zero-width element would still collect the row's own column gap.
    pub(super) run: Option<AnyElement>,
    /// Slots the table reserves, which the Name column's minimum is reduced by.
    pub(super) run_slots: RunSlots,
}

/// One row's participation in the terminal-wide core filter.
///
/// Present only while the preference is on, which is what makes the row inert — no pointer
/// cursor, no handler — for someone who turned the feature off, rather than merely silent.
/// Cloning is two refcount bumps, which is what lets the item builder take one per visible row.
#[derive(Clone)]
pub(super) struct RowSelect {
    /// Cores the row stands for, shared rather than copied per frame.
    pub(super) cores: Rc<[CoreId]>,
    /// Whether the filter currently holds all of them.
    pub(super) selected: bool,
    /// Monitor receiving the click; weak, because a click can outlive a closing window.
    pub(super) owner: WeakEntity<ProfitMonitorView>,
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
///     chrome: Hierarchy role, logo, arrival highlight, and profit-column width.
///     palette: Active MoonUI palette.
///     cx: Render context.
///
/// Returns:
///     One fixed-height table row.
#[allow(clippy::too_many_arguments)]
pub(super) fn table_row(
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
    let role = chrome.role;
    let logo_size = design::ui_px(cx, EXCHANGE_LOGO_SIZE);
    // The arrival tint is the News feed's, from the one shared definition: a full-bleed layer
    // declared BEFORE the cells so it sits under the text, and driven by the owner's own stamp
    // rather than by a GPUI animation, which would repaint the whole window at vblank for its
    // entire duration.
    let row = crate::pulse::with_arrival_tint(
        h_flex()
            .w_full()
            .relative()
            .h(row_h_px(cx))
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
    .when(role == RowRole::SectionSubtotal, |element| {
        element.border_t(px(1.0))
    })
    .when_some(chrome.select, attach_select);
    let run_slots = chrome.run_slots;
    row.children(chrome.run)
        .child(
            h_flex()
                .flex_1()
                .h_full()
                .min_w(design::ui_px(cx, name_min_width(run_slots)))
                .gap(design::ui_px(cx, NAME_LOGO_GAP))
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .when(role != RowRole::Plain, |name| {
                    name.border_l(px(2.0))
                        .border_color(moon_alpha(palette.border_soft, 0.9))
                        .pl(design::ui_px(cx, SECTION_MEMBER_INDENT))
                })
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
