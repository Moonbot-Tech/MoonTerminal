//! The "Chart labels" popup configures WHAT the chart prints beside its plot, WHERE, and HOW.
//!
//! One row per configured caption, in draw order: the arrows move a caption earlier or later, the
//! eye hides it without losing its style, the field and zone dropdowns say what it prints and in
//! which corner, the chain toggle joins it to the previous caption's row, and `⋯` expands that
//! row's style. `＋` adds a caption from the catalogue, grouped by where its value comes from.
//!
//! Like the layout, candle and graphics popups beside it, these settings are PER TAB: the target is
//! the tab strip's active tab or the detached window's panel, persisted to `charts.json` through
//! `ChartTabSpec::chart_labels`, with a tab that has no override following the global
//! `layout.chart_labels` default. ⧉ distributes this target's configuration to all Add/Custom tabs
//! and detached windows and updates that default, including Main only when Main is the source.
//!
//! The style panel expands INLINE rather than in a second popover: a `MoonDropdown` inside a
//! `MoonPopover` is safe (fork fix `0f3ace9`), but a popover inside a popover has no precedent in
//! this codebase and its deferred-layer behaviour is unproven.

use gpui::*;
use moon_core::config::{
    ChartLabelField, ChartLabelGroup, ChartLabelsCfg, LABEL_SIZE_MULT_MAX, LABEL_SIZE_MULT_MIN,
    LabelColor, LabelZone, PnlBasis,
};
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonDropdown,
    MoonMenuItem, MoonMenuSize, MoonPalette, MoonPopover, MoonPopoverPlacement, h_flex, v_flex,
};
use rust_i18n::t;

use super::common::{LayoutPopupHost, StackSetting, seg_row};
use crate::design;
use crate::panels::{
    popup_apply_all_button, popup_close_button, popup_group, popup_group_inset_px, popup_title,
};

/// Selectable font multipliers, inside the config's own clamp range.
///
/// Steps rather than a slider for the reason the graphics popup states: `MoonSlider` needs a state
/// entity on the host, and these chart popups are stateless by design.
const SIZE_STEPS: [f32; 6] = [0.75, 1.0, 1.25, 1.5, 1.7, 2.0];

/// Fixed colors a caption may take, beyond the theme and the by-sign modes.
///
/// A palette row rather than `MoonColorPicker`: the picker needs a state entity per row, and eight
/// legible choices cover "make this one stand out" without one.
const FIXED_COLORS: [u32; 8] = [
    0xffffff, 0xffd166, 0xef476f, 0x06d6a0, 0x4cc9f0, 0xb388ff, 0xff9f1c, 0x8d99ae,
];

/// Popup CONTENT width in rendered pixels.
///
/// Sized on the slot row, which is the widest thing here: three micro buttons, a 104-unit field
/// dropdown, an 88-unit zone dropdown and three more micro buttons, plus their gaps.
pub(super) fn content_width(cx: &App) -> Pixels {
    px(8.0 * 42.0 + popup_group_inset_px(cx))
}

/// Edit the target's configuration: load, mutate, sanitize, apply.
///
/// Sanitizing on every write rather than at read time is what keeps an impossible state from being
/// persisted at all — an inline caption that opens its zone, a size outside the drawable range.
fn write_cfg<T: LabelsPopupHost>(
    entity: &Entity<T>,
    app: &mut App,
    f: impl FnOnce(&mut ChartLabelsCfg),
) {
    entity.update(app, |this, cx| {
        let mut cfg = this.labels_cfg(cx);
        f(&mut cfg);
        cfg.sanitize();
        this.apply_labels(cfg, cx);
    });
}

/// Move a caption one place and carry its expanded style panel with it.
///
/// Without this the panel stays on the INDEX rather than on the caption, so moving a row makes the
/// open panel describe its neighbour.
fn move_slot_keeping_style<T: LabelsPopupHost>(
    entity: &Entity<T>,
    app: &mut App,
    ix: usize,
    up: bool,
) {
    entity.update(app, |this, cx| {
        let mut cfg = this.labels_cfg(cx);
        if !cfg.move_slot(ix, up) {
            return;
        }
        let other = if up { ix - 1 } else { ix + 1 };
        match this.labels_style_open() {
            Some(open) if open == ix => this.set_labels_style_open(Some(other)),
            Some(open) if open == other => this.set_labels_style_open(Some(ix)),
            _ => {}
        }
        this.apply_labels(cfg, cx);
    });
}

/// A micro glyph button, matching the detect popup's slot toggles.
///
/// One builder for the whole row: a toggle and an action differ only in the variant they take, and
/// two builders drifted apart on size and width the moment either was touched.
fn micro_button(
    id: String,
    glyph: &'static str,
    tip: String,
    variant: MoonButtonVariant,
    selected: bool,
    on_click: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    MoonButton::new(SharedString::from(id))
        .label(glyph)
        .size(MoonButtonSize::Micro)
        .width(20.0)
        .variant(variant)
        .selected(selected)
        .tooltip(tip)
        .on_click(move |_, _w, app: &mut App| on_click(app))
        .render()
}

/// The variant a toggle takes for its current state.
fn toggle_variant(on: bool) -> MoonButtonVariant {
    if on {
        MoonButtonVariant::Blue
    } else {
        MoonButtonVariant::Ghost
    }
}

/// Build the catalogue menu, sectioned by where a field's value comes from.
fn catalogue_items<T: LabelsPopupHost>(
    entity: &Entity<T>,
    cfg: &ChartLabelsCfg,
) -> Vec<MoonMenuItem> {
    let mut items = Vec::new();
    for (n, group) in ChartLabelGroup::ALL.into_iter().enumerate() {
        if n > 0 {
            items.push(MoonMenuItem::separator());
        }
        for field in ChartLabelField::ALL
            .into_iter()
            .filter(|f| f.group() == group)
        {
            let entity = entity.clone();
            // Checked rather than disabled: one field in two corners is a legitimate layout, and
            // the mark is there to answer "did I already add this?".
            items.push(
                MoonMenuItem::with_key(
                    SharedString::from(format!("cl-add-{field:?}")),
                    SharedString::from(t!(field.locale_key()).to_string()),
                )
                .checked(cfg.contains(field))
                .on_click(move |_, _w, app| {
                    // Added into the control strip, where the chart's other captions live: a new
                    // label lands somewhere the reader is already looking, and the zone dropdown
                    // moves it from there.
                    write_cfg(&entity, app, |c| {
                        c.push(field, LabelZone::ZoneTop);
                    });
                }),
            );
        }
    }
    items
}

/// One caption's row: order, visibility, field, zone, inline, style and removal.
fn slot_row<T: LabelsPopupHost>(
    entity: &Entity<T>,
    cfg: &ChartLabelsCfg,
    ix: usize,
    style_open: bool,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let slot = cfg.slots[ix];
    let up = {
        let entity = entity.clone();
        micro_button(
            format!("cl-up-{ix}"),
            "↑",
            t!("chart_labels.move_up").to_string(),
            MoonButtonVariant::Ghost,
            false,
            move |app| {
                move_slot_keeping_style(&entity, app, ix, true);
            },
        )
    };
    let down = {
        let entity = entity.clone();
        micro_button(
            format!("cl-down-{ix}"),
            "↓",
            t!("chart_labels.move_down").to_string(),
            MoonButtonVariant::Ghost,
            false,
            move |app| {
                move_slot_keeping_style(&entity, app, ix, false);
            },
        )
    };
    let eye = {
        let entity = entity.clone();
        let on = slot.visible;
        micro_button(
            format!("cl-eye-{ix}"),
            if on { "👁" } else { "–" },
            t!(if on {
                "chart_labels.hide"
            } else {
                "chart_labels.show"
            })
            .to_string(),
            toggle_variant(on),
            on,
            move |app| {
                write_cfg(&entity, app, |c| c.slots[ix].visible = !on);
            },
        )
    };
    let field_dd = {
        let entity = entity.clone();
        let items = crate::panels::radio_items(
            ChartLabelField::ALL.iter().map(|f| {
                (
                    *f,
                    SharedString::from(format!("cl-f-{ix}-{f:?}")),
                    SharedString::from(t!(f.locale_key()).to_string()),
                )
            }),
            slot.field,
            crate::panels::RadioMark::Check,
            move |app, f: ChartLabelField| {
                write_cfg(&entity, app, |c| c.slots[ix].field = f);
            },
        );
        MoonDropdown::new(SharedString::from(format!("cl-field-{ix}")))
            .label(t!(slot.field.locale_key()).to_string())
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Micro)
            .trigger_width_scaled(104.0)
            .menu_width_scaled(150.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    };
    // An inline caption takes its corner from the row it joins, so naming a second one would be a
    // setting with no effect. The dropdown states that instead of offering it.
    let zone_dd = {
        let entity = entity.clone();
        let items = crate::panels::radio_items(
            LabelZone::ALL.iter().map(|z| {
                (
                    *z,
                    SharedString::from(format!("cl-z-{ix}-{z:?}")),
                    SharedString::from(t!(z.locale_key()).to_string()),
                )
            }),
            slot.zone,
            crate::panels::RadioMark::Check,
            move |app, z: LabelZone| {
                write_cfg(&entity, app, |c| c.slots[ix].zone = z);
            },
        );
        MoonDropdown::new(SharedString::from(format!("cl-zone-{ix}")))
            .label(if slot.inline {
                t!("chart_labels.zone_inherited").to_string()
            } else {
                t!(slot.zone.locale_key()).to_string()
            })
            .trigger_caret(!slot.inline)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Micro)
            .trigger_width_scaled(88.0)
            .menu_width_scaled(130.0)
            .menu_size(MoonMenuSize::Compact)
            .disabled(slot.inline)
            .items(items)
    };
    // The first caption of a CORNER has no row to join — not merely the first of the list — and
    // `sanitize` clears the flag there, so offering the toggle would write a value that silently
    // reverts.
    let can_inline = cfg.slots[..ix]
        .iter()
        .any(|s| s.is_drawn() && s.zone == slot.zone);
    let inline_tg = {
        let entity = entity.clone();
        let on = slot.inline;
        micro_button(
            format!("cl-inline-{ix}"),
            "⇢",
            t!(if on {
                "chart_labels.inline_on"
            } else {
                "chart_labels.inline_off"
            })
            .to_string(),
            toggle_variant(on),
            on,
            move |app| {
                write_cfg(&entity, app, |c| c.slots[ix].inline = !on);
            },
        )
    };
    let style_tg = {
        let entity = entity.clone();
        micro_button(
            format!("cl-style-{ix}"),
            "⋯",
            t!("chart_labels.style").to_string(),
            toggle_variant(style_open),
            style_open,
            move |app| {
                entity.update(app, |this, cx| {
                    let next = (!style_open).then_some(ix);
                    this.set_labels_style_open(next);
                    cx.notify();
                });
            },
        )
    };
    let remove = {
        let entity = entity.clone();
        micro_button(
            format!("cl-del-{ix}"),
            "×",
            t!("chart_labels.remove").to_string(),
            MoonButtonVariant::Danger,
            false,
            move |app| {
                entity.update(app, |this, cx| {
                    // Removing renumbers every slot after this one, so an expanded panel below the
                    // removed row would end up attached to a different caption.
                    match this.labels_style_open() {
                        Some(open) if open == ix => this.set_labels_style_open(None),
                        Some(open) if open > ix => this.set_labels_style_open(Some(open - 1)),
                        _ => {}
                    }
                    let mut cfg = this.labels_cfg(cx);
                    cfg.remove(ix);
                    this.apply_labels(cfg, cx);
                });
            },
        )
    };
    let mut row = h_flex()
        .w_full()
        .items_center()
        .gap(px(2.0))
        .child(up)
        .child(down)
        .child(eye)
        .child(field_dd)
        .child(zone_dd);
    if can_inline {
        row = row.child(inline_tg);
    }
    row = row.child(style_tg).child(remove);
    let mut col = v_flex().w_full().gap(design::ui_px(cx, 2.0)).child(row);
    if style_open {
        col = col.child(style_panel(entity, cfg, ix, p, cx));
    }
    col.into_any_element()
}

/// The expanded style panel for one caption.
fn style_panel<T: LabelsPopupHost>(
    entity: &Entity<T>,
    cfg: &ChartLabelsCfg,
    ix: usize,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let slot = cfg.slots[ix];
    let resolved = slot.resolved_style();
    let mut col = v_flex().w_full().gap(design::ui_px(cx, 6.0));
    // Which orders a position figure counts. Offered only by the fields that read it, so a stale
    // basis cannot sit visible on a slot that ignores it.
    if slot.field.uses_pnl_basis() {
        let entity = entity.clone();
        let current = slot.pnl_basis;
        col = col.child(seg_row(
            format!("cl-basis-{ix}"),
            t!("chart_labels.basis").to_string(),
            PnlBasis::ALL
                .iter()
                .map(|b| (t!(b.locale_key()).to_string(), *b == current))
                .collect(),
            72.0,
            p,
            cx,
            move |pick, app| {
                if let Some(b) = PnlBasis::ALL.get(pick).copied() {
                    write_cfg(&entity, app, |c| c.slots[ix].pnl_basis = b);
                }
            },
        ));
    }
    // Size, as a multiplier on the chart's own label size — which already follows the Settings font
    // slider, so a caption scales with the rest of the UI rather than against it.
    col = col.child({
        let entity = entity.clone();
        let current = SIZE_STEPS
            .iter()
            .position(|s| (s - resolved.size_mult).abs() < 0.01);
        seg_row(
            format!("cl-size-{ix}"),
            t!("chart_labels.size").to_string(),
            SIZE_STEPS
                .iter()
                .enumerate()
                .map(|(n, v)| (format!("{v}x"), Some(n) == current))
                .collect(),
            42.0,
            p,
            cx,
            move |pick, app| {
                if let Some(v) = SIZE_STEPS.get(pick).copied() {
                    write_cfg(&entity, app, |c| {
                        c.slots[ix].style.size_mult =
                            Some(v.clamp(LABEL_SIZE_MULT_MIN, LABEL_SIZE_MULT_MAX));
                    });
                }
            },
        )
    });
    // Color MODE. "By profit" reads the value's own sign; a field with no sign keeps the caption
    // color rather than picking one, so the mode is offered everywhere and simply does nothing on
    // a name.
    col = col.child({
        let entity = entity.clone();
        let current = match resolved.color {
            LabelColor::Theme => 0,
            LabelColor::BySign => 1,
            LabelColor::Fixed(_) => 2,
        };
        seg_row(
            format!("cl-color-{ix}"),
            t!("chart_labels.color").to_string(),
            vec![
                (t!("chart_labels.color_theme").to_string(), current == 0),
                (t!("chart_labels.color_sign").to_string(), current == 1),
                (t!("chart_labels.color_fixed").to_string(), current == 2),
            ],
            72.0,
            p,
            cx,
            move |pick, app| {
                write_cfg(&entity, app, |c| {
                    c.slots[ix].style.color = Some(match pick {
                        0 => LabelColor::Theme,
                        1 => LabelColor::BySign,
                        // Keep whatever fixed color the slot already had, so switching modes back
                        // and forth does not silently reset the user's choice.
                        _ => match c.slots[ix].style.color {
                            Some(LabelColor::Fixed(rgb)) => LabelColor::Fixed(rgb),
                            _ => LabelColor::Fixed(FIXED_COLORS[0]),
                        },
                    });
                });
            },
        )
    });
    if matches!(resolved.color, LabelColor::Fixed(_)) {
        let selected = match resolved.color {
            LabelColor::Fixed(rgb) => rgb,
            _ => FIXED_COLORS[0],
        };
        let mut swatches = h_flex().gap(px(3.0));
        for value in FIXED_COLORS {
            let entity = entity.clone();
            // A bare `div` here is the colour itself, not a re-implemented button: the library's
            // own colour control is `MoonColorPicker`, which needs a state entity per row, and
            // there is no component whose job is "a swatch".
            let picked = value == selected;
            swatches = swatches.child(
                div()
                    .id(SharedString::from(format!("cl-sw-{ix}-{value:06x}")))
                    .w(design::ui_px(cx, 18.0))
                    .h(design::ui_px(cx, 14.0))
                    .rounded(design::ui_px(cx, 3.0))
                    .bg(rgb(value))
                    .border_1()
                    .border_color(rgb(if picked { p.text } else { p.border }))
                    .cursor_pointer()
                    .on_click(move |_, _w, app: &mut App| {
                        write_cfg(&entity, app, |c| {
                            c.slots[ix].style.color = Some(LabelColor::Fixed(value));
                        });
                    }),
            );
        }
        col = col.child(swatches);
    }
    let plate_cb = {
        let entity = entity.clone();
        let on = resolved.plate;
        MoonCheckbox::new(SharedString::from(format!("cl-plate-{ix}")))
            .label(t!("chart_labels.plate").to_string())
            .checked(on)
            .size(MoonCheckboxSize::Compact)
            .on_change(move |v: &bool, _w, app| {
                let v = *v;
                write_cfg(&entity, app, |c| c.slots[ix].style.plate = Some(v));
            })
    };
    let caption_cb = {
        let entity = entity.clone();
        let on = resolved.caption;
        MoonCheckbox::new(SharedString::from(format!("cl-caption-{ix}")))
            .label(t!("chart_labels.caption").to_string())
            .checked(on)
            .size(MoonCheckboxSize::Compact)
            .on_change(move |v: &bool, _w, app| {
                let v = *v;
                write_cfg(&entity, app, |c| c.slots[ix].style.caption = Some(v));
            })
    };
    let reset = {
        let entity = entity.clone();
        MoonButton::new(SharedString::from(format!("cl-reset-{ix}")))
            .label(t!("chart_labels.style_reset").to_string())
            .size(MoonButtonSize::Micro)
            .variant(MoonButtonVariant::Ghost)
            .on_click(move |_, _w, app: &mut App| {
                write_cfg(&entity, app, |c| {
                    c.slots[ix].style = moon_core::config::LabelStyle::default();
                });
            })
            .render()
    };
    col = col.child(
        h_flex()
            .w_full()
            .items_center()
            .gap(design::ui_px(cx, 8.0))
            .child(plate_cb)
            .child(caption_cb)
            .child(reset),
    );
    popup_group(
        SharedString::from(format!("cl-style-grp-{ix}")),
        t!("chart_labels.style"),
    )
    .child(col)
    .into_any_element()
}

/// Render the popup content, re-derived from the stored configuration on every render.
fn render_labels_popup<T: LabelsPopupHost>(
    id: &str,
    entity: Entity<T>,
    cfg: ChartLabelsCfg,
    style_open: Option<usize>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let used = cfg.used_len();
    let apply_all_btn = popup_apply_all_button(
        SharedString::from(format!("{id}-apply-all")),
        t!("chart.layout.apply_all_tip").to_string(),
        {
            let entity = entity.clone();
            move |_, _w, app: &mut App| {
                entity.update(app, |this, cx| {
                    let cfg = this.labels_cfg(cx);
                    this.apply_labels_all(cfg, cx);
                });
            }
        },
    );
    let mut list = v_flex().w_full().gap(design::ui_px(cx, 4.0));
    for ix in 0..used {
        list = list.child(slot_row(&entity, &cfg, ix, style_open == Some(ix), p, cx));
    }
    if used == 0 {
        list = list.child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(t!("chart_labels.empty").to_string()),
        );
    }
    let add = MoonDropdown::new(SharedString::from(format!("{id}-add")))
        .label(t!("chart_labels.add").to_string())
        .trigger_caret(true)
        .trigger_variant(MoonButtonVariant::Soft)
        .trigger_size(MoonButtonSize::Micro)
        .trigger_width_scaled(150.0)
        .menu_width_scaled(180.0)
        .menu_size(MoonMenuSize::Compact)
        .disabled(cfg.first_free().is_none())
        .items(catalogue_items(&entity, &cfg));
    let reset_all = {
        let entity = entity.clone();
        MoonButton::new(SharedString::from(format!("{id}-reset")))
            .label(t!("chart_labels.reset").to_string())
            .size(MoonButtonSize::Micro)
            .variant(MoonButtonVariant::Ghost)
            .tooltip(t!("chart_labels.reset_tip").to_string())
            .on_click(move |_, _w, app: &mut App| {
                write_cfg(&entity, app, |c| *c = ChartLabelsCfg::default());
            })
            .render()
    };
    // Chrome is MoonPopover's; see `popover_contents_do_not_paint_a_second_surface`.
    v_flex()
        .id(SharedString::from(format!("{id}-popup")))
        .w_full()
        .gap(design::ui_px(cx, 8.0))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .child(popup_title(t!("chart_labels.title"), p, cx))
                .child(apply_all_btn)
                .child(popup_close_button(
                    SharedString::from(format!("{id}-close")),
                    {
                        let entity = entity.clone();
                        move |_, _w, app: &mut App| {
                            entity.update(app, |this, cx| this.close_labels_popup(cx));
                        }
                    },
                )),
        )
        .child(popup_group("cl-list", t!("chart_labels.frame_list")).child(list))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 8.0))
                .child(add)
                .child(reset_all),
        )
        .into_any_element()
}

/// Host for the labels popup in either the tab strip or a detached-window header.
pub(super) trait LabelsPopupHost: LayoutPopupHost {
    fn labels_popup_open(&self) -> bool;
    fn set_labels_popup_open(&mut self, open: bool);
    /// Which row has its style panel expanded, if any.
    fn labels_style_open(&self) -> Option<usize>;
    fn set_labels_style_open(&mut self, ix: Option<usize>);
    /// The target's per-tab override, or `None` to follow the global default.
    fn labels_override(&self, cx: &App) -> Option<ChartLabelsCfg>;
    /// Apply to all non-Main tabs and windows and update the global default. Main is included only
    /// when the host's source is Main.
    fn apply_labels_all(&mut self, cfg: ChartLabelsCfg, cx: &mut Context<Self>);

    /// The target's effective configuration, SANITIZED to what the chart can actually lay out.
    ///
    /// Sanitized for the reason the graphics popup normalizes: a write starts from this value, so
    /// reading a hand-edited impossibility would persist it back untouched.
    fn labels_cfg(&self, cx: &App) -> ChartLabelsCfg {
        let mut cfg = self
            .labels_override(cx)
            .unwrap_or(self.backend().read(cx).layout.chart_labels);
        cfg.sanitize();
        cfg
    }

    /// Apply the configuration to the target stacks and persist it in the tab spec.
    fn apply_labels(&mut self, cfg: ChartLabelsCfg, cx: &mut Context<Self>) {
        self.apply_tab_setting(StackSetting::Labels(cfg), cx);
    }

    /// Close the popup.
    ///
    /// The already-closed guard is load-bearing for the reason the graphics popup documents:
    /// `Popover` fires `on_open_change(false)` twice when the trigger is clicked while open.
    fn close_labels_popup(&mut self, cx: &mut Context<Self>) {
        if !self.labels_popup_open() {
            return;
        }
        self.set_labels_popup_open(false);
        self.set_labels_style_open(None);
        cx.notify();
    }
}

/// Build the chart-labels popup: a `MoonPopover` anchored to the button that opens it.
///
/// The content is built ONLY while open — `MoonPopover` takes it eagerly, and this sits in a chart
/// host that repaints constantly.
pub(super) fn labels_popup_host<T: LabelsPopupHost>(
    this: &T,
    id_prefix: &'static str,
    trigger: impl IntoElement,
    cx: &mut Context<T>,
) -> MoonPopover {
    let open_entity = cx.entity();
    let mut popover = MoonPopover::new(SharedString::from(format!("{id_prefix}-popover")))
        .placement(MoonPopoverPlacement::BottomEnd)
        .content_width(f32::from(content_width(cx)))
        .close_on_content_click(false)
        // Every row here carries `MoonDropdown`s, and their menus paint in their OWN deferred
        // layers outside this popover's box. `on_mouse_down_out` is bounds-based and runs in the
        // CAPTURE phase, so the click that picks a field or a zone reads as "outside" and shuts the
        // popup before the pick lands. Until MoonUI suppresses that (the Popover entry in
        // docs-internal/FORK_BUGS.md), outside-click dismissal has to be off — the same trade the
        // detects, core-status and tuner popups already make. The ✕ and the toolbar button are the
        // dismissal paths.
        .overlay_closable(false)
        .open(this.labels_popup_open())
        .on_open_change(move |open, _window, app| {
            open_entity.update(app, |this, cx| {
                this.set_labels_popup_open(open);
                if !open {
                    this.set_labels_style_open(None);
                }
                cx.notify();
            });
        })
        .trigger(trigger);
    if !this.labels_popup_open() {
        return popover;
    }
    let p = MoonPalette::active(cx);
    let cfg = this.labels_cfg(cx);
    let style_open = this.labels_style_open();
    let entity = cx.entity();
    popover = popover.content(render_labels_popup(
        id_prefix, entity, cfg, style_open, p, cx,
    ));
    popover
}
