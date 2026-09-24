//! The settings of the "Entry/Exit" axis, laid out as "By filter" lays out its own: the search
//! row keeps the restart count inline and puts the rest behind the ⚙ popover, with the live
//! status on a band above it; the model's settings sit behind their own popover in the deal
//! table's head, beside the ✓ column they decide.
//!
//! The split follows what each setting changes. A search setting changes what the search looks
//! at and how hard; the entry method changes how a VARIANT is replayed, never the ✓ of the fact
//! (the verdict replays the corridor model on the trade's own settings), so it sits with the
//! search too. A model setting — a latency, a clock, a window, a tolerance — changes the replay
//! of every trade, the fact's included, so committing one judges the whole table again
//! (`ticks_replay_again`).
//!
//! Every setting persists (`WindowLayout::analytics_ticks`) but the minimum trades, which the
//! filter axis does not keep either.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonDropdown, MoonInput,
    MoonInputEvent, MoonInputState, MoonPalette, MoonPopover, MoonPopoverPlacement,
    MoonTooltipView, h_flex, v_flex,
};
use rust_i18n::t;

use super::super::super::AnalyticsView;
use super::super::shared::TunerKind;
use super::super::shell::{CfgInput, train_label};
use super::model_cfg::{self, MODEL_FIELDS, ModelField, Section, field_text, parse_field};
use super::state::{DEFAULT_GATE_PCT, SuggState};
use crate::design;
use crate::design::moon;
use moon_core::db::tuner::ticks::{EntryMethod, ModelSettings};

/// Content width of the two popovers, before the font scale and the component's own padding.
const POPUP_W: f32 = 270.0;

/// Width of a settings row's caption, font-scaled px.
const LABEL_W: f32 = 150.0;

/// The input-box cache key of one model setting.
fn model_input_id(field: &ModelField) -> String {
    format!("m:{}", field.id)
}

/// The locale key of an entry method's name, and of its one-line account.
fn method_keys(method: EntryMethod) -> (&'static str, &'static str) {
    match method {
        EntryMethod::Model => (
            "analytics.ticks.method_model",
            "analytics.ticks.method_model_help",
        ),
        EntryMethod::Shift => (
            "analytics.ticks.method_shift",
            "analytics.ticks.method_shift_help",
        ),
    }
}

impl AnalyticsView {
    /// The search row of the axis: the status band, then restarts, the settings gear, Stop,
    /// "Search" on the selected field and "Search all".
    pub(in crate::analytics::tuner) fn ticks_config_row(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let running = matches!(self.ticks.sugg, SuggState::Running { .. });
        let (status, status_color) = match &self.ticks.sugg {
            // Counted against the restarts the run was launched with, not the box's current
            // text.
            SuggState::Running { handle, total } => (
                t!(
                    "analytics.tuner.sugg_progress",
                    done = handle.completed(),
                    total = total
                )
                .to_string(),
                p.text_soft,
            ),
            // A note first; else how the last search went, so a restart or pass count that
            // changed nothing can be seen to have changed nothing.
            SuggState::Idle => match (&self.ticks.sugg_note, &self.ticks.last_result) {
                (Some(note), _) => (note.clone(), p.amber),
                (None, Some(result)) => (
                    super::variants::search_stats_line(&result.stats),
                    p.text_muted,
                ),
                (None, None) => (String::new(), p.text_muted),
            },
        };
        super::variants::probe_painted(&status);
        let placeholder = super::variants::DEFAULT_RESTARTS.to_string();
        let it_input = self.shell_cfg_input(
            TunerKind::Ticks,
            CfgInput::Restarts,
            &placeholder,
            window,
            cx,
        );
        let settings = self.ticks_search_settings(p, window, cx);
        let (one_tip, all_tip) = self.ticks_search_tips();
        let controls = h_flex()
            .w_full()
            .flex_none()
            .px(design::ui_px(cx, 12.0))
            .pb(design::ui_px(cx, 6.0))
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .text_size(design::t_caption(cx))
            .font_family(design::ui_font())
            .child(
                div()
                    .flex_none()
                    .text_color(moon(p.text_muted))
                    .child(t!("analytics.tuner.iters").to_string()),
            )
            .child(
                div()
                    .w(design::font_w_px(cx, 46.0))
                    .flex_none()
                    .font_family(design::mono())
                    .child(
                        MoonInput::new("tun-cfg-it-x")
                            .state(&it_input)
                            .size(design::INPUT_SIZE),
                    ),
            )
            .child(settings)
            .child(div().flex_1())
            .when(running, |el| {
                el.child(
                    div().flex_none().child(
                        MoonButton::new("tun-suggest-stop-x")
                            .variant(MoonButtonVariant::Soft)
                            .label(t!("analytics.tuner.stop").to_string())
                            .on_click(cx.listener(|this, _, _, cx| this.ticks_stop_suggest(cx)))
                            .render(),
                    ),
                )
            })
            .child(
                div()
                    .id("tun-suggest-one-x-box")
                    .flex_none()
                    .tooltip(move |_w, cx| cx.new(|_| MoonTooltipView::new(one_tip.clone())).into())
                    .child(
                        MoonButton::new("tun-suggest-one-x")
                            .variant(MoonButtonVariant::Soft)
                            .label(t!("analytics.tuner.suggest_one").to_string())
                            .disabled(running || self.ticks.sel_field.is_none())
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.ticks_suggest_one(window, cx)
                                }),
                            )
                            .render(),
                    ),
            )
            .child(
                div()
                    .id("tun-suggest-run-x-box")
                    .flex_none()
                    .tooltip(move |_w, cx| cx.new(|_| MoonTooltipView::new(all_tip.clone())).into())
                    .child(
                        MoonButton::new("tun-suggest-run-x")
                            .variant(MoonButtonVariant::Blue)
                            .label(t!("analytics.tuner.suggest_run").to_string())
                            .disabled(running)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.ticks_suggest(window, cx)),
                            )
                            .render(),
                    ),
            );
        v_flex()
            .w_full()
            .flex_none()
            .when(!status.is_empty(), |el| {
                el.child(
                    div()
                        .id("tun-suggest-status-x")
                        .w_full()
                        .min_w_0()
                        .px(design::ui_px(cx, 12.0))
                        .pb(design::ui_px(cx, 4.0))
                        .truncate()
                        .text_size(design::t_caption(cx))
                        .font_family(design::ui_font())
                        .text_color(moon(status_color))
                        .tooltip(crate::panels::common::text_tooltip(status.clone()))
                        .child(status),
                )
            })
            .child(controls)
            .into_any_element()
    }

    /// The search settings popover and its gear. Built only while open, as the filter's is:
    /// `MoonPopover` takes its content eagerly, and the panel repaints through a whole search.
    fn ticks_search_settings(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.ticks.sugg_cfg_open;
        let content = open.then(|| self.ticks_search_settings_content(p, window, cx));
        let entity = cx.entity();
        let mut popover = MoonPopover::new("tun-cfg-popover-x")
            .placement(MoonPopoverPlacement::BottomStart)
            .content_width_font(POPUP_W)
            .close_on_content_click(false)
            // A dropdown inside paints in its own layer, and the outside-click test would shut
            // the popover before the pick landed (docs-internal/FORK_BUGS.md, Popover); the ✕ and
            // the gear close it.
            .overlay_closable(false)
            .open(open)
            .on_open_change(move |open, _window, app| {
                entity.update(app, |this, cx| {
                    this.ticks.sugg_cfg_open = open;
                    cx.notify();
                });
            })
            .trigger(
                MoonButton::new("tun-cfg-gear-x")
                    .label("⚙")
                    .variant(MoonButtonVariant::Soft)
                    .tooltip(t!("analytics.tuner.cfg_title").to_string())
                    .render(),
            );
        if let Some(content) = content {
            popover = popover.content(content);
        }
        popover.into_any_element()
    }

    fn ticks_search_settings_content(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mn_input = self.shell_cfg_input(
            TunerKind::Ticks,
            CfgInput::MinTrades,
            &t!("analytics.tuner.auto_ph"),
            window,
            cx,
        );
        let seed_input = self.shell_cfg_input(
            TunerKind::Ticks,
            CfgInput::Seed,
            &t!("analytics.tuner.seed_ph"),
            window,
            cx,
        );
        let passes_input = self.ticks_text_input(
            "x-cfg-passes",
            self.ticks.passes.clone(),
            moon_core::db::tuner::ticks::search::DEFAULT_MAX_PASSES.to_string(),
            |this, value| this.ticks.passes = value,
            window,
            cx,
        );
        let gate_input = self.ticks_text_input(
            "x-cfg-gate",
            self.ticks.gate_pct.clone(),
            DEFAULT_GATE_PCT.to_string(),
            |this, value| this.ticks.gate_pct = value,
            window,
            cx,
        );
        let train_pct = self.ticks.train_pct;
        let tr_view = cx.entity();
        let tr_items = crate::panels::radio_items(
            super::super::filter::state::TRAIN_OPTIONS.map(|n| {
                (
                    n,
                    SharedString::from(format!("tun-tr-x-{n}")),
                    SharedString::from(train_label(n)),
                )
            }),
            train_pct,
            crate::panels::RadioMark::Highlight,
            move |app, n| {
                tr_view.update(app, |this, cx| {
                    this.ticks.train_pct = n;
                    this.persist_ticks_settings(cx);
                    cx.notify();
                });
            },
        );
        let tr_combo = MoonDropdown::new("tun-cfg-tr-x")
            .label(train_label(train_pct))
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::density(cx))
            .menu_width_scaled(96.0)
            .items(tr_items);
        let method = model_cfg::current().entry_method;
        let me_view = cx.entity();
        let me_items = crate::panels::radio_items(
            [EntryMethod::Model, EntryMethod::Shift].map(|m| {
                (
                    m,
                    SharedString::from(format!("tun-me-x-{m:?}")),
                    SharedString::from(t!(method_keys(m).0).to_string()),
                )
            }),
            method,
            crate::panels::RadioMark::Highlight,
            move |app, m| {
                me_view.update(app, |this, cx| this.ticks_set_entry_method(m, cx));
            },
        );
        let me_combo = MoonDropdown::new("tun-cfg-me-x")
            .label(t!(method_keys(method).0).to_string())
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::density(cx))
            .menu_width_scaled(140.0)
            .items(me_items);
        let box_of = |id: &'static str, state: &Entity<MoonInputState>, w: f32| {
            div()
                .w(design::font_w_px(cx, w))
                .flex_none()
                .font_family(design::mono())
                .child(
                    MoonInput::new(SharedString::from(id))
                        .state(state)
                        .size(design::INPUT_SIZE),
                )
                .into_any_element()
        };
        let mut content = popup_frame("tun-cfg-popup-x", window, cx)
            .child(popup_head(
                t!("analytics.tuner.cfg_title").to_string(),
                "tun-cfg-close-x",
                |this| this.ticks.sugg_cfg_open = false,
                p,
                cx,
            ))
            .child(popup_section(
                t!("analytics.tuner.cfg_search_section").to_string(),
                p,
                cx,
            ))
            .child(popup_row(
                t!("analytics.tuner.min_trades").to_string(),
                None,
                box_of("tun-cfg-mn-x", &mn_input, 76.0),
                p,
                cx,
            ))
            .child(popup_row(
                t!("analytics.ticks.cfg_passes").to_string(),
                Some(t!("analytics.ticks.cfg_passes_tip").to_string()),
                box_of("tun-cfg-passes-x", &passes_input, 76.0),
                p,
                cx,
            ))
            .child(popup_row(
                t!("analytics.ticks.cfg_gate").to_string(),
                Some(t!("analytics.ticks.cfg_gate_tip").to_string()),
                box_of("tun-cfg-gate-x", &gate_input, 76.0),
                p,
                cx,
            ))
            .child(popup_section(
                t!("analytics.ticks.cfg_entry_section").to_string(),
                p,
                cx,
            ))
            .child(popup_row(
                t!("analytics.ticks.cfg_method").to_string(),
                None,
                div().flex_none().child(me_combo).into_any_element(),
                p,
                cx,
            ))
            .child(
                div()
                    .w_full()
                    .text_color(moon(p.text_muted))
                    .child(t!(method_keys(method).1).to_string()),
            )
            .child(
                MoonCheckbox::new("tun-cfg-corridor-x")
                    .label(t!("analytics.ticks.cfg_keep_corridor").to_string())
                    .description(t!("analytics.ticks.cfg_keep_corridor_help").to_string())
                    .checked(self.ticks.keep_corridor)
                    // `on_change` hands the callback an `&mut App`, not a `Context`.
                    .on_change({
                        let view = cx.entity();
                        move |checked: &bool, _w, app| {
                            let on = *checked;
                            view.update(app, |this, cx| {
                                this.ticks.keep_corridor = on;
                                this.persist_ticks_settings(cx);
                                cx.notify();
                            });
                        }
                    }),
            )
            .child(popup_section(
                t!("analytics.tuner.cfg_validation_section").to_string(),
                p,
                cx,
            ))
            .child(popup_row(
                t!("analytics.tuner.train_share").to_string(),
                None,
                div().flex_none().child(tr_combo).into_any_element(),
                p,
                cx,
            ))
            .child(popup_section(
                t!("analytics.tuner.cfg_repeat_section").to_string(),
                p,
                cx,
            ))
            .child(popup_row(
                t!("analytics.tuner.seed").to_string(),
                None,
                box_of("tun-cfg-seed-x", &seed_input, 126.0),
                p,
                cx,
            ));
        // The seed the last search ran with, so an interesting answer can be repeated.
        if let Some(last) = self.ticks.last_seed {
            content = content.child(popup_row(
                t!("analytics.tuner.seed_last").to_string(),
                None,
                h_flex()
                    .items_center()
                    .gap(design::ui_px(cx, 6.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(design::mono())
                            .text_color(moon(p.text_soft))
                            .child(last.to_string()),
                    )
                    .child(
                        MoonButton::new("tun-cfg-seed-pin-x")
                            .label(t!("analytics.tuner.seed_pin").to_string())
                            .variant(MoonButtonVariant::Soft)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.ticks.seed = last.to_string();
                                this.shell_forget_cfg_input(TunerKind::Ticks, CfgInput::Seed);
                                this.persist_ticks_settings(cx);
                                cx.notify();
                            }))
                            .render(),
                    )
                    .into_any_element(),
                p,
                cx,
            ));
        }
        content.into_any_element()
    }

    /// The model settings popover and its button, for the deal table's head. Built only while
    /// open.
    pub(in crate::analytics::tuner) fn ticks_model_settings(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.ticks.model_cfg_open;
        let content = open.then(|| self.ticks_model_settings_content(p, window, cx));
        let entity = cx.entity();
        let mut popover = MoonPopover::new("an-ticks-model-popover")
            .placement(MoonPopoverPlacement::BottomEnd)
            .content_width_font(POPUP_W)
            .close_on_content_click(false)
            .overlay_closable(false)
            .open(open)
            .on_open_change(move |open, _window, app| {
                entity.update(app, |this, cx| {
                    this.ticks.model_cfg_open = open;
                    cx.notify();
                });
            })
            .trigger(
                MoonButton::new("an-ticks-model-btn")
                    .label(t!("analytics.ticks.model_btn").to_string())
                    .variant(MoonButtonVariant::Soft)
                    .tooltip(t!("analytics.ticks.model_title").to_string())
                    .render(),
            );
        if let Some(content) = content {
            popover = popover.content(content);
        }
        popover.into_any_element()
    }

    fn ticks_model_settings_content(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut content = popup_frame("an-ticks-model-popup", window, cx)
            .child(popup_head(
                t!("analytics.ticks.model_title").to_string(),
                "an-ticks-model-close",
                |this| this.ticks.model_cfg_open = false,
                p,
                cx,
            ))
            // What the model does not know, whatever its settings.
            .child(
                div()
                    .w_full()
                    .text_color(moon(p.text_muted))
                    .child(t!("analytics.ticks.assumptions").to_string()),
            );
        let mut section: Option<Section> = None;
        for field in MODEL_FIELDS {
            if section != Some(field.section) {
                section = Some(field.section);
                content = content.child(popup_section(
                    t!(field.section.title_key()).to_string(),
                    p,
                    cx,
                ));
            }
            let input = self.ticks_model_input(field, window, cx);
            content = content.child(popup_row(
                t!(field.label).to_string(),
                Some(t!(field.tip).to_string()),
                div()
                    .w(design::font_w_px(cx, 76.0))
                    .flex_none()
                    .font_family(design::mono())
                    .child(
                        MoonInput::new(SharedString::from(format!("an-ticks-m-{}", field.id)))
                            .state(&input)
                            .size(design::INPUT_SIZE),
                    )
                    .into_any_element(),
                p,
                cx,
            ));
        }
        content
            .children(self.ticks_tail_rows(p, window, cx))
            .child(
                h_flex().w_full().justify_end().child(
                    MoonButton::new("an-ticks-model-reset")
                        .label(t!("analytics.ticks.model_reset").to_string())
                        .variant(MoonButtonVariant::Soft)
                        .disabled(
                            model_cfg::current()
                                == ModelSettings {
                                    entry_method: model_cfg::current().entry_method,
                                    ..ModelSettings::default()
                                },
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.ticks_reset_model(cx)))
                        .render(),
                ),
            )
            .into_any_element()
    }

    /// A plain text box of a search setting, cached in the axis' inputs: every change is taken
    /// at once, as the filter's settings are, and persisted.
    fn ticks_text_input(
        &mut self,
        id: &'static str,
        value: String,
        placeholder: String,
        apply: fn(&mut AnalyticsView, String),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        if let Some(state) = self.ticks.inputs.get(id) {
            return state.clone();
        }
        let state = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .default_value(value)
                .placeholder(placeholder)
        });
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
                    apply(this, state.read(cx).value().to_string());
                    this.persist_ticks_settings(cx);
                    if !matches!(ev, MoonInputEvent::Change) {
                        cx.notify();
                    }
                }
            },
        )
        .detach();
        self.ticks.inputs.insert(id.to_string(), state.clone());
        state
    }

    /// The box of one model setting. Taken on Enter or when the box loses focus, never per
    /// keystroke: each commit judges the whole table again. A value the model cannot take puts
    /// the box back to the setting in force.
    fn ticks_model_input(
        &mut self,
        field: &'static ModelField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        let id = model_input_id(field);
        if let Some(state) = self.ticks.inputs.get(&id) {
            return state.clone();
        }
        let value = field_text(field, &model_cfg::current());
        let state = cx.new(|cx| MoonInputState::new(window, cx).default_value(value));
        cx.subscribe_in(
            &state,
            window,
            move |this, state, ev: &MoonInputEvent, _window, cx| {
                if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                    return;
                }
                let typed = state.read(cx).value().to_string();
                let mut settings = model_cfg::current();
                if let Some(value) = parse_field(&typed) {
                    (field.set)(&mut settings, value);
                    this.ticks_set_model(settings, cx);
                }
                // The box shows the setting in force: the sanitized value, or the old one.
                if typed != field_text(field, &model_cfg::current()) {
                    this.ticks.inputs.remove(&model_input_id(field));
                }
                cx.notify();
            },
        )
        .detach();
        self.ticks.inputs.insert(id, state.clone());
        state
    }

    /// Put model settings in force: persisted, and every row judged again under them.
    fn ticks_set_model(&mut self, settings: ModelSettings, cx: &mut Context<Self>) {
        if model_cfg::replace(settings) {
            self.persist_ticks_settings(cx);
            self.ticks_replay_again(cx);
        }
    }

    /// Every model setting back to its measured default; the entry method, a search setting,
    /// stays.
    fn ticks_reset_model(&mut self, cx: &mut Context<Self>) {
        let settings = ModelSettings {
            entry_method: model_cfg::current().entry_method,
            ..ModelSettings::default()
        };
        self.ticks.inputs.retain(|id, _| !id.starts_with("m:"));
        self.ticks_set_model(settings, cx);
        cx.notify();
    }

    /// Pick how a variant's entry is replayed. The fact's ✓ does not depend on it, so the
    /// table stays as it is; the variant columns are scored again, and the grid greys out the
    /// fields the method does not read.
    fn ticks_set_entry_method(&mut self, method: EntryMethod, cx: &mut Context<Self>) {
        let settings = ModelSettings {
            entry_method: method,
            ..model_cfg::current()
        };
        if model_cfg::replace(settings) {
            self.persist_ticks_settings(cx);
            self.arm_ticks_variants(cx);
        }
        cx.notify();
    }

    /// Write the axis' settings into the saved layout.
    pub(in crate::analytics::tuner) fn persist_ticks_settings(&self, cx: &mut Context<Self>) {
        let value = Some(self.ticks.saved());
        self.persist_setting(cx, |l| &mut l.analytics_ticks, value);
    }
}

/// The popovers' scroll-bounded column. No padding, background, border or corners: the popover
/// supplies them (see the filter's settings popover).
fn popup_frame(id: &'static str, window: &Window, cx: &Context<AnalyticsView>) -> Stateful<Div> {
    let max_h = px(
        (f32::from(window.viewport_size().height) - f32::from(design::ui_px(cx, 24.0)))
            .max(f32::from(design::ui_px(cx, 180.0))),
    );
    v_flex()
        .id(id)
        .w_full()
        .max_h(max_h)
        .overflow_y_scroll()
        .gap(design::ui_px(cx, 6.0))
        .text_size(design::t_caption(cx))
        .font_family(design::ui_font())
}

/// A popover's title line with its ✕.
fn popup_head(
    title: String,
    close_id: &'static str,
    close: fn(&mut AnalyticsView),
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    h_flex()
        .w_full()
        .items_center()
        .child(
            div()
                .flex_1()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(moon(p.text))
                .child(title),
        )
        .child(
            MoonButton::new(close_id)
                .label("✕")
                .variant(MoonButtonVariant::Ghost)
                .on_click(cx.listener(move |this, _, _, cx| {
                    close(this);
                    cx.notify();
                }))
                .render(),
        )
        .into_any_element()
}

/// A section heading inside a popover.
pub(super) fn popup_section(
    title: String,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    div()
        .w_full()
        .pt(design::ui_px(cx, 5.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(moon(p.text_soft))
        .child(title)
        .into_any_element()
}

/// One setting: its caption (with a tooltip when it needs explaining) and its control.
pub(super) fn popup_row(
    caption: String,
    tip: Option<String>,
    body: AnyElement,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    let label = div()
        .id(SharedString::from(format!("tun-cfg-lbl-{caption}")))
        .w(design::font_w_px(cx, LABEL_W))
        .flex_none()
        .truncate()
        .text_color(moon(p.text_muted))
        .child(caption)
        .when_some(tip, |el, tip| {
            el.tooltip(crate::panels::common::text_tooltip(tip))
        });
    h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .child(label)
        .child(body)
        .into_any_element()
}
