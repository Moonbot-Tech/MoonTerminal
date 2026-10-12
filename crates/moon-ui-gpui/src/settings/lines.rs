//! Order lines page: a table of the ten lines, the selected line's card, and the settings shared
//! by every line. Names and labels come from `locales/<lang>/lines.<lang>.yml`. Changes update the
//! live-preview draft and are saved to `orders.toml`. [`Lines`] stores the editor controls,
//! [`LinesUi`] the selected row.

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonCheckbox, MoonColorPickerState, MoonPalette, MoonSliderState, h_flex, rgba_from, v_flex,
};

use super::SettingsView;
use super::common::num_input;
use super::interface::slider_ctl;
use crate::Backend;
use crate::design;
use moon_core::config::{LineStyle, OrdersStyle};
use moon_core::util::fmt;
use rust_i18n::t;

/// Editor controls for one order line; marker and opacity controls are optional by line type.
struct LineEd {
    color: Entity<MoonColorPickerState>,
    thickness: Entity<MoonSliderState>,
    marker_size: Entity<MoonSliderState>,
    marker_thickness: Entity<MoonSliderState>,
    knot_size: Entity<MoonSliderState>,
    /// Opacity for unfilled orders (`fill == 0`), shown only for `buy` and `buy_short`.
    pending_alpha: Entity<MoonSliderState>,
    /// Color for unfilled orders (`fill == 0`), shown only for `buy` and `buy_short`.
    pending_color: Entity<MoonColorPickerState>,
}

/// Build a colour picker for a field in the active colour set's draft `OrdersStyle`.
fn ord_color(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    get: fn(&OrdersStyle) -> [u8; 3],
    set: fn(&mut OrdersStyle, [u8; 3]),
) -> Entity<MoonColorPickerState> {
    let cur = {
        let b = backend.read(cx);
        let is_light = b
            .preview
            .as_ref()
            .unwrap_or(&b.config)
            .ui_theme_mode
            .is_light();
        get(b.preview.as_ref().unwrap_or(&b.config).orders.get(is_light))
    };
    super::draft_color(window, cx, cur, move |p, c| {
        let is_light = p.ui_theme_mode.is_light();
        if get(p.orders.get(is_light)) != c {
            set(p.orders.get_mut(is_light), c);
            true
        } else {
            false
        }
    })
}

/// Build a slider for an `f32` field in the active colour set's draft `OrdersStyle`.
#[allow(clippy::too_many_arguments)]
fn ord_slider(
    backend: &Entity<Backend>,
    cx: &mut Context<SettingsView>,
    get: fn(&OrdersStyle) -> f32,
    set: fn(&mut OrdersStyle, f32),
    min: f32,
    max: f32,
    step: f32,
) -> Entity<MoonSliderState> {
    let cur = {
        let b = backend.read(cx);
        let is_light = b
            .preview
            .as_ref()
            .unwrap_or(&b.config)
            .ui_theme_mode
            .is_light();
        get(b.preview.as_ref().unwrap_or(&b.config).orders.get(is_light))
    };
    super::draft_slider(cx, min, max, step, cur, move |p, f, _bcx| {
        let is_light = p.ui_theme_mode.is_light();
        if get(p.orders.get(is_light)) != f {
            set(p.orders.get_mut(is_light), f);
            true
        } else {
            false
        }
    })
}

/// Build a [`LineEd`] for an `OrdersStyle` line field using function-pointer accessors.
macro_rules! line_ed {
    ($b:expr, $w:expr, $cx:expr, $line:ident) => {
        LineEd {
            color: ord_color($b, $w, $cx, |o| o.$line.color, |o, v| o.$line.color = v),
            thickness: ord_slider(
                $b,
                $cx,
                |o| o.$line.thickness,
                |o, v| o.$line.thickness = v,
                0.5,
                6.0,
                0.1,
            ),
            marker_size: ord_slider(
                $b,
                $cx,
                |o| o.$line.marker_size,
                |o, v| o.$line.marker_size = v,
                2.0,
                24.0,
                0.5,
            ),
            marker_thickness: ord_slider(
                $b,
                $cx,
                |o| o.$line.marker_thickness,
                |o, v| o.$line.marker_thickness = v,
                0.5,
                5.0,
                0.1,
            ),
            knot_size: ord_slider(
                $b,
                $cx,
                |o| o.$line.knot_size,
                |o, v| o.$line.knot_size = v,
                1.0,
                10.0,
                0.5,
            ),
            pending_alpha: ord_slider(
                $b,
                $cx,
                |o| o.$line.pending_alpha,
                |o, v| o.$line.pending_alpha = v,
                0.0,
                1.0,
                0.01,
            ),
            pending_color: ord_color(
                $b,
                $w,
                $cx,
                |o| o.$line.pending_color.unwrap_or(o.$line.color),
                |o, v| o.$line.pending_color = Some(v),
            ),
        }
    };
}

/// Order-line editor state.
pub(super) struct Lines {
    buy: LineEd,
    buy_short: LineEd,
    sell: LineEd,
    sell_short: LineEd,
    stop: LineEd,
    trailing: LineEd,
    take_profit: LineEd,
    vstop: LineEd,
    pending_cond: LineEd,
    liq: LineEd,
    path_color: Entity<MoonColorPickerState>,
    path_thickness: Entity<MoonSliderState>,
    /// `OrdersStyle::trace_alpha`: the path's opacity, edited beside the rest of the path.
    path_alpha: Entity<MoonSliderState>,
    active_alpha: Entity<MoonSliderState>,
    closed_alpha: Entity<MoonSliderState>,
}

/// Build the order-line editor from the current draft for `SettingsView::new`.
pub(super) fn build(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> Lines {
    // Edit the draft's active colour set. Changing the mode rebuilds these controls so their
    // retained widget state cannot keep editing the previous set.
    Lines {
        buy: line_ed!(backend, window, cx, buy),
        buy_short: line_ed!(backend, window, cx, buy_short),
        sell: line_ed!(backend, window, cx, sell),
        sell_short: line_ed!(backend, window, cx, sell_short),
        stop: line_ed!(backend, window, cx, stop),
        trailing: line_ed!(backend, window, cx, trailing),
        take_profit: line_ed!(backend, window, cx, take_profit),
        vstop: line_ed!(backend, window, cx, vstop),
        pending_cond: line_ed!(backend, window, cx, pending_cond),
        liq: line_ed!(backend, window, cx, liq),
        path_color: ord_color(
            backend,
            window,
            cx,
            |o| o.path.color,
            |o, v| o.path.color = v,
        ),
        path_thickness: ord_slider(
            backend,
            cx,
            |o| o.path.thickness,
            |o, v| o.path.thickness = v,
            0.5,
            6.0,
            0.1,
        ),
        path_alpha: ord_slider(
            backend,
            cx,
            |o| o.trace_alpha,
            |o, v| o.trace_alpha = v,
            0.05,
            1.0,
            0.01,
        ),
        active_alpha: ord_slider(
            backend,
            cx,
            |o| o.active_alpha,
            |o, v| o.active_alpha = v,
            0.05,
            1.0,
            0.01,
        ),
        closed_alpha: ord_slider(
            backend,
            cx,
            |o| o.closed_alpha,
            |o, v| o.closed_alpha = v,
            0.0,
            1.0,
            0.01,
        ),
    }
}

/// One row of the lines table: the `OrdersStyle` field it edits and how it is presented.
pub(super) struct LineKind {
    /// The `OrdersStyle` field (and `orders.toml` key) of this line.
    pub(super) key: &'static str,
    /// Locale key of the line's name.
    pub(super) name: &'static str,
    /// Whether the line has end crosses and knots (the liquidation line is continuous).
    pub(super) markers: bool,
    /// Whether the line has a separate look for an order that is not filled yet (entries only).
    pub(super) pending: bool,
}

/// The ten order lines, in table order.
pub(super) const LINE_KINDS: [LineKind; 10] = [
    LineKind {
        key: "buy",
        name: "lines.name.buy_long",
        markers: true,
        pending: true,
    },
    LineKind {
        key: "buy_short",
        name: "lines.name.buy_short",
        markers: true,
        pending: true,
    },
    LineKind {
        key: "sell",
        name: "lines.name.sell_long",
        markers: true,
        pending: false,
    },
    LineKind {
        key: "sell_short",
        name: "lines.name.sell_short",
        markers: true,
        pending: false,
    },
    LineKind {
        key: "stop",
        name: "lines.name.stop",
        markers: true,
        pending: false,
    },
    LineKind {
        key: "trailing",
        name: "lines.name.trailing",
        markers: true,
        pending: false,
    },
    LineKind {
        key: "take_profit",
        name: "lines.name.take_profit",
        markers: true,
        pending: false,
    },
    LineKind {
        key: "vstop",
        name: "lines.name.vstop",
        markers: true,
        pending: false,
    },
    LineKind {
        key: "pending_cond",
        name: "lines.name.pending",
        markers: true,
        pending: false,
    },
    LineKind {
        key: "liq",
        name: "lines.name.liquidation",
        markers: false,
        pending: false,
    },
];

/// The style of table row `idx` (clamped to the last row).
pub(super) fn line_style(o: &OrdersStyle, idx: usize) -> &LineStyle {
    match idx {
        0 => &o.buy,
        1 => &o.buy_short,
        2 => &o.sell,
        3 => &o.sell_short,
        4 => &o.stop,
        5 => &o.trailing,
        6 => &o.take_profit,
        7 => &o.vstop,
        8 => &o.pending_cond,
        _ => &o.liq,
    }
}

/// The style of table row `idx`, for writing (clamped to the last row).
pub(super) fn line_style_mut(o: &mut OrdersStyle, idx: usize) -> &mut LineStyle {
    match idx {
        0 => &mut o.buy,
        1 => &mut o.buy_short,
        2 => &mut o.sell,
        3 => &mut o.sell_short,
        4 => &mut o.stop,
        5 => &mut o.trailing,
        6 => &mut o.take_profit,
        7 => &mut o.vstop,
        8 => &mut o.pending_cond,
        _ => &mut o.liq,
    }
}

impl Lines {
    /// The editor of table row `idx` (clamped to the last row).
    fn ed(&self, idx: usize) -> &LineEd {
        match idx {
            0 => &self.buy,
            1 => &self.buy_short,
            2 => &self.sell,
            3 => &self.sell_short,
            4 => &self.stop,
            5 => &self.trailing,
            6 => &self.take_profit,
            7 => &self.vstop,
            8 => &self.pending_cond,
            _ => &self.liq,
        }
    }
}

/// `text` with its first letter upper-cased: the attribute labels are written lower-case for
/// use inside sentences.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// A line sample: the stroke in its colour, width and dash, with a cross at each marked end.
fn line_sample(style: &LineStyle, markers: bool, w: f32, cx: &App) -> AnyElement {
    let color = Hsla::from(rgb(u32::from_be_bytes([
        0,
        style.color[0],
        style.color[1],
        style.color[2],
    ])));
    let thick = px(style.thickness.clamp(0.5, 6.0));
    let stroke = if style.dashed {
        h_flex()
            .flex_1()
            .gap(px(3.0))
            .children((0..8).map(move |_| div().w(px(5.0)).h(thick).bg(color)))
            .overflow_hidden()
    } else {
        h_flex().flex_1().child(div().w_full().h(thick).bg(color))
    };
    let cross = |show: bool| {
        div()
            .w(px(10.0))
            .flex_none()
            .text_color(color)
            .text_size(design::t_caption(cx))
            .child(if show { "\u{00d7}" } else { "" })
    };
    h_flex()
        .w(design::ui_px(cx, w))
        .flex_none()
        .items_center()
        .gap(px(2.0))
        .child(cross(markers && style.start_marker))
        .child(stroke)
        .child(cross(markers && style.end_marker))
        .into_any_element()
}

/// Which ends carry a cross, as glyphs: both, one, or none.
fn crosses_text(style: &LineStyle, markers: bool) -> &'static str {
    match (markers && style.start_marker, markers && style.end_marker) {
        (true, true) => "\u{00d7}\u{2014}\u{00d7}",
        (true, false) => "\u{00d7}\u{2014}",
        (false, true) => "\u{2014}\u{00d7}",
        (false, false) => "\u{2014}",
    }
}

/// Table column widths in unscaled pixels: sample, width, dash, crosses.
const COLS: [f32; 4] = [90.0, 56.0, 56.0, 64.0];

/// The lines table: a header and one clickable row per line, the selected one highlighted.
///
/// Args:
///     orders: The colour set being edited.
///     selected: Index of the selected row.
///     on_select: Selects a row by index.
///     cx: Application context for theme and scale.
fn lines_table(
    orders: &OrdersStyle,
    selected: usize,
    on_select: Rc<dyn Fn(usize, &mut Window, &mut App)>,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let muted = rgba_from(p.text_muted, 1.0);
    let cell = |w: f32| div().w(design::font_w_px(cx, w)).flex_none();
    let header = h_flex()
        .w_full()
        .gap(design::ui_px(cx, 8.0))
        .px(design::ui_px(cx, 10.0))
        .py(design::ui_px(cx, 6.0))
        .text_color(muted)
        .text_size(design::t_caption(cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(t!("lines.col.line").to_string()),
        )
        .child(cell(COLS[0]).child(t!("lines.col.sample").to_string()))
        .child(
            cell(COLS[1])
                .text_align(TextAlign::Right)
                .child(t!("lines.col.width").to_string()),
        )
        .child(
            cell(COLS[2])
                .text_align(TextAlign::Center)
                .child(t!("lines.col.dash").to_string()),
        )
        .child(
            cell(COLS[3])
                .text_align(TextAlign::Center)
                .child(t!("lines.col.crosses").to_string()),
        );
    let rows = LINE_KINDS.iter().enumerate().map(|(idx, kind)| {
        let style = line_style(orders, idx);
        let on_select = on_select.clone();
        let id = SharedString::from(format!("lines-row-{idx}"));
        h_flex()
            .id(id)
            .debug_selector(move || format!("lines-row-{idx}"))
            .w_full()
            .gap(design::ui_px(cx, 8.0))
            .px(design::ui_px(cx, 10.0))
            .py(design::ui_px(cx, 5.0))
            .items_center()
            .cursor_pointer()
            .border_t_1()
            .border_color(rgba_from(p.border, 1.0))
            .when(idx == selected, |row| {
                row.bg(design::moon_alpha(p.blue, 0.14))
            })
            .on_click(move |_, window, cx| on_select(idx, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(t!(kind.name).to_string()),
            )
            .child(cell(COLS[0]).child(line_sample(style, kind.markers, COLS[0], cx)))
            .child(
                cell(COLS[1])
                    .font_family(design::mono())
                    .text_align(TextAlign::Right)
                    .child(format!("{:.1}", style.thickness)),
            )
            .child(
                cell(COLS[2])
                    .text_align(TextAlign::Center)
                    .text_color(muted)
                    .child(if style.dashed { "\u{2713}" } else { "\u{2014}" }),
            )
            .child(
                cell(COLS[3])
                    .text_align(TextAlign::Center)
                    .text_color(muted)
                    .child(crosses_text(style, kind.markers)),
            )
    });
    super::general::page_box(Vec::new(), cx)
        .px_0()
        .py_0()
        .debug_selector(|| "lines-table".into())
        .child(header)
        .children(rows)
        .into_any_element()
}

/// One labelled row of the selected-line card.
fn card_row(id: &'static str, label: String, control: AnyElement, cx: &App) -> AnyElement {
    h_flex()
        .w_full()
        .flex_wrap()
        .items_center()
        .gap(design::ui_px(cx, 10.0))
        .debug_selector(move || id.to_string())
        .child(
            div()
                .w(design::font_w_px(cx, 120.0))
                .flex_none()
                .text_color(rgba_from(MoonPalette::active(cx).text_soft, 1.0))
                .child(label),
        )
        .child(control)
        .into_any_element()
}

/// The selected line's card: its name, a large sample, then its rows.
fn line_card(name: String, sample: AnyElement, rows: Vec<AnyElement>, cx: &App) -> AnyElement {
    let mut children = vec![
        div()
            .debug_selector(|| "lines-card-name".into())
            .font_weight(FontWeight::SEMIBOLD)
            .child(name)
            .into_any_element(),
        div()
            .w_full()
            .py(design::ui_px(cx, 6.0))
            .child(sample)
            .into_any_element(),
    ];
    children.extend(rows);
    super::general::page_box(children, cx)
        .gap(design::ui_px(cx, 6.0))
        .py(design::ui_px(cx, 10.0))
        .into_any_element()
}

/// The parts of the Lines page, built by the view and laid out by [`lines_body`].
pub(super) struct LinesParts {
    /// Which colour set is edited.
    pub(super) set_note: String,
    pub(super) table: AnyElement,
    pub(super) card: AnyElement,
    pub(super) active_alpha: AnyElement,
    pub(super) closed_alpha: AnyElement,
    pub(super) max_closed: AnyElement,
    pub(super) pending_dashed: AnyElement,
    pub(super) path: AnyElement,
}

/// The Lines page body: the table and the selected line's card side by side (stacked when
/// narrow), then the settings shared by every line.
pub(super) fn lines_body(
    parts: LinesParts,
    more: &dyn Fn(&'static str) -> (bool, super::common::Action),
    cx: &App,
) -> AnyElement {
    let muted = rgba_from(MoonPalette::active(cx).text_muted, 1.0);
    let row =
        |id: &'static str, label: String, hint: Option<String>, long: Option<String>, control| {
            let (open, toggle) = more(id);
            div()
                .debug_selector(move || id.to_string())
                .child(super::common::hint_row(
                    id, label, hint, long, control, open, toggle, cx,
                ))
                .into_any_element()
        };
    v_flex()
        .w_full()
        .min_w_0()
        .gap(design::ui_px(cx, 16.0))
        .child(
            div()
                .debug_selector(|| "lines-set-note".into())
                .text_color(muted)
                .child(parts.set_note),
        )
        .child(
            h_flex()
                .w_full()
                .flex_wrap()
                .items_start()
                .gap(design::ui_px(cx, 14.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(design::ui_px(cx, 380.0))
                        .child(parts.table),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(design::ui_px(cx, 280.0))
                        .debug_selector(|| "lines-card".into())
                        .child(parts.card),
                ),
        )
        .child(
            v_flex()
                .w_full()
                .gap(design::ui_px(cx, 6.0))
                .child(super::general::page_section(
                    "lines-all",
                    t!("lines.all.title").to_string(),
                    vec![
                        row(
                            "lines-active-alpha",
                            t!("lines.active_alpha").to_string(),
                            None,
                            None,
                            parts.active_alpha,
                        ),
                        row(
                            "lines-closed-alpha",
                            t!("lines.closed_alpha").to_string(),
                            Some(t!("lines.closed_alpha.hint").to_string()),
                            None,
                            parts.closed_alpha,
                        ),
                        row(
                            "lines-max-closed",
                            t!("lines.max_closed.short").to_string(),
                            None,
                            None,
                            parts.max_closed,
                        ),
                        row(
                            "lines-pending-dashed",
                            t!("lines.pending_dashed").to_string(),
                            None,
                            None,
                            parts.pending_dashed,
                        ),
                        row(
                            "lines-path",
                            t!("lines.path_title").to_string(),
                            Some(t!("lines.path.hint_short").to_string()),
                            Some(t!("lines.path_hint").to_string()),
                            parts.path,
                        ),
                    ],
                    cx,
                ))
                .child(
                    div()
                        .debug_selector(|| "lines-note".into())
                        .text_color(muted)
                        .child(t!("lines.note.after_entry").to_string()),
                ),
        )
        .into_any_element()
}

/// Every hint-row id of the shared box, top to bottom.
#[cfg(test)]
pub(super) const LINES_ALL_ROWS: [&str; 5] = [
    "lines-active-alpha",
    "lines-closed-alpha",
    "lines-max-closed",
    "lines-pending-dashed",
    "lines-path",
];

impl SettingsView {
    /// Build a checkbox for a boolean `OrdersStyle` field in the active draft colour set.
    fn ord_check(
        &self,
        cx: &Context<Self>,
        id: impl Into<SharedString>,
        get: impl Fn(&OrdersStyle) -> bool + 'static,
        set: impl Fn(&mut OrdersStyle, bool) + 'static,
    ) -> MoonCheckbox {
        let cur = {
            let b = self.backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            get(d.orders.get(d.ui_theme_mode.is_light()))
        };
        self.draft_checkbox(cx, id, cur, move |p, v| {
            let is_light = p.ui_theme_mode.is_light();
            if get(p.orders.get(is_light)) != v {
                set(p.orders.get_mut(is_light), v);
                true
            } else {
                false
            }
        })
    }

    /// The selected line's card rows: colour, width, dash, the not-filled look of an entry, and
    /// the crosses and knots of a line that has them.
    fn line_card_rows(&self, idx: usize, cx: &Context<Self>) -> Vec<AnyElement> {
        let kind = &LINE_KINDS[idx];
        let ed = self.lines.ed(idx);
        let pct = |v: f32| {
            fmt::pct((v * 100.0) as f64, 0).map_or_else(|| "0%".to_string(), |(text, _)| text)
        };
        let px1 = |v: f32| format!("{} px", fmt::compact(v as f64, 1));
        let picker =
            |st: &Entity<MoonColorPickerState>| super::common::swatch_picker(st).into_any_element();
        // Element ids carry the row so a retained checkbox never shows another line's state.
        let check_id = |what: &str| SharedString::from(format!("line-{}-{what}", kind.key));
        let mut rows = vec![
            card_row(
                "lines-card-color",
                t!("badges.col.color").to_string(),
                picker(&ed.color),
                cx,
            ),
            card_row(
                "lines-card-width",
                t!("lines.col.width").to_string(),
                slider_ctl(&ed.thickness, &px1, cx),
                cx,
            ),
            card_row(
                "lines-card-dash",
                t!("lines.col.dash").to_string(),
                self.ord_check(
                    cx,
                    check_id("dash"),
                    move |o| line_style(o, idx).dashed,
                    move |o, v| line_style_mut(o, idx).dashed = v,
                )
                .into_any_element(),
                cx,
            ),
        ];
        // Only entry lines have a separate colour and opacity for unfilled orders (`fill == 0`).
        // An absent pending colour falls back to the line's primary colour.
        if kind.pending {
            rows.push(card_row(
                "lines-card-pending",
                t!("lines.not_filled").to_string(),
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 10.0))
                    .items_center()
                    .child(picker(&ed.pending_color))
                    .child(slider_ctl(&ed.pending_alpha, &pct, cx))
                    .into_any_element(),
                cx,
            ));
        }
        if kind.markers {
            rows.push(card_row(
                "lines-card-crosses",
                t!("lines.col.crosses").to_string(),
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 12.0))
                    .child(
                        self.ord_check(
                            cx,
                            check_id("start"),
                            move |o| line_style(o, idx).start_marker,
                            move |o, v| line_style_mut(o, idx).start_marker = v,
                        )
                        .label(t!("lines.cross_at_start").to_string()),
                    )
                    .child(
                        self.ord_check(
                            cx,
                            check_id("end"),
                            move |o| line_style(o, idx).end_marker,
                            move |o, v| line_style_mut(o, idx).end_marker = v,
                        )
                        .label(t!("lines.cross_at_end").to_string()),
                    )
                    .into_any_element(),
                cx,
            ));
            rows.push(card_row(
                "lines-card-cross-size",
                capitalized(&t!("lines.cross_size")),
                slider_ctl(&ed.marker_size, &px1, cx),
                cx,
            ));
            rows.push(card_row(
                "lines-card-cross-width",
                capitalized(&t!("lines.cross_thickness")),
                slider_ctl(&ed.marker_thickness, &px1, cx),
                cx,
            ));
            rows.push(card_row(
                "lines-card-knots",
                capitalized(&t!("lines.knots")),
                h_flex()
                    .gap(design::ui_px(cx, 10.0))
                    .items_center()
                    .child(self.ord_check(
                        cx,
                        check_id("knots"),
                        move |o| line_style(o, idx).knots,
                        move |o, v| line_style_mut(o, idx).knots = v,
                    ))
                    .child(slider_ctl(&ed.knot_size, &px1, cx))
                    .into_any_element(),
                cx,
            ));
        }
        rows
    }

    /// The path's look: own colour or the line's, dash, width and opacity. Whether a tab draws
    /// the path at all is the per-tab checkbox in the chart-graphics popup (#612).
    fn path_controls(&self, cx: &Context<Self>) -> AnyElement {
        let l = &self.lines;
        let use_line_color = {
            let b = self.backend.read(cx);
            let draft = b.preview.as_ref().unwrap_or(&b.config);
            draft
                .orders
                .get(draft.ui_theme_mode.is_light())
                .path
                .use_line_color
        };
        let pct = |v: f32| {
            fmt::pct((v * 100.0) as f64, 0).map_or_else(|| "0%".to_string(), |(text, _)| text)
        };
        let px1 = |v: f32| format!("{} px", fmt::compact(v as f64, 1));
        v_flex()
            .items_end()
            .gap(design::ui_px(cx, 6.0))
            .child(
                h_flex()
                    .gap(design::ui_px(cx, 10.0))
                    .items_center()
                    .child(
                        self.ord_check(
                            cx,
                            "path-line-color",
                            |o| o.path.use_line_color,
                            |o, v| o.path.use_line_color = v,
                        )
                        .label(t!("lines.path_line_color").to_string()),
                    )
                    // With "line colour" on the picker is a dead knob: disabled, not hidden, so
                    // the row keeps its shape while saying why it does nothing.
                    .child(super::common::swatch_picker(&l.path_color).disabled(use_line_color)),
            )
            .child(
                h_flex()
                    .gap(design::ui_px(cx, 10.0))
                    .items_center()
                    .child(
                        self.ord_check(
                            cx,
                            "path-dash",
                            |o| o.path.dashed,
                            |o, v| o.path.dashed = v,
                        )
                        .label(t!("lines.dashed").to_string()),
                    )
                    .child(slider_ctl(&l.path_thickness, &px1, cx)),
            )
            .child(
                h_flex()
                    .gap(design::ui_px(cx, 10.0))
                    .items_center()
                    .child(div().child(t!("lines.path_alpha").to_string()))
                    .child(slider_ctl(&l.path_alpha, &pct, cx)),
            )
            .into_any_element()
    }

    /// Build the Lines page body.
    ///
    /// Args:
    ///     cx: Settings context used for the active palette and scaled controls.
    ///
    /// Returns:
    ///     The assembled page content.
    pub(super) fn lines_tab(&self, cx: &Context<Self>) -> AnyElement {
        let idx = self.lines_ui.selected.min(LINE_KINDS.len() - 1);
        let (orders, light) = {
            let b = self.backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            let light = d.ui_theme_mode.is_light();
            (d.orders.get(light).clone(), light)
        };
        let pct = |v: f32| {
            fmt::pct((v * 100.0) as f64, 0).map_or_else(|| "0%".to_string(), |(text, _)| text)
        };
        let this = cx.entity().downgrade();
        let on_select: Rc<dyn Fn(usize, &mut Window, &mut App)> = Rc::new(move |idx, _, cx| {
            let _ = this.update(cx, |view, cx| {
                view.lines_ui.selected = idx;
                cx.notify();
            });
        });
        let kind = &LINE_KINDS[idx];
        let card = line_card(
            t!(kind.name).to_string(),
            line_sample(line_style(&orders, idx), kind.markers, 260.0, cx),
            self.line_card_rows(idx, cx),
            cx,
        );
        let parts = LinesParts {
            set_note: t!(if light {
                "iface.set_note.light"
            } else {
                "iface.set_note.dark"
            })
            .to_string(),
            table: lines_table(&orders, idx, on_select, cx),
            card,
            active_alpha: slider_ctl(&self.lines.active_alpha, &pct, cx),
            closed_alpha: slider_ctl(&self.lines.closed_alpha, &pct, cx),
            max_closed: num_input(
                "lines-max-closed-input",
                &self.lines_ui.max_closed_orders,
                0.0..=5000.0,
                50.0,
                None,
            ),
            pending_dashed: self
                .ord_check(
                    cx,
                    "pending-dash",
                    |o| o.pending_dashed,
                    |o, v| o.pending_dashed = v,
                )
                .into_any_element(),
            path: self.path_controls(cx),
        };
        lines_body(parts, &|key| self.more_toggle(key, cx), cx)
    }
}

impl SettingsView {
    /// The Order lines page, Copy and Paste in its header.
    pub(super) fn lines_page(&self, cx: &Context<Self>) -> super::frame::PageParts {
        let body = self.lines_tab(cx);
        self.page_parts(super::Tab::Lines, body, self.share_actions(cx), cx)
    }
}

/// UI state of the Order lines page that the draft does not hold. Branch-owned: fields are added
/// here, never on `SettingsView`.
pub(super) struct LinesUi {
    /// Number field for `max_closed_orders` of the order-line set the theme edits.
    pub(super) max_closed_orders: Entity<moon_ui::MoonInputState>,
    /// Table row whose card is shown, an index into [`LINE_KINDS`].
    pub(super) selected: usize,
}

impl LinesUi {
    /// Fields seeded from the draft `config` and bound to the draft.
    pub(super) fn init(
        config: &moon_core::config::AppConfig,
        window: &mut Window,
        cx: &mut Context<SettingsView>,
    ) -> Self {
        let max_closed = |p: &moon_core::config::AppConfig| {
            f64::from(p.orders.get(p.ui_theme_mode.is_light()).max_closed_orders)
        };
        Self {
            max_closed_orders: super::general::draft_number(
                max_closed(config),
                window,
                cx,
                move |p| Some(max_closed(p)),
                |p, v| {
                    let v = v.round().clamp(0.0, 5000.0) as u32;
                    let set = p.orders.get_mut(p.ui_theme_mode.is_light());
                    let changed = set.max_closed_orders != v;
                    set.max_closed_orders = v;
                    changed
                },
            ),
            selected: 0,
        }
    }
}

#[cfg(test)]
mod tests;
