//! The figure settings surface: ONE panel, two things it can be aimed at.
//!
//! Universal by construction, in both directions. Across tools: what every figure has — line
//! colour, opacity, thickness, line kind, and a fill for the tools that enclose an area — is
//! rendered here once, and what differs per tool comes from the tool itself as a list of switches
//! ([`moon_core::figures::ToolSetting`]), so a ratio scale offers its levels and a rectangle offers
//! nothing without this module or the chart panel naming either. Across targets: the same rows
//! serve a figure already drawn ([`Target::Figure`], opened by right-click) and the style the NEXT
//! figure will be drawn with ([`Target::ToolDefaults`], opened from the toolbar's settings button).
//!
//! Only the WRITE differs between the two, and it differs in exactly two functions — [`edit_style`]
//! and [`edit_switch`]. A figure's edit goes through `Backend::edit_figure`, which persists the
//! store and re-upserts an armed figure's blob; a tool default goes to `Backend::fig_style_mut` and
//! `Backend::set_tool_setting`, where the next draft picks it up. Every row of this module calls
//! one of those two and knows nothing else about where it is writing.
//!
//! Two asymmetries between the targets are deliberate and are NOT "the same rows drifting apart":
//! a host may inject an arbitrary-colour wheel (`custom_color`), which needs an `Entity` only the
//! host can own, and only the tab strip has one today; and a figure the core owns is offered no
//! fill, because the alert blob has no field for one and the next reconcile would revert it.
//!
//! The CONTAINER is the host's, not this module's: over a chart the panel is placed at the clicked
//! point and kept inside the slot that clips it, in the tab strip it hangs under its button, and in
//! the Alerts table it is a `MoonPopover` anchored to the row's gear — that host takes [`rows`]
//! alone, because a popover paints its own surface. [`shell`] is the one frame this module owns,
//! carrying the surface and the input guards for the two hosts that float over a chart.

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonPalette, MoonTooltipView, h_flex, v_flex,
};

use moon_core::figures::{
    DEFAULT_FILL_ALPHA, DrawStyle, Figure, FigureTool, LineKind, ToolSetting,
};
use moon_core::session::CoreId;
use rust_i18n::t;

use crate::Backend;
use crate::design;

/// WHICH figure the panel edits. Identity only — where the panel is put is the frame's business,
/// and each frame takes it as an argument, so a host that pins the panel to its own corner has no
/// coordinate to invent.
#[derive(Clone, PartialEq)]
pub(crate) struct FigStyleTarget {
    pub core: CoreId,
    pub market: String,
    pub id: u64,
}

/// What the panel is aimed at.
#[derive(Clone, PartialEq)]
pub(crate) enum Target {
    /// One figure already on a chart. Edits reach the store, the file and the core.
    Figure(FigStyleTarget),
    /// The style and switches the next figure of this tool will be drawn with. Edits reach nothing
    /// already on the chart.
    ToolDefaults(FigureTool),
}

/// Everything the panel draws, read once per render so the store is not borrowed while building
/// elements that will want to write to it.
struct Snapshot {
    style: DrawStyle,
    fills: bool,
    level_palette: bool,
    switches: Vec<ToolSetting>,
}

fn snapshot(backend: &Backend, target: &Target) -> Option<Snapshot> {
    match target {
        Target::Figure(t) => {
            let store = backend.figures.borrow();
            let fig: &Figure = store.get(t.core, &t.market, t.id)?;
            let def = fig.tool().def();
            Some(Snapshot {
                style: fig.style(),
                // A figure the core owns has no fill in its blob (`sync_remote_alerts` reads none),
                // so one set here would be reverted by the next reconcile — a control that changes
                // nothing.
                fills: def.fills && !fig.from_server,
                level_palette: def.level_palette,
                switches: fig.kind.shape().settings(),
            })
        }
        Target::ToolDefaults(tool) => {
            let def = tool.def();
            Some(Snapshot {
                style: backend.fig_style(*tool),
                fills: def.fills,
                level_palette: def.level_palette,
                switches: backend.tool_settings(*tool),
            })
        }
    }
}

/// The swatch palette every settings surface offers, whichever target it is aimed at.
const SWATCHES: [[u8; 4]; 8] = [
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
/// The previous +/-24-of-255 arithmetic jumped from 100 to 91 to 81 and made values such as 15%
/// unreachable; every multiple of five in `5..=100` is now reachable.
fn opacity_step(a: u8, up: bool) -> u8 {
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

/// Applies a style edit to whichever target the panel is aimed at.
///
/// The single write path for everything but the tool's own switches, so no row can reach past it
/// and edit one target while the panel is showing the other.
/// Returns whether anything changed, so a click that moved nothing wakes no observer.
fn edit_style(b: &mut Backend, target: &Target, f: impl FnOnce(&mut DrawStyle) -> bool) -> bool {
    match target {
        Target::Figure(t) => b.edit_figure(t.core, &t.market, t.id, |fig| {
            let mut style = fig.style();
            f(&mut style) && fig.set_style(style)
        }),
        Target::ToolDefaults(tool) => f(b.fig_style_mut(*tool)),
    }
}

/// Applies one of the tool's own switches to whichever target the panel is aimed at.
fn edit_switch(b: &mut Backend, target: &Target, key: &str, on: bool) -> bool {
    match target {
        Target::Figure(t) => b.edit_figure(t.core, &t.market, t.id, |fig| {
            fig.kind.shape_mut().set_setting(key, on)
        }),
        Target::ToolDefaults(tool) => b.set_tool_setting(*tool, key, on),
    }
}

/// The panel's content: every row, in the order a reader looks for them.
///
/// `None` when there is nothing to show — the figure was deleted from another window, or dropped
/// when its core disconnected while the panel was open.
/// `custom_color` is the host's arbitrary-colour picker, placed at the end of the swatch row. It
/// needs an `Entity` of its own to hold the open/closed wheel, which belongs to the view that hosts
/// the panel rather than to the panel; a host without one passes `None` and offers the swatches.
pub(crate) fn rows<V: 'static>(
    backend: &Entity<Backend>,
    target: &Target,
    custom_color: Option<AnyElement>,
    cx: &mut Context<V>,
) -> Option<AnyElement> {
    let p = MoonPalette::active(cx);
    let snap = snapshot(backend.read(cx), target)?;
    let label = |s: &str| div().text_color(rgb(p.text_muted)).child(s.to_string());

    let mut rows = v_flex().gap(design::ui_px(cx, 6.0));
    rows = rows
        .child(label(&t!("chart.fig.color")))
        .child(
            h_flex()
                .items_center()
                .gap(design::ui_px(cx, 3.0))
                .flex_wrap()
                .child(swatch_row(
                    backend,
                    target,
                    "figset-color",
                    snap.style.color,
                    true,
                    cx,
                ))
                .children(custom_color),
        )
        .child(stepper_row(
            backend,
            target,
            "figset-th",
            &t!("chart.fig.thickness"),
            format!("{:.1}", snap.style.thickness),
            p,
            cx,
            |s, up| {
                let next = if up {
                    (s.thickness + 0.5).min(6.0)
                } else {
                    (s.thickness - 0.5).max(0.5)
                };
                let changed = next != s.thickness;
                s.thickness = next;
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
            |s, up| {
                let next = opacity_step(s.color[3], up);
                let changed = next != s.color[3];
                s.color[3] = next;
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
            |s, up| {
                let next = match (s.fill[3], up) {
                    (0, false) => 0,
                    (0, true) => DEFAULT_FILL_ALPHA,
                    (a, up) => opacity_step(a, up),
                };
                let changed = next != s.fill[3];
                s.fill[3] = next;
                changed
            },
        ));
    }

    // The tool's own switches, last: they are what makes this tool's settings different from every
    // other tool's, and a reader looks for the shared controls first.
    if !snap.switches.is_empty() {
        rows = rows.child(label(&t!("chart.fig.parts"))).child(switch_row(
            backend,
            target,
            &snap.switches,
        ));
    }
    Some(rows.into_any_element())
}

/// The shared look of the panel's frame: surface, border, radius, shadow, text size.
fn shell<V: 'static>(id: &'static str, cx: &mut Context<V>) -> Stateful<Div> {
    let p = MoonPalette::active(cx);
    div()
        .id(id)
        .absolute()
        .p(design::ui_px(cx, 8.0))
        .bg(rgb(p.surface))
        .border_1()
        .border_color(rgb(p.border))
        .rounded(design::r_container(cx))
        .shadow_lg()
        .text_size(design::t_body(cx))
        // The chart under the panel must take none of the panel's input, and BOTH frames float
        // over it: the tab strip's cluster is painted after the chart, so its copy of the panel
        // hangs over the plot exactly like the chart's own. Every button matters here — left-down
        // places a figure node, starts a pan or moves an ORDER, left-up finishes a draft,
        // right-down opens the chart's menu, and the wheel zooms the chart out from under a panel
        // still pinned to the old coordinates.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        // The right-button RELEASE too: the chart only suppresses that release when it saw the
        // press, and the press stops here — so without this the Main stack would leave fullscreen
        // when someone right-clicks the panel.
        .on_mouse_up(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        // And movement, or the crosshair, the figure hover and the draft preview would keep
        // tracking a cursor that is over a panel, not over the chart.
        .on_mouse_move(|_, _, cx| cx.stop_propagation())
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
}

/// Renders the panel over a chart, anchored at the point it was opened from.
///
/// `at` is that point in WINDOW coordinates — the same point the figure's context menu is opened
/// with — and the frame is deferred and snapped to the window, exactly as a menu is. That is what
/// replaced a hand-written clamp against the chart slot: the clamp had to know the slot's size, its
/// scale factor and a guess at how much of the panel must stay visible, and it still left the panel
/// running under the dock below, where the slot clipped it instead of scrolling it. The window is
/// the only box that can answer "does this fit", so it is asked.
pub(crate) fn render<V: 'static>(
    backend: &Entity<Backend>,
    target: &FigStyleTarget,
    at: Point<Pixels>,
    cx: &mut Context<V>,
) -> Option<AnyElement> {
    let content = rows(backend, &Target::Figure(target.clone()), None, cx)?;
    Some(
        deferred(
            anchored()
                .position(at)
                .snap_to_window_with_margin(design::ui_px(cx, 8.0))
                .child(
                    shell("figstyle-panel", cx)
                        // Grows with the font, like the toolbar's copy of this panel.
                        .w(design::font_w_px(cx, 232.0))
                        // A ceiling, not a fit: a Fibonacci's eleven switches scroll inside it
                        // rather than making a panel taller than the window it must stay inside.
                        .max_h(design::ui_px(cx, 420.0))
                        .overflow_y_scroll()
                        .child(content),
                ),
        )
        .into_any_element(),
    )
}

/// Renders the panel under the toolbar button that opened it, for the tool's own defaults.
///
/// Positioned by its trigger rather than by the window: it hangs from a button in the tab strip,
/// which never moves out from under it. The height is a fixed cap and the content scrolls inside
/// it, which is what keeps a ratio scale's eleven switches reachable in a short window.
pub(crate) fn render_tool_defaults<V: 'static>(
    backend: &Entity<Backend>,
    tool: FigureTool,
    custom_color: Option<AnyElement>,
    cx: &mut Context<V>,
) -> Option<AnyElement> {
    let content = rows(backend, &Target::ToolDefaults(tool), custom_color, cx)?;
    Some(
        shell("figstyle-tool-panel", cx)
            .top_full()
            .left_0()
            .mt(design::ui_px(cx, 4.0))
            // Grow with font size; otherwise labels and values wrap at +6.
            .w(design::font_w_px(cx, 232.0))
            .max_h(design::ui_px(cx, 420.0))
            .overflow_y_scroll()
            .child(content)
            .into_any_element(),
    )
}

/// A row of colour swatches writing either the line colour or the fill colour.
fn swatch_row<V: 'static>(
    backend: &Entity<Backend>,
    target: &Target,
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
                        let changed = edit_style(b, &target, |s| {
                            // Opacity is a separate control; picking a colour must not reset it.
                            if line {
                                let next = [sw[0], sw[1], sw[2], s.color[3]];
                                let changed = s.color != next;
                                s.color = next;
                                changed
                            } else {
                                // Picking a colour turns the fill on: at its current strength, or
                                // at the default when it was off, since the ∅ cell zeroes the
                                // alpha. A fill that stayed invisible after a deliberate click
                                // would read as broken.
                                let a = match s.fill[3] {
                                    0 => DEFAULT_FILL_ALPHA,
                                    a => a,
                                };
                                let next = [sw[0], sw[1], sw[2], a];
                                let changed = s.fill != next;
                                s.fill = next;
                                changed
                            }
                        });
                        if changed {
                            bcx.notify();
                        }
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
    target: &Target,
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
                    if edit_style(b, &target_off, |s| {
                        let changed = s.fill[3] != 0;
                        s.fill[3] = 0;
                        changed
                    }) {
                        bcx.notify();
                    }
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
                        if edit_style(b, &target_on, |s| {
                            if s.fill[3] != 0 {
                                return false;
                            }
                            s.fill[3] = DEFAULT_FILL_ALPHA;
                            true
                        }) {
                            bcx.notify();
                        }
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
    target: &Target,
    id_prefix: &'static str,
    text: &str,
    value: String,
    p: MoonPalette,
    cx: &mut Context<V>,
    edit: fn(&mut DrawStyle, bool) -> bool,
) -> impl IntoElement {
    // MoonButton rather than a bespoke div: hover and press states, sizing and theming come with it.
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
                if edit_style(b, &target, |s| edit(s, up)) {
                    bcx.notify();
                }
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
fn kind_row(backend: &Entity<Backend>, target: &Target, current: LineKind) -> impl IntoElement {
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
                    if edit_style(b, &target, |s| {
                        let changed = s.kind != kind;
                        s.kind = kind;
                        changed
                    }) {
                        bcx.notify();
                    }
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
    target: &Target,
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
                    if edit_switch(b, &target, &key, !on) {
                        bcx.notify();
                    }
                });
            })
            .render(),
        );
    }
    row
}

#[cfg(test)]
mod tests;
