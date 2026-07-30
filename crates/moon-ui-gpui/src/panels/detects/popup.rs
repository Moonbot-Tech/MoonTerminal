//! Detection-feed display configurator: the panel toolbar's ⚙ button triggers a [`MoonPopover`].
//! Its moonui overlay layer is not clipped by the panel. Outside-click dismissal is off to protect
//! the nested field menus; the close button or the gear dismisses it. The size
//! tab selects the ACTIVE feed size, so switching tabs updates the feed immediately as a live
//! preview. Width, height, rail, and gradient sliders, the chart type, and a slot grid mirror the
//! field positions on each card (mini 2×2, medium 3×2, large 3×3) inside a rail-backed card frame.
//! Controls follow the chart-settings popup idiom: framed groups, segments, and stateless controls.
//! Copy and Paste exchange one group's configuration as detects_view.toml text, like Settings tabs.

use gpui::*;
use moon_ui::{
    MoonAccent, MoonButton, MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonMenuSize,
    MoonNotification, MoonPalette, MoonPopover, MoonPopoverPlacement, MoonSegmentItem,
    MoonSegmentedControl, MoonSlider, MoonWindowExt as _, h_flex, v_flex,
};
use rust_i18n::t;

use moon_core::config::{
    DETECT_SIZE_LARGE, DETECT_SIZE_MEDIUM, DETECT_SIZE_MINI, DetectChart, DetectField,
    DetectViewCfg, detect_slot_count,
};

use super::{DetectsPanel, cards};
use crate::design;
use crate::panels::{
    POPUP_GROUP_INSET, RadioMark, popup_close_button, popup_group, popup_title, radio_items,
};

/// Popup width in logical pixels: three large-slot columns (76-pixel dropdown plus three 20-pixel
/// toggles), with room for gaps, borders, and padding.
///
/// This is the CONTENT width handed to `MoonPopover::content_width_ui`; the popover adds its own
/// padding and border around it.
const POPUP_W: f32 = 560.0;

/// Size tabs as `(size code, localization key)` pairs.
const TABS: [(u8, &str); 3] = [
    (DETECT_SIZE_MINI, "detects.view.size_mini"),
    (DETECT_SIZE_MEDIUM, "detects.view.size_medium"),
    (DETECT_SIZE_LARGE, "detects.view.size_large"),
];

/// Chart types as `(value, localization key)` pairs.
const CHARTS: [(DetectChart, &str); 3] = [
    (DetectChart::None, "detects.cfg.chart_none"),
    (DetectChart::Candles, "detects.cfg.chart_candles"),
    (DetectChart::Line, "detects.cfg.chart_line"),
];

/// Returns the localization key for a slot-field label.
fn field_key(f: DetectField) -> &'static str {
    match f {
        DetectField::None => "detects.field.none",
        DetectField::Coin => "detects.field.coin",
        DetectField::Time => "detects.view.time",
        DetectField::Badge => "detects.view.badge",
        DetectField::Core => "detects.view.core",
        DetectField::Delta24h => "detects.field.d24",
        DetectField::Delta1h => "detects.field.d1",
        DetectField::Exchange => "detects.view.exchange",
        DetectField::ExchangeKind => "detects.view.exchange_kind",
    }
}

impl DetectsPanel {
    /// Seeds the sliders from the edited tab's configuration; `write_view` suppresses unchanged
    /// echoes.
    fn seed_sliders(&self, window: &mut Window, cx: &mut Context<Self>) {
        let cfg = self.view_cfg(cx);
        let s = *cfg.size_cfg(self.popup_tab);
        for (sl, v) in [
            (&self.w_slider, f32::from(s.w)),
            (&self.h_slider, f32::from(s.h)),
            (&self.rail_slider, f32::from(s.rail_w_clamped())),
            (&self.grad_slider, f32::from(s.rail_grad)),
        ] {
            sl.update(cx, |s, c| s.set_value(v, window, c));
        }
    }

    /// Copies the group's configuration to the clipboard as detects_view.toml text.
    fn copy_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.view_cfg(cx).to_share_string() else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        window.push_notification(
            MoonNotification::success(t!("settings.copied").to_string()),
            cx,
        );
    }

    /// Parses a group configuration from the clipboard, persists it, and reseeds the sliders.
    fn paste_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default();
        let Some(cfg) = DetectViewCfg::parse_share(&text) else {
            window.push_notification(
                MoonNotification::error(t!("settings.paste_wrong").to_string()),
                cx,
            );
            return;
        };
        self.write_view(cx, |c| *c = cfg);
        self.popup_tab = cfg.size_clamped();
        self.seed_sliders(window, cx);
    }
}

/// Builds the panel toolbar whose ⚙ button triggers the configurator's overlay popover.
pub(super) fn toolbar(
    this: &DetectsPanel,
    cfg: &DetectViewCfg,
    p: MoonPalette,
    cx: &mut Context<DetectsPanel>,
) -> Div {
    let entity = cx.entity();
    let gear = MoonButton::new("detects-view-gear")
        .label("⚙")
        .size(MoonButtonSize::Micro)
        .variant(MoonButtonVariant::Ghost)
        .tooltip(t!("detects.cfg.title").to_string())
        .render();
    // Built ONLY while open: `MoonPopover` takes its content eagerly, so a shut popover would
    // otherwise rebuild three group boxes, three sliders and the whole slot grid with its per-slot
    // dropdowns on every render of the panel, and throw the tree away.
    let body = this.popup_open.then(|| content(this, cfg, p, cx));
    let mut popover = MoonPopover::new("detects-view-popover")
        .placement(MoonPopoverPlacement::BottomStart)
        .content_width_ui(POPUP_W)
        .close_on_content_click(false)
        // A `MoonDropdown` menu inside this popover paints in its OWN deferred layer, outside
        // this popover's box, and `on_mouse_down_out` is bounds-based and runs in the CAPTURE
        // phase — so the click that picks an item reads as "outside" and would shut the popover
        // before the pick landed. Until MoonUI suppresses that (see the Popover entry in
        // docs-internal/FORK_BUGS.md), outside-click dismissal has to be off here; the ✕ and the
        // gear are the dismissal paths.
        .overlay_closable(false)
        .open(this.popup_open)
        .on_open_change(move |open, window, app| {
            entity.update(app, |this, cx| {
                this.popup_open = open;
                if open {
                    this.popup_tab = this.view_cfg(cx).size_clamped();
                    this.seed_sliders(window, cx);
                }
                cx.notify();
            });
        })
        .trigger(gear);
    if let Some(body) = body {
        popover = popover.content(body);
    }
    h_flex()
        .w_full()
        .flex_none()
        .h(design::fit_h_px(cx, 28.0, 13.0, 7.5))
        .items_center()
        .px_2()
        .bg(rgb(p.tabbar))
        .child(popover)
}

/// Builds a slider row with a left caption and a right value label.
fn slider_row(
    caption: String,
    slider: &Entity<moon_ui::MoonSliderState>,
    value_label: String,
    p: MoonPalette,
    cx: &App,
) -> Div {
    h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .child(
            div()
                .w(design::ui_px(cx, 64.0))
                .flex_none()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text))
                .child(caption),
        )
        .child(div().flex_1().child(MoonSlider::new(slider).height(16.0)))
        .child(
            div()
                .w(design::ui_px(cx, 34.0))
                .flex_none()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(value_label),
        )
}

/// Builds a micro slot toggle with a glyph and tooltip, styled like the drawing-tool buttons.
#[allow(clippy::too_many_arguments)]
fn slot_toggle(
    id: SharedString,
    glyph: &'static str,
    tip: String,
    on: bool,
    entity: Entity<DetectsPanel>,
    tab: u8,
    slot_ix: usize,
    set: fn(&mut moon_core::config::DetectSlot, bool),
) -> impl IntoElement {
    MoonButton::new(id)
        .label(glyph)
        .size(MoonButtonSize::Micro)
        .width(20.0)
        .variant(if on {
            MoonButtonVariant::Blue
        } else {
            MoonButtonVariant::Ghost
        })
        .selected(on)
        .tooltip(tip)
        .on_click(move |_, _w, app| {
            let next = !on;
            entity.update(app, |this, cx| {
                this.write_view(cx, |cfg| {
                    set(&mut cfg.size_cfg_mut(tab).slots[slot_ix], next);
                });
            });
        })
        .render()
}

/// Builds a slot cell with a field dropdown and tightly adjacent toggles.
///
/// A `MoonDropdown` nested in `MoonPopover` is safe since fork fix `0f3ace9`, which keeps deferred
/// rounds in place.
fn slot_cell(
    cfg: &DetectViewCfg,
    tab: u8,
    slot_ix: usize,
    cx: &mut Context<DetectsPanel>,
) -> impl IntoElement {
    let scfg = cfg.size_cfg(tab);
    let slot = scfg.slots[slot_ix];
    let entity = cx.entity();

    let entity_pick = entity.clone();
    let items = radio_items(
        DetectField::ALL.iter().map(|f| {
            (
                *f,
                SharedString::from(format!("ds-{tab}-{slot_ix}-{f:?}")),
                SharedString::from(t!(field_key(*f)).to_string()),
            )
        }),
        slot.field,
        RadioMark::Check,
        move |app, f: DetectField| {
            entity_pick.update(app, |this, cx| {
                this.write_view(cx, |cfg| {
                    cfg.size_cfg_mut(tab).slots[slot_ix].field = f;
                });
            });
        },
    );
    let dropdown = MoonDropdown::new(SharedString::from(format!("det-slot-{tab}-{slot_ix}")))
        .label(t!(field_key(slot.field)).to_string())
        .trigger_caret(true)
        .trigger_variant(MoonButtonVariant::Soft)
        .trigger_size(MoonButtonSize::Micro)
        .trigger_width_scaled(76.0)
        .menu_width_scaled(130.0)
        .menu_size(MoonMenuSize::Compact)
        .items(items);

    // Keep buttons adjacent to the field with no spacers, packing the cell to the left.
    let mut row = h_flex().items_center().gap(px(2.0)).child(dropdown);
    let chart_on = scfg.chart != DetectChart::None;
    if tab != DETECT_SIZE_MINI && chart_on {
        row = row.child(slot_toggle(
            SharedString::from(format!("det-over-{tab}-{slot_ix}")),
            "▧",
            if slot.over {
                t!("detects.cfg.over_on").to_string()
            } else {
                t!("detects.cfg.over_off").to_string()
            },
            slot.over,
            entity.clone(),
            tab,
            slot_ix,
            |s, v| s.over = v,
        ));
    }
    row = row.child(slot_toggle(
        SharedString::from(format!("det-align-{tab}-{slot_ix}")),
        if slot.right { "⇥" } else { "⇤" },
        if slot.right {
            t!("detects.cfg.align_right").to_string()
        } else {
            t!("detects.cfg.align_left").to_string()
        },
        slot.right,
        entity.clone(),
        tab,
        slot_ix,
        |s, v| s.right = v,
    ));
    if tab == DETECT_SIZE_LARGE {
        row = row.child(slot_toggle(
            SharedString::from(format!("det-vpos-{tab}-{slot_ix}")),
            if slot.below { "↓" } else { "↑" },
            if slot.below {
                t!("detects.cfg.vpos_below").to_string()
            } else {
                t!("detects.cfg.vpos_above").to_string()
            },
            slot.below,
            entity.clone(),
            tab,
            slot_ix,
            |s, v| s.below = v,
        ));
    }
    row
}

/// Builds popup content from configuration values read on every render; slider entities are seeded
/// separately when the popup or active size changes.
fn content(
    this: &DetectsPanel,
    cfg: &DetectViewCfg,
    p: MoonPalette,
    cx: &mut Context<DetectsPanel>,
) -> AnyElement {
    let tab = this.popup_tab.min(DETECT_SIZE_LARGE);
    let scfg = cfg.size_cfg(tab);
    let entity = cx.entity();

    // Header: title plus Copy and Paste actions.
    let head = h_flex()
        .w_full()
        .items_center()
        .child(popup_title(t!("detects.cfg.title"), p, cx))
        .child(
            MoonButton::new("det-view-copy")
                .label(t!("settings.copy").to_string())
                .size(MoonButtonSize::Micro)
                .variant(MoonButtonVariant::Ghost)
                .on_click(cx.listener(|this, _, window, cx| this.copy_view(window, cx)))
                .render(),
        )
        .child(
            MoonButton::new("det-view-paste")
                .label(t!("settings.paste").to_string())
                .size(MoonButtonSize::Micro)
                .variant(MoonButtonVariant::Ghost)
                .on_click(cx.listener(|this, _, window, cx| this.paste_view(window, cx)))
                .render(),
        )
        .child(popup_close_button(
            "det-view-close",
            cx.listener(|this, _, _w, cx| {
                this.popup_open = false;
                cx.notify();
            }),
        ));

    // Size tabs select the active feed size, making the feed itself the live preview.
    let entity_tab = entity.clone();
    let tabs = MoonSegmentedControl::new("det-view-tabs")
        .accent(MoonAccent::Blue)
        .items(TABS.iter().map(|(sz, key)| {
            let mut it = MoonSegmentItem::new("", t!(*key).to_string()).width(92.0);
            if *sz == tab {
                it = it.selected(true);
            }
            it
        }))
        .on_click(move |ix, _, window, app| {
            if let Some((sz, _)) = TABS.get(ix) {
                let sz = *sz;
                entity_tab.update(app, |this, cx| {
                    this.popup_tab = sz;
                    this.write_view(cx, |c| c.size = sz);
                    this.seed_sliders(window, cx);
                    cx.notify();
                });
            }
        })
        .render();
    // Delta precision (zero, one, or two decimals) is one setting shared by every size.
    let entity_dec = entity.clone();
    let cur_dec = cfg.delta_decimals_clamped();
    let dec_seg = MoonSegmentedControl::new("det-view-decimals")
        .accent(MoonAccent::Blue)
        .items((0..=2usize).map(|d| {
            let mut it = MoonSegmentItem::new("", format!("{d}")).width(26.0);
            if d == cur_dec {
                it = it.selected(true);
            }
            it
        }))
        .on_click(move |ix, _, _w, app| {
            entity_dec.update(app, |this, cx| {
                this.write_view(cx, |c| c.delta_decimals = ix as u8);
            });
        })
        .render();
    let tabs_row = h_flex()
        .w_full()
        .items_center()
        .child(tabs)
        .child(div().flex_1())
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .mr(design::ui_px(cx, 5.0))
                .child(t!("detects.cfg.decimals").to_string()),
        )
        .child(dec_seg);

    // Card dimensions and chart type.
    let w_row = slider_row(
        t!("detects.cfg.width").to_string(),
        &this.w_slider,
        format!("{}", scfg.w),
        p,
        cx,
    );
    let h_row = slider_row(
        t!("detects.cfg.height").to_string(),
        &this.h_slider,
        format!("{}", scfg.h),
        p,
        cx,
    );
    let entity_chart = entity.clone();
    let chart_seg = MoonSegmentedControl::new("det-view-chart")
        .accent(MoonAccent::Blue)
        .items(CHARTS.iter().map(|(k, key)| {
            let mut it = MoonSegmentItem::new("", t!(*key).to_string()).width(70.0);
            if *k == scfg.chart {
                it = it.selected(true);
            }
            it
        }))
        .on_click(move |ix, _, _w, app| {
            if let Some((k, _)) = CHARTS.get(ix) {
                let k = *k;
                entity_chart.update(app, |this, cx| {
                    let tab = this.popup_tab;
                    this.write_view(cx, |c| c.size_cfg_mut(tab).chart = k);
                });
            }
        })
        .render();
    let chart_row = h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .child(
            div()
                .w(design::ui_px(cx, 64.0))
                .flex_none()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text))
                .child(t!("detects.cfg.chart").to_string()),
        )
        .child(chart_seg);

    // Server rail: swatch caption plus width and gradient sliders.
    let rail_caption = h_flex()
        .items_center()
        .gap(design::ui_px(cx, 5.0))
        .child(
            div()
                .w(px(3.0))
                .h(px(12.0))
                .rounded(px(2.0))
                .bg(rgb(p.blue)),
        )
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(t!("detects.cfg.rail_hint").to_string()),
        );
    let rail_row = slider_row(
        t!("detects.cfg.rail_w").to_string(),
        &this.rail_slider,
        format!("{}", scfg.rail_w_clamped()),
        p,
        cx,
    );
    let grad_row = slider_row(
        t!("detects.cfg.rail_grad").to_string(),
        &this.grad_slider,
        format!("{}", scfg.rail_grad_clamped()),
        p,
        cx,
    );

    // Slot grid mirrors card positions (mini 2×2, medium 3×2, large 3×3) inside a rail-backed
    // card frame, making the controls resemble the detection card they configure.
    let cols = if tab == DETECT_SIZE_MINI { 2 } else { 3 };
    let n = detect_slot_count(tab);
    let mut grid = v_flex().w_full().gap(design::ui_px(cx, 8.0));
    let mut ix = 0usize;
    while ix < n {
        let mut row = h_flex().w_full().gap(design::ui_px(cx, 6.0));
        for col in 0..cols {
            let i = ix + col;
            let cell = div().flex_1().min_w(px(0.0));
            row = row.child(if i < n {
                cell.child(slot_cell(cfg, tab, i, cx))
            } else {
                cell
            });
        }
        grid = grid.child(row);
        ix += cols;
    }
    // `panel_high` lifts the mock card off the popup body. It used to be `shell_high` against a
    // `panel_high` content fill; that fill is gone now that `MoonPopover` owns the surface, and
    // `MoonPopover` paints `shell_high` itself — keeping the old token would have flattened the
    // card into its own background and left it as a bare 1px outline.
    let grid_card = div()
        .relative()
        .w_full()
        .rounded(design::ui_px(cx, 6.0))
        .border_1()
        .border_color(rgb(p.border))
        .bg(rgb(p.panel_high))
        .overflow_hidden()
        // The rail overlay is drawn at an explicit width, so it has to be told how much of the
        // content box the group frame around it consumes.
        .children(cards::rail_layers(
            p.blue,
            3.0,
            20.0,
            POPUP_W - POPUP_GROUP_INSET,
            cx,
        ))
        .child(
            div()
                .w_full()
                .pl(px(12.0))
                .pr(design::ui_px(cx, 8.0))
                .py(design::ui_px(cx, 8.0))
                .child(grid),
        );

    // Chrome is MoonPopover's; see `popover_contents_do_not_paint_a_second_surface`.
    v_flex()
        .id("detects-view-popup")
        .w_full()
        .gap(design::ui_px(cx, 8.0))
        .child(head)
        .child(tabs_row)
        .child(
            popup_group(
                "detects-frame-size",
                t!("detects.cfg.frame_size").to_string(),
            )
            .child(
                v_flex()
                    .w_full()
                    .gap(design::ui_px(cx, 6.0))
                    .child(w_row)
                    .child(h_row)
                    .child(chart_row),
            ),
        )
        .child(
            popup_group(
                "detects-frame-rail",
                t!("detects.cfg.rail_frame").to_string(),
            )
            .child(
                v_flex()
                    .w_full()
                    .gap(design::ui_px(cx, 6.0))
                    .child(rail_caption)
                    .child(rail_row)
                    .child(grad_row),
            ),
        )
        .child(
            popup_group(
                "detects-frame-fields",
                t!("detects.cfg.frame_fields").to_string(),
            )
            .child(grid_card),
        )
        .into_any_element()
}
