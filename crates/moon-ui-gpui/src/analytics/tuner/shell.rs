//! The SHARED tuner shell: the toolbar (rounding / Make a copy / Save) and the
//! suggestion row (restarts / trades ≥ / depth / Search / Search all).
//! Rendered by ONE piece of code for every axis ("By filter", "By time", …) —
//! only the grid rows and the actions differ. Actions are dispatched by `TunerKind`
//! to the concrete tuner; for "By time" they arrive in phase 2b (disabled for now).

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonDropdown,
    MoonInput, MoonInputEvent, MoonInputState, MoonMenuSize, MoonPalette, h_flex,
};
use rust_i18n::t;

use super::super::AnalyticsView;
use super::filter::state::{EDGE_OPTIONS, iters_of};
use super::shared::TunerKind;
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
        let k = match kind {
            TunerKind::Filter => "f",
            TunerKind::Time => "t",
            TunerKind::Coins => "c",
        };
        // `None` — the axis has nothing to round. The coin list is text; a rounding control
        // beside it would be a switch that does nothing.
        let round = match kind {
            TunerKind::Filter => Some(self.tuner.round_results),
            TunerKind::Time => Some(self.time_tuner.round_results),
            TunerKind::Coins => None,
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
        if let Some(round) = round {
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
                                            // No rounding on this axis — the control is hidden.
                                            TunerKind::Coins => {}
                                        }
                                        cx.notify();
                                    });
                                }
                            }),
                    ),
                );
        }
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
                                TunerKind::Coins => this.coins_open_copy_dialog(window, cx),
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
                    // The list differs from what the strategies hold — the same condition
                    // the coin table's "changed" badge and its Revert button read.
                    TunerKind::Coins => self.coins.has_changes(),
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
                            TunerKind::Coins => this.coins_open_save_dialog(cx),
                        }
                        cx.notify();
                    }))
                    .render()
            });
        }
        header.into_any_element()
    }

    /// The shared suggestion row: restarts / trades ≥ / depth (quantiles) +
    /// "Search" / "Search all". The settings come from the axis state; the buttons
    /// dispatch into its auto-suggestion (for "By time" still disabled — phase 2b).
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
            // The coin axis does not draw this row yet — its own controls arrive with the
            // selection metrics. Answering here keeps the match exhaustive rather than
            // letting a fourth axis compile into a silent default.
            TunerKind::Coins => false,
        };
        let edges = match kind {
            TunerKind::Filter => self.tuner.edges,
            TunerKind::Time => self.time_tuner.edges,
            TunerKind::Coins => 0,
        };
        let it_input = self.shell_cfg_input(kind, 0, "20", window, cx);
        let mn_input = self.shell_cfg_input(kind, 1, &t!("analytics.tuner.auto_ph"), window, cx);
        // The number of quantiles — a combo box (4/8/…/128).
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
                    match kind {
                        TunerKind::Filter => {
                            this.tuner.edges = n;
                            // Only this axis persists its depth — the layout key is the filter
                            // tuner's, and the time sweep fixes its own.
                            this.persist_tuner_edges(cx);
                        }
                        TunerKind::Time => this.time_tuner.edges = n,
                        // This row is not drawn on the coin axis, so the control that
                        // would fire this cannot exist there.
                        TunerKind::Coins => {}
                    }
                    cx.notify();
                });
            },
        );
        let ed_combo = MoonDropdown::new(SharedString::from(format!(
            "tun-cfg-ed-{}",
            if time { "t" } else { "f" }
        )))
        .label(edges.to_string())
        .trigger_caret(true)
        .trigger_variant(MoonButtonVariant::Soft)
        .trigger_size(MoonButtonSize::Micro)
        .menu_width_scaled(64.0)
        .menu_size(MoonMenuSize::Compact)
        .items(ed_items);
        let mut cfg_row = h_flex()
            .w_full()
            // Pinned above the scrollable rows — must not shrink.
            .flex_none()
            .px(design::ui_px(cx, 12.0))
            .pb(design::ui_px(cx, 6.0))
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .text_size(design::t_caption(cx))
            // "restarts" — the filter's coordinate descent; unused for time (hidden).
            .when(!time, |el| {
                el.child(
                    div()
                        .text_color(moon(p.text_muted))
                        .child(t!("analytics.tuner.iters").to_string()),
                )
                .child(
                    div().w(design::font_w_px(cx, 46.0)).flex_none().child(
                        MoonInput::new(SharedString::from("tun-cfg-it-f"))
                            .state(&it_input)
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
            // "depth" (quantiles) — hidden for time: there the max precision is fixed.
            .when(!time, |el| {
                el.child(
                    div()
                        .text_color(moon(p.text_muted))
                        .child(t!("analytics.tuner.edges").to_string()),
                )
                .child(div().flex_none().child(ed_combo))
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
                        // No suggestion on the coin axis yet - the row is not drawn there.
                        TunerKind::Coins => {}
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

    /// A suggestion settings input (restarts / min. trades) for the `kind` axis, with a
    /// lazy cache in its state. `which`: 0 = restarts, 1 = min. trades.
    fn shell_cfg_input(
        &mut self,
        kind: TunerKind,
        which: usize,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        let id = format!(
            "{}-cfg-{which}",
            if kind == TunerKind::Time { "t" } else { "f" }
        );
        let cached = match kind {
            TunerKind::Filter => self.tuner.inputs.get(&id),
            TunerKind::Time => self.time_tuner.inputs.get(&id),
            TunerKind::Coins => None,
        };
        if let Some(state) = cached {
            return state.clone();
        }
        let value = match (kind, which) {
            (TunerKind::Filter, 1) => self.tuner.min_trades.clone(),
            (TunerKind::Filter, _) => self.tuner.iters.clone(),
            (TunerKind::Time, 1) => self.time_tuner.min_trades.clone(),
            (TunerKind::Time, _) => self.time_tuner.iters.clone(),
            (TunerKind::Coins, _) => String::new(),
        };
        let ph = placeholder.to_string();
        let state = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .default_value(value)
                .placeholder(ph)
        });
        cx.subscribe(&state, move |this, state, ev: &MoonInputEvent, cx| {
            // Change is committed too: the value takes effect right away on a "Search" click.
            if matches!(
                ev,
                MoonInputEvent::Change | MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }
            ) {
                let value = state.read(cx).value().to_string();
                match (kind, which) {
                    (TunerKind::Filter, 1) => this.tuner.min_trades = value,
                    (TunerKind::Filter, _) => {
                        this.tuner.iters = value;
                        this.persist_tuner_iters(cx);
                    }
                    (TunerKind::Time, 1) => this.time_tuner.min_trades = value,
                    (TunerKind::Time, _) => this.time_tuner.iters = value,
                    (TunerKind::Coins, _) => {}
                }
                if !matches!(ev, MoonInputEvent::Change) {
                    cx.notify();
                }
            }
        })
        .detach();
        match kind {
            TunerKind::Filter => self.tuner.inputs.insert(id, state.clone()),
            TunerKind::Time => self.time_tuner.inputs.insert(id, state.clone()),
            TunerKind::Coins => None,
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
