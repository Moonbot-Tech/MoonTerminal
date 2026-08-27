//! Right pane of the Strategies window: the parameter-pane model and renderer, including
//! selected-strategy badges/value editors (read-only YES/NO, input/memo, formula helper), the
//! per-section/full-mode body dispatch, and the full-value popover. The methods extend
//! `StrategiesView` from [`super`].

use std::rc::Rc;

use super::param_entries::{self, FlatParams};
use super::versions::StagedOutcome;
use super::*;
use rust_i18n::t;

/// The parameter pane's body content: one schema section or every surviving section in full mode.
///
/// `Rc` lets `full_params::full_params_list` move the flattened model into its retained row
/// factory without cloning its entries for each row.
pub(super) enum ParamsBody {
    Section(SchemaSection),
    Full(Rc<FlatParams>),
}

/// Return `v` up to its first newline, appending `…` when content follows it.
///
/// Used only for a compact full-mode row's memo preview and its version notes: a fixed row pitch
/// clips an embedded newline instead of wrapping it, and `.truncate()` alone only elides overflow
/// within one line.
fn compact_first_line(v: &str) -> String {
    match v.split_once('\n') {
        Some((first, _)) => format!("{first}…"),
        None => v.to_string(),
    }
}

pub(super) enum ParamsPanelModel {
    NoSelection,
    NoSchema,
    Content {
        /// Prepared per-section or full-mode body for the current selection.
        body: ParamsBody,
        values: Values,
        row_pairs: Vec<(Key, StrategyRow)>,
        multi: bool,
        common: Option<HashSet<String>>,
        differ: bool,
        /// Still-open strategy edit per selected key, cloned while `store` is in scope so the
        /// renderer (which needs `&mut self`/`cx.listener` and cannot hold a live store borrow)
        /// can resolve the pending value tier and the row marker without it.
        pending: HashMap<Key, StrategyEditRow>,
        /// Resolved-edit notes not yet acknowledged by each note's OWN core cursor
        /// (`StrategiesView::last_edit_note_seq`), for the `edit_state_banner` Adjusted/Superseded
        /// tiers. Also cloned here for the same store-borrow reason as `pending`. Paired with the
        /// core that produced each note: `StrategyEditNote` carries no core id of its own and
        /// strategy ids are core-local and repeat across cores, so flattening notes from more
        /// than one selected core without keeping this association would let a note from one
        /// core match a row on another.
        edit_notes: Vec<(CoreId, StrategyEditNote)>,
    },
}

impl StrategiesView {
    /// A version's `valid_from` as the pane states it: bare `HH:MM` when the version is from
    /// today, `DD.MM HH:MM` otherwise.
    ///
    /// One helper for both banners so they can never end up rendering the same instant against
    /// different `now_ms` snapshots — which is the only way two dates for one version could ever
    /// disagree on this screen.
    fn version_date(&self, vf: i64) -> String {
        moon_core::util::display_time::format_chart_clock(
            vf,
            self.display_zone,
            false,
            moon_core::util::now_unix_ms_i64(),
        )
    }

    /// Build the selected parameter-pane model from dependency values shared across both panes.
    ///
    /// Accepting `values` keeps field and schema normalization to one pass per frame. The model
    /// selects either the active section or the filtered full-mode flatten, according to the
    /// persisted display preference and any version-diff filter.
    ///
    /// Args:
    ///     store: Core data that supplies the selected strategies and schema.
    ///     values: Dependency values calculated once for the sections and parameters panes.
    ///
    /// Returns:
    ///     Prepared content, or the reason that no parameters can be rendered.
    pub(super) fn params_model(&self, store: &CoreStore, values: Values) -> ParamsPanelModel {
        if selected_row(self, store).is_none() {
            return ParamsPanelModel::NoSelection;
        }
        let Some(sections) = selected_sections(self, store) else {
            return ParamsPanelModel::NoSchema;
        };
        // `multi` / `common` / `differ` are computed before the body so a full-mode flatten can
        // consume them; the per-section path below applies the same three filters at render time
        // in `params_panel`, unchanged from before this move.
        let row_pairs: Vec<(Key, StrategyRow)> = multi_row_pairs(self, store)
            .into_iter()
            .map(|(key, row)| (key, row.clone()))
            .collect();
        let multi = row_pairs.len() > 1;
        let common = common_fields(self, store);
        let differ = kinds_differ(self, store);

        let body = if self.prefs.params_full {
            let orphans = t!("strat.params_other_fields").to_string();
            let flat = param_entries::flatten_params(
                sections,
                self.version_changed_filter(),
                multi,
                common.as_ref(),
                differ,
                param_entries::ParamLabels { orphans: &orphans },
            );
            ParamsBody::Full(Rc::new(flat))
        } else if let Some(ch) = self.version_changed_filter() {
            // When viewing a persisted snapshot with a diff, show ONLY changed fields, either
            // across all sections (the default "All" view) or within the selected section.
            match self.versions.section {
                None => {
                    let mut seen = HashSet::new();
                    let mut fields: Vec<SchemaField> = sections
                        .iter()
                        .flat_map(|s| &s.fields)
                        .filter(|f| ch.contains_key(&f.name.to_lowercase()))
                        .filter(|f| seen.insert(f.name.to_lowercase()))
                        .cloned()
                        .collect();
                    // Add synthetic rows for changed fields absent from the current kind's schema
                    // (the core removed the field in an update, or it belongs to another kind).
                    // Otherwise the list could report "(2)" changes while displaying zero fields.
                    // Full mode synthesizes the same rows from the same helper.
                    fields.extend(param_entries::orphan_fields(ch, &seen));
                    ParamsBody::Section(SchemaSection {
                        title: t!("strat.sections_all").to_string(),
                        fields,
                    })
                }
                Some(i) => {
                    let Some(sec) = sections.get(i) else {
                        return ParamsPanelModel::NoSchema;
                    };
                    ParamsBody::Section(SchemaSection {
                        title: sec.title.clone(),
                        fields: sec
                            .fields
                            .iter()
                            .filter(|f| ch.contains_key(&f.name.to_lowercase()))
                            .cloned()
                            .collect(),
                    })
                }
            }
        } else {
            let Some(section) = sections.get(self.selected_section).cloned() else {
                return ParamsPanelModel::NoSchema;
            };
            ParamsBody::Section(section)
        };
        let pending: HashMap<Key, StrategyEditRow> = row_pairs
            .iter()
            .filter_map(|(key, _)| {
                store
                    .core(key.0)?
                    .strategy_edit(key.1)
                    .cloned()
                    .map(|edit| (*key, edit))
            })
            .collect();
        // Each core's notes come off ITS OWN cursor: two selected cores must never share one
        // watermark, or dismissing one core's notice would silently drop the other's.
        let mut edit_notes: Vec<(CoreId, StrategyEditNote)> = Vec::new();
        let mut cores_seen: HashSet<CoreId> = HashSet::new();
        for (core, _) in row_pairs.iter().map(|(key, _)| *key) {
            if !cores_seen.insert(core) {
                continue;
            }
            if let Some(cd) = store.core(core) {
                let since = self.last_edit_note_seq.get(&core).copied().unwrap_or(0);
                edit_notes.extend(
                    cd.strategy_edit_notes_since(since)
                        .cloned()
                        .map(|note| (core, note)),
                );
            }
        }
        ParamsPanelModel::Content {
            body,
            values,
            row_pairs,
            multi,
            common,
            differ,
            pending,
            edit_notes,
        }
    }

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
    pub(super) fn params_panel(
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
        if let Some((key, outcome, _)) = self.versions.staged_note {
            if matches!(outcome, StagedOutcome::Staged(_)) {
                let remaining = self
                    .field_edits
                    .keys()
                    .filter(|(core, id, _)| (*core, *id) == key)
                    .count();
                if remaining == 0 {
                    self.versions.staged_note = None;
                }
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
                .child(div().mt_2().text_color(moon(p.text_muted)).child(text))
                .into_any_element();
        };
        let keys: Vec<Key> = row_pairs.iter().map(|(key, _)| *key).collect();

        // Title and field total come from the body; the multi selection-count branch keeps
        // priority exactly as before the body could also be a full-mode list.
        let (title, field_total) = match &body {
            ParamsBody::Section(s) => (s.title.clone(), s.fields.len()),
            ParamsBody::Full(f) => (t!("strat.params_full_title").to_string(), f.field_count),
        };
        let count = if multi {
            t!("strat.selected_count", n = row_pairs.len()).to_string()
        } else {
            t!("strat.fields_count", n = field_total).to_string()
        };
        let dirty = field_edit_count(self);
        // Capture the complete visible draft set in the rendered Apply button. If the singleton
        // workspace moves before its callback runs, `apply_field_edits` rejects this plan whole.
        let apply_plan = Arc::new(self.field_edit_plan(cx));
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
            .h(design::fit_h_px(cx, 28.0, 14.0, 7.0))
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
                    .items_center()
                    .gap_2()
                    .child(mode_switch)
                    .child(
                        div()
                            .text_size(design::t_body(cx))
                            .text_color(moon(p.text_muted))
                            .child(count),
                    )
                    .when(dirty > 0, |row| {
                        row.child(
                            MoonButton::new("strat-fields-apply")
                                .success()
                                .size(MoonButtonSize::Micro)
                                .label(t!("strat.fields_apply", n = dirty).to_string())
                                .on_click({
                                    let apply_plan = apply_plan.clone();
                                    cx.listener(move |this, _, _, cx| {
                                        this.apply_field_edits(apply_plan.as_ref(), cx)
                                    })
                                })
                                .render(),
                        )
                        .child(
                            MoonButton::new("strat-fields-revert")
                                .ghost()
                                .size(MoonButtonSize::Micro)
                                .label(t!("strat.fields_revert").to_string())
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.discard_field_edits(cx)),
                                )
                                .render(),
                        )
                    }),
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
                                .size(MoonButtonSize::Micro)
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
                    if let Some(c) = &common {
                        if !c.contains(&lname) {
                            continue;
                        }
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
        if !self.prefs.params_full {
            if let Some(helper) = self.formula_helper(cx) {
                pane_body = pane_body.child(helper);
            }
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
        let mut note_banner: Option<(StrategyEditResult, CoreId, u64)> = None;
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
                note_banner = Some((note.result, core, note.seq));
            }
        }

        if let Some((result, core, seq)) = note_banner {
            let key = match result {
                StrategyEditResult::Adjusted => "strat.edit_adjusted",
                StrategyEditResult::Superseded => "strat.edit_superseded",
                StrategyEditResult::Confirmed => unreachable!("filtered above"),
            };
            let message = t!(key).to_string();
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
                            .size(MoonButtonSize::Micro)
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

    /// Render a field row with the name on the left and its current value control on the right.
    ///
    /// `active=false` dims and disables the row. `merged=None` means the selected values differ,
    /// so the row displays `≠` without a value and remains editable only when active.
    /// `pending_phase` marks a field touched by a still-open edit (see
    /// `logic::field_pending_phase`). An unsent local draft remains the higher-priority displayed
    /// value when present; this marker still records the edit beneath it. It never colours the row
    /// itself, only the trailing badge, so it can never collide with the unsent-draft amber this
    /// row already uses for `dirty`.
    /// `compact` is `None` in per-section mode (identical behaviour to before full mode existed);
    /// `Some(section)` marks a full-mode compact row, carrying the owning section index (`None`
    /// inside for a version-diff orphan row absent from any section).
    ///
    /// Args:
    ///     f: Schema field whose label, control kind, and rules define the row.
    ///     keys: Effective selected strategy keys used for retained editor identity and edits.
    ///     merged: Shared field value, or `None` when the selected values differ.
    ///     active: Whether dependency rules permit editing this field.
    ///     pending_phase: Open core edit phase shown by the trailing status badge.
    ///     compact: Full-mode marker and optional owning section, or `None` for a normal row.
    ///     window: Strategies window that owns retained editor state.
    ///     cx: View context used to create controls and callbacks.
    ///
    /// Returns:
    ///     The complete interactive or read-only field-row element.
    pub(super) fn field_row(
        &mut self,
        f: &SchemaField,
        keys: &[Key],
        merged: Option<String>,
        active: bool,
        pending_phase: Option<StrategyEditPhase>,
        compact: Option<Option<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Disable every control when viewing a persisted DB snapshot (stage_field_value is the
        // authoritative gate) while keeping values readable. Changed fields show the prior value
        // below as "was: ...".
        let frozen = self.viewing_version();
        let active = active && !frozen;
        let old_note = if frozen {
            self.versions
                .changed
                .get(&f.name.to_lowercase())
                .map(|(_, old)| old.clone())
        } else {
            None
        };
        let p = MoonPalette::active(cx);
        let name_col = if active { p.text_soft } else { p.text_muted };
        let val_col = if active { p.text } else { p.text_muted };

        let dirty = keys
            .iter()
            .any(|(core, id)| self.field_edits.contains_key(&(*core, *id, f.name.clone())));
        let field_name = f.name.clone();
        let row_id = editor_state_id(keys, &field_name);
        let view = cx.entity();

        // `merged == None` leaves the row editable with a `≠` marker and highlight;
        // `stage_field_value` applies entered text to every selected key, unifying their values.
        let differ = merged.is_none();
        let value = merged.unwrap_or_default();
        // Preserve the version value before moving it into a control so it can be compared with the
        // current value and used by the "copy to current" button.
        let version_val = value.clone();
        // Computed before `value` moves into the control below, and reused there: a memo/formula
        // field needs its diff arrow stacked vertically rather than beside a control it cannot
        // share a line with.
        let stacked = is_memo_field(f, &value);
        let control: AnyElement = match f.ui {
            SchemaFieldUi::Checkbox => {
                let on = is_on(&value);
                let keys = keys.to_vec();
                let field = field_name.clone();
                MoonCheckbox::new(SharedString::from(format!("field-check-{row_id}")))
                    .checked(on)
                    .indeterminate(differ)
                    .disabled(!active)
                    .size(MoonCheckboxSize::Compact)
                    .on_change(cx.listener(move |this, ch: &bool, _, cx| {
                        this.stage_field_value(
                            &keys,
                            &field,
                            if *ch { "Yes" } else { "No" }.to_string(),
                            cx,
                        );
                    }))
                    .into_any_element()
            }
            // A color field combines a hex input with a clickable palette swatch, exposing the
            // actual color and palette selection rather than only a color index.
            SchemaFieldUi::Color => {
                let keys_arc = Arc::new(keys.to_vec());
                let state = self.field_input_state(
                    row_id.clone(),
                    value.clone(),
                    keys_arc.clone(),
                    field_name.clone(),
                    window,
                    cx,
                );
                let picker = self.field_color_state(
                    row_id.clone(),
                    &value,
                    keys_arc,
                    field_name.clone(),
                    window,
                    cx,
                );
                let mut input = MoonInput::new(SharedString::from(format!("field-input-{row_id}")))
                    .state(&state)
                    .small()
                    .tone(MoonTone::Warning)
                    .selected(dirty || differ)
                    .disabled(!active);
                if differ {
                    input = input.placeholder(t!("strat.mixed_values").to_string());
                }
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_1()
                    .child(div().flex_1().min_w_0().child(input))
                    .child(
                        MoonColorPicker::new(&picker)
                            .colors(design::picker_palette())
                            .disabled(!active),
                    )
                    .into_any_element()
            }
            SchemaFieldUi::Combo if !f.picklist.is_empty() => {
                let mut items = Vec::with_capacity(f.picklist.len());
                for option in &f.picklist {
                    let option_value = option.clone();
                    let label = if option.is_empty() {
                        "—".to_string()
                    } else {
                        option.clone()
                    };
                    let keys = keys.to_vec();
                    let field = field_name.clone();
                    let view = view.clone();
                    items.push(
                        MoonMenuItem::with_key(format!("field-{row_id}-{option}"), label)
                            .selected(!differ && option_value == value)
                            .on_click(move |_, _, app| {
                                view.update(app, |this, cx| {
                                    this.stage_field_value(&keys, &field, option_value.clone(), cx);
                                });
                            }),
                    );
                }
                let trigger_label = if differ {
                    "≠".to_string()
                } else {
                    if value.is_empty() {
                        "—".to_string()
                    } else {
                        value.clone()
                    }
                };
                MoonDropdown::new(SharedString::from(format!("field-combo-{row_id}")))
                    .label(trigger_label)
                    .trigger_caret(true)
                    .trigger_variant(if dirty || differ {
                        MoonButtonVariant::Amber
                    } else {
                        MoonButtonVariant::Soft
                    })
                    .trigger_size(MoonButtonSize::Action)
                    .trigger_width_scaled(180.0)
                    .menu_width_scaled(220.0)
                    .menu_size(MoonMenuSize::Compact)
                    .menu_max_height_ui(220.0)
                    .disabled(!active)
                    .items(items)
                    .into_any_element()
            }
            _ => {
                let keys_arc = Arc::new(keys.to_vec());
                // Render differing values as an EMPTY input with a placeholder, never as a memo;
                // entered text applies to all selected strategies at once.
                if compact.is_none() && !differ && stacked {
                    let state = self.field_memo_state(
                        row_id.clone(),
                        value,
                        keys_arc,
                        field_name.clone(),
                        window,
                        cx,
                    );
                    MoonTextArea::new(SharedString::from(format!("field-memo-{row_id}")))
                        .state(&state)
                        .formula()
                        .tone(MoonTone::Warning)
                        .selected(dirty)
                        .disabled(!active)
                        .into_any_element()
                } else if compact.is_some() && !differ && is_memo_field(f, &value) {
                    // A disabled `MoonInput` here would need a retained state entity and a
                    // synchronization path to stay honest as drafts and version selection move
                    // underneath it. A static element carries the same look, is rebuilt from
                    // `merged` every frame, and touches neither `field_inputs` nor `field_memos`.
                    let display = compact_first_line(&value);
                    let preview = div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .h(design::ui_px(cx, 22.0))
                        .flex()
                        .items_center()
                        .px(design::ui_px(cx, 7.0))
                        .rounded(design::ui_px(cx, 4.0))
                        .border_1()
                        .border_color(moon(p.border))
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_muted))
                        .child(display);
                    let mut row = h_flex()
                        .w_full()
                        .items_center()
                        .gap(design::ui_px(cx, 6.0))
                        .child(preview);
                    if let Some(Some(section)) = compact {
                        let field_for_edit = field_name.clone();
                        row = row.child(
                            MoonButton::new(SharedString::from(format!("field-edit-{row_id}")))
                                .ghost()
                                .size(MoonButtonSize::Micro)
                                .label(t!("strat.params_edit_in_sections").to_string())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    // Two selectors, one per view: `params_model` resolves a
                                    // persisted snapshot's per-section body from `versions.section`
                                    // and the live one from `selected_section`. Writing only the
                                    // live selector would land a version-view jump on whatever the
                                    // diff had selected -- `None`, i.e. the synthetic "Все" body.
                                    if this.viewing_version() {
                                        this.versions.section = Some(section);
                                    } else {
                                        this.selected_section = section;
                                    }
                                    this.focused_field = Some(field_for_edit.clone());
                                    this.set_params_full(false, cx);
                                }))
                                .render(),
                        );
                    }
                    row.into_any_element()
                } else {
                    let state = self.field_input_state(
                        row_id.clone(),
                        value,
                        keys_arc,
                        field_name.clone(),
                        window,
                        cx,
                    );
                    let mut input =
                        MoonInput::new(SharedString::from(format!("field-input-{row_id}")))
                            .state(&state)
                            .small()
                            .tone(if differ || matches!(f.ui, SchemaFieldUi::Color) {
                                MoonTone::Warning
                            } else {
                                MoonTone::Info
                            })
                            .selected(dirty || differ)
                            .disabled(!active);
                    if differ {
                        input = input.placeholder(t!("strat.mixed_values").to_string());
                    }
                    input.into_any_element()
                }
            }
        };
        // In persisted-snapshot view, show "was: X" (the value before this snapshot) before the
        // snapshot control and "current: Y" (the live value now, when available) after it. The
        // "copy to current" button stages the snapshot value in the LIVE strategy with a yellow dirty marker;
        // "Apply N" sends the actual change to the core and creates a new version. Always show
        // "current" while the strategy is live, but show the copy button only when the live value
        // differs from the snapshot value.
        let cur_note: Option<String> = if frozen {
            let b = self.backend.read(cx);
            let store = b.session.store();
            selected_key(self)
                .and_then(|(c, id)| row(store, c, id))
                .map(|r| field_value(r, f))
        } else {
            None
        };
        // When a prior value exists, reading order is `before -> snapshot -> current` (defect 6):
        // the version being viewed frames the live control it stands above, and the live value
        // trails as context rather than leading it.
        //
        // Full mode's fixed row pitch clips an untruncated note (see
        // `full_params::full_row_h_value`), so compact mode flattens each note to a single line;
        // per-section mode keeps the note as the core sent it.
        let control: AnyElement = match old_note {
            None => control,
            // No "before" to point an arrow from: the field did not exist in the prior version.
            Some(old) if old.is_empty() => v_flex()
                .w_full()
                .gap(px(1.0))
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_soft))
                        .child(t!("strat.version_added").to_string()),
                )
                .child(control)
                .into_any_element(),
            Some(old) => {
                let display_old = if compact.is_some() {
                    compact_first_line(&old)
                } else {
                    old.clone()
                };
                let was = div()
                    .flex_none()
                    .min_w_0()
                    .truncate()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_soft))
                    .child(t!("strat.version_was", v = display_old).to_string());
                let arrow = div()
                    .id(SharedString::from(format!("diff-arrow-{row_id}")))
                    .flex_none()
                    .text_color(moon(p.text_muted))
                    .tooltip(crate::panels::common::text_tooltip(
                        t!("strat.version_diff_tip").to_string(),
                    ))
                    .child(if stacked { "↓" } else { "→" });
                if stacked {
                    v_flex()
                        .w_full()
                        .gap(px(1.0))
                        .child(was)
                        .child(arrow)
                        .child(control)
                        .into_any_element()
                } else {
                    h_flex()
                        .w_full()
                        .items_start()
                        .gap(design::ui_px(cx, 6.0))
                        .child(was)
                        .child(arrow)
                        .child(div().flex_1().min_w_0().child(control))
                        .into_any_element()
                }
            }
        };
        // When available, append the live value after the snapshot value and keep it visually
        // subordinate to the diff.
        let control: AnyElement = if let Some(cur) = cur_note {
            // Compare semantically: `YES` from the import era equals `Yes`, and `1` equals `1.0`.
            let differs = !values_equal(&cur, &version_val);
            let fname = field_name.clone();
            let vval = version_val.clone();
            let display_cur = if compact.is_some() {
                compact_first_line(&cur)
            } else {
                cur.clone()
            };
            let mut line = h_flex().items_center().gap(design::ui_px(cx, 6.0)).child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(design::t_caption(cx))
                    // Use blue when the live value differs and can be copied; dim matching values.
                    .text_color(moon(if differs { p.blue } else { p.text_soft }))
                    .child(t!("strat.version_cur", v = display_cur).to_string()),
            );
            if differs {
                line = line.child(
                    MoonButton::new(SharedString::from(format!("copy-cur-{row_id}")))
                        .ghost()
                        .size(MoonButtonSize::Micro)
                        .label(t!("strat.copy_to_current").to_string())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            // Intentionally bypass the viewing_version gate: copying from a
                            // version is the only permitted edit in this view.
                            if let Some((core, id)) = selected_key(this) {
                                this.field_edits
                                    .insert((core, id, fname.clone()), vval.clone());
                                this.focused_field = Some(fname.clone());
                                cx.notify();
                            }
                        }))
                        .render(),
                );
            }
            v_flex()
                .w_full()
                .gap(px(1.0))
                .child(control)
                .child(line)
                .into_any_element()
        } else {
            control
        };
        // Prefix an editable control with `≠` when the selected values differ.
        let value_el: AnyElement = if differ {
            h_flex()
                .items_center()
                .gap_1()
                .w_full()
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(moon(p.blue))
                        .child("≠"),
                )
                .child(control)
                .into_any_element()
        } else {
            control
        };

        let field_for_focus = field_name.clone();
        h_flex()
            .id(SharedString::from(format!("field-row-{row_id}")))
            .w_full()
            .items_start()
            .gap(design::ui_px(cx, 14.0))
            .min_h(design::fit_h_px(cx, 30.0, 14.0, 8.0))
            .py(design::ui_px(cx, 4.0))
            .border_l(px(2.0))
            .border_color(moon_alpha(p.amber, if dirty { 0.72 } else { 0.0 }))
            .pl(px(8.0))
            .pr_2()
            .rounded(design::ui_px(cx, 3.0))
            .when(dirty, |s| s.bg(moon_alpha(p.amber, 0.06)))
            .hover(move |s| s.bg(moon_alpha(p.panel, 0.46)))
            .child(
                h_flex()
                    .w(design::font_w_px(cx, 180.0))
                    .flex_none()
                    .pt(px(5.0))
                    .items_start()
                    .gap_1()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(moon(name_col))
                            .child(f.name.clone()),
                    )
                    // Mark edits that have not been applied so changed fields remain visible in a
                    // long parameter list before the user presses "apply".
                    .when(dirty, |row| {
                        row.child(
                            div()
                                .flex_none()
                                .font_weight(FontWeight::BOLD)
                                .text_color(moon(p.red))
                                .child("**"),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    // Clip values to their cells so long memo text cannot overlap adjacent rows.
                    .overflow_hidden()
                    .text_color(moon(val_col))
                    .child(value_el),
            )
            .child(
                div()
                    .flex_none()
                    .pt(px(2.0))
                    .when_some(pending_phase, |el, phase| {
                        // Never MoonTone::Warning here: it resolves to palette.amber, the exact
                        // colour this row already uses for `dirty`'s left border and background.
                        let (label, tone) = match phase {
                            StrategyEditPhase::Pending => {
                                (t!("strat.edit_pending").to_string(), MoonTone::Info)
                            }
                            StrategyEditPhase::TimedOut => {
                                (t!("strat.edit_timeout").to_string(), MoonTone::Notice)
                            }
                        };
                        el.child(
                            MoonBadge::new(label)
                                .variant(MoonBadgeVariant::Soft)
                                .size(MoonBadgeSize::Status)
                                .tone(tone)
                                .render(),
                        )
                    }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.focused_field = Some(field_for_focus.clone());
                cx.notify();
            }))
            .into_any_element()
    }

    fn formula_helper(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let field = self.focused_field.clone()?;
        if !is_formula_field(&field) {
            return None;
        }
        // Offer the formula helper only for editable STRING fields. Name matching also caught
        // checkboxes (`IgnoreFilters` contains `filter`), so clicking one opened EMA suggestions;
        // inspect the schema field type and control instead.
        {
            let b = self.backend.read(cx);
            let store = b.session.store();
            if let Some(sections) = selected_sections(self, store) {
                if let Some(f) = sections
                    .iter()
                    .flat_map(|s| &s.fields)
                    .find(|f| f.name == field)
                {
                    if f.type_name != "String" || !matches!(f.ui, SchemaFieldUi::Edit) {
                        return None;
                    }
                }
            }
        }
        let p = MoonPalette::active(cx);
        let snippets = formula_snippets();
        let mut list = v_flex().w_full().gap_1();
        for (label, detail, insert) in snippets {
            let field = field.clone();
            list = list.child(
                v_flex()
                    .id(SharedString::from(format!("helper-{label}")))
                    .w_full()
                    .rounded(design::r_button(cx))
                    .border_1()
                    .border_color(moon(p.border))
                    .bg(moon(p.panel))
                    .px(design::ui_px(cx, 8.0))
                    .py(design::ui_px(cx, 6.0))
                    .cursor_pointer()
                    .hover(move |s| s.border_color(moon_alpha(p.amber, 0.72)))
                    .child(
                        div()
                            .font_family(design::mono())
                            .text_size(design::t_body(cx))
                            .text_color(moon(p.text))
                            .child(label),
                    )
                    .child(
                        div()
                            .font_family(design::mono())
                            .text_size(design::t_body(cx))
                            .text_color(moon(p.text_muted))
                            .child(detail),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.append_formula_snippet(&field, insert, cx);
                    })),
            );
        }
        Some(
            v_flex()
                .w(design::font_w_px(cx, 280.0))
                .h_full()
                .flex_none()
                .gap(design::ui_px(cx, 10.0))
                .px(design::ui_px(cx, 16.0))
                .py(design::ui_px(cx, 14.0))
                .bg(moon(p.shell_high))
                .border_l_1()
                .border_color(moon(p.border))
                .child(
                    div()
                        .text_size(design::t_body(cx))
                        .text_color(moon(p.text_muted))
                        .child(format!("{field} · formula helper")),
                )
                .child(list)
                .into_any_element(),
        )
    }
}
