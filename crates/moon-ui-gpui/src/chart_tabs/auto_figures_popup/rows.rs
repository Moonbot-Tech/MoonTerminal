//! The row list: one line per auto-figure row, and the menu that adds one.
//!
//! Every control on a line answers "which rows, in which order, shown or not". What a row looks
//! for and how it looks is not asked here: the name opens [`open_auto_figure_row_editor`].

use gpui::*;
use moon_core::config::ChartTheme;
use moon_core::config::auto_figures::{
    AF_ROWS_MAX, AutoFigurePreset, AutoFigureRow, AutoFiguresCfg, DetectorKind,
};
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonMenuItem, MoonPalette, h_flex,
    v_flex,
};
use rust_i18n::t;

use super::status::RowStatus;
use super::{AutoFiguresPopupHost, MICRO_W, NAME_W, ROW_GAP, SWATCH_W, TF_W, write_cfg};
use crate::design;
use crate::panels::{
    line_swatch, micro_button, open_auto_figure_row_editor, open_new_auto_figure_row, row_title,
    tf_source_label, toggle_variant,
};

/// Opacity of the list while the master switch is off: still readable and editable, visibly not
/// in force.
const OFF_OPACITY: f32 = 0.55;

/// Rendered width the name button leaves to its own text, once its padding is taken out.
///
/// MoonUI paints a button label as a `flex_none` child with no ellipsis, so a longer name would
/// paint past the border over the timeframe beside it; the fit happens before the string reaches
/// the widget (the captions popup's `name_budget`).
fn name_budget(cx: &App) -> f32 {
    (design::font_w(cx, NAME_W)
        - 2.0 * design::ui_value(cx, design::CONTROL_TIER.control_metrics().pad_x))
    .max(0.0)
}

/// A row's name fitted to the name button.
pub(super) fn fitted_name(
    row: &AutoFigureRow,
    budget: f32,
    measure: impl Fn(&str) -> f32,
) -> String {
    design::fit_text(&row_title(row), budget, &measure).0
}

/// One row: order, visibility, its line, its name (which opens the editor), its timeframe,
/// removal — and under it, what its last run could not do, in words.
fn row_line<T: AutoFiguresPopupHost>(
    entity: &Entity<T>,
    cfg: &AutoFiguresCfg,
    ix: usize,
    status: Option<RowStatus>,
    theme: &ChartTheme,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let row = &cfg.rows[ix];
    let known = row.detector.known().is_some();
    let micro_w = design::font_w(cx, MICRO_W);
    let order = |up: bool| {
        let entity = entity.clone();
        micro_button(
            format!("af-row-{ix}-{}", if up { "up" } else { "down" }),
            if up { "↑" } else { "↓" },
            t!(if up {
                "auto_figures.move_up"
            } else {
                "auto_figures.move_down"
            })
            .to_string(),
            MoonButtonVariant::Ghost,
            false,
            micro_w,
            move |_w, app| {
                write_cfg(&entity, app, |c| {
                    let to = if up { ix.checked_sub(1) } else { Some(ix + 1) };
                    if let Some(to) = to.filter(|to| *to < c.rows.len()) {
                        c.rows.swap(ix, to);
                    }
                });
            },
        )
    };
    let spacer = |w: f32| div().flex_none().w(px(w));
    // An unknown detector draws nothing whatever its switch says, so the eye would be a control
    // that changes nothing; the slot stays so the columns line up.
    let eye = match known {
        true => {
            let entity = entity.clone();
            let on = row.visible;
            micro_button(
                format!("af-eye-{ix}"),
                if on { "👁" } else { "–" },
                t!(if on {
                    "auto_figures.hide_row"
                } else {
                    "auto_figures.show_row"
                })
                .to_string(),
                toggle_variant(on),
                on,
                micro_w,
                move |_w, app| {
                    write_cfg(&entity, app, |c| {
                        if let Some(r) = c.rows.get_mut(ix) {
                            r.visible = !on;
                        }
                    });
                },
            )
            .into_any_element()
        }
        false => spacer(micro_w).into_any_element(),
    };
    let swatch_w = design::font_w(cx, SWATCH_W);
    let swatch = match known {
        true => div()
            .flex_none()
            .w(px(swatch_w))
            .child(line_swatch(
                row.style,
                theme,
                swatch_w,
                f32::from(design::ui_px(cx, 12.0)),
            ))
            .into_any_element(),
        false => spacer(swatch_w).into_any_element(),
    };
    let name_btn = {
        let entity = entity.clone();
        let label = fitted_name(row, name_budget(cx), |text| {
            design::ui_text_width_zoomed(cx, text, design::BODY_TEXT, 400.0, true)
        });
        MoonButton::new(SharedString::from(format!("af-open-{ix}")))
            .label(label)
            .width(design::font_w(cx, NAME_W))
            .variant(MoonButtonVariant::Ghost)
            // A row this build cannot interpret is listed, moved and removed — never edited.
            .disabled(!known)
            .tooltip(t!("auto_figures.row_edit").to_string())
            .on_click(move |_, window: &mut Window, app: &mut App| {
                edit_row(&entity, window, app, ix);
            })
            .render()
    };
    let tf = div()
        .flex_none()
        .w(px(design::font_w(cx, TF_W)))
        .text_size(design::t_caption(cx))
        .text_color(rgb(p.text_muted))
        .child(if known {
            tf_source_label(row.tf)
        } else {
            String::new()
        });
    let remove = {
        let entity = entity.clone();
        micro_button(
            format!("af-del-{ix}"),
            "×",
            t!("auto_figures.remove_row").to_string(),
            MoonButtonVariant::Danger,
            false,
            micro_w,
            move |_w, app| {
                write_cfg(&entity, app, |c| {
                    if ix < c.rows.len() {
                        c.rows.remove(ix);
                    }
                });
            },
        )
    };
    let line = h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, ROW_GAP))
        .child(order(true))
        .child(order(false))
        .child(eye)
        .child(swatch)
        .child(name_btn)
        .child(tf)
        .child(remove);
    // The mark under the line, in words: a tooltip inside a popover is invisible in this fork, and
    // a glyph alone would leave the reader guessing which of four problems it means.
    let note = match (&row.detector, status) {
        (moon_core::config::auto_figures::RowDetector::Unknown(u), _) => {
            Some(t!("auto_figures.unknown", kind = u.kind()).to_string())
        }
        (_, Some(status)) if row.visible => Some(t!(status.locale_key()).to_string()),
        _ => None,
    };
    let mut block = v_flex().w_full().gap(design::ui_px(cx, 1.0)).child(line);
    if let Some(note) = note {
        block = block.child(
            div()
                .w_full()
                .pl(px(
                    2.0 * micro_w + 2.0 * f32::from(design::ui_px(cx, ROW_GAP))
                ))
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.orange))
                .child(note),
        );
    }
    block.into_any_element()
}

/// The list, or the line that says there is nothing in it yet. Dimmed while the switch is off.
pub(super) fn row_list<T: AutoFiguresPopupHost>(
    entity: &Entity<T>,
    cfg: &AutoFiguresCfg,
    statuses: &[Option<RowStatus>],
    theme: &ChartTheme,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let mut list = v_flex().w_full().gap(design::ui_px(cx, 4.0));
    for ix in 0..cfg.rows.len() {
        let status = statuses.get(ix).copied().flatten();
        list = list.child(row_line(entity, cfg, ix, status, theme, p, cx));
    }
    if cfg.rows.is_empty() {
        list = list.child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(t!("auto_figures.empty").to_string()),
        );
    }
    if !cfg.enabled {
        list = list.opacity(OFF_OPACITY);
    }
    list.into_any_element()
}

/// The "add a row" control: the ready-made rows, then an empty row of each detector kind.
pub(super) fn add_row_dropdown<T: AutoFiguresPopupHost>(
    id: &str,
    entity: &Entity<T>,
    cfg: &AutoFiguresCfg,
    cx: &App,
) -> impl IntoElement {
    let mut items = Vec::new();
    for preset in AutoFigurePreset::ALL {
        let entity = entity.clone();
        items.push(
            MoonMenuItem::with_key(
                SharedString::from(format!("af-preset-{preset:?}")),
                SharedString::from(t!(preset.locale_key()).to_string()),
            )
            .on_click(move |_, window: &mut Window, app: &mut App| {
                // The NAME is not stored: the row remembers its preset and is named from the
                // dictionary every time it is drawn, so it follows a language switch.
                let mut at = None;
                write_cfg(&entity, app, |c| {
                    if c.rows.len() < AF_ROWS_MAX {
                        c.rows.push(preset.row());
                        at = Some(c.rows.len() - 1);
                    }
                });
                if let Some(ix) = at {
                    edit_row(&entity, window, app, ix);
                }
            }),
        );
    }
    items.push(MoonMenuItem::separator());
    for kind in DetectorKind::ALL {
        let entity = entity.clone();
        items.push(
            MoonMenuItem::with_key(
                SharedString::from(format!("af-new-{kind:?}")),
                SharedString::from(
                    t!(
                        "auto_figures.new_row",
                        kind = t!(crate::panels::auto_figure_kind_key(kind))
                    )
                    .to_string(),
                ),
            )
            .on_click(move |_, window: &mut Window, app: &mut App| {
                new_row(&entity, window, app, kind);
            }),
        );
    }
    MoonDropdown::new(SharedString::from(format!("{id}-add-row")))
        .label(format!("＋ {}", t!("auto_figures.add_row")))
        .trigger_caret(true)
        .trigger_variant(MoonButtonVariant::Soft)
        .trigger_size(MoonButtonSize::density(cx))
        .trigger_width_scaled(NAME_W)
        .fit_menu_width(190.0, 560.0)
        .disabled(cfg.rows.len() >= AF_ROWS_MAX)
        .items(items)
}

/// Open the editor on the row at `ix`, stepping the popup aside while it is up.
fn edit_row<T: AutoFiguresPopupHost>(
    entity: &Entity<T>,
    window: &mut Window,
    app: &mut App,
    ix: usize,
) {
    let (backend, kind) = {
        let host = entity.read(app);
        (host.backend().clone(), host.source_kind(app))
    };
    let reopen = entity.clone();
    let opened = open_auto_figure_row_editor(&backend, kind, ix, window, app, move |app| {
        reopen.update(app, |this, cx| this.resume_auto_figures_popup(cx));
    });
    if opened {
        entity.update(app, |this, cx| this.suspend_auto_figures_popup(cx));
    }
}

/// Open the editor on a new row of `kind`, added only if it is accepted.
fn new_row<T: AutoFiguresPopupHost>(
    entity: &Entity<T>,
    window: &mut Window,
    app: &mut App,
    detector: DetectorKind,
) {
    let (backend, kind) = {
        let host = entity.read(app);
        (host.backend().clone(), host.source_kind(app))
    };
    let reopen = entity.clone();
    open_new_auto_figure_row(&backend, kind, detector, window, app, move |app| {
        reopen.update(app, |this, cx| this.resume_auto_figures_popup(cx));
    });
    entity.update(app, |this, cx| this.suspend_auto_figures_popup(cx));
}
