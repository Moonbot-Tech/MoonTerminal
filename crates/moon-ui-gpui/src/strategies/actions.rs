//! Start/stop and field-edit actions dispatched to the session, plus name search.

use super::*;

impl StrategiesView {
    /// Start or stop checked strategies on each core through `session.apply_strategies`.
    /// Sends staged checkbox differences plus the start/stop command when the core has an
    /// effectively checked strategy or any checkbox changes, then clears staging after success.
    pub(super) fn apply_start_stop(
        &mut self,
        cores: &[(CoreId, String)],
        start: bool,
        cx: &mut Context<Self>,
    ) {
        // Collect actions while reading the store, then reacquire the backend to apply them.
        let mut actions: Vec<(CoreId, Vec<(u64, bool)>, bool)> = Vec::new();
        {
            let b = self.backend.read(cx);
            let store = b.session.store();
            for (core, _) in cores {
                let Some(cd) = store.core(*core) else {
                    continue;
                };
                let mut checks = Vec::new();
                let mut has_checked = false;
                for r in &cd.strategies {
                    let eff = self
                        .staged
                        .get(&(*core, r.id))
                        .copied()
                        .unwrap_or(r.checked);
                    if eff != r.checked {
                        checks.push((r.id, eff));
                    }
                    if eff {
                        has_checked = true;
                    }
                }
                if !checks.is_empty() || has_checked {
                    actions.push((*core, checks, start));
                }
            }
        }
        if actions.is_empty() {
            return;
        }
        let b = self.backend.read(cx);
        for (core, checks, st) in actions {
            if let Err(error) = b.session.apply_strategies(core, checks, Some(st)) {
                log::warn!("apply strategies failed: {error}");
                return;
            }
        }
        self.staged.clear();
        cx.notify();
    }

    pub(super) fn stage_field_value(
        &mut self,
        keys: &[Key],
        field: &str,
        value: String,
        cx: &mut Context<Self>,
    ) {
        // Persisted snapshot views are read-only, including the snapshot labeled current. Live mode
        // has no selected snapshot. Controls are disabled, and this setter guard blocks indirect
        // paths such as the color picker as a backstop.
        if keys.is_empty() || self.viewing_version() {
            return;
        }
        self.focused_field = Some(field.to_string());
        for (core, id) in keys {
            self.field_edits
                .insert((*core, *id, field.to_string()), value.clone());
        }
        cx.notify();
    }

    pub(super) fn apply_field_edits(&mut self, cx: &mut Context<Self>) {
        if self.field_edits.is_empty() {
            return;
        }
        // Group by core and then strategy, sending one command with all edits for each core.
        // Separate commands for multiple selected strategies on one core would let successive
        // `sync_local_strategies` calls overwrite each other and retain only one strategy's edits.
        let mut per_core: HashMap<CoreId, HashMap<u64, Vec<(String, String)>>> = HashMap::new();
        for ((core, id, field), value) in &self.field_edits {
            per_core
                .entry(*core)
                .or_default()
                .entry(*id)
                .or_default()
                .push((field.clone(), value.clone()));
        }
        let b = self.backend.read(cx);
        for (core, strat_edits) in per_core {
            let edits: Vec<(u64, Vec<(String, String)>)> = strat_edits.into_iter().collect();
            if let Err(error) = b.session.edit_strategies(core, edits) {
                log::warn!("edit strategies failed: {error}");
                return;
            }
        }
        self.clear_field_draft();
        cx.notify();
    }

    pub(super) fn discard_field_edits(&mut self, cx: &mut Context<Self>) {
        if self.field_edits.is_empty() {
            return;
        }
        self.clear_field_draft();
        cx.notify();
    }

    pub(super) fn clear_field_draft(&mut self) {
        self.field_edits.clear();
        self.field_inputs.clear();
        self.field_memos.clear();
        self.field_colors.clear();
        self.focused_field = None;
    }

    /// Put a strategy's full name into the search filter and focus the input for Find All by Name.
    /// Writes both the filter and input because `set_value` does not emit Change.
    pub(super) fn search_by_name(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.filter.search = name.clone();
        self.search.update(cx, |st, cx| {
            st.set_value(name, window, cx);
            st.focus(window, cx);
        });
        cx.notify();
    }
}
