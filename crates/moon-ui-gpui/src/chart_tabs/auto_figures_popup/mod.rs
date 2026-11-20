//! The "Auto figures" popup: which auto-figure rows a KIND of chart draws, in which order, and
//! whether the feature is on at all.
//!
//! The captions popup's shape (`labels_popup`): one line per row — order, visibility, a sample of
//! its line, its name, its timeframe, removal — and an "add" menu. What a row LOOKS FOR and how it
//! looks is a different question with a few dozen settings, answered in its own dialog
//! ([`crate::panels::open_auto_figure_row_editor`]), opened from the row's name.
//!
//! Unlike the captions, these settings are stored per tab KIND only (`WindowLayout::
//! auto_figures_for` / `set_auto_figures_default`): the popup edits the default of the kind its
//! target IS, and every chart of that kind follows. ⧉ copies the set to other kinds, the way the
//! captions' ⧉ stores a default for the kinds it names.
//!
//! Master switch off keeps every row — dimmed and still editable — and stops the detectors.
//!
//! Under it, whether the lines take the mouse only while Ctrl is held (`AutoFiguresCfg::
//! modifier_only`, on by default), so a right-drag across the chart does not catch on them.
//!
//! Under the switch, one opacity for the whole block (`AutoFiguresCfg::opacity`): a row of steps,
//! the graphics popup's control for its own opacities, because every other control here reads its
//! value straight from the configuration and a slider would need a state entity on each host.

use gpui::*;
use moon_core::config::ChartTabKind;
use moon_core::config::auto_figures::{AutoFigureRow, AutoFiguresCfg};
use moon_ui::{
    MoonButton, MoonButtonVariant, MoonCheckbox, MoonPalette, MoonPopover, MoonPopoverPlacement,
    h_flex, v_flex,
};
use rust_i18n::t;

use super::common::{LayoutPopupHost, seg_row};
use super::graphics_popup::{nearest, percent_label};
use super::popup_slot::ChartPopup;
use crate::design;
use crate::panels::{
    ChartPanel, popup_apply_all_button, popup_close_button, popup_group, popup_group_inset_px,
    popup_title, write_auto_figures,
};

mod rows;
pub(crate) mod status;

#[cfg(test)]
mod tests;

use status::RowStatus;

/// Width of one micro glyph button (↑ ↓ 👁 ×).
const MICRO_W: f32 = 20.0;
/// Micro buttons on a row line.
const MICRO_COUNT: f32 = 4.0;
/// Width of the line sample.
const SWATCH_W: f32 = 26.0;
/// Width of the row's name button, which is also what opens its editor.
const NAME_W: f32 = 150.0;
/// Width of the timeframe column.
const TF_W: f32 = 40.0;
/// Gap between two controls on one line.
const ROW_GAP: f32 = 2.0;
/// Slack over the computed line width, for a trigger that renders wider than asked.
const ROW_SLACK: f32 = 10.0;
/// The block opacities offered, in display order. The shipped 40 % is one of them, and so are
/// both bounds the repair keeps (`AF_OPACITY_MIN`, `AF_OPACITY_MAX`), so every stored value lights
/// the step nearest it.
const OPACITY_STEPS: [f32; 6] = [0.1, 0.25, 0.4, 0.6, 0.8, 1.0];

/// Popup CONTENT width, derived from the row line rather than guessed — the captions popup's
/// lesson: two numbers that have to agree end with only one of them changed.
fn content_width(cx: &App) -> Pixels {
    let line = MICRO_COUNT * MICRO_W
        + SWATCH_W
        + NAME_W
        + TF_W
        + (MICRO_COUNT + 3.0) * ROW_GAP
        + ROW_SLACK;
    px(design::font_w(cx, line) + popup_group_inset_px(cx))
}

/// Host of the auto-figures popup: the tab strip (its active tab) or a detached window.
pub(super) trait AutoFiguresPopupHost: LayoutPopupHost {
    /// Every chart panel the popup's target draws with — where the per-row status comes from.
    fn auto_figure_panels(&self, cx: &App) -> Vec<Entity<ChartPanel>>;

    /// The target kind's configuration, SANITIZED: a write starts from this value, so reading a
    /// hand-edited impossibility would persist it back untouched.
    fn auto_figures_cfg(&self, cx: &App) -> AutoFiguresCfg {
        let kind = self.source_kind(cx);
        let mut cfg = self
            .backend()
            .read(cx)
            .layout
            .auto_figures_for(kind)
            .clone();
        cfg.sanitize();
        cfg
    }

    /// One status mark per row, the worst across the target's panes.
    fn auto_figure_statuses(&self, rows: &[AutoFigureRow], cx: &App) -> Vec<Option<RowStatus>> {
        let outcomes: Vec<_> = self
            .auto_figure_panels(cx)
            .iter()
            .flat_map(|panel| panel.read(cx).auto_row_outcomes())
            .collect();
        status::merge(outcomes, rows)
    }

    /// Close the popup; ownership is checked by the slot.
    fn close_auto_figures_popup(&mut self, cx: &mut Context<Self>) {
        self.close_chart_popup(ChartPopup::AutoFigures, cx);
    }

    /// Take the popup off screen while its row editor is up — a popover paints ABOVE the dialog's
    /// overlay — keeping an armed ⧉ row for the way back, as the captions popup does.
    fn suspend_auto_figures_popup(&mut self, cx: &mut Context<Self>) {
        let press = self.apply_press().clone();
        self.close_chart_popup(ChartPopup::AutoFigures, cx);
        *self.apply_press_mut() = press;
    }

    /// Put the popup back after the editor closes, keeping an armed ⧉ row.
    fn resume_auto_figures_popup(&mut self, cx: &mut Context<Self>) {
        let press = self.apply_press().clone();
        self.open_chart_popup(ChartPopup::AutoFigures, cx);
        *self.apply_press_mut() = press;
    }
}

/// Edit the target kind's configuration through the one write door, then repaint the host.
pub(super) fn write_cfg<T: AutoFiguresPopupHost>(
    entity: &Entity<T>,
    app: &mut App,
    f: impl FnOnce(&mut AutoFiguresCfg),
) {
    let (backend, kind) = {
        let host = entity.read(app);
        (host.backend().clone(), host.source_kind(app))
    };
    write_auto_figures(&backend, kind, app, f);
    entity.update(app, |_, cx| cx.notify());
}

/// The ⧉ row: which kinds the set is stored for, and the two buttons that store or reset it.
///
/// The captions' ⧉ walks tabs and per-tab overrides (`apply_all`); auto figures have neither — a
/// kind's default IS the setting — so this press is a direct store per ticked kind. It reuses the
/// host's one [`super::apply_row::ApplyPress`] for its open flag and ticks, so it closes with the
/// popup and cannot show over a popup that did not arm it.
fn copy_row<T: AutoFiguresPopupHost>(
    this: &T,
    id: &'static str,
    cfg: &AutoFiguresCfg,
    p: MoonPalette,
    cx: &mut Context<T>,
) -> Option<AnyElement> {
    let press = this.apply_press();
    if !press.open {
        return None;
    }
    let targets = press.targets;
    let entity = cx.entity();
    let mut ticks = h_flex().gap_2().flex_wrap();
    for kind in ChartTabKind::ALL {
        let entity = entity.clone();
        ticks = ticks.child(
            MoonCheckbox::new(SharedString::from(format!("{id}-copy-{kind:?}")))
                .label(t!(kind.locale_key()).to_string())
                .checked(targets.has(kind))
                .on_change(move |checked: &bool, _w, app| {
                    let checked = *checked;
                    entity.update(app, |this, cx| {
                        this.apply_press_mut().targets.set(kind, checked);
                        cx.notify();
                    });
                }),
        );
    }
    let set = {
        let entity = entity.clone();
        let cfg = cfg.clone();
        MoonButton::new(SharedString::from(format!("{id}-copy-set")))
            .label(t!("chart.defaults.set").to_string())
            .variant(MoonButtonVariant::Soft)
            .disabled(!targets.any())
            .on_click(move |_, _w, app| {
                let backend = entity.read(app).backend().clone();
                for kind in ChartTabKind::ALL.into_iter().filter(|k| targets.has(*k)) {
                    let cfg = cfg.clone();
                    write_auto_figures(&backend, kind, app, move |c| *c = cfg);
                }
                entity.update(app, |this, cx| {
                    this.apply_press_mut().open = false;
                    cx.notify();
                });
            })
            .render()
    };
    let reset = {
        let entity = entity.clone();
        MoonButton::new(SharedString::from(format!("{id}-copy-reset")))
            .label(t!("chart.defaults.reset").to_string())
            .variant(MoonButtonVariant::Ghost)
            .disabled(!targets.any())
            .on_click(move |_, _w, app| {
                let backend = entity.read(app).backend().clone();
                backend.update(app, |b, bcx| {
                    let mut moved = false;
                    // Main first: see `ChartTabKind::RESET_ORDER`.
                    for kind in ChartTabKind::RESET_ORDER {
                        if targets.has(kind) {
                            moved |= b.layout.reset_auto_figures_default(kind);
                        }
                    }
                    if moved {
                        b.layout_dirty = true;
                        bcx.notify();
                    }
                });
                entity.update(app, |this, cx| {
                    this.apply_press_mut().open = false;
                    cx.notify();
                });
            })
            .render()
    };
    Some(
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text_muted))
                    .child(t!("auto_figures.copy_hint").to_string()),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .flex_wrap()
                    .child(ticks)
                    .child(h_flex().gap_2().flex_wrap().child(set).child(reset)),
            )
            .into_any_element(),
    )
}

/// Render the popup content, re-derived from the stored configuration on every render.
fn render_popup<T: AutoFiguresPopupHost>(
    this: &T,
    id: &'static str,
    cfg: AutoFiguresCfg,
    p: MoonPalette,
    cx: &mut Context<T>,
) -> AnyElement {
    let entity = cx.entity();
    let kind = this.source_kind(cx);
    let statuses = this.auto_figure_statuses(&cfg.rows, cx);
    let theme = {
        let b = this.backend().read(cx);
        b.preview
            .as_ref()
            .unwrap_or(&b.config)
            .chart_theme()
            .clone()
    };
    let header = h_flex()
        .w_full()
        .items_center()
        .child(popup_title(t!("auto_figures.title"), p, cx))
        .child(popup_apply_all_button(
            SharedString::from(format!("{id}-copy")),
            t!("auto_figures.copy_tip").to_string(),
            {
                let entity = entity.clone();
                move |_, _w, app: &mut App| {
                    entity.update(app, |this, cx| this.arm_apply_press(cx));
                }
            },
        ))
        .child(popup_close_button(
            SharedString::from(format!("{id}-close")),
            {
                let entity = entity.clone();
                move |_, _w, app: &mut App| {
                    entity.update(app, |this, cx| this.close_auto_figures_popup(cx));
                }
            },
        ));
    let master = {
        let entity = entity.clone();
        MoonCheckbox::new(SharedString::from(format!("{id}-enabled")))
            .label(t!("auto_figures.enabled").to_string())
            .checked(cfg.enabled)
            .on_change(move |on: &bool, _w, app| {
                let on = *on;
                write_cfg(&entity, app, |c| c.enabled = on);
            })
    };
    // Whether the lines take the mouse only under Ctrl (`AutoFiguresCfg::modifier_only`).
    let modifier_only = {
        let entity = entity.clone();
        MoonCheckbox::new(SharedString::from(format!("{id}-modifier-only")))
            .label(t!("auto_figures.modifier_only").to_string())
            .checked(cfg.modifier_only)
            .on_change(move |on: &bool, _w, app| {
                let on = *on;
                write_cfg(&entity, app, |c| c.modifier_only = on);
            })
    };
    let opacity = {
        let entity = entity.clone();
        let current = nearest(&OPACITY_STEPS, cfg.opacity);
        // The row sits straight in the popup, which is as wide as the row list's frame.
        let seg_w =
            (f32::from(content_width(cx)) - popup_group_inset_px(cx)) / OPACITY_STEPS.len() as f32;
        seg_row(
            format!("{id}-opacity"),
            t!("auto_figures.opacity").to_string(),
            OPACITY_STEPS
                .iter()
                .enumerate()
                .map(|(ix, v)| (percent_label(*v), ix == current))
                .collect(),
            seg_w,
            p,
            cx,
            move |ix, app| {
                if let Some(v) = OPACITY_STEPS.get(ix) {
                    let v = *v;
                    write_cfg(&entity, app, |c| c.opacity = v);
                }
            },
        )
    };
    let scope = div()
        .w_full()
        .text_size(design::t_caption(cx))
        .text_color(rgb(p.text_muted))
        .child(t!("auto_figures.for_kind", kind = t!(kind.locale_key())).to_string());
    let reset = {
        let entity = entity.clone();
        MoonButton::new(SharedString::from(format!("{id}-reset")))
            .label(t!("auto_figures.reset").to_string())
            .variant(MoonButtonVariant::Ghost)
            .on_click(move |_, _w, app: &mut App| {
                let (backend, kind) = {
                    let host = entity.read(app);
                    (host.backend().clone(), host.source_kind(app))
                };
                backend.update(app, |b, bcx| {
                    if b.layout.reset_auto_figures_default(kind) {
                        b.layout_dirty = true;
                        bcx.notify();
                    }
                });
                entity.update(app, |_, cx| cx.notify());
            })
            .render()
    };
    let legend = format!(
        "↑↓ {} · 👁 {} · {} · × {}",
        t!("auto_figures.legend.order"),
        t!("auto_figures.legend.shown"),
        t!("auto_figures.legend.name"),
        t!("auto_figures.legend.remove"),
    );
    let mut list_group = v_flex().w_full().gap(design::ui_px(cx, 4.0)).child(
        div()
            .w_full()
            .text_size(design::t_caption(cx))
            .text_color(rgb(p.text_muted))
            .child(legend),
    );
    if !cfg.enabled {
        list_group = list_group.child(
            div()
                .w_full()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.orange))
                .child(t!("auto_figures.off_hint").to_string()),
        );
    }
    list_group = list_group.child(rows::row_list(&entity, &cfg, &statuses, &theme, p, cx));
    // Chrome is MoonPopover's; see `popover_contents_do_not_paint_a_second_surface`.
    v_flex()
        .id(SharedString::from(format!("{id}-popup")))
        .w_full()
        .gap(design::ui_px(cx, 8.0))
        .child(header)
        .child(scope)
        .child(master)
        .child(modifier_only)
        .child(opacity)
        .child(popup_group("af-list", t!("auto_figures.frame_rows")).child(list_group))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .flex_wrap()
                .gap(design::ui_px(cx, 8.0))
                .child(rows::add_row_dropdown(id, &entity, &cfg, cx))
                .child(reset),
        )
        .into_any_element()
}

/// Build the auto-figures popup: a `MoonPopover` anchored to the button that opens it.
///
/// The content is built ONLY while open — `MoonPopover` takes it eagerly, and this sits in a chart
/// host that repaints constantly.
pub(super) fn auto_figures_popup_host<T: AutoFiguresPopupHost>(
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
        // The "add" menu is a `MoonDropdown` whose items paint in their OWN deferred layer, so an
        // outside-click dismissal would close the popup before the pick lands — the captions
        // popup's trade (docs-internal/FORK_BUGS.md, the Popover entry). The ✕, the toolbar button
        // and any neighbouring popup button close it instead.
        .overlay_closable(false)
        .open(this.popup_shows(ChartPopup::AutoFigures))
        .on_open_change(move |open, _window, app| {
            open_entity.update(app, |this, cx| {
                this.report_chart_popup(ChartPopup::AutoFigures, open, cx)
            });
        })
        .trigger(trigger);
    if !this.popup_shows(ChartPopup::AutoFigures) {
        return popover;
    }
    let p = MoonPalette::active(cx);
    let cfg = this.auto_figures_cfg(cx);
    let copy = copy_row(this, id_prefix, &cfg, p, cx);
    popover = popover.content(
        v_flex()
            .gap_2()
            .children(copy)
            .child(render_popup(this, id_prefix, cfg, p, cx)),
    );
    popover
}
