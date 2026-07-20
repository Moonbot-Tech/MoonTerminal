//! The tuner's write-confirmation dialog: assembling it (the parameters' current values are
//! pulled in the background), rendering the "now → next" overlay, and executing it on confirm.
//! Edits go out grouped BY CORE — several strategies on one core must travel in a single
//! command — and "Make a copy" creates a NEW strategy instead of editing the source.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonInput, MoonPalette, h_flex, v_flex,
};
use rust_i18n::t;

use super::super::AnalyticsView;
use super::state::{SaveDialog, SaveTarget};
use crate::design;
use crate::design::{moon, moon_alpha};

impl AnalyticsView {
    /// Open the confirmation dialog (write/copy): the parameters' current values
    /// ("now → next") are pulled from strategies.sqlite in the background.
    pub(super) fn open_change_dialog(
        &mut self,
        targets: Vec<SaveTarget>,
        changes: Vec<(String, String)>,
        warns: Vec<String>,
        copy: bool,
        cx: &mut Context<Self>,
    ) {
        // The "now → next" preview is the anchor's (first target); the rest write blind. Read
        // its current values from the anchor's OWN core (rows are per-core).
        let Some((sid, core)) = targets.first().map(|t| (t.sid, t.core)) else {
            return;
        };
        let keys: Vec<String> = changes.iter().map(|(k, _)| k.clone()).collect();
        self.op_started();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let olds_map = executor
                .spawn(
                    async move { moon_core::db::tuner::strategy_current_values(sid, core, &keys) },
                )
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.op_finished(cx);
                    let olds = changes
                        .iter()
                        .map(|(k, _)| olds_map.get(k).cloned())
                        .collect();
                    this.tuner.save_dialog = Some(Arc::new(SaveDialog {
                        targets,
                        changes,
                        olds,
                        warns,
                        copy,
                    }));
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// "Yes" in the confirmation dialog: write into the strategy or create a copy.
    pub(super) fn confirm_save_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(dlg) = self.tuner.save_dialog.take() else {
            return;
        };
        if dlg.copy {
            self.create_strategy_copy(&dlg, cx);
        } else {
            self.tuner.staged_ignore.clear();
            self.send_bulk_changes(dlg.targets.clone(), dlg.changes.clone(), cx);
        }
        cx.notify();
    }

    /// Create a copy of the strategy with the changes applied on every core where
    /// the original lives (per the live store). The name comes from the dialog input
    /// (empty = auto); it is additionally uniqued on each core. The copy arrives
    /// DISABLED (the create_strategies rule) — which is safe.
    fn create_strategy_copy(&mut self, dlg: &super::state::SaveDialog, cx: &mut Context<Self>) {
        use crate::strategies::tree_ops::{STRATEGY_NAME_FIELD, set_field, unique_name};
        let Some(target) = dlg.targets.first() else {
            return;
        };
        let sid = target.sid as u64;
        let target_core = target.core;
        let desired = {
            let t = self.tuner.copy_name.trim();
            if t.is_empty() {
                target.name.clone()
            } else {
                t.to_string()
            }
        };
        let b = self.backend.read(cx);
        let mut plans: Vec<(
            moon_core::session::CoreId,
            moon_core::feed::NewStrategySpec,
            String,
        )> = Vec::new();
        for (cid, cd) in b.session.store().cores() {
            // Per-core: create the copy only on the selected core (None = legacy all-cores).
            if let Some(tc) = target_core {
                if cid != tc {
                    continue;
                }
            }
            let Some(row) = cd.strategies.iter().find(|r| r.id == sid) else {
                continue;
            };
            let taken: std::collections::HashSet<String> =
                cd.strategies.iter().map(|r| r.name.clone()).collect();
            let name = unique_name(&taken, &desired);
            let mut fields = row.fields.clone();
            for (k, v) in &dlg.changes {
                set_field(&mut fields, k, v);
            }
            set_field(&mut fields, STRATEGY_NAME_FIELD, &name);
            plans.push((
                cid,
                moon_core::feed::NewStrategySpec {
                    kind_ordinal: row.kind_ordinal,
                    folder_path: row.folder_path.clone(),
                    fields,
                },
                name,
            ));
        }
        if plans.is_empty() {
            log::warn!(
                "analytics: 'Make a copy' — strategy {} not found in the live store (core {:?})",
                target.sid,
                target_core
            );
            return;
        }
        let total = plans.len();
        let mut sent = 0usize;
        for (cid, spec, name) in plans {
            match b.session.create_strategies(cid, vec![spec]) {
                Ok(()) => {
                    sent += 1;
                    log::info!("analytics: copy '{name}' created on core {cid}");
                }
                Err(e) => log::warn!("analytics: copy to core {cid} was not sent: {e:#}"),
            }
        }
        log::info!("analytics: 'Make a copy' — cores {sent}/{total}");
    }

    /// Write the same changes to one OR MANY targets, grouped BY core. `edit_strategies`
    /// applies a whole core's set in a single `sync_local_strategies`, so several strategies on
    /// the SAME core MUST go in one command (separate commands would clobber each other — see
    /// the `edit_strategies` doc). Each `(sid, core)` is validated against the strategy's live
    /// core set (strategies.sqlite) so a stale/foreign core writes nowhere; a `None`-core legacy
    /// target fans out to all of its live cores. One reload afterwards (not per target).
    fn send_bulk_changes(
        &mut self,
        targets: Vec<SaveTarget>,
        changes: Vec<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        if targets.is_empty() {
            return;
        }
        let n_fields = changes.len();
        let mut sids: Vec<i64> = targets.iter().map(|t| t.sid).collect();
        sids.sort_unstable();
        sids.dedup();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            // Live core set per strategy (strategies.sqlite) — read off the UI thread.
            let live: HashMap<i64, Vec<u64>> = executor
                .spawn(async move {
                    sids.into_iter()
                        .map(|sid| (sid, moon_core::db::tuner::strategy_cores(sid)))
                        .collect()
                })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    let changes = Arc::new(changes);
                    // Group validated targets by core: one edit command per core.
                    let mut by_core: HashMap<u64, Vec<(u64, Vec<(String, String)>)>> =
                        HashMap::new();
                    let mut skipped = 0usize;
                    for t in &targets {
                        let live_cores = live.get(&t.sid).map(Vec::as_slice).unwrap_or(&[]);
                        let cores: Vec<u64> = match t.core {
                            Some(c) if live_cores.contains(&c) => vec![c],
                            Some(c) => {
                                log::warn!(
                                    "analytics: changes → '{}': core {c} is not in the \
                                     strategy's live set — skipping",
                                    t.name
                                );
                                skipped += 1;
                                continue;
                            }
                            None => live_cores.to_vec(),
                        };
                        for c in cores {
                            by_core
                                .entry(c)
                                .or_default()
                                .push((t.sid as u64, changes.as_ref().clone()));
                        }
                    }
                    let mut sent_cores = 0usize;
                    {
                        let b = this.backend.read(cx);
                        for (core, edits) in &by_core {
                            let n = edits.len();
                            match b.session.edit_strategies(*core, edits.clone()) {
                                Ok(()) => sent_cores += 1,
                                Err(e) => log::warn!(
                                    "analytics: changes → core {core} ({n} strategies) \
                                     not sent: {e:#}"
                                ),
                            }
                        }
                    }
                    log::info!(
                        "analytics: changes → {} targets ({n_fields} fields), cores {sent_cores}, \
                         skipped {skipped}",
                        targets.len()
                    );
                    // The snapshot echo will refresh strategies.sqlite — re-read the chips
                    // of the ACTIVE axis (filter/time).
                    this.reload_active_tuner(cx);
                    cx.notify();
                });
            });
            // The core's echo arrives with a lag — re-read twice more so the
            // chips/ignores show the state that was ACTUALLY applied.
            for delay_ms in [1500u64, 3500] {
                executor
                    .timer(std::time::Duration::from_millis(delay_ms))
                    .await;
                let _ = cx.update(|cx| {
                    let _ = this.update(cx, |this, cx| this.reload_active_tuner(cx));
                });
            }
        })
        .detach();
    }

    /// The "Write to strategy" overlay: the list of edits + Yes/No.
    pub(super) fn save_dialog_overlay(
        &self,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let dlg = self.tuner.save_dialog.clone()?;
        // The header/preview use the anchor (first target); bulk shows a count instead of a
        // name and drops the "now" column — each target has its own current values.
        let bulk = dlg.targets.len() > 1;
        let anchor_name = dlg
            .targets
            .first()
            .map(|t| t.name.clone())
            .unwrap_or_default();
        let title = if dlg.copy {
            t!("analytics.tuner.copy_title", name = anchor_name.as_str()).to_string()
        } else if bulk {
            t!("analytics.tuner.save_title_bulk", n = dlg.targets.len()).to_string()
        } else {
            t!("analytics.tuner.save_title", name = anchor_name.as_str()).to_string()
        };
        let mut list = v_flex().w_full().gap_0();
        // Copy dialog: the editable name of the new strategy as the first row.
        if dlg.copy {
            if let Some(input) = self.tuner.inputs.get("copy-name") {
                list = list.child(
                    h_flex()
                        .w_full()
                        .px(design::ui_px(cx, 10.0))
                        .py(design::ui_px(cx, 5.0))
                        .gap(design::ui_px(cx, 8.0))
                        .items_center()
                        .child(
                            div()
                                .flex_none()
                                .text_size(design::t_caption(cx))
                                .text_color(moon(p.text_muted))
                                .child(t!("analytics.tuner.copy_name_lbl").to_string()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(MoonInput::new("tun-copy-name").state(input).small()),
                        ),
                );
            }
        }
        // Bulk: name the strategies that will be written — some may be filtered out of the list,
        // and the count-only title would otherwise let a hidden target be written unseen.
        if bulk && !dlg.copy {
            let names: Vec<String> = dlg
                .targets
                .iter()
                .map(|t| super::super::summary::strat_display(&t.name))
                .collect();
            list = list.child(
                div()
                    .w_full()
                    .px(design::ui_px(cx, 10.0))
                    .py(design::ui_px(cx, 5.0))
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .child(names.join(", ")),
            );
        }
        for (i, (k, v)) in dlg.changes.iter().enumerate() {
            // Single: "now → next" (current muted, new amber). Bulk: the "now" differs per
            // target, so show only the value written to all of them (no misleading anchor "now").
            let old = dlg.olds.get(i).cloned().flatten();
            let mut row = h_flex()
                .w_full()
                .h(design::fit_h_px(cx, 22.0, 13.0, 4.5))
                .px(design::ui_px(cx, 10.0))
                .gap(design::ui_px(cx, 6.0))
                .items_center()
                .border_t_1()
                .border_color(moon_alpha(p.border, 0.5))
                .child(div().flex_1().min_w_0().truncate().child(k.clone()));
            if !bulk {
                row = row
                    .child(
                        div()
                            .flex_none()
                            .max_w(design::font_w_px(cx, 90.0))
                            .truncate()
                            .text_color(moon(p.text_muted))
                            .child(old.unwrap_or_else(|| "—".to_string())),
                    )
                    .child(div().flex_none().text_color(moon(p.text_muted)).child("→"));
            }
            row = row.child(
                div()
                    .flex_none()
                    .max_w(design::font_w_px(cx, 120.0))
                    .truncate()
                    .text_color(moon(p.amber))
                    .child(v.clone()),
            );
            list = list.child(row);
        }
        // Warnings (overwriting a foreign slot type and the like) — in orange,
        // with line wrapping (not truncated).
        for w in &dlg.warns {
            list = list.child(
                div()
                    .w_full()
                    .px(design::ui_px(cx, 10.0))
                    .py(design::ui_px(cx, 5.0))
                    .border_t_1()
                    .border_color(moon_alpha(p.border, 0.5))
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.orange))
                    .child(w.clone()),
            );
        }
        Some(
            div()
                .id("an-save-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(moon_alpha(p.shell, 0.6))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.tuner.save_dialog = None;
                    cx.notify();
                }))
                .child(
                    v_flex()
                        .id("an-save-dialog")
                        .w(design::font_w_px(cx, 360.0))
                        .max_h(design::ui_px(cx, 480.0))
                        .rounded(design::ui_px(cx, 8.0))
                        .bg(moon(p.panel_high))
                        .border_1()
                        .border_color(moon(p.border))
                        .overflow_hidden()
                        .occlude()
                        .child(
                            h_flex()
                                .px(design::ui_px(cx, 12.0))
                                .py(design::ui_px(cx, 8.0))
                                .items_center()
                                .gap(design::ui_px(cx, 8.0))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(design::t_title(cx))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(title),
                                )
                                // "now → next" caption only makes sense for a single target.
                                .children((!bulk).then(|| {
                                    div()
                                        .flex_none()
                                        .text_size(design::t_caption(cx))
                                        .text_color(moon(p.text_muted))
                                        .child(t!("analytics.tuner.save_now_next").to_string())
                                })),
                        )
                        .child(
                            div()
                                .id("an-save-list")
                                .w_full()
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .child(list),
                        )
                        .child(
                            h_flex()
                                .w_full()
                                .px(design::ui_px(cx, 12.0))
                                .py(design::ui_px(cx, 8.0))
                                .gap(design::ui_px(cx, 8.0))
                                .justify_end()
                                .border_t_1()
                                .border_color(moon_alpha(p.border, 0.6))
                                .child(
                                    MoonButton::new("an-save-no")
                                        .variant(MoonButtonVariant::Ghost)
                                        .size(MoonButtonSize::Micro)
                                        .label(t!("analytics.tuner.save_no").to_string())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.tuner.save_dialog = None;
                                            cx.notify();
                                        }))
                                        .render(),
                                )
                                .child(
                                    MoonButton::new("an-save-yes")
                                        .variant(MoonButtonVariant::Blue)
                                        .size(MoonButtonSize::Micro)
                                        .label(t!("analytics.tuner.save_yes").to_string())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.confirm_save_dialog(cx);
                                        }))
                                        .render(),
                                ),
                        ),
                ),
        )
    }
}
