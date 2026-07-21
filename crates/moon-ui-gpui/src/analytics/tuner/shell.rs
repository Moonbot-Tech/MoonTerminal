//! The SHARED tuner shell: the toolbar (rounding / Make a copy / Save) and the
//! suggestion row (restarts / trades ≥ / depth / Search / Search all).
//! Rendered by ONE piece of code for every axis ("By filter", "By time", …) —
//! only the grid rows and the actions differ. Actions are dispatched by `TunerKind` to the
//! concrete tuner. The axes are not symmetric: "By time" fixes its own search depth and has
//! no restarts, so the restarts box and the depth dropdown render for "By filter" only.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonDropdown,
    MoonInput, MoonInputEvent, MoonInputState, MoonMenuSize, MoonPalette, h_flex,
};
use rust_i18n::t;

use super::super::AnalyticsView;
use super::state::{DEFAULT_ITERS, EDGE_OPTIONS, TunerKind, iters_of};
use crate::design;
use crate::design::moon;

impl AnalyticsView {
    /// The shared toolbar of the tuner card: the title + (when a strategy is selected)
    /// "round results" + "Make a copy" + "Save". The actions are per-axis.
    pub(super) fn shell_toolbar(
        &self,
        kind: TunerKind,
        title: String,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let time = kind == TunerKind::Time;
        let k = if time { "t" } else { "f" };
        let round = match kind {
            TunerKind::Filter => self.tuner.round_results,
            TunerKind::Time => self.time_tuner.round_results,
        };
        let mut header = h_flex()
            .w_full()
            // Pinned above the scrollable rows in both tuner panels — must not shrink.
            .flex_none()
            .px(design::ui_px(cx, 12.0))
            .py(design::ui_px(cx, 8.0))
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .child(
                div()
                    .text_size(design::t_title(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(div().flex_1());
        // Rounding the result affects the SUGGESTION — always shown (the suggestion is
        // available without a selected strategy too, over the current scope).
        header = header
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .child(t!("analytics.tuner.round_lbl").to_string()),
            )
            .child(
                div().flex_none().child(
                    MoonCheckbox::new(SharedString::from(format!("tun-round-{k}")))
                        .checked(round)
                        .size(MoonCheckboxSize::Compact)
                        .on_change({
                            let view = cx.entity();
                            move |ch: &bool, _w, app| {
                                let on = *ch;
                                view.update(app, |this, cx| {
                                    match kind {
                                        TunerKind::Filter => this.tuner.round_results = on,
                                        TunerKind::Time => this.time_tuner.round_results = on,
                                    }
                                    cx.notify();
                                });
                            }
                        }),
                ),
            );
        // Writing (Copy / Save) — ONLY into a selected strategy.
        if self.sel_strategy.is_some() {
            // Copy is single-target only — hidden in multi-select (many addressees, no
            // per-target preview); bulk Save is the multi path.
            if !self.is_multi() {
                header = header.child(
                    MoonButton::new(SharedString::from(format!("tun-copy-{k}")))
                        .variant(MoonButtonVariant::Soft)
                        .size(MoonButtonSize::Micro)
                        .label(t!("analytics.tuner.copy_btn").to_string())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            // One "copy" for all axes: axis changes → a NEW strategy.
                            match kind {
                                TunerKind::Filter => this.open_copy_dialog(window, cx),
                                TunerKind::Time => this.time_open_copy_dialog(window, cx),
                            }
                            cx.notify();
                        }))
                        .render(),
                );
            }
            header = header.child({
                // "Save" lights up amber when there is something to write (filter
                // thresholds OR a time schedule that differs from the current one).
                // Works for both axes; the action is dispatched by `kind`.
                let dirty = match kind {
                    TunerKind::Filter => self.save_dirty(),
                    TunerKind::Time => self.time_tuner.is_dirty(),
                };
                MoonButton::new(SharedString::from(format!("tun-save-{k}")))
                    .variant(if dirty {
                        MoonButtonVariant::Amber
                    } else {
                        MoonButtonVariant::Soft
                    })
                    .size(MoonButtonSize::Micro)
                    .label(t!("analytics.tuner.save_btn").to_string())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        match kind {
                            TunerKind::Filter => this.open_save_dialog(cx),
                            TunerKind::Time => this.time_open_save_dialog(cx),
                        }
                        cx.notify();
                    }))
                    .render()
            });
        }
        header.into_any_element()
    }

    /// The shared suggestion row: restarts / trades ≥ / depth (quantiles) + "Search" /
    /// "Search all". The settings come from the axis state and the buttons dispatch into its
    /// auto-suggestion. "By time" shows only "trades ≥" and "Search all": it sweeps at a fixed
    /// depth and has no per-field pass to restart.
    pub(super) fn shell_config_row(
        &mut self,
        kind: TunerKind,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let time = kind == TunerKind::Time;
        let busy = match kind {
            TunerKind::Filter => self.tuner.sugg_busy,
            TunerKind::Time => self.time_tuner.sugg_busy,
        };
        let mn_input = self.shell_min_trades_input(kind, window, cx);
        // The time sweep fixes its depth and has no restarts, so avoid constructing widget state
        // that this axis cannot render.
        let it_input = (!time).then(|| self.shell_restarts_input(window, cx));
        // The number of quantiles — a combo box (4/8/…/128).
        let ed_combo = (!time).then(|| {
            let edges = self.tuner.edges;
            let ed_view = cx.entity();
            let ed_items = crate::panels::radio_items(
                EDGE_OPTIONS.map(|n| {
                    (
                        n,
                        SharedString::from(format!("tun-ed-{n}")),
                        SharedString::from(n.to_string()),
                    )
                }),
                edges,
                crate::panels::RadioMark::Highlight,
                move |app, n| {
                    ed_view.update(app, |this, cx| {
                        this.tuner.edges = n;
                        this.persist_tuner_edges(cx);
                        cx.notify();
                    });
                },
            );
            MoonDropdown::new(SharedString::from("tun-cfg-ed-f"))
                .label(format!("{edges} ▾"))
                .trigger_variant(MoonButtonVariant::Soft)
                .trigger_size(MoonButtonSize::Micro)
                .menu_width(design::font_w(cx, 64.0))
                .menu_size(MoonMenuSize::Compact)
                .items(ed_items)
        });
        let mut cfg_row = h_flex()
            .w_full()
            // Pinned above the scrollable rows — must not shrink.
            .flex_none()
            .px(design::ui_px(cx, 12.0))
            .pb(design::ui_px(cx, 6.0))
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .text_size(design::t_caption(cx))
            // "restarts" — the filter's coordinate descent.
            .when_some(it_input, |el, input| {
                el.child(
                    div()
                        .text_color(moon(p.text_muted))
                        .child(t!("analytics.tuner.iters").to_string()),
                )
                .child(
                    div().w(design::font_w_px(cx, 46.0)).flex_none().child(
                        MoonInput::new(SharedString::from("tun-cfg-it-f"))
                            .state(&input)
                            .small(),
                    ),
                )
            })
            .child(
                div()
                    .text_color(moon(p.text_muted))
                    .child(t!("analytics.tuner.min_trades").to_string()),
            )
            .child(
                div().w(design::font_w_px(cx, 52.0)).flex_none().child(
                    MoonInput::new(SharedString::from(format!(
                        "tun-cfg-mn-{}",
                        if time { "t" } else { "f" }
                    )))
                    .state(&mn_input)
                    .small(),
                ),
            )
            // "depth" (quantiles) — absent for time: there the sweep fixes its own precision.
            .when_some(ed_combo, |el, combo| {
                el.child(
                    div()
                        .text_color(moon(p.text_muted))
                        .child(t!("analytics.tuner.edges").to_string()),
                )
                .child(div().flex_none().child(combo))
            })
            .child(div().flex_1());
        // The suggestion buttons are ALWAYS visible — a suggestion can be run over the current
        // scope (no selected strategy = over everything shown). "Search" (a single field) exists
        // only for the filter; "Search all" — for both axes. The result can only be written
        // into a selected strategy (that gate sits on Copy/Save in the toolbar).
        cfg_row = cfg_row
            // "Search" (a single field) — filter only; hidden for time.
            .when(!time, |el| {
                el.child(
                    MoonButton::new(SharedString::from("tun-suggest-one-f"))
                        .variant(MoonButtonVariant::Soft)
                        .size(MoonButtonSize::Micro)
                        .label(if busy {
                            "…".to_string()
                        } else {
                            t!("analytics.tuner.suggest_one").to_string()
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if kind == TunerKind::Filter && !this.tuner.sugg_busy {
                                this.suggest_one_into_v1(cx);
                                cx.notify();
                            }
                        }))
                        .render(),
                )
            })
            .child(
                MoonButton::new(SharedString::from(format!(
                    "tun-suggest-run-{}",
                    if time { "t" } else { "f" }
                )))
                .variant(MoonButtonVariant::Blue)
                .size(MoonButtonSize::Micro)
                .label(if busy {
                    "…".to_string()
                } else {
                    t!("analytics.tuner.suggest_run").to_string()
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    match kind {
                        TunerKind::Filter => {
                            if !this.tuner.sugg_busy {
                                this.suggest_into_v1(cx);
                                cx.notify();
                            }
                        }
                        // time_suggest gates on its own sugg_busy itself.
                        TunerKind::Time => {
                            this.time_suggest(cx);
                            cx.notify();
                        }
                    }
                }))
                .render(),
            );
        cfg_row.into_any_element()
    }

    /// Lazily cache the filter tuner's "restarts" box and keep its raw text current on change.
    ///
    /// Handling `Change` is required because a search or window close need not blur the input.
    fn shell_restarts_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        const ID: &str = "f-cfg-0";
        if let Some(state) = self.tuner.inputs.get(ID) {
            return state.clone();
        }
        let value = self.tuner.iters.clone();
        let state = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .default_value(value)
                .placeholder(DEFAULT_ITERS.to_string())
        });
        cx.subscribe(&state, move |this, state, ev: &MoonInputEvent, cx| {
            // Change is committed too: the value takes effect right away on a "Search" click.
            if matches!(
                ev,
                MoonInputEvent::Change | MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }
            ) {
                this.tuner.iters = state.read(cx).value().to_string();
                this.persist_tuner_iters(cx);
                if !matches!(ev, MoonInputEvent::Change) {
                    cx.notify();
                }
            }
        })
        .detach();
        self.tuner.inputs.insert(ID.to_string(), state.clone());
        state
    }

    /// Lazily cache the "trades ≥" box in the selected axis's state.
    ///
    /// It is not persisted because it scopes one search rather than expressing a search
    /// preference. Empty selects the automatic threshold.
    fn shell_min_trades_input(
        &mut self,
        kind: TunerKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        let id = match kind {
            TunerKind::Filter => "f-cfg-1",
            TunerKind::Time => "t-cfg-1",
        };
        let cached = match kind {
            TunerKind::Filter => self.tuner.inputs.get(id),
            TunerKind::Time => self.time_tuner.inputs.get(id),
        };
        if let Some(state) = cached {
            return state.clone();
        }
        let value = match kind {
            TunerKind::Filter => self.tuner.min_trades.clone(),
            TunerKind::Time => self.time_tuner.min_trades.clone(),
        };
        let state = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .default_value(value)
                .placeholder(t!("analytics.tuner.auto_ph").to_string())
        });
        cx.subscribe(&state, move |this, state, ev: &MoonInputEvent, cx| {
            if matches!(
                ev,
                MoonInputEvent::Change | MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }
            ) {
                let value = state.read(cx).value().to_string();
                match kind {
                    TunerKind::Filter => this.tuner.min_trades = value,
                    TunerKind::Time => this.time_tuner.min_trades = value,
                }
                if !matches!(ev, MoonInputEvent::Change) {
                    cx.notify();
                }
            }
        })
        .detach();
        match kind {
            TunerKind::Filter => self.tuner.inputs.insert(id.to_string(), state.clone()),
            TunerKind::Time => self.time_tuner.inputs.insert(id.to_string(), state.clone()),
        };
        state
    }

    /// Persist a changed quantile depth through the layout dirty-flag drain.
    fn persist_tuner_edges(&self, cx: &mut Context<Self>) {
        let value = Some(self.tuner.edges as u32);
        self.backend.update(cx, |b, _| {
            if b.layout.analytics_tuner_edges != value {
                b.layout.analytics_tuner_edges = value;
                b.layout_dirty = true;
            }
        });
    }

    /// Persist the normalized restart count through the layout dirty-flag drain.
    ///
    /// `Change` events must persist because closing the window does not guarantee a blur.
    fn persist_tuner_iters(&self, cx: &mut Context<Self>) {
        let value = Some(iters_of(&self.tuner.iters) as u32);
        self.backend.update(cx, |b, _| {
            if b.layout.analytics_tuner_iters != value {
                b.layout.analytics_tuner_iters = value;
                b.layout_dirty = true;
            }
        });
    }
}
