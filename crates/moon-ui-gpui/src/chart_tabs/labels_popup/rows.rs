//! The module list: one line per caption module, and the button that adds one.
//!
//! Every control here answers "what is shown and WHERE" — the band, the edge, the stacking order,
//! and whether the module is drawn at all. What a module PRINTS is not asked here: the name opens
//! [`crate::panels::open_label_edit`], and that window owns the captions, their styles and the
//! sample of the result.

use gpui::*;
use moon_core::config::{
    ChartLabelRow, ChartLabelsCfg, LabelAlign, LabelFlow, LabelPreset, LabelZone,
};
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonMenuItem, MoonMenuSize,
    h_flex, v_flex,
};
use rust_i18n::t;

use super::{
    FIELD_W, GAP_STEPS, GAP_W, LabelsPopupHost, MICRO_W, NAME_W, ROW_GAP, ZONE_W, write_cfg,
};
use crate::controls::field_menu_items;
use crate::design;
use crate::panels::{micro_button, open_label_edit, toggle_variant};

/// What the line calls this module: its name, or the captions it prints.
///
/// A module is not required to have a name — most never get one — so the list falls back to naming
/// it by what it does. An empty module with no name says so rather than showing a blank line.
pub(super) fn display_name(row: &ChartLabelRow) -> String {
    if !row.name.trim().is_empty() {
        return row.name.clone();
    }
    let used = row.used_parts();
    if used == 0 {
        return t!("chart_labels.row_empty").to_string();
    }
    let mut out = String::new();
    for part in &row.parts[..used.min(2)] {
        if !out.is_empty() {
            out.push_str(" · ");
        }
        out.push_str(&t!(part.field.locale_key()));
    }
    if used > 2 {
        out.push_str(" · …");
    }
    out
}

/// One module: order, visibility, name (which opens the editor), band, edge, removal.
fn row_line<T: LabelsPopupHost>(
    entity: &Entity<T>,
    cfg: &ChartLabelsCfg,
    ix: usize,
    cx: &App,
) -> AnyElement {
    let row = &cfg.rows[ix];
    let micro_w = design::font_w(cx, MICRO_W);
    let (up, down) = order_buttons(
        format!("cl-row-{ix}"),
        ("chart_labels.move_up", "chart_labels.move_down"),
        micro_w,
        {
            let entity = entity.clone();
            move |up, app: &mut App| {
                write_cfg(&entity, app, |c| {
                    c.move_row(ix, up);
                });
            }
        },
    );
    let eye = {
        let entity = entity.clone();
        let on = row.visible;
        micro_button(
            format!("cl-eye-{ix}"),
            if on { "👁" } else { "–" },
            t!(if on {
                "chart_labels.hide_row"
            } else {
                "chart_labels.show_row"
            })
            .to_string(),
            toggle_variant(on),
            on,
            micro_w,
            move |_w, app| {
                write_cfg(&entity, app, |c| c.rows[ix].visible = !on);
            },
        )
    };
    // Where this module goes relative to the one above it. TWO visible states rather than one
    // toggle, like the edge control beside it: which way a module runs has to be readable without
    // clicking it, and a single glyph answers "what is it now" only from memory.
    let placement_group = {
        let mut group = h_flex().gap(design::ui_px(cx, ROW_GAP));
        for flow in LabelFlow::ALL {
            let entity = entity.clone();
            let selected = row.placement == flow;
            group = group.child(micro_button(
                format!("cl-flow-{ix}-{flow:?}"),
                flow.glyph(),
                t!(if flow.is_row() {
                    "chart_labels.placement_row"
                } else {
                    "chart_labels.placement_column"
                })
                .to_string(),
                toggle_variant(selected),
                selected,
                micro_w,
                move |_w, app| {
                    write_cfg(&entity, app, |c| c.rows[ix].placement = flow);
                },
            ));
        }
        group
    };
    // The name IS the edit button: one target for "which module is this" and "open it", which is
    // how the reader already treats the line.
    let name_btn = {
        let entity = entity.clone();
        MoonButton::new(SharedString::from(format!("cl-open-{ix}")))
            .label(format!("{}  ·{}", display_name(row), row.used_parts()))
            .size(MoonButtonSize::Micro)
            .width(design::font_w(cx, NAME_W))
            .variant(MoonButtonVariant::Ghost)
            .tooltip(t!("chart_labels.row_edit").to_string())
            .on_click(move |_, window: &mut Window, app: &mut App| {
                edit_row(&entity, window, app, ix);
            })
            .render()
    };
    let zone_dd = MoonDropdown::new(SharedString::from(format!("cl-zone-{ix}")))
        .label(t!(row.zone.locale_key()).to_string())
        .trigger_caret(true)
        .trigger_variant(MoonButtonVariant::Soft)
        .trigger_size(MoonButtonSize::Micro)
        .trigger_width_scaled(ZONE_W)
        .menu_width_scaled(168.0)
        .menu_size(MoonMenuSize::Compact)
        .items(zone_items(entity, cfg, ix));
    // Where in the band the module sits. Three visible states rather than one cycling button: the
    // current edge has to be readable at a glance, and a cycler answers "what is it now" only from
    // memory.
    let align_group = {
        let mut group = h_flex().gap(design::ui_px(cx, ROW_GAP));
        for align in LabelAlign::ALL {
            let entity = entity.clone();
            let selected = row.align == align;
            group = group.child(micro_button(
                format!("cl-al-{ix}-{align:?}"),
                align.glyph(),
                t!(align.locale_key()).to_string(),
                toggle_variant(selected),
                selected,
                micro_w,
                move |_w, app| {
                    write_cfg(&entity, app, |c| c.rows[ix].align = align);
                },
            ));
        }
        group
    };
    // Space before this module, in the direction its band runs — see `ChartLabelRow::gap`. Here
    // rather than in the editor because spacing is judged against the CHART, and this popup stays
    // open while the chart redraws.
    let gap_dd = {
        let entity = entity.clone();
        let current = row.gap;
        let items = crate::panels::radio_items(
            GAP_STEPS.iter().map(|g| {
                (
                    *g,
                    SharedString::from(format!("cl-gap-{ix}-{g}")),
                    SharedString::from(g.to_string()),
                )
            }),
            current,
            crate::panels::RadioMark::Check,
            move |app, g: u8| {
                write_cfg(&entity, app, |c| c.rows[ix].gap = g);
            },
        );
        MoonDropdown::new(SharedString::from(format!("cl-gapdd-{ix}")))
            .label(row.gap.to_string())
            .trigger_caret(false)
            .trigger_variant(if row.gap == 0 {
                MoonButtonVariant::Ghost
            } else {
                MoonButtonVariant::Soft
            })
            .trigger_size(MoonButtonSize::Micro)
            .trigger_width_scaled(GAP_W)
            .menu_width_scaled(70.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    };
    let remove = {
        let entity = entity.clone();
        micro_button(
            format!("cl-del-{ix}"),
            "×",
            t!("chart_labels.remove_row").to_string(),
            MoonButtonVariant::Danger,
            false,
            micro_w,
            move |_w, app| {
                write_cfg(&entity, app, |c| c.remove_row(ix));
            },
        )
    };
    h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, ROW_GAP))
        .child(up)
        .child(down)
        .child(eye)
        .child(placement_group)
        .child(name_btn)
        .child(zone_dd)
        .child(align_group)
        .child(gap_dd)
        .child(remove)
        .into_any_element()
}

/// The list, or the line that says there is nothing in it yet.
pub(super) fn row_list<T: LabelsPopupHost>(
    entity: &Entity<T>,
    cfg: &ChartLabelsCfg,
    empty_color: u32,
    cx: &App,
) -> AnyElement {
    let used = cfg.used_rows();
    let mut list = v_flex().w_full().gap(design::ui_px(cx, 4.0));
    for ix in 0..used {
        list = list.child(row_line(entity, cfg, ix, cx));
    }
    if used == 0 {
        list = list.child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(empty_color))
                .child(t!("chart_labels.empty").to_string()),
        );
    }
    list.into_any_element()
}

/// The "add a module" control: ready-made modules first, then the whole catalogue.
///
/// Either way the editor opens on what was just created — a new module is exactly the thing the
/// user then wants to fill in, which is the "add" half of what this button does.
pub(super) fn add_row_dropdown<T: LabelsPopupHost>(
    id: &str,
    entity: &Entity<T>,
    cfg: &ChartLabelsCfg,
) -> impl IntoElement {
    let mut items = Vec::new();
    for preset in LabelPreset::ALL {
        let entity = entity.clone();
        let name = t!(preset.locale_key()).to_string();
        items.push(
            MoonMenuItem::with_key(
                SharedString::from(format!("cl-preset-{preset:?}")),
                SharedString::from(name.clone()),
            )
            .on_click(move |_, window: &mut Window, app: &mut App| {
                let name = name.clone();
                add_and_edit(&entity, window, app, move |cfg| {
                    cfg.push_preset(preset, name)
                });
            }),
        );
    }
    items.push(MoonMenuItem::separator());
    items.extend({
        let entity = entity.clone();
        let configured = cfg.clone();
        field_menu_items(
            "cl-addrow",
            move |f| configured.contains(f),
            move |field, window, app| {
                add_and_edit(&entity, window, app, move |cfg| {
                    cfg.push_row(
                        field,
                        ChartLabelRow::DEFAULT_ZONE,
                        ChartLabelRow::DEFAULT_ALIGN,
                    )
                });
            },
        )
    });
    MoonDropdown::new(SharedString::from(format!("{id}-add-row")))
        .label(t!("chart_labels.add_row").to_string())
        .trigger_caret(true)
        .trigger_variant(MoonButtonVariant::Soft)
        .trigger_size(MoonButtonSize::Micro)
        .trigger_width_scaled(NAME_W + FIELD_W)
        .menu_width_scaled(190.0)
        .menu_size(MoonMenuSize::Compact)
        .disabled(cfg.first_free_row().is_none())
        .items(items)
}

/// Band choices, the plot's family first and the control strip's after a separator.
fn zone_items<T: LabelsPopupHost>(
    entity: &Entity<T>,
    cfg: &ChartLabelsCfg,
    ix: usize,
) -> Vec<MoonMenuItem> {
    let entity = entity.clone();
    let mut items = crate::panels::radio_items(
        LabelZone::ALL.iter().map(|z| {
            (
                *z,
                SharedString::from(format!("cl-z-{ix}-{z:?}")),
                SharedString::from(t!(z.locale_key()).to_string()),
            )
        }),
        cfg.rows[ix].zone,
        crate::panels::RadioMark::Check,
        move |app, z: LabelZone| {
            write_cfg(&entity, app, |c| c.rows[ix].zone = z);
        },
    );
    // `LabelZone::ALL` lists the plot's two bands and then the strip's, so the boundary is a fixed
    // position rather than a second pass with a filter.
    let strip_at = LabelZone::ALL
        .iter()
        .position(|z| z.is_control_zone())
        .unwrap_or(items.len());
    items.insert(strip_at, MoonMenuItem::separator());
    items
}

/// The ↑/↓ pair, built once for both of its uses.
fn order_buttons(
    id: String,
    tips: (&'static str, &'static str),
    width: f32,
    on_move: impl Fn(bool, &mut App) + Clone + 'static,
) -> (impl IntoElement, impl IntoElement) {
    let down_move = on_move.clone();
    (
        micro_button(
            format!("{id}-up"),
            "↑",
            t!(tips.0).to_string(),
            MoonButtonVariant::Ghost,
            false,
            width,
            move |_w, app| on_move(true, app),
        ),
        micro_button(
            format!("{id}-down"),
            "↓",
            t!(tips.1).to_string(),
            MoonButtonVariant::Ghost,
            false,
            width,
            move |_w, app| down_move(false, app),
        ),
    )
}

/// Open the editor on the module at `ix` and write back what it accepts.
///
/// The dialog edits a COPY, and the write is stamped with the TAB it came from. The overlay blocks
/// this window's own input, but not a ⧉ press from a detached window, a hotkey that switches tabs,
/// or Main's idle-close — and an index alone would then name a different module, or a blank slot
/// that `sanitize` would resurrect as a module the user had removed.
fn edit_row<T: LabelsPopupHost>(entity: &Entity<T>, window: &mut Window, app: &mut App, ix: usize) {
    let (row, key) = {
        let host = entity.read(app);
        let Some(row) = host.labels_cfg(app).rows.get(ix).cloned() else {
            return;
        };
        (row, host.spec_key())
    };
    let title = format!("{}: {}", t!("chart_labels.row_edit"), display_name(&row));
    // The popup is a POPOVER: it paints in a deferred layer above the dialog's overlay, so leaving
    // it open would put the list on top of the editor. It closes here and comes back when the
    // editor does, which is also where the user expects to land after editing a module.
    entity.update(app, |this, cx| {
        this.set_labels_popup_open(false);
        cx.notify();
    });
    let apply_entity = entity.clone();
    let reopen_entity = entity.clone();
    open_label_edit(
        title,
        row,
        window,
        app,
        move |edited, app| {
            if apply_entity.read(app).spec_key() != key {
                log::warn!("подписи чарта: вкладка сменилась, правка модуля не применена");
                return;
            }
            write_cfg(&apply_entity, app, |c| {
                // Past the used run the slot is blank, and writing there would bring back a module
                // somebody else removed while the editor was up.
                if ix < c.used_rows() {
                    c.rows[ix] = edited.clone();
                }
            });
        },
        move |app| {
            reopen_entity.update(app, |this, cx| {
                this.set_labels_popup_open(true);
                cx.notify();
            });
        },
    );
}

/// Create a module through `add`, store it, and open the editor on it.
fn add_and_edit<T: LabelsPopupHost>(
    entity: &Entity<T>,
    window: &mut Window,
    app: &mut App,
    add: impl FnOnce(&mut ChartLabelsCfg) -> Option<usize>,
) {
    let created = entity.update(app, |this, cx| {
        let mut cfg = this.labels_cfg(cx);
        let ix = add(&mut cfg)?;
        cfg.sanitize();
        this.apply_labels(cfg, cx);
        Some(ix)
    });
    if let Some(ix) = created {
        edit_row(entity, window, app, ix);
    }
}
