//! Right pane of the Strategies window: the parameter-pane model and renderer, including
//! selected-strategy badges/value editors (read-only YES/NO, input/memo, formula helper) and the
//! full-value popover. The methods extend `StrategiesView` from [`super`].

use super::*;
use rust_i18n::t;

pub(super) enum ParamsPanelModel {
    NoSelection,
    NoSchema,
    Content {
        section: SchemaSection,
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
    /// Builds the selected parameter-section model from dependency values shared across both panes.
    ///
    /// Accepting `values` keeps field and schema normalization to one pass per frame.
    pub(super) fn params_model(&self, store: &CoreStore, values: Values) -> ParamsPanelModel {
        if selected_row(self, store).is_none() {
            return ParamsPanelModel::NoSelection;
        }
        let Some(sections) = selected_sections(self, store) else {
            return ParamsPanelModel::NoSchema;
        };
        // When viewing a persisted snapshot with a diff, show ONLY changed fields, either across
        // all sections (the default "All" view) or within the selected section.
        let section = if let Some(ch) = self.version_changed_filter() {
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
                    let mut extra: Vec<&String> = ch
                        .iter()
                        .filter(|(lc, _)| !seen.contains(lc.as_str()))
                        .map(|(_, (name, _))| name)
                        .collect();
                    extra.sort();
                    fields.extend(extra.into_iter().map(|name| SchemaField {
                        name: name.clone(),
                        type_name: "String".to_string(),
                        ui: SchemaFieldUi::Edit,
                        picklist: Vec::new(),
                        default: None,
                    }));
                    SchemaSection {
                        title: t!("strat.sections_all").to_string(),
                        fields,
                    }
                }
                Some(i) => {
                    let Some(sec) = sections.get(i) else {
                        return ParamsPanelModel::NoSchema;
                    };
                    SchemaSection {
                        title: sec.title.clone(),
                        fields: sec
                            .fields
                            .iter()
                            .filter(|f| ch.contains_key(&f.name.to_lowercase()))
                            .cloned()
                            .collect(),
                    }
                }
            }
        } else {
            let Some(section) = sections.get(self.selected_section).cloned() else {
                return ParamsPanelModel::NoSchema;
            };
            section
        };
        let row_pairs: Vec<(Key, StrategyRow)> = multi_row_pairs(self, store)
            .into_iter()
            .map(|(key, row)| (key, row.clone()))
            .collect();
        let multi = row_pairs.len() > 1;
        let common = common_fields(self, store);
        let differ = kinds_differ(self, store);
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
            section,
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
            section,
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

        // Section title with the field/selection count on the right.
        let count = if multi {
            t!("strat.selected_count", n = row_pairs.len()).to_string()
        } else {
            t!("strat.fields_count", n = section.fields.len()).to_string()
        };
        let dirty = field_edit_count(self);
        // Capture the complete visible draft set in the rendered Apply button. If the singleton
        // workspace moves before its callback runs, `apply_field_edits` rejects this plan whole.
        let apply_plan = Arc::new(self.field_edit_plan(cx));
        let mut header = h_flex()
            .w_full()
            .h(design::fit_h_px(cx, 28.0, 14.0, 7.0))
            .items_center()
            .justify_between()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(moon(p.text))
                    .child(section.title.clone()),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
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
        // (for example, a created/baseline snapshot), explain why all fields are displayed.
        if let Some(vf) = self.versions.sel {
            let date = moon_core::strat_db::stats::short_date(vf, self.display_zone);
            let text = if self.version_changed_filter().is_some() {
                t!("strat.version_view", date = date).to_string()
            } else {
                t!("strat.version_view_nodiff", date = date).to_string()
            };
            col = col.child(
                div()
                    .w_full()
                    .px(design::ui_px(cx, 8.0))
                    .py(design::ui_px(cx, 4.0))
                    .rounded(design::ui_px(cx, 3.0))
                    .bg(moon_alpha(p.amber, 0.10))
                    .border_l_2()
                    .border_color(moon_alpha(p.amber, 0.72))
                    .text_color(moon(p.amber))
                    .child(text),
            );
        }
        col = col
            .child(header)
            .child(div().w_full().h(px(1.0)).bg(moon(p.border)));

        if let Some(banner) = self.edit_state_banner(&row_pairs, &pending, &edit_notes, cx) {
            col = col.child(banner);
        }

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
            list = list.child(self.field_row(f, &keys, merged, active, pending_phase, window, cx));
        }
        let scroll = div()
            .id("strat-params-scroll")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .child(list);
        let mut body = h_flex()
            .flex_1()
            .w_full()
            .min_h_0()
            .items_start()
            .gap_2()
            .child(scroll);
        if let Some(helper) = self.formula_helper(cx) {
            body = body.child(helper);
        }
        col = col.child(body);
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

    /// Render a field row with the name on the left and value on the right.
    ///
    /// `active=false` dims and disables the row. `merged=None` means the selected values differ,
    /// so the row displays `≠` without a value and remains editable only when active.
    /// `pending_phase` marks a field touched by a still-open edit (see
    /// `logic::field_pending_phase`). An unsent local draft remains the higher-priority displayed
    /// value when present; this marker still records the edit beneath it. It never colours the row
    /// itself, only the trailing badge, so it can never collide with the unsent-draft amber this
    /// row already uses for `dirty`.
    fn field_row(
        &mut self,
        f: &SchemaField,
        keys: &[Key],
        merged: Option<String>,
        active: bool,
        pending_phase: Option<StrategyEditPhase>,
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
                if !differ && is_memo_field(f, &value) {
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
        // In persisted-snapshot view, show "was: X" (the value before this snapshot) and
        // "current: Y" (the live value now, when available) below the control. The "copy to current"
        // button stages the snapshot value in the LIVE strategy with a yellow dirty marker;
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
        let control: AnyElement = if old_note.is_some() || cur_note.is_some() {
            let mut wrap = v_flex().w_full().gap(px(1.0)).child(control);
            if let Some(old) = old_note {
                let note = if old.is_empty() {
                    t!("strat.version_added").to_string()
                } else {
                    t!("strat.version_was", v = old).to_string()
                };
                wrap = wrap.child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_soft))
                        .child(note),
                );
            }
            if let Some(cur) = cur_note {
                // Compare semantically: `YES` from the import era equals `Yes`, and `1` equals `1.0`.
                let differs = !values_equal(&cur, &version_val);
                let fname = field_name.clone();
                let vval = version_val.clone();
                let mut line = h_flex().items_center().gap(design::ui_px(cx, 6.0)).child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(design::t_caption(cx))
                        // Use blue when the live value differs and can be copied; dim matching values.
                        .text_color(moon(if differs { p.blue } else { p.text_soft }))
                        .child(t!("strat.version_cur", v = cur).to_string()),
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
                wrap = wrap.child(line);
            }
            wrap.into_any_element()
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
