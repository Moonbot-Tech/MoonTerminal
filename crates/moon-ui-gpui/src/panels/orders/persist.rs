//! Per-context column persistence, view restore, and the mutate/persist helpers.

use super::*;

impl OrdersPanel {
    /// Reconstruct the panel from `docks.json` by applying the `PanelInfo` view state after `new`.
    ///
    /// Sorting, order kind, current-market filtering, and column settings are restored. The core
    /// selection is intentionally not persisted and therefore starts as all cores, matching the
    /// original egui product behavior.
    pub fn restored(
        backend: Entity<Backend>,
        group: String,
        info: &PanelInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::new(backend, group, window, cx);
        this.view = view_from_info(info);
        // A per-context visible-column set in shared storage overrides the legacy `docks.json` set.
        // Without one, the legacy set remains as a migration seed until the first column toggle
        // writes it to shared storage.
        this.apply_ctx_columns(cx);
        // Persist drag-defined column order separately because it is not part of the copyable view.
        let order = column_order_from_info(info);
        if !order.is_empty() {
            this.col_order_cache = order.clone();
            this.table_state.update(cx, |s, _| s.column_order = order);
        }
        this
    }

    /// Apply a saved per-context visible-column set for `widths_id`, when valid.
    ///
    /// Docked `:dock` and detached `:win` modes use distinct shared-storage entries. An empty or
    /// invalid entry with no recognized keys leaves the current view intact rather than producing
    /// an empty table.
    pub(super) fn apply_ctx_columns(&mut self, cx: &App) {
        if let Some(keys) =
            crate::persistence::table_persist::visible(self.backend.read(cx), &self.widths_id)
        {
            let mask = keys
                .iter()
                .filter_map(|k| OrdCol::from_key(k))
                .fold(0u32, |m, c| m | c.bit());
            if mask != 0 {
                self.view.columns = mask;
            }
        }
    }

    /// Save the current visible-column set in per-context storage under `widths_id`.
    ///
    /// Called by [`Self::mutate`] when the visibility mask changes.
    pub(super) fn save_ctx_columns(&self, cx: &mut App) {
        let keys: Vec<String> = self
            .view
            .visible_columns()
            .iter()
            .map(|c| c.key().to_string())
            .collect();
        crate::persistence::table_persist::set_visible(&self.backend, &self.widths_id, keys, cx);
    }

    /// Mutate the copyable view state and, only when it changes, rebuild, repaint, and persist it.
    ///
    /// Dock dumping occurs at the `App` level after releasing the panel borrow because
    /// `dock.dump()` reads this panel and would otherwise re-enter it.
    pub(super) fn mutate(view: &Entity<Self>, app: &mut App, f: impl FnOnce(&mut OrdersViewState)) {
        let changed = view.update(app, |this, cx| {
            let mut next = this.view;
            f(&mut next);
            if next != this.view {
                let cols_changed = next.columns != this.view.columns;
                this.view = next;
                let backend = this.backend.clone();
                this.rebuild_cache(backend.read(cx));
                // Persist a changed visible-column set through the shared per-context descriptor.
                if cols_changed {
                    this.save_ctx_columns(cx);
                }
                cx.notify();
                true
            } else {
                false
            }
        });
        if changed {
            Self::persist(view, app);
        }
    }

    /// Dump the current dock layout to backend state for `docks.json` persistence.
    ///
    /// Order-view changes emit no `DockEvent`, so this panel persists them directly to prevent the
    /// sort and filter state from resetting when reopened.
    pub(super) fn persist(view: &Entity<Self>, app: &mut App) {
        let (dock, group, backend) = {
            let p = view.read(app);
            (p.dock.clone(), p.group.clone(), p.backend.clone())
        };
        let Some(dock) = dock.and_then(|d| d.upgrade()) else {
            return;
        };
        let state = dock.read(app).dump(app);
        backend.update(app, |b, _| {
            b.store_classic_dock_state(group, state);
        });
    }
}

/// Restore persisted view fields from `PanelInfo`, keeping defaults for absent fields.
///
/// The core selection is stored outside [`OrdersViewState`] and intentionally omitted from
/// persistence, so restored panels start with all cores.
fn view_from_info(info: &PanelInfo) -> OrdersViewState {
    let mut v = OrdersViewState::default();
    if let PanelInfo::Panel(j) = info {
        if let Some(p) = j.get("primary").and_then(|x| x.as_u64()) {
            v.primary = PrimarySort::from_u8(p as u8);
        }
        if let Some(k) = j.get("kind").and_then(|x| x.as_u64()) {
            v.kind = OrderKind::from_u8(k as u8);
        }
        if let Some(n) = j.get("newest_first").and_then(|x| x.as_bool()) {
            v.newest_first = n;
        }
        if let Some(o) = j.get("only_current").and_then(|x| x.as_bool()) {
            v.only_current_market = o;
        }
        if let Some(m) = j.get("main_on_top").and_then(|x| x.as_u64()) {
            v.main_on_top = MainOnTop::from_u8(m as u8);
        }
        // Convert visible-column keys to a mask. A missing or empty list, or one containing no valid
        // keys, retains the all-visible default instead of producing an empty table.
        if let Some(arr) = j.get("columns").and_then(|x| x.as_array()) {
            let mask = arr
                .iter()
                .filter_map(|x| x.as_str())
                .filter_map(OrdCol::from_key)
                .fold(0u32, |m, c| m | c.bit());
            if mask != 0 {
                v.columns = mask;
            }
        }
    }
    v
}

/// Read drag-defined column order from `PanelInfo` as a list of recognized [`OrdCol`] keys.
///
/// Ignoring unknown keys tolerates stale or malformed persistence data. An empty result leaves the
/// table's default [`OrdCol::ALL`] order in effect.
fn column_order_from_info(info: &PanelInfo) -> Vec<SharedString> {
    let PanelInfo::Panel(j) = info else {
        return Vec::new();
    };
    j.get("column_order")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str())
                .filter(|s| OrdCol::from_key(s).is_some())
                .map(SharedString::from)
                .collect()
        })
        .unwrap_or_default()
}
