//! The parameter grid of the "Entry/Exit" axis: the two groups of `TICK_PARAMS`, each behind
//! its own caret and its own "vary" switch, with the "now" value of the selected strategies,
//! the variant columns В1/В2 as input boxes, and a "fix" tick per field that holds it out of
//! the search.
//!
//! A group's "vary" switch is locked while the model does not reproduce enough of the fact
//! for that group (`SHARE_GATE`): searching over a model that cannot replay what happened
//! optimizes noise, and the switch says so in its tooltip. The Entry group is shown only when
//! every kind in the scope has an entry model; otherwise it folds to one line saying whose
//! entry is taken from the fact.

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonVariant, MoonCheckbox, MoonInput, MoonInputEvent, MoonInputState,
    MoonPalette, MoonTooltipView, h_flex, v_flex,
};
use rust_i18n::t;

use super::super::super::AnalyticsView;
use super::super::shared::{N_VAR, TunerKind, collapse_caret};
use super::state::{NowValue, SHARE_GATE, TicksData};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::tuner::ticks::params::{ParamGroup, TickParam, params_for};

/// Width of the "now" and variant cells, font-scaled px.
const CELL_W: f32 = 72.0;
/// Width of the "fix" tick cell.
const FIX_W: f32 = 28.0;

impl AnalyticsView {
    /// The grid panel: the shared toolbar (title, Copy, Save), the search row, then the
    /// assumptions line and the two groups, scrolling.
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
        let mut body = v_flex().w_full().flex_none();
        // What the model does not know, in one line — the spec's §4.4, kept where the user
        // reads the numbers the assumptions shape.
        body = body.child(
            div()
                .w_full()
                .px(design::ui_px(cx, 12.0))
                .pb(design::ui_px(cx, 6.0))
                .font_family(design::ui_font())
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(t!("analytics.ticks.assumptions").to_string()),
        );
        let entry_modelled = data.as_ref().is_some_and(|d| d.entry_modelled());
        let unmodelled: Vec<String> = data
            .as_ref()
            .map(|d| d.unmodelled_kinds().into_iter().map(String::from).collect())
            .unwrap_or_default();
        // Only a LOADED empty scope says so; a load in flight or a failed one has its own note.
        let no_deals = data.as_ref().is_some_and(|d| d.kinds.is_empty());
        let entry_note = if no_deals {
            Some(t!("analytics.ticks.no_deals").to_string())
        } else if entry_modelled {
            None
        } else {
            Some(
                t!(
                    "analytics.ticks.entry_from_fact",
                    kinds = unmodelled.join(", ")
                )
                .to_string(),
            )
        };
        let entry_open = self.ticks.entry_open && entry_modelled;
        let exit_open = self.ticks.exit_open;
        body = body.child(self.ticks_group(
            ParamGroup::Entry,
            t!("analytics.ticks.group_entry").to_string(),
            entry_note,
            entry_open,
            entry_modelled,
            data.as_deref(),
            p,
            window,
            cx,
        ));
        body = body.child(self.ticks_group(
            ParamGroup::Exit,
            t!("analytics.ticks.group_exit").to_string(),
            None,
            exit_open,
            true,
            data.as_deref(),
            p,
            window,
            cx,
        ));
        let tools = self.ticks_grid_tools(cx);
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
                    .child(body),
            )
            .child(
                h_flex()
                    .w_full()
                    .flex_none()
                    .px(design::ui_px(cx, 12.0))
                    .py(design::ui_px(cx, 6.0))
                    .justify_end()
                    .child(tools),
            )
            .into_any_element()
    }

    /// The card's accessory: "В1 → В2" and the two "clear" buttons.
    fn ticks_grid_tools(&self, cx: &Context<Self>) -> AnyElement {
        h_flex()
            .gap(design::ui_px(cx, 4.0))
            .font_family(design::ui_font())
            .child(
                MoonButton::new("an-ticks-v1-to-v2")
                    .variant(MoonButtonVariant::Soft)
                    .label(t!("analytics.ticks.v1_to_v2").to_string())
                    .disabled(!self.ticks.has_changes())
                    .on_click(cx.listener(|this, _, _, cx| this.ticks_copy_v1_to_v2(cx)))
                    .render(),
            )
            .children((0..N_VAR).map(|i| {
                MoonButton::new(SharedString::from(format!("an-ticks-clear-v{i}")))
                    .variant(MoonButtonVariant::Soft)
                    .label(t!("analytics.ticks.clear_v", n = i + 1).to_string())
                    .disabled(self.ticks.variants[i].is_empty())
                    .on_click(cx.listener(move |this, _, _, cx| this.ticks_clear_variant(i, cx)))
                    .render()
            }))
            .into_any_element()
    }

    /// One group: a header line with its switch and caret, then a row per field.
    #[allow(clippy::too_many_arguments)]
    fn ticks_group(
        &mut self,
        group: ParamGroup,
        title: String,
        note: Option<String>,
        open: bool,
        enabled: bool,
        data: Option<&TicksData>,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (id, checkbox_id) = match group {
            ParamGroup::Entry => ("an-ticks-grp-entry", "an-ticks-vary-entry"),
            ParamGroup::Exit => ("an-ticks-grp-exit", "an-ticks-vary-exit"),
        };
        let caret = collapse_caret(
            id,
            !open,
            t!("analytics.ticks.group_collapse").to_string(),
            t!("analytics.ticks.group_expand").to_string(),
            p,
            cx.listener(move |this, _, _, cx| {
                match group {
                    ParamGroup::Entry => this.ticks.entry_open = !this.ticks.entry_open,
                    ParamGroup::Exit => this.ticks.exit_open = !this.ticks.exit_open,
                }
                cx.notify();
            }),
        );
        // The "vary" switch: on by the user, allowed by the gate.
        let passes = data.and_then(|d| d.group_passes(group));
        let share = data
            .map(|d| match group {
                ParamGroup::Entry => d.entry_share,
                ParamGroup::Exit => d.exit_share,
            })
            .unwrap_or((0, 0));
        let gated = enabled && passes == Some(true);
        let vary_on = match group {
            ParamGroup::Entry => self.ticks.vary_entry,
            ParamGroup::Exit => self.ticks.vary_exit,
        };
        let vary_tip = if gated {
            t!("analytics.ticks.vary_tip").to_string()
        } else if passes == Some(false) {
            t!(
                "analytics.ticks.vary_gated",
                hits = share.0,
                n = share.1,
                gate = (SHARE_GATE * 100.0) as i64
            )
            .to_string()
        } else {
            t!("analytics.ticks.vary_unknown").to_string()
        };
        let vary = div()
            .id(SharedString::from(format!("{checkbox_id}-box")))
            .flex_none()
            .tooltip(move |_w, cx| cx.new(|_| MoonTooltipView::new(vary_tip.clone())).into())
            .child(
                MoonCheckbox::new(SharedString::from(checkbox_id))
                    .label(t!("analytics.ticks.vary").to_string())
                    .checked(vary_on && gated)
                    .disabled(!gated)
                    .on_change({
                        let view = cx.entity();
                        move |on: &bool, _w, app| {
                            let on = *on;
                            view.update(app, |this, cx| {
                                match group {
                                    ParamGroup::Entry => this.ticks.vary_entry = on,
                                    ParamGroup::Exit => this.ticks.vary_exit = on,
                                }
                                cx.notify();
                            });
                        }
                    }),
            );
        let mut head = h_flex()
            .w_full()
            .px(design::ui_px(cx, 12.0))
            .py(design::ui_px(cx, 4.0))
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .bg(moon(p.table_head))
            .font_family(design::ui_font())
            .child(
                div()
                    .flex_none()
                    .text_size(design::t_body(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if enabled {
                        moon(p.text)
                    } else {
                        moon(p.text_muted)
                    })
                    .child(title),
            );
        head = match note {
            Some(note) => head.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .child(note),
            ),
            None => head.child(div().flex_1()),
        };
        if enabled {
            head = head.child(vary).child(div().flex_none().child(caret));
        }
        let mut out = v_flex().w_full().flex_none().child(head);
        if !(open && enabled) {
            return out.into_any_element();
        }
        // The fields every kind in the scope understands: the union over the kinds present,
        // in descriptor order.
        let kinds: Vec<String> = data.map(|d| d.kinds.clone()).unwrap_or_default();
        let fields: Vec<&'static TickParam> = moon_core::db::tuner::ticks::TICK_PARAMS
            .iter()
            .filter(|f| f.group == group)
            .filter(|f| {
                kinds
                    .iter()
                    .any(|k| params_for(group, k).any(|g| g.key == f.key))
            })
            .collect();
        out = out.child(self.ticks_grid_header(p, cx));
        let now: Vec<(&'static str, Option<NowValue>)> = fields
            .iter()
            .map(|f| (f.key, data.and_then(|d| d.now.get(f.key).cloned())))
            .collect();
        for (key, now) in now {
            out = out.child(self.ticks_field_row(key, now, p, window, cx));
        }
        out.into_any_element()
    }

    /// The column headings of a group: field · now · В1 · В2 · fix.
    fn ticks_grid_header(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        let cell = |text: String| {
            div()
                .w(design::font_w_px(cx, CELL_W))
                .flex_none()
                .text_right()
                .child(text)
        };
        h_flex()
            .w_full()
            .px(design::ui_px(cx, 12.0))
            .py(design::ui_px(cx, 2.0))
            .gap(design::ui_px(cx, 6.0))
            .text_size(design::t_caption(cx))
            .text_color(moon(p.text_soft))
            .child(
                div()
                    .flex_1()
                    .child(t!("analytics.tuner.field").to_string()),
            )
            .child(cell(t!("analytics.ticks.now").to_string()))
            .children((0..N_VAR).map(|i| cell(t!("analytics.ticks.var_n", n = i + 1).to_string())))
            .child(
                div()
                    .w(design::font_w_px(cx, FIX_W))
                    .flex_none()
                    .text_center()
                    .child(t!("analytics.ticks.fix").to_string()),
            )
            .into_any_element()
    }

    /// One field: its key, the "now" value, an input per variant, the "fix" tick.
    fn ticks_field_row(
        &mut self,
        key: &'static str,
        now: Option<NowValue>,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (text, muted) = match now {
            Some(NowValue::Same(v)) if !v.is_empty() => (v, false),
            Some(NowValue::Same(_)) | None => ("—".to_string(), true),
            Some(NowValue::Differs) => (t!("analytics.time.cur_varies").to_string(), true),
        };
        let inputs: Vec<Entity<MoonInputState>> = (0..N_VAR)
            .map(|i| self.ticks_cell_input(i, key, window, cx))
            .collect();
        let locked = self.ticks.locked.contains(key);
        let mut row = h_flex()
            .w_full()
            .px(design::ui_px(cx, 12.0))
            .py(design::ui_px(cx, 2.0))
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .border_t_1()
            .border_color(moon_alpha(p.border, 0.5))
            .text_size(design::t_body(cx))
            .child(div().flex_1().min_w_0().truncate().child(key))
            .child(
                div()
                    .w(design::font_w_px(cx, CELL_W))
                    .flex_none()
                    .text_right()
                    .text_color(if muted {
                        moon(p.text_muted)
                    } else {
                        moon(p.text)
                    })
                    .child(text),
            );
        for (i, input) in inputs.iter().enumerate() {
            row = row.child(
                div()
                    .w(design::font_w_px(cx, CELL_W))
                    .flex_none()
                    .font_family(design::mono())
                    .child(
                        MoonInput::new(SharedString::from(format!("an-ticks-in-v{i}-{key}")))
                            .state(input)
                            .size(design::INPUT_SIZE),
                    ),
            );
        }
        row = row.child(
            div()
                .w(design::font_w_px(cx, FIX_W))
                .flex_none()
                .flex()
                .justify_center()
                .child(
                    MoonCheckbox::new(SharedString::from(format!("an-ticks-fix-{key}")))
                        .checked(locked)
                        .on_change({
                            let view = cx.entity();
                            move |on: &bool, _w, app| {
                                let on = *on;
                                view.update(app, |this, cx| {
                                    if on {
                                        this.ticks.locked.insert(key.to_string());
                                    } else {
                                        this.ticks.locked.remove(key);
                                    }
                                    cx.notify();
                                });
                            }
                        }),
                ),
        );
        row.into_any_element()
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
