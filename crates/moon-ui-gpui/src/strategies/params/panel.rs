//! Strategies parameter panel implementation.

use super::*;

impl StrategiesView {
    /// Render parameters for the workspace-visible selection captured in `model`.
    ///
    /// Editor callbacks receive only the model's effective row keys, so retained selection and
    /// drafts on hidden Classic cores cannot be staged or dispatched from the Auto panel.
    ///
    /// Args:
    ///     model: Prepared parameter content or the reason no effective content is available.
    ///     window: Strategies window owning retained input widgets.
    ///     cx: View context used to construct controls and their callbacks.
    ///
    /// Returns:
    ///     The parameter panel for the effective selection.
    pub(in crate::strategies) fn params_panel(
        &mut self,
        model: ParamsPanelModel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Retire a "staged N fields" note once the SELECTED strategy has no remaining staged
        // drafts. Keyed to that strategy specifically, not `field_edit_count`'s workspace-wide
        // total: Apply and Revert both empty `field_edits` for it. Only `Staged` is retired this
        // way — `ClearedOnly` has already discarded stale drafts and `Identical` never had any,
        // so testing either here would vanish the note the very frame it is set.
        if let Some((key, outcome, _)) = self.versions.staged_note
            && matches!(outcome, StagedOutcome::Staged(_))
        {
            let remaining = self
                .field_edits
                .keys()
                .filter(|(core, id, _)| (*core, *id) == key)
                .count();
            if remaining == 0 {
                self.versions.staged_note = None;
            }
        }
        let p = MoonPalette::active(cx);
        let mut col = v_flex()
            .flex_1()
            .h_full()
            .min_w(px(420.0))
            .px(design::ui_px(cx, 24.0))
            .py(design::ui_px(cx, 18.0))
            .gap(design::ui_px(cx, 10.0))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0));

        let ParamsPanelModel::Content {
            body,
            values,
            row_pairs,
            multi,
            common,
            differ,
            pending,
            edit_notes,
        } = model
        else {
            let text = match model {
                ParamsPanelModel::NoSelection => t!("strat.no_selection").to_string(),
                ParamsPanelModel::NoSchema => t!("strat.no_schema").to_string(),
                ParamsPanelModel::Content { .. } => unreachable!(),
            };
            return col
                .child(
                    div()
                        .mt_2()
                        .font_family(design::ui_font())
                        .text_color(moon(p.text_muted))
                        .child(text),
                )
                .into_any_element();
        };
        let keys: Vec<Key> = row_pairs.iter().map(|(key, _)| *key).collect();

        // Title and field total come from the body; the multi selection-count branch keeps
        // priority exactly as before the body could also be a full-mode list.
        let (title, field_total) = match &body {
            ParamsBody::Section(s) => (
                section_display_title(&s.title, self.prefs.human_labels),
                s.fields.len(),
            ),
            ParamsBody::Full(f) => (t!("strat.params_full_title").to_string(), f.field_count),
        };
        let count = if multi {
            t!("strat.selected_count", n = row_pairs.len()).to_string()
        } else {
            t!("strat.fields_count", n = field_total).to_string()
        };
        // The amber edge still marks a pane holding drafts; the buttons that act on them sit on
        // the tab strip above both tabs (`field_edit_actions`).
        let dirty = field_edit_count(self);
        // Two-item switch between per-section and full mode, built per the pinned MoonUI source:
        // `on_click` takes a plain indexed `Fn`, not a `cx.listener`.
        let mode_view = cx.entity();
        let mode_switch = MoonSegmentedControl::new("strat-params-mode")
            .items([
                MoonSegmentItem::new("", t!("strat.params_mode_sections").to_string())
                    .fit_width(cx, 64.0, 120.0)
                    .tooltip(t!("strat.params_mode_sections_tip").to_string())
                    .selected(!self.prefs.params_full),
                MoonSegmentItem::new("", t!("strat.params_mode_full").to_string())
                    .fit_width(cx, 64.0, 120.0)
                    .tooltip(t!("strat.params_mode_full_tip").to_string())
                    .selected(self.prefs.params_full),
            ])
            .on_click(move |ix, _, _window, app| {
                mode_view.update(app, |this, cx| this.set_params_full(ix == 1, cx));
            })
            .render();
        let mut header = h_flex()
            .w_full()
            .min_h(design::fit_h_px(cx, 28.0, 14.0, 7.0))
            .flex_wrap()
            .gap_2()
            .items_center()
            .justify_between()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(moon(p.text))
                    .child(title),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .min_w_0()
                    .max_w_full()
                    .items_center()
                    .gap_2()
                    .child(mode_switch)
                    .child(
                        div()
                            .text_size(design::t_body(cx))
                            .text_color(moon(p.text_muted))
                            .child(count),
                    ),
            );
        if dirty > 0 {
            header = header
                .border_l_2()
                .border_color(moon_alpha(p.amber, 0.72))
                .pl_2();
        }
        // The persisted-snapshot banner marks parameters as read-only. When there is no diff
        // (for example, a created/baseline snapshot), explain why all fields are displayed. It is
        // never purely prohibitive: it always carries the restore affordance too (invariant 13).
        if let Some(vf) = self.versions.sel {
            let date = self.version_date(vf);
            let text = if self.version_changed_filter().is_some() {
                t!("strat.version_view", date = date).to_string()
            } else {
                t!("strat.version_view_nodiff", date = date).to_string()
            };
            col = col.child(
                h_flex()
                    .w_full()
                    .gap(design::ui_px(cx, 6.0))
                    .items_start()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(MoonAlert::warning("strat-version-view", text)),
                    )
                    .child(self.version_restore_button(vf, false, cx))
                    .into_any_element(),
            );
        }
        // Confirmation of the last "restore into current", shown only when the params pane is
        // actually displaying the one strategy the note belongs to: a note keyed to a different
        // strategy must never bleed across a selection change (plan amendment A3), and it must
        // not bleed onto a multi-selection view either — checking the PRIMARY `selected_key` alone
        // let a Ctrl-click deselect leave `self.selected` on the note's strategy while the panes
        // below render the merged fields of a different multi-selection. Judge against the same
        // effective-selection source `params_model` renders from (`multi_row_pairs` ->
        // `selected_keys`), requiring exactly that one strategy be selected.
        if let Some((key, outcome, vf)) = self.versions.staged_note {
            let effective = selected_keys(self);
            if effective.len() == 1 && effective[0] == key {
                let date = self.version_date(vf);
                // One wording per outcome. `ClearedOnly` may not borrow either neighbour:
                // `version_staged` would claim fields were staged when Apply has nothing to send,
                // and `version_staged_none` would claim nothing happened when unsaved edits were
                // in fact discarded. Both would be false.
                let message = match outcome {
                    StagedOutcome::Staged(n) => {
                        t!("strat.version_staged", n = n, date = date).to_string()
                    }
                    StagedOutcome::ClearedOnly(n) => {
                        t!("strat.version_staged_cleared", n = n, date = date).to_string()
                    }
                    StagedOutcome::Identical => {
                        t!("strat.version_staged_none", date = date).to_string()
                    }
                };
                col = col.child(
                    h_flex()
                        .w_full()
                        .gap(design::ui_px(cx, 6.0))
                        .items_start()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(MoonAlert::info("strat-version-staged", message)),
                        )
                        .child(
                            MoonButton::new("strat-version-staged-dismiss")
                                .ghost()
                                .label(t!("strat.edit_banner_dismiss").to_string())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.versions.staged_note = None;
                                    cx.notify();
                                }))
                                .render(),
                        )
                        .into_any_element(),
                );
            }
        }
        col = col
            .child(header)
            .child(div().w_full().h(px(1.0)).bg(moon(p.border)));

        if let Some(banner) = self.edit_state_banner(&row_pairs, &pending, &edit_notes, cx) {
            col = col.child(banner);
        }

        let content: AnyElement = match body {
            ParamsBody::Section(section) => {
                // Preserve schema field order and look up snapshot values by name.
                let mut list = v_flex().w_full().gap(design::ui_px(cx, 2.0));
                for f in &section.fields {
                    let lname = f.name.to_lowercase();
                    if multi && lname == "strategyname" {
                        continue;
                    }
                    if let Some(c) = &common
                        && !c.contains(&lname)
                    {
                        continue;
                    }
                    if differ && lname == "signaltype" {
                        continue;
                    }
                    let active = self.rules.field_active(&f.name, &values);
                    let merged = merged_value_for_owned(self, &row_pairs, f, &pending);
                    let pending_phase = field_pending_phase(&row_pairs, &pending, f);
                    list = list.child(self.field_row(
                        f,
                        &keys,
                        merged,
                        active,
                        pending_phase,
                        None,
                        window,
                        cx,
                    ));
                }
                div()
                    .id("strat-params-scroll")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .child(list)
                    .into_any_element()
            }
            ParamsBody::Full(flat) => {
                self.full_params_list(flat, &keys, values, row_pairs, pending, window, cx)
            }
        };
        let mut pane_body = h_flex()
            .flex_1()
            .w_full()
            .min_h_0()
            .items_start()
            .gap_2()
            .child(content);
        // Per-section mode only. A full-mode formula row is a STATIC preview that creates no
        // `MoonTextAreaState`, so `append_formula_snippet` would have no editor to write into --
        // either doing visibly nothing, or, worse, silently staging into a retained state left
        // over from an earlier per-section visit that this pane is not displaying. Editing a
        // formula in full mode goes through the row's own edit-in-sections button, which switches back.
        if !self.prefs.params_full
            && let Some(helper) = self.formula_helper(cx)
        {
            pane_body = pane_body.child(helper);
        }
        col = col.child(pane_body);
        col.into_any_element()
    }

    /// EXACTLY ONE `MoonAlert` for the current selection, chosen by strict priority —
    /// `Superseded > Adjusted > TimedOut`, `Pending` gets no banner at all (its badge alone is
    /// enough, see `field_row`) — never a stack and never one per row.
    ///
    /// `edit_notes` already carries only what is unacknowledged for EACH note's own core cursor
    /// (`params_model` reads `strategy_edit_notes_since` per core); dismissing here advances that
    /// same core's `last_edit_note_seq`, never a shared scalar, so acknowledging one core's
    /// notice can never suppress another core's still-unseen one.
    fn edit_state_banner(
        &mut self,
        row_pairs: &[(Key, StrategyRow)],
        pending: &HashMap<Key, StrategyEditRow>,
        edit_notes: &[(CoreId, StrategyEditNote)],
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let mut note_banner: Option<(StrategyEditResult, CoreId, u64, Vec<StrategyFieldChange>)> =
            None;
        for (core, id) in row_pairs.iter().map(|(key, _)| *key) {
            let Some(note) = edit_notes
                .iter()
                .filter(|(note_core, n)| *note_core == core && n.id == id)
                .map(|(_, n)| n)
                .max_by_key(|n| n.seq)
            else {
                continue;
            };
            if note.result == StrategyEditResult::Confirmed {
                continue;
            }
            let outranks = match note_banner {
                None => true,
                Some((StrategyEditResult::Superseded, ..)) => false,
                Some(_) => note.result == StrategyEditResult::Superseded,
            };
            if outranks {
                note_banner = Some((note.result, core, note.seq, note.changes.clone()));
            }
        }

        if let Some((result, core, seq, changes)) = note_banner {
            let message = match result {
                StrategyEditResult::Adjusted => t!(
                    "strat.edit_adjusted",
                    diffs = super::adjusted_diff_suffix(&changes)
                )
                .to_string(),
                StrategyEditResult::Superseded => t!("strat.edit_superseded").to_string(),
                StrategyEditResult::Confirmed => unreachable!("filtered above"),
            };
            return Some(
                h_flex()
                    .w_full()
                    .gap(design::ui_px(cx, 6.0))
                    .items_start()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(MoonAlert::error("strat-edit-note", message)),
                    )
                    .child(
                        MoonButton::new("strat-edit-note-dismiss")
                            .ghost()
                            .label(t!("strat.edit_banner_dismiss").to_string())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let entry = this.last_edit_note_seq.entry(core).or_insert(0);
                                *entry = (*entry).max(seq);
                                cx.notify();
                            }))
                            .render(),
                    )
                    .into_any_element(),
            );
        }

        // A timeout is explicitly NOT a rejection in the upstream contract — the core may have
        // applied the edit and lost the echo, and a late confirmation still resolves it. Blue
        // (MoonAlert::info) reads as informational rather than a failure, matches the badge's
        // Notice/yellow escalation from Pending's Info/blue, and is the only banner Pending ever
        // produces, so it can never collide with anything else on screen.
        let timed_out = row_pairs.iter().any(|(key, _)| {
            pending
                .get(key)
                .is_some_and(|edit| edit.phase == StrategyEditPhase::TimedOut)
        });
        if timed_out {
            return Some(
                div()
                    .w_full()
                    .child(MoonAlert::info(
                        "strat-edit-timeout",
                        t!("strat.edit_timeout").to_string(),
                    ))
                    .into_any_element(),
            );
        }
        None
    }
}
