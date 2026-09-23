//! The parameter grid of the "Entry/Exit" axis, laid out as the "By filter" grid is: a tick per
//! field that admits it to the search (the header's tick admits them all), the field's name —
//! a click selects it for "Search" on one field — the value the selected strategies hold, which
//! a click sends to В1, and the two variant columns with the copy arrows and the clear crosses.
//!
//! The rows come in two groups, Entry and Exit. A group the model does not reproduce well enough
//! (the share gate of the search settings) is not searched, and its heading says so; the Entry
//! group folds to one line where a kind in the scope has no entry model. A field the entry
//! method does not read — the path-only fields under the shift — is greyed out: varying it would
//! move no column.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonCheckbox, MoonInput, MoonInputEvent, MoonInputState, MoonPalette, h_flex, v_flex,
};
use rust_i18n::t;

use super::super::super::AnalyticsView;
use super::super::shared::{N_VAR, TunerKind, glyph_btn};
use super::state::{NowValue, TicksData};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::tuner::ticks::params::{ParamGroup, TickParam, params_for};

/// Width of the strategy and variant cells, font-scaled px.
const CELL_W: f32 = 60.0;

impl AnalyticsView {
    /// The grid panel: the shared toolbar (title, Copy, Save), the search row, then the two
    /// groups, scrolling.
    pub(in crate::analytics::tuner) fn ticks_grid(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let header = self.shell_toolbar(
            TunerKind::Ticks,
            t!("analytics.ticks.params_title").to_string(),
            cx,
        );
        let cfg_row = self.shell_config_row(TunerKind::Ticks, p, window, cx);
        let data = self.ticks.data.data().cloned();
        let fields = scope_fields(data.as_deref());
        let mut grid = v_flex()
            .w_full()
            .flex_none()
            .child(self.ticks_grid_header(&fields, p, cx));
        for group in [ParamGroup::Entry, ParamGroup::Exit] {
            grid = grid.child(self.ticks_group_header(group, data.as_deref(), p, cx));
            if group == ParamGroup::Entry && !data.as_ref().is_some_and(|d| d.entry_modelled()) {
                continue;
            }
            for field in fields.iter().filter(|f| f.group == group) {
                let now = data.as_ref().and_then(|d| d.now.get(field.key).cloned());
                grid = grid.child(self.ticks_field_row(field.key, now, p, window, cx));
            }
        }
        v_flex()
            .w_full()
            .flex_1()
            .min_h_0()
            .rounded(design::ui_px(cx, 8.0))
            .bg(moon(p.panel))
            .border_1()
            .border_color(moon(p.border))
            .overflow_hidden()
            .child(header)
            .child(cfg_row)
            .child(
                div()
                    .id("an-ticks-grid-scroll")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(grid),
            )
            .into_any_element()
    }

    /// Tick or untick every field the grid shows — the header's tick. Unticked is held at its
    /// base value by the search.
    fn ticks_set_all(&mut self, fields: &[&'static str], on: bool, cx: &mut Context<Self>) {
        for key in fields {
            if on {
                self.ticks.locked.remove(*key);
            } else {
                self.ticks.locked.insert((*key).to_string());
            }
        }
        self.persist_ticks_settings(cx);
        cx.notify();
    }

    /// The column headings: the master tick, field · strategy · В1 → ✕ · В2 ← ✕.
    fn ticks_grid_header(
        &self,
        fields: &[&'static TickParam],
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let keys: Vec<&'static str> = fields.iter().map(|f| f.key).collect();
        let all_on = !keys.is_empty() && keys.iter().all(|k| !self.ticks.locked.contains(*k));
        let cell = |text: String| {
            div()
                .w(design::font_w_px(cx, CELL_W))
                .flex_none()
                .text_center()
                .truncate()
                .child(text)
        };
        let mut head = h_flex()
            .w_full()
            .px(design::ui_px(cx, 8.0))
            .h(design::fit_h_px(cx, 22.0, 12.0, 5.0))
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .text_size(design::t_caption(cx))
            .text_color(moon(p.text_soft))
            .bg(moon(p.table_head))
            .child(
                div().flex_none().child(
                    MoonCheckbox::new("an-ticks-en-all")
                        .checked(all_on)
                        .size(design::CONTROL_TIER)
                        .on_change({
                            let view = cx.entity();
                            let keys = keys.clone();
                            move |on: &bool, _w, app| {
                                let on = *on;
                                view.update(app, |this, cx| this.ticks_set_all(&keys, on, cx));
                            }
                        }),
                ),
            )
            // The caption toggles the lot too, as the filter grid's does — a click target rather
            // than the checkbox's own label, which would widen the checkbox and push every
            // heading after it out of line.
            .child(
                div()
                    .id("an-ticks-en-all-lbl")
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .cursor_pointer()
                    .child(t!("analytics.tuner.field").to_string())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.ticks_set_all(&keys, !all_on, cx);
                    })),
            )
            .child(cell(t!("analytics.tuner.strat_chip").to_string()));
        for vi in 0..N_VAR {
            head = head
                .child(cell(t!("analytics.ticks.var_n", n = vi + 1).to_string()))
                // The only two copy buttons, both "the WHOLE column": → carries В1 into В2, ←
                // В2 into В1. Rows keep a matching spacer.
                .child(if vi == 0 {
                    glyph_btn(
                        "an-ticks-cp-col",
                        "→",
                        t!("analytics.time.tip_to_v2").to_string(),
                        p.amber,
                        p,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.ticks_copy_variant(0, 1, cx)))
                } else {
                    glyph_btn(
                        "an-ticks-cpb-col",
                        "←",
                        t!("analytics.time.tip_to_v1").to_string(),
                        p.amber,
                        p,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.ticks_copy_variant(1, 0, cx)))
                })
                .child(
                    glyph_btn(
                        SharedString::from(format!("an-ticks-clr-col-{vi}")),
                        "✕",
                        t!("analytics.time.tip_clear_all").to_string(),
                        p.orange,
                        p,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.ticks_clear_variant(vi, cx))),
                );
        }
        head.into_any_element()
    }

    /// A group's heading: its name, and why it is not searched when it is not — the kinds whose
    /// entry is taken from the fact, or a share of reproduced trades under the gate.
    fn ticks_group_header(
        &self,
        group: ParamGroup,
        data: Option<&TicksData>,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let title = match group {
            ParamGroup::Entry => t!("analytics.ticks.group_entry"),
            ParamGroup::Exit => t!("analytics.ticks.group_exit"),
        }
        .to_string();
        let gate = self.ticks.gate();
        let note: Option<(String, u32)> = match data {
            // Only a LOADED empty scope says so; a load in flight or a failed one has its own
            // note in the table.
            Some(d) if d.kinds.is_empty() => {
                Some((t!("analytics.ticks.no_deals").to_string(), p.text_muted))
            }
            Some(d) if group == ParamGroup::Entry && !d.entry_modelled() => Some((
                t!(
                    "analytics.ticks.entry_from_fact",
                    kinds = d.unmodelled_kinds().join(", ")
                )
                .to_string(),
                p.text_muted,
            )),
            Some(d) => {
                let (hits, n) = d.share_of(group);
                match d.group_passes(group, gate) {
                    Some(false) => Some((
                        t!(
                            "analytics.ticks.vary_gated",
                            hits = hits,
                            n = n,
                            gate = (gate * 100.0).round() as i64
                        )
                        .to_string(),
                        p.orange,
                    )),
                    None => Some((t!("analytics.ticks.vary_unknown").to_string(), p.text_muted)),
                    Some(true) => None,
                }
            }
            None => None,
        };
        h_flex()
            .w_full()
            .px(design::ui_px(cx, 8.0))
            .py(design::ui_px(cx, 2.0))
            .gap(design::ui_px(cx, 6.0))
            .items_center()
            .bg(moon_alpha(p.table_head, 0.6))
            .border_t_1()
            .border_color(moon_alpha(p.border, 0.7))
            .text_size(design::t_caption(cx))
            .font_family(design::ui_font())
            .child(div().flex_none().text_color(moon(p.text_soft)).child(title))
            .when_some(note, |el, (note, color)| {
                el.child(
                    div()
                        .id(SharedString::from(format!("an-ticks-grp-note-{group:?}")))
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(moon(color))
                        .tooltip(crate::panels::common::text_tooltip(note.clone()))
                        .child(note),
                )
            })
            .into_any_element()
    }

    /// One field: its tick, its name, the strategies' value, an input per variant with its
    /// clear cross.
    fn ticks_field_row(
        &mut self,
        key: &'static str,
        now: Option<NowValue>,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let read = super::model_cfg::current().entry_method.reads(key);
        let selected = self.ticks.sel_field == Some(key);
        let on = !self.ticks.locked.contains(key);
        let inputs: Vec<Entity<MoonInputState>> = (0..N_VAR)
            .map(|i| self.ticks_cell_input(i, key, window, cx))
            .collect();
        let mut row = h_flex()
            .id(SharedString::from(format!("an-ticks-field-{key}")))
            .w_full()
            .px(design::ui_px(cx, 8.0))
            .py(design::ui_px(cx, 2.0))
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .border_t_1()
            .border_color(moon_alpha(p.border, 0.5))
            .text_size(design::t_body(cx))
            .when(selected, |el| el.bg(moon_alpha(p.amber, 0.08)))
            .child(
                div().flex_none().child(
                    MoonCheckbox::new(SharedString::from(format!("an-ticks-en-{key}")))
                        .checked(on && read)
                        .disabled(!read)
                        .size(design::CONTROL_TIER)
                        .on_change({
                            let view = cx.entity();
                            move |on: &bool, _w, app| {
                                let on = *on;
                                view.update(app, |this, cx| {
                                    if on {
                                        this.ticks.locked.remove(key);
                                    } else {
                                        this.ticks.locked.insert(key.to_string());
                                    }
                                    this.persist_ticks_settings(cx);
                                    cx.notify();
                                });
                            }
                        }),
                ),
            )
            .child(
                div()
                    .id(SharedString::from(format!("an-ticks-name-{key}")))
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .cursor_pointer()
                    .text_color(if selected {
                        moon(p.amber)
                    } else if !read {
                        moon(p.text_muted)
                    } else {
                        moon(p.text)
                    })
                    .child(key)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.ticks.sel_field = Some(key);
                        cx.notify();
                    })),
            );
        // The strategies' value: a click sends it to В1 and holds the field out of the search —
        // it becomes a fixed value, as the filter grid's chip makes a fixed filter. A field the
        // method does not read says so instead.
        row = row.child(match now {
            _ if !read => div()
                .w(design::font_w_px(cx, CELL_W))
                .flex_none()
                .truncate()
                .font_family(design::ui_font())
                .text_size(design::t_caption(cx))
                .text_color(moon_alpha(p.text_muted, 0.7))
                .child(t!("analytics.ticks.not_read").to_string())
                .into_any_element(),
            Some(NowValue::Same(value)) if !value.is_empty() => div()
                .id(SharedString::from(format!("an-ticks-chip-{key}")))
                .w(design::font_w_px(cx, CELL_W))
                .flex_none()
                .truncate()
                .text_right()
                .cursor_pointer()
                .text_color(moon(p.amber))
                .hover(move |st| st.text_color(moon(p.text)))
                .child(value.clone())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.ticks.locked.insert(key.to_string());
                    this.persist_ticks_settings(cx);
                    this.ticks_set_cell(0, key, value.clone(), cx);
                }))
                .into_any_element(),
            Some(NowValue::Differs) => div()
                .w(design::font_w_px(cx, CELL_W))
                .flex_none()
                .truncate()
                .text_right()
                .text_color(moon(p.text_muted))
                .child(t!("analytics.time.cur_varies").to_string())
                .into_any_element(),
            _ => div()
                .w(design::font_w_px(cx, CELL_W))
                .flex_none()
                .text_right()
                .text_color(moon(p.text_muted))
                .child("—")
                .into_any_element(),
        });
        for (vi, input) in inputs.iter().enumerate() {
            row = row
                .child(
                    div()
                        .w(design::font_w_px(cx, CELL_W))
                        .flex_none()
                        .font_family(design::mono())
                        .child(
                            MoonInput::new(SharedString::from(format!("an-ticks-in-v{vi}-{key}")))
                                .state(input)
                                .size(design::INPUT_SIZE),
                        ),
                )
                // Under the header's copy arrow.
                .child(div().w(design::ui_px(cx, 12.0)).flex_none())
                .child(
                    glyph_btn(
                        SharedString::from(format!("an-ticks-clr-{vi}-{key}")),
                        "✕",
                        t!("analytics.time.tip_clear").to_string(),
                        p.orange,
                        p,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.ticks_set_cell(vi, key, String::new(), cx)
                    })),
                );
        }
        row.into_any_element()
    }

    /// Set one variant cell from outside its box — the strategy chip, a row's cross — and have
    /// the box show it.
    fn ticks_set_cell(&mut self, index: usize, key: &str, value: String, cx: &mut Context<Self>) {
        self.set_ticks_variant(index, key, value, cx);
        // The box is recreated from the stored value on the next frame.
        self.ticks.inputs.remove(&format!("v{index}:{key}"));
        cx.notify();
    }

    /// The input box of one variant cell, created on first use from the stored value and
    /// kept across repaints; a change stores the value and rescores the columns.
    fn ticks_cell_input(
        &mut self,
        index: usize,
        key: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        let id = format!("v{index}:{key}");
        if let Some(state) = self.ticks.inputs.get(&id) {
            return state.clone();
        }
        let value = self.ticks.variants[index]
            .get(key)
            .cloned()
            .unwrap_or_default();
        let state = cx.new(|cx| MoonInputState::new(window, cx).default_value(value));
        cx.subscribe_in(
            &state,
            window,
            move |this, state, ev: &MoonInputEvent, _window, cx| {
                if matches!(
                    ev,
                    MoonInputEvent::Change
                        | MoonInputEvent::Blur
                        | MoonInputEvent::PressEnter { .. }
                ) {
                    let value = state.read(cx).value().to_string();
                    if this.ticks.variants[index].get(key).map(String::as_str)
                        != Some(value.as_str())
                    {
                        this.set_ticks_variant(index, key, value, cx);
                    }
                    if !matches!(ev, MoonInputEvent::Change) {
                        cx.notify();
                    }
                }
            },
        )
        .detach();
        self.ticks.inputs.insert(id, state.clone());
        state
    }
}

/// The fields the scope's kinds understand — the union over the kinds present, in descriptor
/// order.
fn scope_fields(data: Option<&TicksData>) -> Vec<&'static TickParam> {
    let kinds: Vec<String> = data.map(|d| d.kinds.clone()).unwrap_or_default();
    moon_core::db::tuner::ticks::TICK_PARAMS
        .iter()
        .filter(|f| {
            kinds
                .iter()
                .any(|k| params_for(f.group, k).any(|g| g.key == f.key))
        })
        .collect()
}
