//! Strategies parameter editors implementation.

use super::*;

impl StrategiesView {
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
    /// Known list fields in live multi-selection also offer append/remove dialogs; their tooltips
    /// report how many selected strategies have changed drafts, without replacing plain typing.
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
    pub(in crate::strategies) fn field_row(
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
        // The human name is a preference (`StrategiesPrefs::human_labels`); off, the row keeps
        // only the name the core speaks, while the help tooltip is unaffected.
        let (field_tooltip, field_label) = match field_keys(&field_name) {
            Some((help, label)) => (
                help.map(|key| t!(key).to_string()),
                label
                    .filter(|_| self.prefs.human_labels)
                    .map(|key| t!(key).to_string()),
            ),
            None => (None, None),
        };
        let view = cx.entity();

        // `merged == None` leaves the row editable with a `≠` marker and highlight;
        // `stage_field_value` applies entered text to every selected key, unifying their values.
        let differ = merged.is_none();
        let value = merged.unwrap_or_default();
        // Preserve the version value before moving it into a control so it can be compared with the
        // current value and used by the "copy to current" button.
        let version_val = value.clone();
        // More than one selected strategy has no single row to reveal into, so the flash, the
        // token strip, and the remove button stay hidden.
        let reveal = (keys.len() <= 1)
            .then(|| self.coin_field_reveal(&field_name, &value))
            .flatten();
        if reveal.as_ref().is_some_and(|item| item.alpha > 0.0) {
            window.request_animation_frame();
        }
        let token_source = reveal
            .as_ref()
            .filter(|item| item.marked)
            .map(|_| value.clone());
        // Computed before `value` moves into the control below, and reused there: a memo/formula
        // field needs its diff arrow stacked vertically rather than beside a control it cannot
        // share a line with.
        let stacked = is_memo_field(f, &value);
        // Computed here, before `value` moves into a control: text the core would refuse to store
        // paints the input red, and `sendable_field_edits` keeps it out of Apply, so it says so
        // instead of accepting the press and quietly restoring the old value. Only a DRAFT can be
        // rejected — a value the core itself holds is not the user's to answer for, and a core that
        // reports a `NaN` would otherwise paint an untouched row red for good. A mixed selection's
        // empty control is not a draft either.
        let rejected = dirty && !differ && draft_rejected(f, &value);
        let list_actions = keys.len() > 1 && !frozen && is_list_field(f);
        // ONE control table, shared with `draft_rejected` through `field_control`: the marker has
        // to know which rows are free text, and a second copy of this decision would drift.
        let control: AnyElement = match field_control(f) {
            FieldControl::Checkbox => {
                let on = is_on(&value);
                let keys = keys.to_vec();
                let field = field_name.clone();
                MoonCheckbox::new(SharedString::from(format!("field-check-{row_id}")))
                    .checked(on)
                    .indeterminate(differ)
                    .disabled(!active)
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
            FieldControl::Color => {
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
                    .size(design::INPUT_SIZE)
                    .tone(MoonTone::Warning)
                    .selected(dirty || differ)
                    .disabled(!active);
                if differ {
                    input = input.placeholder(t!("common.mixed_values").to_string());
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
            FieldControl::Picklist => {
                let picklist = effective_picklist(f, &value);
                let mut items = Vec::with_capacity(picklist.len());
                for option in &picklist {
                    let option_value = option.clone();
                    let label = if option.is_empty() {
                        "—".to_string()
                    } else {
                        option.clone()
                    };
                    let keys = keys.to_vec();
                    let field = field_name.clone();
                    let view = view.clone();
                    // The mark is MoonUI's muted trailing text, not a change to the value the
                    // click sends. Disabling the row would hide a name the user still has to
                    // be able to keep.
                    let mut item =
                        MoonMenuItem::with_key(format!("field-{row_id}-{option}"), label)
                            .selected(!differ && picklist_row_is(f, &option_value, &value));
                    if let Some(mark) = picklist_missing_mark(f, option) {
                        item = item.right_label(mark);
                    }
                    items.push(item.on_click(move |_, _, app| {
                        view.update(app, |this, cx| {
                            this.stage_field_value(&keys, &field, option_value.clone(), cx);
                        });
                    }));
                }
                let trigger_label = if differ {
                    design::MIXED_MARK.to_string()
                } else if value.is_empty() {
                    "—".to_string()
                } else if let Some(mark) = picklist_missing_mark(f, &value) {
                    format!("{value} ({mark})")
                } else {
                    value.clone()
                };
                MoonDropdown::new(SharedString::from(format!("field-combo-{row_id}")))
                    .label(trigger_label)
                    .trigger_caret(true)
                    .trigger_variant(if dirty || differ {
                        MoonButtonVariant::Amber
                    } else {
                        MoonButtonVariant::Soft
                    })
                    .trigger_size(MoonButtonSize::density(cx))
                    .trigger_width_scaled(180.0)
                    .fit_menu_width(220.0, 560.0)
                    .menu_max_height_ui(220.0)
                    .disabled(!active)
                    .items(items)
                    .into_any_element()
            }
            _ => {
                let keys_arc = Arc::new(keys.to_vec());
                // Render differing values as an EMPTY input with a placeholder, never as a memo;
                // entered text applies to all selected strategies at once.
                if compact.is_none() && !differ && stacked && !list_actions {
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
                } else if compact.is_some() && !differ && is_memo_field(f, &value) && !list_actions
                {
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
                                        this.persist_session(cx);
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
                            .size(design::INPUT_SIZE)
                            // No colour case here: a colour field draws its own input in the arm
                            // above, so this one only ever renders free text.
                            .tone(if rejected {
                                MoonTone::Danger
                            } else if differ {
                                MoonTone::Warning
                            } else {
                                MoonTone::Info
                            })
                            .selected(dirty || differ)
                            .disabled(!active);
                    if differ {
                        input = input.placeholder(t!("common.mixed_values").to_string());
                    }
                    input.into_any_element()
                }
            }
        };
        // Separate token entry is essential: typing into the ordinary input has already staged
        // a replacement for every target, so using that input as an operand would lose originals.
        let control = if list_actions {
            let store = self.backend.read(cx).session.store();
            let editable = active && list_field_values(self, store, keys, &field_name).is_some();
            let changed = keys
                .iter()
                .filter(|&&(core, id)| {
                    let Some(draft) = self.field_edits.get(&(core, id, field_name.clone())) else {
                        return false;
                    };
                    let Some(strategy) = row(store, core, id) else {
                        return false;
                    };
                    let Some(schema) =
                        schema_field_in_kind(store, core, strategy.kind_ordinal, &field_name)
                    else {
                        return false;
                    };
                    let pending = store.core(core).and_then(|cd| cd.strategy_edit(id));
                    *draft
                        != pending_field_value(pending, strategy, schema)
                            .unwrap_or_else(|| field_value(strategy, schema))
                })
                .count();
            let mut controls = h_flex()
                .w_full()
                .min_w_0()
                .items_start()
                .gap(design::ui_px(cx, 4.0))
                .child(div().flex_1().min_w_0().child(control));
            for (operation, icon, label) in [
                (ListEdit::Append, "icons/plus.svg", t!("strat.list_append")),
                (ListEdit::Remove, "icons/minus.svg", t!("strat.list_remove")),
            ] {
                let keys = keys.to_vec();
                let field = field_name.clone();
                controls = controls.child(
                    MoonButton::new(SharedString::from(format!(
                        "field-list-{operation:?}-{row_id}"
                    )))
                    .ghost()
                    .icon(icon)
                    .size(MoonButtonSize::density(cx))
                    .tooltip(format!(
                        "{label}\n{}",
                        t!("strat.list_changed", n = changed)
                    ))
                    .disabled(!editable)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_list_field_dialog(
                            keys.clone(),
                            field.clone(),
                            operation,
                            window,
                            cx,
                        );
                    }))
                    .render(),
                );
            }
            controls.into_any_element()
        } else {
            control
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
                        .child(design::MIXED_MARK),
                )
                .child(control)
                .into_any_element()
        } else {
            control
        };

        let compact_row = compact.is_some();
        let flash_alpha = reveal.as_ref().map(|item| item.alpha).unwrap_or(0.0);
        let reveal_strip = reveal.as_ref().and_then(|item| {
            let text = token_source.as_deref()?;
            item.marked.then(|| {
                let size = if compact_row {
                    design::t_caption(cx)
                } else {
                    design::t_body(cx)
                };
                // A bounded strip windows the list so the coin stays visible at any position.
                let max_width = Some(design::font_w_px(
                    cx,
                    if compact_row { 160.0 } else { 480.0 },
                ));
                crate::controls::coin_token::coin_token_strip(text, &item.coin, size, max_width, cx)
            })
        });
        let reveal_button = reveal
            .as_ref()
            .filter(|item| item.marked && active)
            .map(|item| {
                let weak = cx.entity().downgrade();
                let editor_id = row_id.clone();
                let coin = item.coin.clone();
                let error = item.error.clone();
                strategy_blacklist_remove(
                    &row_id,
                    &coin,
                    error.as_deref(),
                    compact_row,
                    move |_window, app| {
                        let editor_id = editor_id.clone();
                        let _ = weak.update(app, |this, cx| {
                            this.remove_revealed_coin(&editor_id, cx);
                        });
                    },
                    cx,
                )
            });
        let field_for_focus = field_name.clone();
        // Line 1 is always Moonbot's own identifier: it is what the manual, a forum post and the
        // strategy file call the field, so it leads the row. Line 2 is the human name when there is
        // one, so a field with no label looks exactly as it did before. Full mode's row pitch is
        // fixed (`full_params::full_row_h_value`) and would clip a second line, so a compact row
        // keeps one line and moves the human name into its tooltip.
        let subtitle = (!compact_row).then_some(field_label.clone()).flatten();
        let headline = f.name.clone();
        let name_tooltip = match (compact_row, field_label, field_tooltip) {
            (true, Some(label), Some(help)) => Some(format!("{label} - {help}")),
            (true, Some(label), None) => Some(label),
            (_, _, help) => help,
        };
        h_flex()
            .id(SharedString::from(format!("field-row-{row_id}")))
            .w_full()
            .items_start()
            .gap(design::ui_px(cx, 14.0))
            .min_h(design::fit_h_px(cx, 30.0, 14.0, 8.0))
            .py(design::ui_px(cx, 4.0))
            .border_l(px(2.0))
            .border_color(moon_alpha(p.amber, if dirty { 0.72 } else { 0.0 }))
            .when(flash_alpha > 0.0, |row| {
                row.border(px(1.5))
                    .border_color(moon_alpha(p.amber, flash_alpha))
            })
            .pl(px(8.0))
            .pr_2()
            .rounded(design::ui_px(cx, 3.0))
            .when(dirty, |s| s.bg(moon_alpha(p.amber, 0.06)))
            .hover(move |s| s.bg(moon_alpha(p.panel, 0.46)))
            .child(
                // The width owner is this column, and every box between it and a `.truncate()`
                // leaf carries a definite width of its own: an intermediate flex sized by its
                // content collapses the whole line to a bare ellipsis, which is exactly what a
                // one-line-per-segment label cell invites.
                v_flex()
                    .id(SharedString::from(format!("field-label-{row_id}")))
                    .w(design::font_w_px(cx, 180.0))
                    // Reserve room for the input and both list actions at narrow widths and
                    // larger densities. Wide rows retain the normal label-column width.
                    .when(list_actions, |cell| cell.max_w(relative(0.4)))
                    .flex_none()
                    .min_w_0()
                    .pt(px(5.0))
                    .items_start()
                    .when_some(name_tooltip, |cell, tooltip| {
                        cell.tooltip(crate::panels::common::text_tooltip(tooltip))
                    })
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .items_start()
                            .gap_1()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(moon(name_col))
                                    .child(headline),
                            )
                            // Mark edits that have not been applied so changed fields remain
                            // visible in a long parameter list before the user presses "apply".
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
                    // The human name, kept under the identifier rather than hidden in a
                    // tooltip, so a row reads in the user's language without losing the name the
                    // core actually speaks.
                    .when_some(subtitle, |cell, raw| {
                        cell.child(
                            div()
                                .w_full()
                                .min_w_0()
                                .truncate()
                                .text_size(design::t_caption(cx))
                                .line_height(design::line_px(cx, 12.0))
                                .text_color(moon(p.text_muted))
                                .child(raw),
                        )
                    }),
            )
            .child(match reveal_strip {
                Some(strip) if !compact_row => v_flex()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .gap(px(2.0))
                    .text_color(moon(val_col))
                    .child(strip)
                    .child(value_el)
                    .into_any_element(),
                Some(strip) => h_flex()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .items_center()
                    .gap(design::ui_px(cx, 6.0))
                    .text_color(moon(val_col))
                    .child(strip)
                    .child(div().flex_1().min_w_0().child(value_el))
                    .into_any_element(),
                None => div()
                    .flex_1()
                    .min_w_0()
                    // Clip values to their cells so long memo text cannot overlap adjacent rows.
                    .overflow_hidden()
                    .text_color(moon(val_col))
                    .child(value_el)
                    .into_any_element(),
            })
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
                                .tone(tone)
                                .render(),
                        )
                    }),
            )
            .children(reveal_button)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.focused_field = Some(field_for_focus.clone());
                cx.notify();
            }))
            .into_any_element()
    }

    /// The coins-blacklist reveal for this field, when it belongs to the strategy on screen.
    ///
    /// A different field, or a selection that has moved on, draws nothing. The token is marked
    /// only while the value still lists it, so the highlight and the remove button leave together.
    ///
    /// Args:
    ///     field: Schema field name of the row being drawn.
    ///     value: Text the row is showing, draft first.
    ///
    /// Returns:
    ///     The coin, the outline alpha, any refusal, and whether the value still lists the coin.
    fn coin_field_reveal(&self, field: &str, value: &str) -> Option<CoinReveal> {
        let flash = self.field_flash.as_ref()?;
        if flash.key != field || flash.strategy != selected_key(self) {
            return None;
        }
        let marked = moon_core::symbol::coin_list::contains(value, &flash.coin);
        Some(CoinReveal {
            coin: flash.coin.clone(),
            alpha: crate::controls::coin_token::flash_alpha(flash.started),
            error: flash.error.clone(),
            marked,
        })
    }

    /// Remove the revealed coin from the strategy and from an unsent draft of the same field.
    ///
    /// `Ok(false)` still counts: the core text was already without the coin, and a draft that
    /// still names it must not be sent by Apply. No draft is created when none exists. A refusal
    /// leaves the draft, the input, and the flash, and shows the failure beside the button.
    ///
    /// Args:
    ///     editor_id: Retained input id of this row, from the same keys the row was built with.
    ///     cx: View context for the backend write and the input update.
    fn remove_revealed_coin(&mut self, editor_id: &str, cx: &mut Context<Self>) {
        let Some(flash) = self.field_flash.clone() else {
            return;
        };
        let Some((core, sid)) = flash.strategy else {
            return;
        };
        let coin = flash.coin.clone();
        let field = flash.key.clone();
        let outcome = self.backend.update(cx, |backend, _| {
            crate::controls::coin_menu::write_strategy_blacklist(backend, core, sid, &coin, true)
        });
        match outcome {
            Ok(_) => {
                let edit_key = (core, sid, field);
                if let Some(next) = self
                    .field_edits
                    .get(&edit_key)
                    .map(|draft| moon_core::symbol::coin_list::remove(draft, &coin))
                {
                    self.field_edits.insert(edit_key, next.clone());
                    if let Some(input) = self.field_inputs.get(editor_id).cloned() {
                        input.update(cx, |state, cx| state.sync_value(next, cx));
                    }
                }
                self.field_flash = None;
                cx.notify();
            }
            Err(_) => {
                if let Some(flash) = self.field_flash.as_mut() {
                    flash.error = Some(t!("coin_bl_places.remove_failed", coin = coin).to_string());
                }
                cx.notify();
            }
        }
    }
}

/// One field row's coins-blacklist reveal.
struct CoinReveal {
    coin: String,
    alpha: f32,
    error: Option<String>,
    marked: bool,
}

/// The «Убрать <COIN>» control a strategy field row draws at its trailing edge.
///
/// The button is the shared outline control. Per-section mode stacks a refusal under it.
/// Full mode keeps the fixed row pitch: the refusal is one truncated line beside the button,
/// and the tooltip carries the full sentence.
///
/// Args:
///     row_id: Field-row id, so two rows never share a button id.
///     coin: Token the label names.
///     error: Refusal text, when the last remove failed.
///     compact: Full-mode row. The refusal stays on one line.
///     on_remove: Writes the strategy and rewrites an unsent draft.
///     cx: Application context for the type scale and the danger colour.
///
/// Returns:
///     The button, and the refusal when `error` is set.
fn strategy_blacklist_remove(
    row_id: &str,
    coin: &str,
    error: Option<&str>,
    compact: bool,
    on_remove: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let button = crate::shell::coin_blacklist_remove_button(
        SharedString::from(format!("coin-bl-remove-{row_id}")),
        coin,
        on_remove,
    );
    let selector = {
        let row_id = row_id.to_string();
        move || format!("coin-bl-remove-wrap-{row_id}")
    };
    if compact {
        let mut row = h_flex()
            .id(SharedString::from(format!("coin-bl-remove-wrap-{row_id}")))
            .debug_selector(selector)
            .flex_none()
            .items_center()
            .gap(px(4.0))
            .child(button);
        if let Some(error) = error {
            let full = error.to_string();
            row = row.child(
                div()
                    .id(SharedString::from(format!("coin-bl-remove-error-{row_id}")))
                    .max_w(design::font_w_px(cx, 140.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(design::t_caption(cx))
                    .text_color(design::moon(design::danger_color(p)))
                    .tooltip(crate::panels::common::text_tooltip(full.clone()))
                    .child(full),
            );
        }
        return row.into_any_element();
    }
    let mut stack = v_flex()
        .id(SharedString::from(format!("coin-bl-remove-wrap-{row_id}")))
        .debug_selector(selector)
        .flex_none()
        .items_end()
        .gap(px(2.0))
        .child(button);
    if let Some(error) = error {
        stack = stack.child(
            div()
                .max_w(design::font_w_px(cx, 220.0))
                .text_size(design::t_caption(cx))
                .text_color(design::moon(design::danger_color(p)))
                .child(error.to_string()),
        );
    }
    stack.into_any_element()
}

#[cfg(test)]
mod tests;
