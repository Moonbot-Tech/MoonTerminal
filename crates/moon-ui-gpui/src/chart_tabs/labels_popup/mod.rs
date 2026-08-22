//! The "Chart labels" popup answers ONE question: what the chart shows beside its plot, and where.
//!
//! One line per caption module — order, visibility, name, band, edge, removal — and a button that
//! adds one. What a module PRINTS is a different question, with six settings on each of up to eight
//! captions, and it is answered in its own window ([`crate::panels::open_label_edit`]), opened from
//! the module's name. Both lived here once, as a list inside a list inside a popover; that is what
//! this split undoes.
//!
//! Like the layout, candle and graphics popups beside it, these settings are PER TAB: the target is
//! the tab strip's active tab or the detached window's panel, persisted to `charts.json` through
//! `ChartTabSpec::chart_labels`, with a tab that has no override following the default of its KIND
//! of tab (`chart_tabs::apply_all`). ⧉ opens the row that names which kinds a press addresses and
//! stores this configuration as their default.

use gpui::*;
use moon_core::config::ChartLabelsCfg;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonPalette, MoonPopover, MoonPopoverPlacement,
    h_flex, v_flex,
};
use rust_i18n::t;

use super::common::{LayoutPopupHost, StackSetting};
use super::popup_slot::ChartPopup;
use crate::design;
use crate::panels::{
    popup_apply_all_button, popup_close_button, popup_group, popup_group_inset_px, popup_title,
};

mod rows;

/// Width of one micro glyph button (↑ ↓ 👁 ⇤ ≡ ⇥ ×).
pub(super) const MICRO_W: f32 = 20.0;
/// Micro buttons on a module line: ↑ ↓ 👁, two placements, three edges, ×.
const MICRO_COUNT: f32 = 9.0;
/// Width of the module's name button, which is also what opens its editor.
pub(super) const NAME_W: f32 = 132.0;
/// Width the "add" trigger borrows from the caption catalogue's own column.
pub(super) const FIELD_W: f32 = 104.0;
/// Width of the band dropdown's trigger. Sized on the longest localized band name.
pub(super) const ZONE_W: f32 = 104.0;
/// Width of the gap trigger, which only ever shows a small number.
pub(super) const GAP_W: f32 = 34.0;

/// Gaps the popup offers, in the chart's own logical pixels.
///
/// Steps rather than a free field for the reason the size control states: these popups hold no
/// state entity, and a handful of values covers "let it breathe" without one. `0` is first because
/// it is the default and the way back.
pub(super) const GAP_STEPS: [u8; 8] = [0, 2, 4, 6, 8, 12, 16, 24];
/// Gap between two controls on one line.
pub(super) const ROW_GAP: f32 = 2.0;

/// Slack over the computed line width.
///
/// A `MoonDropdown` trigger takes `max(scaled width, minimum readable width)`, so its real width
/// can exceed what was asked for on a long localized label. This is the margin that keeps the
/// trailing buttons inside the box when it does.
const ROW_SLACK: f32 = 10.0;

/// Popup CONTENT width, DERIVED from the module line rather than guessed.
///
/// Writing the width as its own literal is exactly how the trailing buttons ended up outside the
/// box the first time a dropdown got wider: two numbers that have to agree, and only one of them
/// changed. Font-scaled, because the line is.
pub(super) fn content_width(cx: &App) -> Pixels {
    let line =
        MICRO_COUNT * MICRO_W + NAME_W + ZONE_W + GAP_W + (MICRO_COUNT + 2.0) * ROW_GAP + ROW_SLACK;
    px(design::font_w(cx, line) + popup_group_inset_px(cx))
}

/// Edit the target's configuration: load, mutate, sanitize, apply.
///
/// Sanitizing on every write rather than at read time is what keeps an impossible state from being
/// persisted at all — a hole between captions, a size outside the drawable range, a module that
/// lost its last caption and its name with it.
pub(super) fn write_cfg<T: LabelsPopupHost>(
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

/// Render the popup content, re-derived from the stored configuration on every render.
fn render_labels_popup<T: LabelsPopupHost>(
    id: &str,
    entity: Entity<T>,
    cfg: ChartLabelsCfg,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let apply_all_btn = popup_apply_all_button(
        SharedString::from(format!("{id}-apply-all")),
        t!("chart.apply_all_tabs_windows").to_string(),
        {
            let entity = entity.clone();
            move |_, _w, app: &mut App| {
                entity.update(app, |this, cx| this.arm_apply_press(cx));
            }
        },
    );
    let reset_all = {
        let entity = entity.clone();
        MoonButton::new(SharedString::from(format!("{id}-reset")))
            .label(t!("chart_labels.reset").to_string())
            .size(MoonButtonSize::Micro)
            .variant(MoonButtonVariant::Ghost)
            .tooltip(t!("chart_labels.reset_tip").to_string())
            .on_click(move |_, _w, app: &mut App| {
                entity.update(app, |this, cx| {
                    // "Back to default" means the default THIS KIND of tab follows, not the set
                    // the terminal ships: on an untouched profile the two are the same value, and
                    // on one where the windows were given their own default they are not.
                    this.reset_labels(cx);
                });
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
        .child(
            popup_group("cl-list", t!("chart_labels.frame_rows")).child(
                v_flex()
                    .w_full()
                    .gap(design::ui_px(cx, 4.0))
                    // The glyph legend, because a tooltip inside a popover is INVISIBLE: MoonUI
                    // defers a tooltip at priority 2 and a popover at 30 000, so the hint paints
                    // under the surface it belongs to (docs-internal/FORK_BUGS.md). Until the fork
                    // gives the tooltip a priority of its own, the line has to say it outright.
                    .child(
                        div()
                            .w_full()
                            .text_size(design::t_caption(cx))
                            .text_color(rgb(p.text_muted))
                            .child(t!("chart_labels.legend").to_string()),
                    )
                    .child(rows::row_list(&entity, &cfg, p.text_muted, cx)),
            ),
        )
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 8.0))
                .child(rows::add_row_dropdown(id, &entity, &cfg))
                .child(reset_all),
        )
        .into_any_element()
}

/// Host for the labels popup in either the tab strip or a detached-window header.
pub(crate) trait LabelsPopupHost: LayoutPopupHost {
    /// The target's per-tab override, or `None` to follow the global default.
    fn labels_override(&self, cx: &App) -> Option<ChartLabelsCfg>;

    /// The target's effective configuration, SANITIZED to what the chart can actually lay out.
    ///
    /// Sanitized for the reason the graphics popup normalizes: a write starts from this value, so
    /// reading a hand-edited impossibility would persist it back untouched.
    fn labels_cfg(&self, cx: &App) -> ChartLabelsCfg {
        // By KIND, for the reason the other two popups resolve by kind: a write starts from this
        // value, so reading Main's captions on a torn-off window would persist them as that
        // window's own on the first click.
        let kind = self.source_kind(cx);
        let mut cfg = self.labels_override(cx).unwrap_or_else(|| {
            self.backend()
                .read(cx)
                .layout
                .chart_labels_for(kind)
                .clone()
        });
        cfg.sanitize();
        cfg
    }

    /// Put this tab back on the default its KIND follows.
    ///
    /// Not `ChartLabelsCfg::default()`: that is the set the terminal SHIPS, and writing it here
    /// would overwrite a default the reader has since set for windows or comparisons with the one
    /// they replaced. On an untouched profile the two are the same value.
    fn reset_labels(&mut self, cx: &mut Context<Self>) {
        let kind = self.source_kind(cx);
        let cfg = self
            .backend()
            .read(cx)
            .layout
            .chart_labels_for(kind)
            .clone();
        self.apply_labels(cfg, cx);
    }

    /// Apply the configuration to the target stacks and persist it in the tab spec.
    fn apply_labels(&mut self, cfg: ChartLabelsCfg, cx: &mut Context<Self>) {
        self.apply_tab_setting(StackSetting::Labels(cfg), cx);
    }

    /// Close the popup.
    ///
    /// Ownership is checked by the slot, load-bearing for the reason the graphics popup documents:
    /// `Popover` fires `on_open_change(false)` twice when the trigger is clicked while open.
    fn close_labels_popup(&mut self, cx: &mut Context<Self>) {
        self.close_chart_popup(ChartPopup::Labels, cx);
    }

    /// Take the popup off screen while a modal window of its own is up.
    ///
    /// The module editor and the arbitrage roster are both windows this popup opens, and it is a
    /// popover: it paints in a deferred layer ABOVE their overlay, so it has to step aside and come
    /// back with [`Self::resume_labels_popup`].
    ///
    /// An armed ⧉ row survives the round trip, which is why this is not plain
    /// [`LayoutPopupHost::close_chart_popup`]: the reader armed that row against THIS popup and is
    /// coming straight back to it, whereas the general close means the popup is done with.
    fn suspend_labels_popup(&mut self, cx: &mut Context<Self>) {
        let press = self.apply_press().clone();
        self.close_chart_popup(ChartPopup::Labels, cx);
        *self.apply_press_mut() = press;
    }

    /// Put the popup back after its modal window closes, keeping an armed ⧉ row.
    fn resume_labels_popup(&mut self, cx: &mut Context<Self>) {
        let press = self.apply_press().clone();
        self.open_chart_popup(ChartPopup::Labels, cx);
        *self.apply_press_mut() = press;
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
        // Every line here carries `MoonDropdown`s, and their menus paint in their OWN deferred
        // layers outside this popover's box. `on_mouse_down_out` is bounds-based and runs in the
        // CAPTURE phase, so the click that picks a band reads as "outside" and shuts the popup
        // before the pick lands. Until MoonUI suppresses that (the Popover entry in
        // docs-internal/FORK_BUGS.md), outside-click dismissal has to be off — the same trade the
        // detects, core-status and tuner popups already make. The ✕ and the toolbar button are the
        // dismissal paths — and, because every popup on a host shares one slot, a press on any
        // neighbouring settings button, which is what keeps this one from staying up under it.
        .overlay_closable(false)
        .open(this.popup_shows(ChartPopup::Labels))
        .on_open_change(move |open, _window, app| {
            open_entity.update(app, |this, cx| {
                this.report_chart_popup(ChartPopup::Labels, open, cx)
            });
        })
        .trigger(trigger);
    if !this.popup_shows(ChartPopup::Labels) {
        return popover;
    }
    let p = MoonPalette::active(cx);
    let cfg = this.labels_cfg(cx);
    let entity = cx.entity();
    // The ⧉ row rides ABOVE the popup's own content, inline: see `apply_row`.
    let row = crate::chart_tabs::apply_row::render_apply_row(
        this,
        id_prefix,
        vec![StackSetting::Labels(cfg.clone())],
        None,
        p,
        cx,
    );
    popover = popover.content(
        v_flex()
            .gap_2()
            .children(row)
            .child(render_labels_popup(id_prefix, entity, cfg, p, cx)),
    );
    popover
}
