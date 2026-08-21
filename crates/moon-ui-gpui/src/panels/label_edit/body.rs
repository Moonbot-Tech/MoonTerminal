//! The editor's layout: what the module is called, the captions it prints, the settings of the one
//! selected, and a sample of the result.
//!
//! Two panes rather than an expander per caption: a caption has six settings, and opening one of
//! eight rows to reach them is the shape this dialog exists to replace. The list answers "which
//! captions and in what order", the pane answers "what does THIS one look like", and the sample at
//! the bottom answers both at once.

use gpui::*;
use moon_core::config::{
    CHART_LABEL_PARTS, ChartLabelField, ChartLabelPart, LABEL_SIZE_MULT_MAX, LABEL_SIZE_MULT_MIN,
    LabelColor, LabelFlow, LabelStyle, LabelWindow, PnlBasis,
};
use moon_core::util::fmt::DeltaSign;
use moon_ui::{MoonWindowExt as _,

    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonDropdown,
    MoonInput, MoonMenuSize, MoonPalette, h_flex, v_flex,
};
use rust_i18n::t;

use super::LabelEditState;
use crate::chart_tabs::seg_row;
use crate::controls::field_picker;
use crate::design::{self, moon};
use crate::panels::{micro_button, popup_group, popup_group_inset_px, toggle_variant};

/// Selectable font multipliers, inside the config's own clamp range.
///
/// `1.45` is here because it is the Y-scale badge's own default: a control that cannot show the
/// value a caption already carries renders with nothing selected, which reads as "unset".
const SIZE_STEPS: [f32; 7] = [0.75, 1.0, 1.25, 1.45, 1.5, 1.7, 2.0];

/// Fixed colours a caption may take, beyond the theme and the by-sign modes.
const FIXED_COLORS: [u32; 8] = [
    0xffffff, 0xffd166, 0xef476f, 0x06d6a0, 0x4cc9f0, 0xb388ff, 0xff9f1c, 0x8d99ae,
];

/// Colour thresholds offered, in percent. `0` colours everything, which is the default and the
/// way back.
///
/// Steps rather than a free field, like the size control beside it: this dialog holds no numeric
/// input state, and a handful of values covers "stop painting the noise" without one.
const COLOR_MIN_STEPS: [f32; 6] = [0.0, 0.1, 0.25, 0.5, 1.0, 2.0];

/// Width of one micro glyph button.
const MICRO_W: f32 = 20.0;
/// Width of the caption list column.
const LIST_W: f32 = 214.0;

/// Edit the working row and repaint.
fn write_row(state: &Entity<LabelEditState>, cx: &mut App, f: impl FnOnce(&mut LabelEditState)) {
    state.update(cx, |s, cx| {
        f(s);
        s.clamp_selection();
        cx.notify();
    });
}

pub(super) fn dialog_body(state: &Entity<LabelEditState>, cx: &mut App) -> AnyElement {
    let p = MoonPalette::active(cx);
    let (row, selected, name_input) = {
        let s = state.read(cx);
        (s.row.clone(), s.selected, s.name_input.clone())
    };
    // Whether the module has a name to print at all: the user's own, or the preset's.
    let named = !name_input.read(cx).value().trim().is_empty() || row.preset.is_some();

    // What the module is called, and whether the chart prints that name above its figures.
    let name_row = h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(t!("chart_labels.row_name").to_string()),
        )
        .child(
            div()
                .flex_1()
                .child(MoonInput::new("le-name").state(&name_input).small()),
        )
        .child({
            // The MODULE's backing plate: one rectangle behind the whole block, which is why the
            // switch sits here and not on a caption — half a plate under half a line is not a thing
            // the chart can draw.
            let state = state.clone();
            let on = row.plate;
            MoonCheckbox::new("le-plate")
                .label(t!("chart_labels.plate").to_string())
                .checked(on)
                .size(MoonCheckboxSize::Compact)
                .on_change(move |v: &bool, _w, cx| {
                    let v = *v;
                    write_row(&state, cx, |s| s.row.plate = v);
                })
        })
        .child({
            let state = state.clone();
            let on = row.show_name;
            MoonCheckbox::new("le-show-name")
                .label(t!("chart_labels.show_name").to_string())
                .checked(on && named)
                .disabled(!named)
                .size(MoonCheckboxSize::Compact)
                .on_change(move |v: &bool, _w, cx| {
                    let v = *v;
                    write_row(&state, cx, |s| s.row.show_name = v);
                })
        });

    // Which way the module's own captions run. Above the panes because it describes the MODULE,
    // like the name does — the pane on the right answers for one caption only.
    let flow_row = {
        let state = state.clone();
        let current = row.flow;
        seg_row(
            "le-flow".to_string(),
            t!("chart_labels.flow").to_string(),
            LabelFlow::ALL
                .iter()
                .map(|f| (t!(f.locale_key()).to_string(), *f == current))
                .collect(),
            84.0,
            p,
            cx,
            move |pick, cx| {
                if let Some(f) = LabelFlow::ALL.get(pick).copied() {
                    write_row(&state, cx, |s| s.row.flow = f);
                }
            },
        )
    };

    let body = h_flex()
        .w_full()
        .items_start()
        .gap(design::ui_px(cx, 8.0))
        .child(caption_list(state, &row, selected, p, cx))
        .child(caption_settings(state, &row, selected, p, cx));

    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 8.0))
        .child(name_row)
        .child(flow_row)
        .child(body)
        .child(preview(&row, p, cx))
        .into_any_element()
}

/// The module's captions, in print order: pick one, move it, hide it, remove it.
fn caption_list(
    state: &Entity<LabelEditState>,
    row: &moon_core::config::ChartLabelRow,
    selected: usize,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let used = row.used_parts();
    let micro_w = design::font_w(cx, MICRO_W);
    let mut list = v_flex().w_full().gap(design::ui_px(cx, 2.0));
    for ix in 0..used {
        let part = row.parts[ix];
        let is_selected = ix == selected;
        // The name is the pick target: the whole line reads as one control, and the buttons beside
        // it act on the caption the line names.
        let pick = {
            let state = state.clone();
            MoonButton::new(SharedString::from(format!("le-pick-{ix}")))
                .label(t!(part.field.locale_key()).to_string())
                .size(MoonButtonSize::Micro)
                .width(design::font_w(cx, LIST_W - 4.0 * MICRO_W - 10.0))
                .variant(if is_selected {
                    MoonButtonVariant::Soft
                } else {
                    MoonButtonVariant::Ghost
                })
                .selected(is_selected)
                .on_click(move |_, _w, cx: &mut App| {
                    write_row(&state, cx, |s| s.selected = ix);
                })
                .render()
        };
        let up = {
            let state = state.clone();
            micro_button(
                format!("le-up-{ix}"),
                "↑",
                t!("chart_labels.move_left").to_string(),
                MoonButtonVariant::Ghost,
                false,
                micro_w,
                move |_w, cx| {
                    write_row(&state, cx, |s| {
                        if s.row.move_part(ix, true) {
                            s.selected = ix.saturating_sub(1);
                        }
                    })
                },
            )
        };
        let down = {
            let state = state.clone();
            micro_button(
                format!("le-down-{ix}"),
                "↓",
                t!("chart_labels.move_right").to_string(),
                MoonButtonVariant::Ghost,
                false,
                micro_w,
                move |_w, cx| {
                    write_row(&state, cx, |s| {
                        if s.row.move_part(ix, false) {
                            s.selected = ix + 1;
                        }
                    })
                },
            )
        };
        let eye = {
            let state = state.clone();
            let on = part.visible;
            micro_button(
                format!("le-eye-{ix}"),
                if on { "👁" } else { "–" },
                t!(if on {
                    "chart_labels.hide"
                } else {
                    "chart_labels.show"
                })
                .to_string(),
                toggle_variant(on),
                on,
                micro_w,
                move |_w, cx| write_row(&state, cx, |s| s.row.parts[ix].visible = !on),
            )
        };
        let remove = {
            let state = state.clone();
            micro_button(
                format!("le-del-{ix}"),
                "×",
                t!("chart_labels.remove").to_string(),
                MoonButtonVariant::Danger,
                false,
                micro_w,
                move |_w, cx| write_row(&state, cx, |s| s.row.remove_part(ix)),
            )
        };
        list = list.child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 2.0))
                .child(up)
                .child(down)
                .child(eye)
                .child(pick)
                .child(remove),
        );
    }
    if used == 0 {
        list = list.child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(t!("chart_labels.row_empty_hint").to_string()),
        );
    }
    let add = {
        let state = state.clone();
        let row_fields: Vec<ChartLabelField> = row.parts[..used].iter().map(|p| p.field).collect();
        let full = row.first_free_part().is_none();
        let open_state = state.clone();
        // Which picker is up is the DIALOG's state, read where the control is built: a picker whose
        // openness lived in the popover itself would close on mouse-down and eat the pick.
        let picker_open = state.read(cx).picker_open.clone();
        field_picker(
            "le-add",
            match full {
                // A full module says so on the trigger rather than offering a catalogue that cannot
                // be picked from: the count IS the reason, so it is what the label shows.
                true => format!("{}  {used}/{CHART_LABEL_PARTS}", t!("chart_labels.row_full")),
                false => format!("{}  {used}/{CHART_LABEL_PARTS}", t!("chart_labels.add_part")),
            },
            full,
            picker_open.as_deref() == Some("le-add"),
            move |open, cx| LabelEditState::set_picker(&open_state, "le-add", open, cx),
            move |f| row_fields.contains(&f),
            move |field, _window, cx| {
                write_row(&state, cx, |s| {
                    if s.row.push_part(field) {
                        // Selection follows the caption just added: the settings pane beside the
                        // list is where the user is going next.
                        s.selected = s.row.used_parts().saturating_sub(1);
                    }
                    // The pick closes the picker — it is one choice, and the module behind it has
                    // already changed.
                    s.picker_open = None;
                });
            },
            cx,
        )
    };
    // Font-scaled, plus the group's own inset: the line inside is built from `design::font_w`
    // widths, and a column measured with the UI scaler instead drifts from its content the moment
    // the two scales differ — which is what put the remove buttons on top of the pane beside it.
    div()
        .flex_none()
        .w(px(design::font_w(cx, LIST_W) + popup_group_inset_px(cx)))
        .overflow_hidden()
        .child(
            popup_group("le-list", t!("chart_labels.frame_parts")).child(
                v_flex()
                    .w_full()
                    .gap(design::ui_px(cx, 4.0))
                    .child(list)
                    .child(add),
            ),
        )
        .into_any_element()
}

/// Hand the edited module back and put the arbitrage roster up in this dialog's place.
///
/// Applying first is deliberate: the two windows cannot stand on top of each other, so the gear is
/// an OK that opens the other one. Anything typed into the module is therefore kept, not dropped.
fn open_arb_from_editor(state: &Entity<LabelEditState>, window: &mut Window, cx: &mut App) {
    let (row, on_done, on_dismiss, on_open_arb) = {
        let s = state.read(cx);
        (
            s.accepted_row(cx),
            s.on_done.clone(),
            s.on_dismiss.clone(),
            s.on_open_arb.clone(),
        )
    };
    on_done(row, cx);
    on_dismiss(cx);
    window.close_dialog(cx);
    on_open_arb(window, cx);
}

/// Everything about the ONE selected caption.
fn caption_settings(
    state: &Entity<LabelEditState>,
    row: &moon_core::config::ChartLabelRow,
    selected: usize,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let used = row.used_parts();
    if used == 0 {
        return div().flex_1().min_w_0().into_any_element();
    }
    let part: ChartLabelPart = row.parts[selected];
    let resolved = part.resolved_style();
    let mut col = v_flex().w_full().gap(design::ui_px(cx, 6.0));

    // WHAT this caption prints, stated rather than offered: a caption's figure is chosen when the
    // caption is ADDED, and changing one's mind is removing it and adding the one meant instead —
    // which is one click either way and leaves no doubt about where the new caption landed. A
    // selector here was a second door to the same catalogue, and the only one that could silently
    // turn a caption into something the module was never arranged for.
    col = col.child(
        v_flex()
            .w_full()
            .gap(design::ui_px(cx, 2.0))
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .child(t!("chart_labels.field_caption").to_string()),
            )
            .child(
                div()
                    .text_color(moon(p.text))
                    .child(t!(part.field.locale_key()).to_string()),
            ),
    );

    // A field that carries settings of its OWN says so in words. There will be more than one of
    // these — a caption whose subject has a roster, a schedule, a source — and a row of unlabelled
    // gears would leave the reader guessing which is which.
    if part.field.is_column() {
        let state = state.clone();
        col = col.child(
            MoonButton::new("le-arb")
                .label(t!("chart_labels.sub_settings").to_string())
                .size(MoonButtonSize::Micro)
                .variant(MoonButtonVariant::Ghost)
                .on_click(move |_, window: &mut Window, cx: &mut App| {
                    open_arb_from_editor(&state, window, cx);
                })
                .render(),
        );
    }

    // Which orders a position figure counts. Offered only by the fields that read it, so a stale
    // basis cannot sit visible on a caption that ignores it.
    if part.field.uses_pnl_basis() {
        let state = state.clone();
        let current = part.pnl_basis;
        col = col.child(seg_row(
            "le-basis".to_string(),
            t!("chart_labels.basis").to_string(),
            PnlBasis::ALL
                .iter()
                .map(|b| (t!(b.locale_key()).to_string(), *b == current))
                .collect(),
            72.0,
            p,
            cx,
            move |pick, cx| {
                if let Some(b) = PnlBasis::ALL.get(pick).copied() {
                    write_row(&state, cx, |s| s.row.parts[selected].pnl_basis = b);
                }
            },
        ));
    }

    // Which retained-history window a movement or volume figure is read over. A dropdown rather
    // than a segmented row like the basis above: eight windows do not fit the narrower pane, and
    // this control is picked once and then read as a number in the caption itself.
    if part.field.uses_window() {
        let state = state.clone();
        let current = part.window;
        // The FIELD's windows, not every window: the buy/sell split lives only where the retained
        // trades do, and offering the day there offers a pick `sanitize` would take back.
        let items = crate::panels::radio_items(
            part.field.window_choices().iter().map(|w| {
                (
                    *w,
                    SharedString::from(format!("le-w-{w:?}")),
                    SharedString::from(t!(w.locale_key()).to_string()),
                )
            }),
            current,
            crate::panels::RadioMark::Check,
            move |cx, w: LabelWindow| {
                write_row(&state, cx, |s| s.row.parts[selected].window = w);
            },
        );
        col = col.child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text))
                        .child(t!("chart_labels.window").to_string()),
                )
                .child(
                    MoonDropdown::new("le-window")
                        .label(t!(current.locale_key()).to_string())
                        .trigger_caret(true)
                        .trigger_variant(MoonButtonVariant::Soft)
                        .trigger_size(MoonButtonSize::Micro)
                        .trigger_width_scaled(70.0)
                        .menu_width_scaled(90.0)
                        .menu_size(MoonMenuSize::Compact)
                        .items(items),
                ),
        );
    }

    // Size, as a multiplier on the chart's own label size — which already follows the Settings font
    // slider, so a caption scales with the rest of the UI rather than against it.
    col = col.child({
        let state = state.clone();
        let current = SIZE_STEPS
            .iter()
            .position(|s| (s - resolved.size_mult).abs() < 0.01);
        seg_row(
            "le-size".to_string(),
            t!("chart_labels.size").to_string(),
            // Bare numbers, with the multiplier sign moved into the caption: seven segments have to
            // share the narrower of the dialog's two panes, and `1.25x` spends a fifth of a segment
            // on a letter that says the same thing seven times.
            SIZE_STEPS
                .iter()
                .enumerate()
                .map(|(n, v)| (format!("{v}"), Some(n) == current))
                .collect(),
            34.0,
            p,
            cx,
            move |pick, cx| {
                if let Some(v) = SIZE_STEPS.get(pick).copied() {
                    write_row(&state, cx, |s| {
                        s.row.parts[selected].style.size_mult =
                            Some(v.clamp(LABEL_SIZE_MULT_MIN, LABEL_SIZE_MULT_MAX));
                    });
                }
            },
        )
    });

    // Colour MODE. "By profit" reads the value's own sign; a field with no sign keeps the theme
    // colour, so the mode is offered everywhere and simply does nothing on a name.
    col = col.child({
        let state = state.clone();
        let current = match resolved.color {
            LabelColor::Theme => 0,
            LabelColor::BySign => 1,
            LabelColor::Fixed(_) => 2,
        };
        seg_row(
            "le-color".to_string(),
            t!("chart_labels.color").to_string(),
            vec![
                (t!("chart_labels.color_theme").to_string(), current == 0),
                (t!("chart_labels.color_sign").to_string(), current == 1),
                (t!("chart_labels.color_fixed").to_string(), current == 2),
            ],
            72.0,
            p,
            cx,
            move |pick, cx| {
                write_row(&state, cx, |s| {
                    let style = &mut s.row.parts[selected].style;
                    style.color = Some(match pick {
                        0 => LabelColor::Theme,
                        1 => LabelColor::BySign,
                        // Keep whatever fixed colour the caption already had, so switching modes
                        // back and forth does not silently reset the user's choice.
                        _ => match style.color {
                            Some(LabelColor::Fixed(rgb)) => LabelColor::Fixed(rgb),
                            _ => LabelColor::Fixed(FIXED_COLORS[0]),
                        },
                    });
                });
            },
        )
    });
    if let LabelColor::Fixed(picked) = resolved.color {
        let mut swatches = h_flex().gap(design::ui_px(cx, 3.0));
        for value in FIXED_COLORS {
            let state = state.clone();
            // A bare `div` here is the colour itself, not a re-implemented button: the library's
            // own colour control is `MoonColorPicker`, which needs a state entity per caption.
            swatches = swatches.child(
                div()
                    .id(SharedString::from(format!("le-sw-{value:06x}")))
                    .w(design::ui_px(cx, 18.0))
                    .h(design::ui_px(cx, 14.0))
                    .rounded(design::ui_px(cx, 3.0))
                    .bg(rgb(value))
                    .border_1()
                    .border_color(moon(if value == picked { p.text } else { p.border }))
                    .cursor_pointer()
                    .on_click(move |_, _w, cx: &mut App| {
                        write_row(&state, cx, |s| {
                            s.row.parts[selected].style.color = Some(LabelColor::Fixed(value));
                        });
                    }),
            );
        }
        col = col.child(swatches);
    }

    // Whether the colour applies to the figure alone. Offered only when the caption HAS a prefix
    // to leave uncoloured — a bare value has nothing for this switch to spare.
    if part.field.caption_key().is_some() && resolved.caption {
        let state = state.clone();
        let on = resolved.value_only;
        col = col.child(
            MoonCheckbox::new("le-value-only")
                .label(t!("chart_labels.value_only").to_string())
                .checked(on)
                .size(MoonCheckboxSize::Compact)
                .on_change(move |v: &bool, _w, cx| {
                    let v = *v;
                    write_row(&state, cx, |s| {
                        s.row.parts[selected].style.value_only = Some(v)
                    });
                }),
        );
    }

    // From which magnitude a by-sign caption is worth colouring. Percent, so it is offered only by
    // the fields that print one, and only in the mode it applies to: a fixed colour is the user's
    // own choice and a theme colour has nothing to threshold.
    if part.field.is_percent() && resolved.color == LabelColor::BySign {
        let state = state.clone();
        let current = COLOR_MIN_STEPS
            .iter()
            .position(|v| (v - resolved.color_min_pct).abs() < 0.001);
        col = col.child(seg_row(
            "le-color-min".to_string(),
            t!("chart_labels.color_min").to_string(),
            COLOR_MIN_STEPS
                .iter()
                .enumerate()
                .map(|(n, v)| {
                    let label = match *v == 0.0 {
                        true => t!("chart_labels.color_min_off").to_string(),
                        false => format!("{v}%"),
                    };
                    (label, Some(n) == current)
                })
                .collect(),
            48.0,
            p,
            cx,
            move |pick, cx| {
                if let Some(v) = COLOR_MIN_STEPS.get(pick).copied() {
                    write_row(&state, cx, |s| {
                        s.row.parts[selected].style.color_min_pct = Some(v)
                    });
                }
            },
        ));
    }

    let caption_cb = {
        let state = state.clone();
        let on = resolved.caption;
        MoonCheckbox::new("le-caption")
            .label(t!("chart_labels.caption").to_string())
            .checked(on)
            .size(MoonCheckboxSize::Compact)
            .on_change(move |v: &bool, _w, cx| {
                let v = *v;
                write_row(&state, cx, |s| {
                    s.row.parts[selected].style.caption = Some(v)
                });
            })
    };
    let reset = {
        let state = state.clone();
        MoonButton::new("le-style-reset")
            .label(t!("chart_labels.style_reset").to_string())
            .size(MoonButtonSize::Micro)
            .variant(MoonButtonVariant::Ghost)
            .on_click(move |_, _w, cx: &mut App| {
                write_row(&state, cx, |s| {
                    s.row.parts[selected].style = LabelStyle::default();
                });
            })
            .render()
    };
    col = col.child(
        h_flex()
            .w_full()
            .items_center()
            .gap(design::ui_px(cx, 8.0))
            .child(caption_cb)
            .child(reset),
    );
    // `min_w_0` so a long dropdown label cannot push this pane wider than the dialog and slide it
    // under the column beside it.
    div()
        .flex_1()
        .min_w_0()
        .child(popup_group("le-style", t!("chart_labels.frame_caption")).child(col))
        .into_any_element()
}

/// The module as the chart would print it, against sample values.
///
/// Sample rather than live, so every caption answers — see `chartdx::text::preview_row`. Drawn with
/// the real styles: colour mode, size multiplier and prefix all show here, which is what makes this
/// a preview rather than a list of field names.
fn preview(row: &moon_core::config::ChartLabelRow, p: MoonPalette, cx: &App) -> AnyElement {
    let base = design::t_body(cx);
    // The sample runs the way the module does — a column module previews as a block, which is the
    // whole point of asking the question in the editor rather than on the chart.
    let mut line: Div = if row.flow.is_row() {
        h_flex().w_full().flex_wrap().items_baseline().gap_2()
    } else {
        v_flex().w_full().gap_1()
    };
    let captions = crate::chartdx::preview_row(row);
    if captions.is_empty() {
        line = line.child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(t!("chart_labels.preview_empty").to_string()),
        );
    }
    for caption in captions {
        let color = match caption.style.color {
            LabelColor::Theme => p.text,
            LabelColor::Fixed(rgb) => rgb,
            LabelColor::BySign => match caption.sign {
                Some(DeltaSign::Positive) => p.green,
                Some(DeltaSign::Negative) => p.red,
                _ => p.text,
            },
        };
        let size = px(f32::from(base) * caption.style.size_mult);
        // Prefix and value are drawn APART here for the same reason the chart draws them apart:
        // with "colour the value only" on, the word keeps the theme colour and only the figure
        // takes the sign's. Gluing them in the preview would show a colour the chart never paints.
        let split = caption.style.value_only && !caption.prefix.is_empty();
        let (prefix, value) = match split {
            true => (caption.prefix.clone(), caption.text.clone()),
            false => (
                String::new(),
                format!("{}{}", caption.prefix, caption.text),
            ),
        };
        let mut pair = h_flex().items_baseline();
        if !prefix.is_empty() {
            pair = pair.child(
                div()
                    .text_size(size)
                    .text_color(moon(p.text))
                    .child(prefix),
            );
        }
        line = line.child(
            pair.child(
                div()
                    .text_size(size)
                    .text_color(moon(color))
                    .child(value),
            ),
        );
    }
    popup_group("le-preview", t!("chart_labels.preview"))
        .child(
            div()
                .w_full()
                .py(design::ui_px(cx, 4.0))
                .font_family(design::mono())
                .child(line),
        )
        .into_any_element()
}
