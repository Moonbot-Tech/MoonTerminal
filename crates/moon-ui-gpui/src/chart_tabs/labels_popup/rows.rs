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
use crate::controls::row_display_name;
use crate::design;
use crate::panels::{micro_button, open_label_edit, toggle_variant};

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
            .label(format!("{}  ·{}", row_display_name(row), row.used_parts()))
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

/// The "add a module" control: the ready-made modules, and an empty one.
///
/// The FIELD catalogue is deliberately not repeated here. It lives in the editor, which opens on
/// whatever this creates — so picking a field from this menu would be the same catalogue, one level
/// earlier, answering a question ("which single caption") that no module is finished at. What this
/// menu answers is "start from something" or "start from nothing".
pub(super) fn add_row_dropdown<T: LabelsPopupHost>(
    id: &str,
    entity: &Entity<T>,
    cfg: &ChartLabelsCfg,
) -> impl IntoElement {
    let mut items = Vec::new();
    for preset in LabelPreset::ALL {
        let entity = entity.clone();
        items.push(
            MoonMenuItem::with_key(
                SharedString::from(format!("cl-preset-{preset:?}")),
                SharedString::from(t!(preset.locale_key()).to_string()),
            )
            .on_click(move |_, window: &mut Window, app: &mut App| {
                // The NAME is not passed: the module remembers the preset and is named from the
                // dictionary every time it is drawn, so it follows a language switch.
                let created = add_row(&entity, app, move |cfg| cfg.push_preset(preset));
                let Some(ix) = created else {
                    return;
                };
                // The arbitrage module is finished the moment it is created — its captions are the
                // venues the core reports — so it opens the ROSTER instead of the caption editor,
                // which is the question the user actually has next.
                match preset {
                    LabelPreset::Arbitrage => open_arb_settings(&entity, window, app),
                    _ => edit_row(&entity, window, app, ix),
                }
            }),
        );
    }
    items.push(MoonMenuItem::separator());
    items.push(
        MoonMenuItem::with_key(
            SharedString::from("cl-new-row"),
            SharedString::from(t!("chart_labels.new_row").to_string()),
        )
        .on_click({
            let entity = entity.clone();
            move |_, window: &mut Window, app: &mut App| new_row(&entity, window, app)
        }),
    );
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
fn edit_row<T: LabelsPopupHost>(entity: &Entity<T>, window: &mut Window, app: &mut App, ix: usize) {
    let Some(row) = entity.read(app).labels_cfg(app).rows.get(ix).cloned() else {
        return;
    };
    let title = format!("{}: {}", t!("chart_labels.row_edit"), row_display_name(&row));
    open_module_editor(entity, window, app, row, title, move |cfg, edited| {
        // Past the used run the slot is blank, and writing there would bring back a module
        // somebody else removed while the editor was up.
        if ix < cfg.used_rows() {
            cfg.rows[ix] = edited;
        }
    });
}

/// Open the editor on a module that does not exist yet, and add it only if it is accepted.
///
/// Nothing is written before the editor opens, deliberately: an empty module is blank, `sanitize`
/// drops a blank module on the next write, and a slot reserved for one would either vanish under
/// the open editor or survive a Cancel as a module the user never made.
fn new_row<T: LabelsPopupHost>(entity: &Entity<T>, window: &mut Window, app: &mut App) {
    let row = ChartLabelRow::new(ChartLabelRow::DEFAULT_ZONE, ChartLabelRow::DEFAULT_ALIGN);
    let title = t!("chart_labels.new_row").to_string();
    open_module_editor(entity, window, app, row, title, |cfg, edited| {
        if cfg.push_prepared(edited).is_none() {
            log::warn!("подписи чарта: новый модуль не добавлен — нет свободного слота или он пуст");
        }
    });
}

/// Put the editor up on `row` and hand what it accepts to `apply`, on the tab it came from.
///
/// The dialog edits a COPY, and the write is stamped with the TAB it was opened from. The overlay
/// blocks this window's own input, but not a ⧉ press from a detached window, a hotkey that switches
/// tabs, or Main's idle-close — and a write that ignored the tab would then land on a different
/// chart's configuration.
fn open_module_editor<T: LabelsPopupHost>(
    entity: &Entity<T>,
    window: &mut Window,
    app: &mut App,
    row: ChartLabelRow,
    title: String,
    apply: impl Fn(&mut ChartLabelsCfg, ChartLabelRow) + 'static,
) {
    let key = entity.read(app).spec_key();
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
            write_cfg(&apply_entity, app, |cfg| apply(cfg, edited.clone()));
        },
        move |app| {
            reopen_entity.update(app, |this, cx| {
                this.set_labels_popup_open(true);
                cx.notify();
            });
        },
        {
            let entity = entity.clone();
            move |window: &mut Window, app: &mut App| open_arb_settings(&entity, window, app)
        },
    );
}

/// Create a module through `add` and store it, returning where it landed.
fn add_row<T: LabelsPopupHost>(
    entity: &Entity<T>,
    app: &mut App,
    add: impl FnOnce(&mut ChartLabelsCfg) -> Option<usize>,
) -> Option<usize> {
    entity.update(app, |this, cx| {
        let mut cfg = this.labels_cfg(cx);
        let ix = add(&mut cfg)?;
        cfg.sanitize();
        this.apply_labels(cfg, cx);
        Some(ix)
    })
}

/// Put up the GLOBAL arbitrage roster and publish every edit it makes.
///
/// Global, so it does not go through `write_cfg` — nothing here belongs to a tab. It writes the
/// backend's own handle and saves the file, which is what makes the change reach every open chart:
/// each panel hands that handle to its engine on the next render.
fn open_arb_settings<T: LabelsPopupHost>(entity: &Entity<T>, window: &mut Window, app: &mut App) {
    let backend = entity.read(app).backend().clone();
    let cfg = backend.read(app).arb_view.as_ref().clone();
    // The popup is a popover and paints above this window's overlay; it closes here like it does
    // for the module editor, and comes back with it.
    entity.update(app, |this, cx| {
        this.set_labels_popup_open(false);
        cx.notify();
    });
    let reopen = entity.clone();
    crate::panels::open_arb_edit(
        cfg,
        window,
        app,
        move |cfg, app| {
            cfg.save();
            backend.update(app, |b, cx| {
                b.arb_view = std::rc::Rc::new(cfg);
                cx.notify();
            });
        },
        // The popup comes back when the WINDOW closes, not on every edit: it is a popover and
        // paints above this window's overlay, so reopening it per click would cover the roster
        // being edited.
        move |app| {
            reopen.update(app, |this, cx| {
                this.set_labels_popup_open(true);
                cx.notify();
            });
        },
    );
}
