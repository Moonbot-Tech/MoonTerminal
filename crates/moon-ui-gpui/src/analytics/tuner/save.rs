//! The tuner's write-confirmation dialog: assembling it (the parameters' current values are
//! pulled in the background), rendering the "now → next" overlay, and executing it on confirm.
//! Edits go out grouped BY CORE — several strategies on one core must travel in a single
//! command — and "Make a copy" creates a NEW strategy instead of editing the source.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonInput, MoonNotification, MoonPalette,
    MoonWindowExt as _, h_flex, v_flex,
};
use rust_i18n::t;

use super::super::AnalyticsView;
use super::shared::{SaveAuthority, SaveDialog, SaveTarget};
use crate::design;
use crate::design::{moon, moon_alpha};

#[cfg(test)]
mod tests;

/// Normalize an unordered concrete core set for stable authority comparison.
fn normalized_cores(cores: impl IntoIterator<Item = u64>) -> Vec<u64> {
    let mut cores: Vec<u64> = cores.into_iter().collect();
    cores.sort_unstable();
    cores.dedup();
    cores
}

/// Return the ordered strategy identities whose per-target payload positions must stay aligned.
fn save_target_identities(targets: &[SaveTarget]) -> Vec<(i64, Option<u64>)> {
    targets
        .iter()
        .map(|target| (target.sid, target.core))
        .collect()
}

/// Verify a Save authority without admitting a surviving subset of its original targets.
///
/// Args:
///     authority: Producer token carried by the confirmation.
///     dialog_seq: Current draft/selection sequence.
///     workspace_generation: Current Auto generation, or `None` in Classic.
///     workspace_cores: Current concrete Auto scope, or `None` in Classic.
///     targets: Current complete effective target list.
///
/// Returns:
///     `true` only when generation, scope, and ordered target identity all still match.
fn save_authority_matches(
    authority: &SaveAuthority,
    dialog_seq: u64,
    workspace_generation: Option<u64>,
    workspace_cores: Option<&[u64]>,
    targets: &[SaveTarget],
) -> bool {
    let current_cores = workspace_cores.map(|cores| normalized_cores(cores.iter().copied()));
    authority.dialog_seq == dialog_seq
        && authority.workspace_generation == workspace_generation
        && authority.workspace_cores == current_cores
        && authority.targets == save_target_identities(targets)
}

/// Resolve every target's live core set atomically before grouping commands.
///
/// A concrete target missing from its live strategy set rejects the complete operation. Legacy
/// `None` targets retain Classic's intentional fan-out over their current live cores.
fn resolve_complete_target_cores(
    targets: &[SaveTarget],
    live: &HashMap<i64, Vec<u64>>,
) -> Option<Vec<Vec<u64>>> {
    targets
        .iter()
        .map(|target| {
            let live_cores = live.get(&target.sid).map(Vec::as_slice).unwrap_or(&[]);
            match target.core {
                Some(core) if live_cores.contains(&core) => Some(vec![core]),
                Some(_) => None,
                None => (!live_cores.is_empty()).then(|| live_cores.to_vec()),
            }
        })
        .collect()
}

impl AnalyticsView {
    /// Capture the complete workspace and selection identity for a Save or Copy producer.
    ///
    /// Args:
    ///     targets: Ordered effective strategy targets represented by the producer.
    ///     cx: Application context used to read the shared workspace generation.
    ///
    /// Returns:
    ///     Immutable authority token; Classic deliberately carries no workspace generation/scope.
    fn capture_save_authority(&self, targets: &[SaveTarget], cx: &App) -> SaveAuthority {
        let workspace_cores = self
            .workspace_scope
            .as_ref()
            .map(|scope| normalized_cores(scope.core_ids.iter().copied()));
        let workspace_generation = workspace_cores.as_ref().map(|_| {
            let revision = self.backend.read(cx).workspace_revision();
            revision.read(cx).generation()
        });
        SaveAuthority {
            dialog_seq: self.tuner.dialog_seq,
            workspace_generation,
            workspace_cores,
            targets: save_target_identities(targets),
        }
    }

    /// Return whether a carried Save authority still equals the live workspace and selection.
    ///
    /// Args:
    ///     authority: Producer authority carried through preview and confirmation.
    ///     targets: Exact dialog targets whose payload would be dispatched.
    ///     cx: Application context used to read current revision and effective selection.
    ///
    /// Returns:
    ///     `true` only when no scope, selection, or draft transition occurred.
    fn save_authority_is_current(
        &self,
        authority: &SaveAuthority,
        targets: &[SaveTarget],
        cx: &App,
    ) -> bool {
        let current_targets = self.selected_targets();
        let workspace_cores = self
            .workspace_scope
            .as_ref()
            .map(|scope| scope.core_ids.as_slice());
        let workspace_generation = workspace_cores.map(|_| {
            let revision = self.backend.read(cx).workspace_revision();
            revision.read(cx).generation()
        });
        save_authority_matches(
            authority,
            self.tuner.dialog_seq,
            workspace_generation,
            workspace_cores,
            &current_targets,
        ) && authority.targets == save_target_identities(targets)
    }

    /// Open the confirmation dialog (write/copy): the parameters' current values
    /// ("now → next") are pulled from strategies.sqlite in the background.
    /// `notes` is indexed like `changes` and may be shorter or empty — an axis whose fields
    /// read fine as "now → next" supplies nothing.
    ///
    /// Args:
    ///     targets: Ordered complete strategy identities and labels represented by the action.
    ///     changes: Shared serialized field edits.
    ///     per_target: Optional ordered field edits specific to each target.
    ///     notes: Optional human-readable descriptions aligned with `changes`.
    ///     warns: Confirmation warnings produced while preparing the edit.
    ///     copy: Whether confirmation creates a copy instead of editing targets.
    ///     cx: View context used to capture authority and prepare current values.
    ///
    /// Returns:
    ///     Nothing; empty or stale preparations do not publish a dialog.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn open_change_dialog(
        &mut self,
        targets: Vec<SaveTarget>,
        changes: Vec<(String, String)>,
        per_target: Option<Vec<Vec<(String, String)>>>,
        notes: Vec<Option<String>>,
        warns: Vec<String>,
        copy: bool,
        cx: &mut Context<Self>,
    ) {
        // The "now → next" preview is the anchor's (first target); the rest write blind. Read
        // its current values from the anchor's OWN core (rows are per-core).
        let Some((sid, core)) = targets.first().map(|t| (t.sid, t.core)) else {
            return;
        };
        let authority = self.capture_save_authority(&targets, cx);
        // A row that carries a NOTE never draws the "now" side, so reading it would be a
        // round trip whose result is discarded. The coins axis annotates every row.
        let needs_olds =
            notes.len() < changes.len() || notes.iter().any(std::option::Option::is_none);
        if !needs_olds {
            let olds = vec![None; changes.len()];
            self.tuner.save_dialog = Some(Arc::new(SaveDialog {
                authority,
                targets,
                changes,
                per_target,
                olds,
                notes,
                warns,
                copy,
            }));
            cx.notify();
            return;
        }
        let keys: Vec<String> = changes.iter().map(|(k, _)| k.clone()).collect();
        // The user scope and draft this dialog belongs to. Report generations retire only
        // analytical results, while scope changes and draft edits advance `dialog_seq`. Without
        // this separate guard, a live report refresh dropped valid Save clicks; using no guard
        // instead resurrected dialogs for selections the panel had already discarded.
        let req = authority.clone();
        self.spawn_db(
            true,
            cx,
            move || moon_core::db::tuner::strategy_current_values(sid, core, &keys),
            move |this, olds_map, cx| {
                if !this.save_authority_is_current(&req, &targets, cx) {
                    log::info!("analytics: the confirmation's scope changed, dialog dropped");
                    return;
                }
                let olds = changes
                    .iter()
                    .map(|(k, _)| olds_map.get(k).cloned())
                    .collect();
                this.tuner.save_dialog = Some(Arc::new(SaveDialog {
                    authority: req,
                    targets,
                    changes,
                    per_target,
                    olds,
                    notes,
                    warns,
                    copy,
                }));
                cx.notify();
            },
        );
    }

    /// "Yes" in the confirmation dialog: write into the strategy or create a copy.
    ///
    /// `window` is threaded through for the copy path alone: a created strategy never appears in
    /// this list (it is grouped from TRADES, and a new one has none), so the only way to say
    /// where it went is a notification.
    ///
    /// Args:
    ///     window: Analytics window used by copy notifications.
    ///     cx: View context used to revalidate authority and dispatch the chosen operation.
    ///
    /// Returns:
    ///     Nothing; stale confirmation authority sends no commands.
    pub(super) fn confirm_save_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dlg) = self.tuner.save_dialog.take() else {
            return;
        };
        // A new attempt supersedes the previous complaint — it is about to be answered by
        // this write's own outcome.
        self.write_error = None;
        if !self.save_authority_is_current(&dlg.authority, &dlg.targets, cx) {
            log::info!("analytics: stale Save/Copy confirmation refused before dispatch");
            return;
        }
        if dlg.copy {
            self.create_strategy_copy(&dlg, window, cx);
        } else {
            // Keep staged filter toggles until at least one write is dispatched so a total
            // delivery failure remains recoverable. `send_bulk_changes` clears them after send.
            self.send_bulk_changes(
                dlg.targets.clone(),
                dlg.changes.clone(),
                dlg.per_target.clone(),
                dlg.authority.clone(),
                cx,
            );
        }
        cx.notify();
    }

    /// Create a disabled copy with the requested changes on each target core.
    ///
    /// The dialog name is made unique per core; an empty name reuses the source name as its base.
    /// Each copy is sent to the core root because the receiver re-splits nonempty flat paths; see
    /// [`crate::strategies::tree::ops::path_segments`] for the path ambiguity. That covers THIS
    /// COPY and no more: a create is applied by resending the core's whole strategy set
    /// (`rebuild_sync`), so every existing strategy's flat path travels along either way.
    ///
    /// The raw-list anchor places the copy after its source. Once a command is dispatched, the
    /// Strategies window is asked to reveal the echoed copy and a notification names its core.
    ///
    /// Names are not reserved between store snapshots, so concurrent requests can choose the same
    /// generated name before either copy is echoed.
    fn create_strategy_copy(
        &mut self,
        dlg: &super::shared::SaveDialog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::strategies::tree::ops::{
            NewStrategy, STRATEGY_NAME_FIELD, set_field, unique_name,
        };
        let Some(target) = dlg.targets.first() else {
            return;
        };
        if !self.save_target_in_workspace(target) {
            return;
        }
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
            if !self.workspace_core_visible(cid) {
                continue;
            }
            // In Classic, a missing target core applies the copy to every core containing the
            // strategy; Auto rejects that legacy aggregate target before reaching this loop.
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
                // Use the shared intent conversion so every creation path populates the same spec.
                moon_core::feed::NewStrategySpec::from(NewStrategy {
                    kind_ordinal: row.kind_ordinal,
                    // An empty path sends the copy to the core root; see this function's docstring.
                    folder_path: String::new(),
                    fields,
                    // The core-qualified anchor places the copy beside its source.
                    insert_after: Some((cid, row.id)),
                }),
                name,
            ));
        }
        if plans.is_empty() {
            let (sid, core) = (target.sid, target_core);
            // The closed dialog needs a visible failure when no live source can be copied.
            self.set_write_error(
                t!(
                    "analytics.copy_failed_missing",
                    sid = sid,
                    core = format!("{core:?}")
                )
                .to_string(),
                cx,
            );
            return;
        }
        let total = plans.len();
        let mut failed: Vec<String> = Vec::new();
        // Keep the first successful dispatch for the reveal request and notification.
        let mut landed: Option<(moon_core::session::CoreId, String)> = None;
        for (cid, spec, name) in plans {
            match b.session.create_strategies(cid, vec![spec]) {
                Ok(()) => {
                    log::info!("analytics: copy '{name}' created on core {cid}");
                    landed.get_or_insert((cid, name));
                }
                Err(e) => {
                    log::warn!("analytics: copy to core {cid} was not sent: {e:#}");
                    failed.push(format!("{cid}: {e:#}"));
                }
            }
        }
        log::info!(
            "analytics: 'Make a copy' — cores {}/{total}",
            total - failed.len()
        );
        if failed.is_empty() {
            self.write_error = None;
        } else {
            // Report partial delivery separately when at least one core accepted the command.
            let err = failed.join("; ");
            let msg = if landed.is_some() {
                t!("analytics.copy_partial", err = err.as_str())
            } else {
                t!("analytics.copy_failed", err = err.as_str())
            };
            self.set_write_error(msg.to_string(), cx);
        }
        if let Some((cid, name)) = landed {
            // Dispatch confirms only that the command was queued, so the notification says "sent."
            //
            // The reveal takes effect when the core echoes the strategy into an open Strategies
            // window; this trade-derived view cannot display a copy with no trades.
            let note = t!(
                "analytics.copy_created_root",
                name = name.as_str(),
                core = cid
            )
            .to_string();
            crate::strategies::reveal_name(&self.backend, cid, name, cx);
            window.push_notification(MoonNotification::success(note), cx);
        }
    }

    /// Write the same changes to one OR MANY targets, grouped BY core. `edit_strategies`
    /// applies a whole core's set in a single `sync_local_strategies`, so several strategies on
    /// the SAME core MUST go in one command (separate commands would clobber each other — see
    /// the `edit_strategies` doc). Each `(sid, core)` is validated against the strategy's live
    /// core set (strategies.sqlite) so a stale/foreign core writes nowhere. In Classic only, a
    /// `None`-core legacy target fans out to all of its live cores. One reload afterwards (not per
    /// target).
    ///
    /// Args:
    ///     targets: Complete ordered strategy identities captured by the confirmation.
    ///     changes: Shared serialized edits applied when no per-target value is supplied.
    ///     per_target: Optional ordered edits whose positions correspond to `targets`.
    ///     authority: Workspace generation, dialog sequence, and target identity producer token.
    ///     cx: View context used to run the live-core read and dispatch commands.
    ///
    /// Returns:
    ///     Nothing; authority or target drift rejects the whole plan before its first command.
    fn send_bulk_changes(
        &mut self,
        targets: Vec<SaveTarget>,
        changes: Vec<(String, String)>,
        per_target: Option<Vec<Vec<(String, String)>>>,
        authority: SaveAuthority,
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
            let wrote = cx.update(|cx| {
                this.update(cx, |this, cx| {
                    let changes = Arc::new(changes);
                    if !this.save_authority_is_current(&authority, &targets, cx) {
                        log::info!(
                            "analytics: Save authority changed during live-target read; nothing sent"
                        );
                        return false;
                    }
                    let Some(resolved_cores) = resolve_complete_target_cores(&targets, &live)
                    else {
                        log::warn!(
                            "analytics: at least one Save target left its captured core; nothing sent"
                        );
                        this.set_write_error(
                            t!("analytics.write_failed_stale", n = targets.len()).to_string(),
                            cx,
                        );
                        return false;
                    };
                    // Group validated targets by core: one edit command per core.
                    let mut by_core: HashMap<u64, Vec<(u64, Vec<(String, String)>)>> =
                        HashMap::new();
                    // A Classic aggregate target with no live copies retains the legacy skipped
                    // count; concrete stale targets already refused the whole operation above.
                    let skipped = resolved_cores.iter().filter(|cores| cores.is_empty()).count();
                    for (i, (t, cores)) in targets.iter().zip(resolved_cores).enumerate() {
                        // Each target's OWN value when the axis supplied one — the coin
                        // lists differ per strategy, and one shared value would copy the
                        // first strategy's list onto all of them.
                        //
                        // A missing entry is an INVARIANT BREACH, not a default: falling back
                        // to the shared value would write the anchor's list onto this target,
                        // which is precisely the defect per-target values exist to prevent.
                        // Refuse the whole payload before any command and say so.
                        let mine = match per_target.as_ref() {
                            None => changes.as_ref().clone(),
                            Some(v) => match v.get(i) {
                                Some(m) => m.clone(),
                                None => {
                                    log::error!(
                                        "analytics: no value for target '{}' ({} of {}) — \
                                         whole write refused",
                                        t.name,
                                        i,
                                        v.len()
                                    );
                                    this.set_write_error(
                                        t!("analytics.write_failed_stale", n = targets.len())
                                            .to_string(),
                                        cx,
                                    );
                                    return false;
                                }
                            },
                        };
                        for c in cores {
                            by_core
                                .entry(c)
                                .or_default()
                                .push((t.sid as u64, mine.clone()));
                        }
                    }
                    let mut sent_cores = 0usize;
                    let mut failed: Vec<String> = Vec::new();
                    {
                        let b = this.backend.read(cx);
                        for (core, edits) in &by_core {
                            let n = edits.len();
                            match b.session.edit_strategies(*core, edits.clone()) {
                                Ok(()) => sent_cores += 1,
                                Err(e) => {
                                    log::warn!(
                                        "analytics: changes → core {core} ({n} strategies) \
                                         not sent: {e:#}"
                                    );
                                    failed.push(format!("{core}: {e:#}"));
                                }
                            }
                        }
                    }
                    log::info!(
                        "analytics: changes → {} targets ({n_fields} fields), cores {sent_cores}, \
                         skipped {skipped}",
                        targets.len()
                    );
                    // NOTHING left the terminal — every core refused, or every target was
                    // filtered out as stale. Reloading now would put the strategies' current
                    // values back over the pending edit, which is precisely what a SUCCESSFUL
                    // write looks like: the user would be shown a saved-looking panel with
                    // their change silently discarded. Keep the edit, say what happened.
                    if sent_cores == 0 {
                        let detail = if failed.is_empty() {
                            t!("analytics.write_failed_stale", n = skipped).to_string()
                        } else {
                            failed.join("; ")
                        };
                        this.set_write_error(
                            t!("analytics.write_failed", err = detail).to_string(),
                            cx,
                        );
                        return false;
                    }
                    // Something went out, so the staged toggles it carried are spent.
                    this.tuner.staged_ignore.clear();
                    // Skipped targets and refusing cores are INDEPENDENT: reporting one as
                    // an alternative to the other hid the refusals behind a sentence claiming
                    // everything else went through. Both are said, or neither.
                    if skipped > 0 || !failed.is_empty() {
                        let mut parts = Vec::new();
                        if !failed.is_empty() {
                            parts.push(
                                t!("analytics.write_partial", err = failed.join("; ")).to_string(),
                            );
                        }
                        if skipped > 0 {
                            parts.push(t!("analytics.write_skipped", n = skipped).to_string());
                        }
                        this.set_write_error(parts.join(" · "), cx);
                    } else {
                        // WHAT "sent" ACTUALLY PROVES, so nobody reads more into it than it
                        // carries: `edit_strategies` returns as soon as the command is queued
                        // on the core's channel (`session::commands::send_core_cmd`). It
                        // catches a core that is gone from the session set and a closed
                        // channel — not a connected core that receives the command and never
                        // applies it. For that, the only real evidence is the echo the
                        // reloads below re-read.
                        //
                        // A clean write clears the previous complaint; leaving it up would
                        // outlive the problem it describes.
                        this.write_error = None;
                    }
                    // The snapshot echo will refresh strategies.sqlite — re-read the ACTIVE
                    // axis (filters, time, or the coin lists).
                    this.reload_active_tuner(cx);
                    cx.notify();
                    true
                })
                .unwrap_or(false)
            });
            // Nothing was sent, so there is no echo coming and nothing to re-read. Running
            // the follow-up reloads anyway would erase the edit the failure just preserved.
            if !wrote {
                return;
            }
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

    /// Return whether a concrete core remains inside the effective Analytics workspace.
    ///
    /// Args:
    ///     core: Core that a pending write would target.
    ///
    /// Returns:
    ///     `true` in Classic or while the core belongs to the current Auto scope.
    fn workspace_core_visible(&self, core: u64) -> bool {
        self.workspace_scope
            .as_ref()
            .is_none_or(|scope| scope.core_ids.contains(&core))
    }

    /// Validate one dialog target immediately before a Save or Copy dispatch.
    ///
    /// Args:
    ///     target: Retained dialog target awaiting dispatch.
    ///
    /// Returns:
    ///     `true` only for a concrete visible core, or a legacy aggregate target in Classic.
    fn save_target_in_workspace(&self, target: &SaveTarget) -> bool {
        match target.core {
            Some(core) => self.workspace_core_visible(core),
            None => self.workspace_scope.is_none(),
        }
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
            // An EMPTY value is a real edit — "erase this field" — and it used to render as a
            // blank cell, which reads as "nothing happens here". A coin list wiped by an
            // accidental untick-all went out looking like a no-op.
            let blank = |s: &str| s.trim().is_empty();
            let note = dlg.notes.get(i).cloned().flatten();
            let old = dlg
                .olds
                .get(i)
                .cloned()
                .flatten()
                .filter(|s| !blank(s))
                .unwrap_or_else(|| "—".to_string());
            let mut row = h_flex()
                // MIN height, not fixed: a long value (a blacklist runs to hundreds of
                // coins) wraps over several lines, and a fixed height would clip it.
                .min_h(design::fit_h_px(cx, 22.0, 13.0, 4.5))
                .w_full()
                .px(design::ui_px(cx, 10.0))
                .py(design::ui_px(cx, 3.0))
                .gap(design::ui_px(cx, 6.0))
                .items_start()
                .border_t_1()
                .border_color(moon_alpha(p.border, 0.5))
                .child(
                    div()
                        .flex_none()
                        .max_w(design::font_w_px(cx, 110.0))
                        .truncate()
                        .child(k.clone()),
                );
            if let Some(note) = note {
                // A field the axis can describe in words: print WHAT changed and nothing
                // else. For a SET this is the only readable form — two 376-entry lists that
                // differ by one coin wrap differently on every line, so putting them side by
                // side showed that something changed while hiding what.
                row = row.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(moon(p.amber))
                        .child(note),
                );
            } else {
                if !bulk {
                    row = row
                        .child(
                            // Wraps like the new value beside it. Truncating only one side
                            // let a long value be confirmed against a 90px stub of the one it
                            // replaces — the comparison the dialog exists to make.
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_color(moon(p.text_muted))
                                .child(old),
                        )
                        .child(div().flex_none().text_color(moon(p.text_muted)).child("→"));
                }
                row = row.child(
                    // Wraps instead of truncating: this is the value about to be written, and
                    // a 120px prefix of it confirms nothing. The dialog scrolls.
                    div()
                        .flex_1()
                        .min_w_0()
                        .when(blank(v), |el| el.text_color(moon(p.orange)))
                        .when(!blank(v), |el| el.text_color(moon(p.amber)))
                        .child(if blank(v) {
                            t!("analytics.tuner.save_clears").to_string()
                        } else {
                            v.clone()
                        }),
                );
            }
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
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.confirm_save_dialog(window, cx);
                                        }))
                                        .render(),
                                ),
                        ),
                ),
        )
    }
}
