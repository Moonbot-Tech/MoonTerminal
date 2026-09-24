//! The warning about the exit fields the model does not have (`moon_core::db::tuner::ticks::
//! unmodelled`): before a search, a dialog listing them per strategy with Continue / Cancel; in the
//! write dialog, the same list as its warning lines. Which fields and when a field counts as on is
//! the core crate's; this module only reads them per strategy at load and draws them.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::*;
use moon_core::db::tuner::ticks::unmodelled::{UnmodelledField, unmodelled_fields};
use moon_core::feed::strategy_deps::FieldDeps;
use moon_ui::{MoonButton, MoonButtonVariant, MoonPalette, MoonWindowExt as _, h_flex, v_flex};
use rust_i18n::t;

use super::super::super::AnalyticsView;
use crate::design;
use crate::design::{moon, moon_alpha};

#[cfg(test)]
mod tests;

/// The fields each strategy switches on outside the model, by `(strategy_id, core_uid)` — every
/// strategy the load read, one with none under an empty list: a strategy that is absent was not
/// read, which a write to a strategy selected after the load started meets, and says so.
pub(in crate::analytics::tuner) type UnmodelledMap =
    HashMap<(i64, Option<u64>), Vec<UnmodelledField>>;

/// One strategy's line of the warning, ready to draw: its name and, per field, the field, the
/// value and the strategy window's section, and whether the model leaves its trades unjudged.
type WarnRow = (String, Vec<(String, String, String, bool)>);

/// Read the fields off each strategy's values, once per load, off the UI thread.
///
/// Args:
///     strategies: Every strategy the axis holds values of, by `(strategy_id, core_uid)`.
///     defaults: The live schema's numeric defaults (`strategy_field_defaults`).
///     deps: The fields' dependency rules, read for this load.
pub(super) fn unmodelled_map<'a>(
    strategies: impl IntoIterator<Item = ((i64, Option<u64>), &'a HashMap<String, String>)>,
    defaults: &HashMap<String, f64>,
    deps: &FieldDeps,
) -> UnmodelledMap {
    strategies
        .into_iter()
        .map(|(key, values)| (key, unmodelled_fields(values, defaults, deps)))
        .collect()
}

impl AnalyticsView {
    /// The warning's rows for `strategies`, in the order given, a strategy once; the ones without
    /// a field outside the model left out. The names of the strategies the load has not read —
    /// selected after it started, or before any finished — come back beside them.
    fn ticks_unmodelled_rows(
        &self,
        strategies: impl IntoIterator<Item = (i64, Option<u64>)>,
        cx: &App,
    ) -> (Vec<WarnRow>, Vec<String>) {
        let empty = UnmodelledMap::new();
        let map = self
            .ticks
            .data
            .data()
            .map_or(&empty, |data| data.unmodelled.as_ref());
        let backend = self.backend.read(cx);
        let human = crate::strategies::settings::human_labels(&backend.layout);
        let store = backend.session.store();
        let selected: HashMap<(i64, Option<u64>), String> = self
            .selected_targets()
            .into_iter()
            .map(|t| ((t.sid, t.core), t.name))
            .collect();
        let mut seen = Vec::new();
        let mut out = Vec::new();
        let mut unread = Vec::new();
        for key in strategies {
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            let fields = map.get(&key);
            if fields.is_some_and(Vec::is_empty) {
                continue;
            }
            let (sid, core) = key;
            // The store's name first — the scope's strategies need not be selected — then the
            // selection's, then the id.
            let name = core
                .and_then(|core| store.core(core))
                .and_then(|c| c.strategies.iter().find(|r| r.id == sid as u64))
                .map(|r| r.name.clone())
                .or_else(|| selected.get(&key).cloned())
                .unwrap_or_else(|| format!("#{sid}"));
            let name = super::super::super::summary::strat_display(&name);
            let Some(fields) = fields else {
                unread.push(name);
                continue;
            };
            let fields = fields
                .iter()
                .map(|f| {
                    (
                        f.key.to_string(),
                        f.value.clone(),
                        crate::strategies::sections::section_display_title(
                            f.section.schema_title(),
                            human,
                        ),
                        f.rule.is_some(),
                    )
                })
                .collect();
            out.push((name, fields));
        }
        (out, unread)
    }

    /// The strategies a search runs on — every strategy of the scope's deals — and the ones
    /// selected, which a found point is written to.
    fn ticks_search_strategies(&self) -> Vec<(i64, Option<u64>)> {
        let mut keys: Vec<(i64, Option<u64>)> = self
            .selected_targets()
            .into_iter()
            .map(|t| (t.sid, t.core))
            .collect();
        if let Some(data) = self.ticks.data.data() {
            let mut own: Vec<(i64, Option<u64>)> = data
                .own
                .keys()
                .map(|&(sid, core)| (sid, Some(core)))
                .collect();
            own.sort_unstable();
            keys.extend(own);
        }
        keys
    }

    /// Open the warning before a search when a strategy of the scope switches on an exit field
    /// the model does not have; answers whether it opened — the search then waits for Continue.
    pub(super) fn ticks_warn_before_search(
        &mut self,
        only: Option<&'static str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // A search runs on the loaded scope's deals, so a strategy the load has not read is not
        // one it searches: only the rows speak here.
        let (rows, _) = self.ticks_unmodelled_rows(self.ticks_search_strategies(), cx);
        if rows.is_empty() {
            return false;
        }
        let rows = Arc::new(rows);
        let view = cx.entity();
        window.open_unique_moon_dialog(
            "an-ticks-unmodelled-dialog",
            cx,
            move |dialog, _window, cx| {
                let p = MoonPalette::active(cx);
                let rows = rows.clone();
                let go = view.clone();
                dialog
                    .w(design::font_w_px(cx, 520.0))
                    .close_button(false)
                    .overlay(true)
                    .overlay_closable(true)
                    .bg(moon(p.shell_high))
                    .border_color(moon(p.border))
                    .rounded(design::r_container(cx))
                    .text_color(moon(p.text))
                    .header(
                        div()
                            .w_full()
                            .py_2()
                            .border_b_1()
                            .border_color(moon(p.border))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("analytics.ticks.unmodelled_title").to_string()),
                    )
                    .content(move |content, _window, cx| content.child(warn_body(&rows, cx)))
                    .footer(
                        h_flex()
                            .w_full()
                            .justify_end()
                            .gap(design::ui_px(cx, 8.0))
                            .font_family(design::ui_font())
                            .child(
                                MoonButton::new("an-ticks-unmodelled-cancel")
                                    .variant(MoonButtonVariant::Ghost)
                                    .label(t!("dialogs.cancel").to_string())
                                    .on_click(|_, window, cx| window.close_dialog(cx))
                                    .render(),
                            )
                            .child(
                                MoonButton::new("an-ticks-unmodelled-go")
                                    .variant(MoonButtonVariant::Blue)
                                    .label(t!("analytics.ticks.unmodelled_go").to_string())
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                        go.update(cx, |this, cx| this.ticks_start_search(only, cx));
                                    })
                                    .render(),
                            ),
                    )
            },
        );
        true
    }

    /// The write dialog's warning lines for the strategies a write goes to: a heading and one line
    /// per strategy that switches on an exit field the model does not have, when there is one;
    /// then one line naming the targets the axis has not read.
    pub(super) fn ticks_unmodelled_warns(
        &self,
        targets: &[super::super::shared::SaveTarget],
        cx: &App,
    ) -> Vec<String> {
        let (rows, unread) =
            self.ticks_unmodelled_rows(targets.iter().map(|t| (t.sid, t.core)), cx);
        let mut warns = Vec::new();
        if !rows.is_empty() {
            warns.push(t!("analytics.ticks.unmodelled_save").to_string());
        }
        warns.extend(rows.into_iter().map(|(name, fields)| {
            let fields: Vec<String> = fields
                .into_iter()
                .map(|(key, value, section, _)| format!("{key} = {value} ({section})"))
                .collect();
            format!("{name}: {}", fields.join(", "))
        }));
        if !unread.is_empty() {
            warns.push(
                t!(
                    "analytics.ticks.unmodelled_unread",
                    names = unread.join(", ")
                )
                .to_string(),
            );
        }
        warns
    }
}

/// The dialog's body: why it asks, then each strategy with its fields as rows — field, value,
/// section — and a note on the fields whose trades the model does not judge.
fn warn_body(rows: &[WarnRow], cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    let mut list = v_flex()
        .id("an-ticks-unmodelled-list")
        .w_full()
        .max_h(design::ui_px(cx, 360.0))
        .overflow_y_scroll()
        .font_family(design::ui_font())
        .text_size(design::t_caption(cx));
    for (i, (name, fields)) in rows.iter().enumerate() {
        list = list.child(
            div()
                .w_full()
                .pt(design::ui_px(cx, if i == 0 { 2.0 } else { 8.0 }))
                .pb(design::ui_px(cx, 2.0))
                .text_size(design::t_body(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .child(name.clone()),
        );
        for (key, value, section, rule) in fields {
            list = list.child(
                h_flex()
                    .w_full()
                    .py(design::ui_px(cx, 2.0))
                    .gap(design::ui_px(cx, 8.0))
                    .border_t_1()
                    .border_color(moon_alpha(p.border, 0.5))
                    .child(
                        div()
                            .w(design::font_w_px(cx, 150.0))
                            .flex_none()
                            .truncate()
                            .child(key.clone()),
                    )
                    .child(
                        div()
                            .w(design::font_w_px(cx, 110.0))
                            .flex_none()
                            .truncate()
                            .text_color(moon(p.amber))
                            .child(value.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(moon(p.text_muted))
                            .child(if *rule {
                                format!("{section} · {}", t!("analytics.ticks.unmodelled_rule"))
                            } else {
                                section.clone()
                            }),
                    ),
            );
        }
    }
    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 8.0))
        .child(
            div()
                .w_full()
                .font_family(design::ui_font())
                .text_size(design::t_body(cx))
                .text_color(moon(p.orange))
                .child(t!("analytics.ticks.unmodelled_intro").to_string()),
        )
        .child(list)
        .into_any_element()
}
