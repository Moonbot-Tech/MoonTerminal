//! The Alerts panel's chrome: the filter toolbar at the top, the floating settings panel, and the
//! alert-playback bar at the bottom.

use super::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonInput, MoonMenuItem,
    MoonMenuSize, h_flex,
};
use rust_i18n::t;

use super::model::ALL_COLUMNS_MASK;
use crate::panels::{RadioMark, radio_items};

/// Shared bottom-bar control height in pixels, keeping steppers, dropdowns and input aligned.
const CTRL_H: f32 = 26.0;

impl AlertsPanel {
    /// Builds the filter row: cores, scope, coin search, and the visible-column menu.
    ///
    /// Laid out like the Report toolbar, in the order those panels read left to right — who
    /// (cores), what (scope), which instrument (coin) — so the two panels are not two different
    /// habits.
    pub(super) fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // Spelled exactly as the Report toolbar is, down to the tailwind steps and the coin
        // field's raw 90: `design::ui_px` grows with the font slider and `px_2`/`gap_2` do not, so
        // writing this row "properly" made it drift wider than the panel beside it at any scale
        // above one. Two toolbars the user reads side by side have to be one habit, not two.
        h_flex()
            .flex_none()
            .w_full()
            .flex_wrap()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(self.core_combo(cx))
            .child(self.scope_combo(cx))
            // No caption beside the field; the placeholder carries the word. See the Report's row.
            .child(
                div().w(px(90.0)).child(
                    MoonInput::new("alerts-coin")
                        .state(&self.coin_input)
                        .small()
                        // A filter with no ✕ can only be undone by holding backspace.
                        .cleanable(true),
                ),
            )
            .child(div().flex_1())
            .child(self.columns_menu(cx))
    }

    /// Builds the multi-select core dropdown shared with the Orders, Report and Assets panels.
    fn core_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let exchange_view = view.clone();
        let (cores, exchange_names) = {
            let b = self.backend.read(cx);
            (
                self.group_cores(b),
                b.session.market_source().core_exchange_names(),
            )
        };
        crate::controls::core_combo(
            "alerts-cores",
            &cores,
            &exchange_names,
            &self.sel_cores,
            crate::controls::CoreAllRowMode::ImplicitOrComplete,
            t!("alerts.all_cores").to_string(),
            |n| t!("alerts.cores_n", n = n).to_string(),
            170.0,
            move |id, app| {
                view.update(app, |this, cx| this.toggle_core(id, cx));
            },
            move |exchange_cores, app| {
                exchange_view.update(app, |this, cx| {
                    this.toggle_exchange_cores(exchange_cores, cx)
                });
            },
        )
    }

    /// Builds the scope field: where a figure came from, and whether it is armed — in ONE dropdown.
    ///
    /// Two independent filters, but they are always read as one question ("which figures am I
    /// looking at"), so they share a field split by a separator, the way the Report scope field
    /// groups its own. The trigger summarizes both in short words and says only "all" when neither
    /// narrows anything, rather than repeating the same word twice.
    fn scope_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let kind = self.view.kind;
        let arm = self.view.arm;
        // Each filter contributes a word only when it narrows something, so "neither narrows" is
        // simply the empty list — no second place where both-`All` is spelled out.
        let mut parts: Vec<String> = Vec::new();
        match kind {
            KindFilter::All => {}
            KindFilter::Moonbot => parts.push(t!("alerts.short.moonbot").to_string()),
            KindFilter::Terminal => parts.push(t!("alerts.short.terminal").to_string()),
        }
        match arm {
            ArmFilter::All => {}
            ArmFilter::Armed => parts.push(t!("alerts.short.armed").to_string()),
            ArmFilter::Unarmed => parts.push(t!("alerts.short.unarmed").to_string()),
        }
        let label = match parts.is_empty() {
            true => t!("report.filter.all").to_string(),
            false => parts.join("/"),
        };
        let kind_view = cx.entity();
        let arm_view = kind_view.clone();
        let mut items = radio_items(
            [
                (
                    KindFilter::All,
                    "al-kind-all".into(),
                    t!("alerts.filter.kind.all").to_string().into(),
                ),
                (
                    KindFilter::Moonbot,
                    "al-kind-mb".into(),
                    t!("alerts.filter.kind.moonbot").to_string().into(),
                ),
                (
                    KindFilter::Terminal,
                    "al-kind-term".into(),
                    t!("alerts.filter.kind.terminal").to_string().into(),
                ),
            ],
            kind,
            RadioMark::Check,
            move |app, v| Self::mutate(&kind_view, app, |s| s.kind = v),
        );
        items.push(MoonMenuItem::separator());
        items.extend(radio_items(
            [
                (
                    ArmFilter::All,
                    "al-arm-all".into(),
                    t!("alerts.filter.arm.all").to_string().into(),
                ),
                (
                    ArmFilter::Armed,
                    "al-arm-on".into(),
                    t!("alerts.filter.arm.on").to_string().into(),
                ),
                (
                    ArmFilter::Unarmed,
                    "al-arm-off".into(),
                    t!("alerts.filter.arm.off").to_string().into(),
                ),
            ],
            arm,
            RadioMark::Check,
            move |app, v| Self::mutate(&arm_view, app, |s| s.arm = v),
        ));
        MoonDropdown::new("alerts-scope")
            .label(label)
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            // Report's rule for the same kind of field: 102 is the floor, and it grows only for a
            // label that would otherwise be ellipsised.
            .fit_trigger_width(102.0, 170.0)
            .menu_width_scaled(200.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }

    /// Builds the persisted visible-column menu, matching the other tables' glyph selector.
    ///
    /// `close_on_select(false)` keeps the menu open for several edits. The action column is not
    /// offered at all — it holds the only delete and settings buttons — and the last remaining
    /// hideable column cannot be switched off either.
    fn columns_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let cur = self.view;
        let mut menu = MoonDropdown::new("alerts-columns")
            .segment(moon_ui::MoonButtonSegment::new("▦"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(design::glyph_btn_w(cx))
            .menu_width_scaled(180.0)
            .menu_size(MoonMenuSize::Compact)
            .close_on_select(false);
        let all_on = cur.columns == ALL_COLUMNS_MASK;
        let all_view = view.clone();
        menu = menu.item(
            MoonMenuItem::with_key("al-col-all", t!("report.filter.all").to_string())
                .checked(all_on)
                .selected(all_on)
                .on_click(move |_, _, app| {
                    Self::mutate(&all_view, app, |v| {
                        // Clicking All again strips the table back to its first column plus the
                        // actions, which is the smallest set the panel still works in.
                        v.columns = if v.columns == ALL_COLUMNS_MASK {
                            AlCol::ALL[0].bit() | AlCol::Actions.bit()
                        } else {
                            ALL_COLUMNS_MASK
                        };
                    })
                }),
        );
        for col in AlCol::ALL.into_iter().filter(|c| c.hideable()) {
            let shown = cur.shows(col);
            // The last hideable column stays: an actions-only table shows nothing to act on.
            let last_visible = shown && cur.columns == col.bit() | AlCol::Actions.bit();
            let view = view.clone();
            menu = menu.item(
                MoonMenuItem::with_key(format!("al-col-{}", col.key()), col.title())
                    .checked(shown)
                    .disabled(last_visible)
                    .on_click(move |_, _, app| {
                        Self::mutate(&view, app, |v| {
                            let next = v.columns ^ col.bit();
                            if next & !AlCol::Actions.bit() != 0 {
                                v.columns = next;
                            }
                        })
                    }),
            );
        }
        div()
            .id("alerts-cols-tip")
            .tooltip(crate::panels::common::text_tooltip(
                t!("alerts.columns").to_string(),
            ))
            .child(menu)
    }

    /// Builds the alert-playback bar: sound and its UI-only duration and repeat steppers.
    pub(super) fn bottom_bar(&self, p: MoonPalette, cx: &mut Context<Self>) -> impl IntoElement {
        // Place each caption above a fixed-height control so all controls share a bottom baseline.
        // Every metric is resolved through `design` BEFORE the closures, both so the bar scales with
        // the Font setting and so a closure does not retain `cx` and conflict with the stepper's
        // `cx.listener` calls.
        let cap = design::t_caption(cx);
        let cap_h = design::ui_px(cx, 13.0);
        let ctrl_h = design::ui_px(cx, CTRL_H);
        let gap_s = design::ui_px(cx, 2.0);
        let gap_m = design::ui_px(cx, 3.0);
        let value_w = design::font_w_px(cx, 34.0);
        let field = move |title: String, control: AnyElement| {
            v_flex()
                .gap(gap_m)
                .child(
                    div()
                        .h(cap_h)
                        .text_size(cap)
                        .text_color(moon(p.text_muted))
                        .child(title),
                )
                .child(h_flex().h(ctrl_h).items_center().child(control))
        };
        let stepper = |id: &'static str,
                       val: String,
                       dn: fn(&mut AlertsPanel),
                       up: fn(&mut AlertsPanel),
                       cx: &mut Context<Self>|
         -> AnyElement {
            h_flex()
                .h(ctrl_h)
                .items_center()
                .gap(gap_s)
                .child(
                    MoonButton::new((id, 0u64))
                        .label("‹")
                        .size(MoonButtonSize::Action)
                        .variant(MoonButtonVariant::Soft)
                        .on_click(cx.listener(move |this, _, _w, cx| {
                            dn(this);
                            cx.notify();
                        }))
                        .render(),
                )
                .child(
                    div()
                        .w(value_w)
                        .text_center()
                        .text_color(moon(p.text))
                        .child(val),
                )
                .child(
                    MoonButton::new((id, 1u64))
                        .label("›")
                        .size(MoonButtonSize::Action)
                        .variant(MoonButtonVariant::Soft)
                        .on_click(cx.listener(move |this, _, _w, cx| {
                            up(this);
                            cx.notify();
                        }))
                        .render(),
                )
                .into_any_element()
        };
        h_flex()
            .flex_none()
            .w_full()
            .px(design::ui_px(cx, 10.0))
            .py(design::ui_px(cx, 8.0))
            .gap(design::ui_px(cx, 16.0))
            .bg(moon(p.shell_high))
            .border_t_1()
            .border_color(moon(p.border))
            .items_end()
            .child(field(
                t!("alerts.duration").to_string(),
                stepper(
                    "alert-dur",
                    format!("{}", self.duration_s),
                    |v| v.duration_s = v.duration_s.saturating_sub(5).max(5),
                    |v| v.duration_s = (v.duration_s + 5).min(600),
                    cx,
                ),
            ))
            .child(field(
                t!("alerts.repeat").to_string(),
                stepper(
                    "alert-rep",
                    format!("{}", self.repeat),
                    |v| v.repeat = v.repeat.saturating_sub(1),
                    |v| v.repeat = (v.repeat + 1).min(20),
                    cx,
                ),
            ))
            .child(field(
                t!("alerts.sound").to_string(),
                self.sound_dropdown(cx).into_any_element(),
            ))
            .child(div().flex_1())
            .child(
                div()
                    .text_color(moon(p.text_muted))
                    .child(t!("alerts.count", n = self.rows.len()).to_string()),
            )
    }

    /// Builds the Backend default-sound dropdown for an alert without a strategy sound.
    ///
    /// Selecting an entry updates the in-memory default and plays it immediately as a preview.
    fn sound_dropdown(&self, cx: &Context<Self>) -> impl IntoElement {
        let cur = self.backend.read(cx).default_alert_sound.clone();
        let backend = self.backend.clone();
        // The selection is read from the backend at render time and is not part of the row
        // signature, so the panel is told directly rather than left to catch up on the gate's
        // next second — the label and the check mark would lag a visible beat behind the click.
        let view = cx.entity();
        let options: Vec<(&'static str, SharedString, SharedString)> = crate::media::sound::names()
            .map(|n| {
                (
                    n,
                    SharedString::from(format!("snd-{n}")),
                    SharedString::from(n),
                )
            })
            .collect();
        let items = radio_items(
            options,
            leak_str(&cur),
            RadioMark::Check,
            move |app, name: &'static str| {
                backend.update(app, |b, bcx| {
                    b.default_alert_sound = name.to_string();
                    bcx.notify();
                });
                view.update(app, |_, cx| cx.notify());
                crate::media::sound::play(name);
            },
        );
        MoonDropdown::new("alerts-sound")
            .label(cur)
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width_scaled(120.0)
            .menu_width_scaled(150.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }
}

/// Returns the static sound name matching `cur`, as required by `radio_items`' copied selection.
fn leak_str(cur: &str) -> &'static str {
    crate::media::sound::names()
        .find(|n| *n == cur)
        .unwrap_or("ding1")
}
