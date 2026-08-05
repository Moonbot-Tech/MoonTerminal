//! The per-figure settings surface: one panel that styles ANY figure already on a chart.
//!
//! Universal by construction. What every figure has — line colour, opacity, thickness, line kind,
//! and a fill for the tools that enclose an area — is rendered here once. What differs per tool
//! comes from the tool itself as a list of switches ([`moon_core::figures::ToolSetting`]), so a
//! ratio scale offers its eleven levels and a rectangle offers nothing, and neither this module
//! nor the chart panel names a tool to make that happen. A tool that grows a setting grows it in
//! its own module; this panel draws it without being touched.
//!
//! Writes go through `Backend::edit_figure`, which persists the store and re-upserts an armed
//! figure's blob, so a change made here reaches the file and the core as well as the screen.
//!
//! Distinct from the pencil popup in `chart_tabs::fig_tools`, which edits the style the NEXT
//! figure will be drawn with. Same vocabulary, different target: that one changes nothing already
//! on the chart, this one changes nothing about the next figure.

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonPalette, MoonTooltipView, h_flex, v_flex,
};

use moon_core::figures::{DEFAULT_FILL_ALPHA, DrawStyle, Figure, LineKind, ToolSetting};
use moon_core::session::CoreId;
use rust_i18n::t;

use crate::Backend;
use crate::design;

/// Which figure the panel edits, and where it was opened.
#[derive(Clone, PartialEq)]
pub(crate) struct FigStyleTarget {
    pub core: CoreId,
    pub market: String,
    pub id: u64,
    /// Where to put the panel: the clicked point in the chart slot's DEVICE pixels, which is what
    /// every chart hit test speaks. The renderer divides by pixels-per-point, because layout is in
    /// logical ones — get that wrong and the panel opens a scale factor away from the click.
    pub at: (f32, f32),
}

/// Everything the panel draws, read once per render so the store is not borrowed while building
/// elements that will want to write to it.
struct Snapshot {
    style: DrawStyle,
    fills: bool,
    level_palette: bool,
    switches: Vec<ToolSetting>,
}

fn snapshot(backend: &Backend, target: &FigStyleTarget) -> Option<Snapshot> {
    let store = backend.figures.borrow();
    let fig: &Figure = store.get(target.core, &target.market, target.id)?;
    let def = fig.tool().def();
    Some(Snapshot {
        style: DrawStyle {
            color: fig.color,
            thickness: fig.thickness,
            kind: fig.line_kind,
            fill: fig.fill,
        },
        // A figure the core owns has no fill in its blob (`sync_remote_alerts` reads none), so one
        // set here would be reverted by the next reconcile — a control that changes nothing.
        fills: def.fills && !fig.from_server,
        level_palette: def.level_palette,
        switches: fig.kind.shape().settings(),
    })
}

/// The swatch palette both figure-style surfaces offer: this panel and the pencil popup, which
/// imports it from here. One list, so a figure can always be given the colour the pencil drew it in.
pub(crate) const SWATCHES: [[u8; 4]; 8] = [
    [64, 196, 255, 255],  // blue (default)
    [80, 220, 120, 255],  // green
    [240, 90, 90, 255],   // red
    [250, 200, 60, 255],  // yellow
    [245, 150, 40, 255],  // orange
    [200, 110, 240, 255], // purple
    [240, 240, 240, 255], // white
    [150, 160, 175, 255], // gray
];

/// Step opacity by 5%, snapping to whole percentage points.
///
/// Shared with the pencil popup so the two surfaces cannot disagree about what one press does. The
/// previous +/-24-of-255 arithmetic jumped from 100 to 91 to 81 and made values such as 15%
/// unreachable; every multiple of five in `5..=100` is now reachable.
pub(crate) fn opacity_step(a: u8, up: bool) -> u8 {
    let pct = (a as f32 / 255.0 * 100.0).round() as i32;
    let next = if up {
        pct / 5 * 5 + 5
    } else if pct % 5 == 0 {
        pct - 5
    } else {
        pct / 5 * 5
    }
    .clamp(5, 100);
    (next as f32 / 100.0 * 255.0).round() as u8
}

/// Renders the panel for `target`, or nothing when the figure is gone — deleted from another
/// window, or dropped when its core disconnected while the panel was open.
pub(crate) fn render<V: 'static>(
    backend: &Entity<Backend>,
    target: &FigStyleTarget,
    slot: (f32, f32),
    ppp: f32,
    cx: &mut Context<V>,
) -> Option<AnyElement> {
    let p = MoonPalette::active(cx);
    let snap = snapshot(backend.read(cx), target)?;
    let label = |s: &str| div().text_color(rgb(p.text_muted)).child(s.to_string());

    // Device pixels — what the click was measured in — into the logical ones layout uses.
    let at = (target.at.0 / ppp.max(0.1), target.at.1 / ppp.max(0.1));
    // Kept inside the chart slot, which clips its children: a panel opened near an edge would
    // otherwise be cut in half. The width scales with the font like the pencil popup's, and the
    // height is not guessed — the panel is capped at the room below it and scrolls inside that,
    // so a Fibonacci's eleven switches stay reachable in a short pane.
    let w = f32::from(design::font_w_px(cx, 232.0));
    // Enough of the panel has to be ON screen to use: opened near the bottom it moves UP rather
    // than hanging into a slot that clips its children, and only then is capped to what is left.
    const MIN_H: f32 = 180.0;
    let left = at.0.min((slot.0 - w).max(0.0)).max(0.0);
    let top = at.1.min((slot.1 - MIN_H).max(0.0)).max(0.0);
    let max_h = (slot.1 - top - 8.0).max(MIN_H.min(slot.1));

    let mut rows = v_flex().gap(px(6.0));
    rows = rows
        .child(label(&t!("chart.fig.color")))
        .child(swatch_row(
            backend,
            target,
            "figset-color",
            snap.style.color,
            true,
            cx,
        ))
        .child(stepper_row(
            backend,
            target,
            "figset-th",
            &t!("chart.fig.thickness"),
            format!("{:.1}", snap.style.thickness),
            p,
            cx,
            |f, up| {
                let next = if up {
                    (f.thickness + 0.5).min(6.0)
                } else {
                    (f.thickness - 0.5).max(0.5)
                };
                let changed = next != f.thickness;
                f.thickness = next;
                changed
            },
        ))
        .child(stepper_row(
            backend,
            target,
            "figset-op",
            &t!("chart.fig.opacity"),
            format!(
                "{}%",
                (snap.style.color[3] as f32 / 255.0 * 100.0).round() as i32
            ),
            p,
            cx,
            |f, up| {
                let next = opacity_step(f.color[3], up);
                let changed = next != f.color[3];
                f.color[3] = next;
                changed
            },
        ))
        .child(label(&t!("chart.fig.kind")))
        .child(kind_row(backend, target, snap.style.kind));

    if snap.fills {
        rows = rows
            .child(label(&t!("chart.fig.fill")))
            .child(fill_row(backend, target, &snap, p, cx));
        rows = rows.child(stepper_row(
            backend,
            target,
            "figset-fop",
            &t!("chart.fig.fill_opacity"),
            format!(
                "{}%",
                (snap.style.fill[3] as f32 / 255.0 * 100.0).round() as i32
            ),
            p,
            cx,
            |f, up| {
                let next = match (f.fill[3], up) {
                    (0, false) => 0,
                    (0, true) => DEFAULT_FILL_ALPHA,
                    (a, up) => opacity_step(a, up),
                };
                let changed = next != f.fill[3];
                f.fill[3] = next;
                changed
            },
        ));
    }

    // The tool's own switches, last: they are what makes this figure's settings different from
    // every other figure's, and a reader looks for the shared controls first.
    if !snap.switches.is_empty() {
        rows = rows.child(label(&t!("chart.fig.parts"))).child(switch_row(
            backend,
            target,
            &snap.switches,
        ));
    }

    Some(
        div()
            .id("figstyle-panel")
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(w))
            .max_h(px(max_h))
            .overflow_y_scroll()
            .p(design::ui_px(cx, 8.0))
            .bg(rgb(p.surface))
            .border_1()
            .border_color(rgb(p.border))
            .rounded(design::r_container(cx))
            .shadow_lg()
            .text_size(design::t_body(cx))
            // The chart under the panel must take none of the panel's input. Every button matters
            // here: left-down places a figure node or starts a pan, left-UP finishes a draft,
            // right-down opens the chart's own menu, and the wheel zooms the chart out from under
            // a panel still pinned to the old coordinates.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            // The right-button RELEASE too: the chart only suppresses that release when it saw the
            // press, and the press stops here — so without this the Main stack would leave
            // fullscreen when someone right-clicks the panel.
            .on_mouse_up(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            // And movement, or the crosshair, the figure hover and the draft preview would keep
            // tracking a cursor that is over a panel, not over the chart.
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(rows)
            .into_any_element(),
    )
}

/// A row of colour swatches writing either the line colour or the fill colour.
fn swatch_row<V: 'static>(
    backend: &Entity<Backend>,
    target: &FigStyleTarget,
    id_prefix: &'static str,
    current: [u8; 4],
    line: bool,
    cx: &mut Context<V>,
) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    let mut row = h_flex().items_center().gap(px(3.0)).flex_wrap();
    for (i, sw) in SWATCHES.iter().enumerate() {
        let sw = *sw;
        let backend = backend.clone();
        let target = target.clone();
        let selected = current[..3] == sw[..3] && (line || current[3] > 0);
        row = row.child(
            div()
                .id((id_prefix, i))
                .w(px(16.0))
                .h(px(16.0))
                .rounded(design::ui_px(cx, 3.0))
                .bg(gpui::rgb(
                    ((sw[0] as u32) << 16) | ((sw[1] as u32) << 8) | sw[2] as u32,
                ))
                .border_2()
                .border_color(if selected {
                    rgb(p.accent)
                } else {
                    rgb(p.border)
                })
                .cursor_pointer()
                .on_click(move |_, _w, app| {
                    backend.update(app, |b, bcx| {
                        b.edit_figure(target.core, &target.market, target.id, |f| {
                            // Opacity is a separate control; picking a colour must not reset it.
                            if line {
                                let a = f.color[3];
                                let next = [sw[0], sw[1], sw[2], a];
                                let changed = f.color != next;
                                f.color = next;
                                changed
                            } else {
                                let a = match f.fill[3] {
                                    0 => DEFAULT_FILL_ALPHA,
                                    a => a,
                                };
                                let next = [sw[0], sw[1], sw[2], a];
                                let changed = f.fill != next;
                                f.fill = next;
                                changed
                            }
                        });
                        bcx.notify();
                    });
                }),
        );
    }
    row
}

/// The fill row: the "no fill" cell plus either the swatches or, for a tool that colours itself
/// from a typed scale, a single cell that switches the fill back on in the scale's own hues.
fn fill_row<V: 'static>(
    backend: &Entity<Backend>,
    target: &FigStyleTarget,
    snap: &Snapshot,
    p: MoonPalette,
    cx: &mut Context<V>,
) -> impl IntoElement {
    let has_fill = snap.style.fill[3] > 0;
    let backend_off = backend.clone();
    let target_off = target.clone();
    let mut row = h_flex().items_center().gap(px(3.0)).flex_wrap().child(
        div()
            .id("figset-fill-off")
            .w(px(16.0))
            .h(px(16.0))
            .rounded(design::ui_px(cx, 3.0))
            .border_2()
            .border_color(if has_fill {
                rgb(p.border)
            } else {
                rgb(p.accent)
            })
            .text_color(rgb(p.text_muted))
            .text_center()
            .child("∅")
            .cursor_pointer()
            .tooltip(|_window, cx| {
                cx.new(|_| MoonTooltipView::new(t!("chart.fig.no_fill").to_string()))
                    .into()
            })
            .on_click(move |_, _w, app| {
                backend_off.update(app, |b, bcx| {
                    b.edit_figure(target_off.core, &target_off.market, target_off.id, |f| {
                        let changed = f.fill[3] != 0;
                        f.fill[3] = 0;
                        changed
                    });
                    bcx.notify();
                });
            }),
    );
    if snap.level_palette {
        let [r, g, b] = moon_core::figures::levels::scale_swatch();
        let backend_on = backend.clone();
        let target_on = target.clone();
        row = row.child(
            div()
                .id("figset-fill-scale")
                .w(px(16.0))
                .h(px(16.0))
                .rounded(design::ui_px(cx, 3.0))
                .bg(gpui::rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32))
                .border_2()
                .border_color(if has_fill {
                    rgb(p.accent)
                } else {
                    rgb(p.border)
                })
                .cursor_pointer()
                .tooltip(|_window, cx| {
                    cx.new(|_| MoonTooltipView::new(t!("chart.fig.scale_fill").to_string()))
                        .into()
                })
                .on_click(move |_, _w, app| {
                    backend_on.update(app, |b, bcx| {
                        b.edit_figure(target_on.core, &target_on.market, target_on.id, |f| {
                            if f.fill[3] != 0 {
                                return false;
                            }
                            f.fill[3] = DEFAULT_FILL_ALPHA;
                            true
                        });
                        bcx.notify();
                    });
                }),
        );
        return row.into_any_element();
    }
    row.child(swatch_row(
        backend,
        target,
        "figset-fill",
        snap.style.fill,
        false,
        cx,
    ))
    .into_any_element()
}

/// A label, a minus, a value and a plus — the shape every numeric setting takes here.
#[allow(clippy::too_many_arguments)]
fn stepper_row<V: 'static>(
    backend: &Entity<Backend>,
    target: &FigStyleTarget,
    id_prefix: &'static str,
    text: &str,
    value: String,
    p: MoonPalette,
    cx: &mut Context<V>,
    edit: fn(&mut Figure, bool) -> bool,
) -> impl IntoElement {
    // MoonButton rather than a bespoke div: hover and press states, sizing and theming come with
    // it, and the pencil popup's own steppers are built the same way.
    let btn = |glyph: &'static str, up: bool| {
        let backend = backend.clone();
        let target = target.clone();
        MoonButton::new(ElementId::Name(SharedString::from(format!(
            "{id_prefix}-{}",
            if up { "up" } else { "dn" }
        ))))
        .label(glyph)
        .size(MoonButtonSize::Micro)
        .variant(MoonButtonVariant::Ghost)
        .on_click(move |_, _w, app| {
            backend.update(app, |b, bcx| {
                b.edit_figure(target.core, &target.market, target.id, |f| edit(f, up));
                bcx.notify();
            });
        })
        .render()
    };
    h_flex()
        .items_center()
        .gap(px(4.0))
        .child(div().text_color(rgb(p.text_muted)).child(text.to_string()))
        .child(btn("−", false))
        .child(
            div()
                .w(design::font_w_px(cx, 34.0))
                .text_center()
                .text_color(rgb(p.text))
                .child(value),
        )
        .child(btn("+", true))
}

/// The five line kinds as buttons rather than a dropdown: five is few enough to show at once, and
/// a row of chips needs no state of its own to live in the view that hosts this panel.
fn kind_row(
    backend: &Entity<Backend>,
    target: &FigStyleTarget,
    current: LineKind,
) -> impl IntoElement {
    let mut row = h_flex().items_center().gap(px(3.0)).flex_wrap();
    for (i, kind) in LineKind::ALL.into_iter().enumerate() {
        let backend = backend.clone();
        let target = target.clone();
        let selected = kind == current;
        row = row.child(
            MoonButton::new(ElementId::Name(SharedString::from(format!(
                "figset-kind-{i}"
            ))))
            .label(kind.label())
            .size(MoonButtonSize::Micro)
            .variant(if selected {
                MoonButtonVariant::Blue
            } else {
                MoonButtonVariant::Ghost
            })
            .selected(selected)
            .on_click(move |_, _w, app| {
                backend.update(app, |b, bcx| {
                    b.edit_figure(target.core, &target.market, target.id, |f| {
                        let changed = f.line_kind != kind;
                        f.line_kind = kind;
                        changed
                    });
                    bcx.notify();
                });
            })
            .render(),
        );
    }
    row
}

/// The tool's own switches, as chips that read as pressed when on. Labelled by the tool, so this
/// row draws a ratio scale's levels without knowing what a level is.
fn switch_row(
    backend: &Entity<Backend>,
    target: &FigStyleTarget,
    switches: &[ToolSetting],
) -> impl IntoElement {
    let mut row = h_flex().items_center().gap(px(3.0)).flex_wrap();
    for (i, s) in switches.iter().enumerate() {
        let backend = backend.clone();
        let target = target.clone();
        let key = s.key.clone();
        let on = s.on;
        row = row.child(
            MoonButton::new(ElementId::Name(SharedString::from(format!(
                "figset-switch-{i}"
            ))))
            .label(s.label.clone())
            .size(MoonButtonSize::Micro)
            .variant(if on {
                MoonButtonVariant::Blue
            } else {
                MoonButtonVariant::Ghost
            })
            .selected(on)
            .on_click(move |_, _w, app| {
                backend.update(app, |b, bcx| {
                    b.edit_figure(target.core, &target.market, target.id, |f| {
                        f.kind.shape_mut().set_setting(&key, !on)
                    });
                    bcx.notify();
                });
            })
            .render(),
        );
    }
    row
}
