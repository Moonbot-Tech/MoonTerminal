//! Strategies parameter model implementation.

use super::*;

impl StrategiesView {
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
    pub(in crate::strategies) fn params_model(
        &self,
        store: &CoreStore,
        values: Values,
    ) -> ParamsPanelModel {
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
                param_entries::ParamLabels {
                    orphans: &orphans,
                    section_title: &|raw| section_display_title(raw, self.prefs.human_labels),
                },
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
}
