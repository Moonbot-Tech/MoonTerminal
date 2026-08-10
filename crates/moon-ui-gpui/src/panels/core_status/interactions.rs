//! Core Status user-interaction handlers: chart-span and server selection, tree expansion, the core
//! multi-select filter, inline server rename, sort, and presentation mode. Split out of `mod.rs` as
//! the mutation half of the panel, distinct from its render-cache pipeline and its rendering.

use std::collections::HashSet;

use gpui::*;
use moon_ui::{MoonInputEvent, MoonInputState};

use super::model::ServerKey;
use super::{ChartWindow, CoreStatusMode, CoreStatusView};
use moon_core::session::CoreId;

impl CoreStatusView {
    /// Switch the detached-window chart span and repaint.
    ///
    /// Args:
    ///     window: Requested X-axis span.
    ///     cx: View context used to repaint.
    ///
    /// Returns:
    ///     Nothing; only the span and a repaint change.
    pub(super) fn set_chart_window(&mut self, window: ChartWindow, cx: &mut Context<Self>) {
        if self.chart_window != window {
            self.chart_window = window;
            cx.notify();
        }
    }

    /// Select which server's chart the detached window shows (from a server-row click).
    ///
    /// Args:
    ///     key: Clicked server identity.
    ///     cx: View context used to repaint.
    ///
    /// Returns:
    ///     Nothing; only the selection and a repaint change.
    pub(super) fn select_chart_server(&mut self, key: ServerKey, cx: &mut Context<Self>) {
        // A server-row click charts the machine aggregate, so any per-core selection is cleared.
        if self.chart_server != Some(key) || self.chart_core.is_some() {
            self.chart_server = Some(key);
            self.chart_core = None;
            cx.notify();
        }
    }

    /// Chart one specific core (from a core-row click in the expanded server list) instead of the
    /// server aggregate. Clicking the already-charted core clears it, reverting to the server.
    ///
    /// Args:
    ///     id: Clicked core identity.
    ///     cx: View context used to repaint.
    ///
    /// Returns:
    ///     Nothing; only the selection and a repaint change.
    pub(super) fn select_chart_core(&mut self, id: CoreId, cx: &mut Context<Self>) {
        self.chart_core = if self.chart_core == Some(id) {
            None
        } else {
            Some(id)
        };
        cx.notify();
    }

    /// Toggle one server's expansion from a chevron click (the headless tree does not do it).
    ///
    /// Args:
    ///     key: Server identity whose folder to expand or collapse.
    ///     cx: View context used to update the tree state and repaint.
    ///
    /// Returns:
    ///     Nothing; the tree's expanded set flips for this server.
    pub(super) fn toggle_server_expand(&mut self, key: ServerKey, cx: &mut Context<Self>) {
        let id = SharedString::from(key.tree_id());
        self.tree_state.update(cx, |state, cx| {
            let mut ids = state.expanded_ids().into_iter().collect::<HashSet<_>>();
            if !ids.remove(&id) {
                ids.insert(id);
            }
            state.set_expanded(ids, cx);
        });
        cx.notify();
    }

    /// Apply a flat-table header sort from a column click.
    ///
    /// Args:
    ///     key: Column key from the table header.
    ///     ascending: Whether the click requested ascending order.
    ///     cx: View context used to repaint with the new order.
    ///
    /// Returns:
    ///     Nothing; only the sort state and a repaint change.
    pub(super) fn set_flat_sort(&mut self, key: &str, ascending: bool, cx: &mut Context<Self>) {
        let next = Some((key.to_string(), ascending));
        if self.flat_sort != next {
            self.flat_sort = next;
            cx.notify();
        }
    }

    /// Apply a By IP header sort from a column click: flip direction on the active column, else select
    /// the newly clicked column ascending. Warnings still pin to the top (enforced in `rebuild_cache`).
    ///
    /// Args:
    ///     field: The column the header click chose.
    ///     cx: View context; the cache is rebuilt so the tree reorders, then a repaint is requested.
    ///
    /// Returns:
    ///     Nothing; the group sort state, group order, and tree change.
    pub(super) fn set_group_sort(
        &mut self,
        field: super::ordering::GroupSortField,
        cx: &mut Context<Self>,
    ) {
        let (current, ascending) = self.group_sort;
        self.group_sort = if current == field {
            (field, !ascending)
        } else {
            (field, true)
        };
        self.rebuild_cache(cx);
        cx.notify();
    }

    /// Toggle one core in the retained Classic filter, or toggle its All item.
    ///
    /// `Some(id)` toggles one core. `None` clears the explicit selection back to the
    /// empty-means-all state. Auto mode owns and pins the effective scope, so this method becomes a
    /// no-op.
    ///
    /// Args:
    ///     id: Core to toggle, or `None` for the All row.
    ///     cx: View context used to rebuild cached rows and request a repaint.
    ///
    /// Returns:
    ///     Nothing; Classic updates the retained filter and cache, while Auto changes neither.
    pub(super) fn toggle_core(&mut self, id: Option<CoreId>, cx: &mut Context<Self>) {
        if self
            .effective_scope(self.backend.read(cx))
            .is_workspace_owned()
        {
            return;
        }
        if !crate::controls::toggle_core_selection(&mut self.sel_cores, id) {
            return;
        }
        self.rebuild_cache(cx);
        cx.notify();
    }

    /// Replace the retained Classic filter with the one the Profit Monitor broadcast.
    ///
    /// `apply_core_broadcast` owns the release / ignore / intersect rule shared by every adopting
    /// panel. The retained set is written even under Auto, where it is dormant, so a later switch
    /// back to Classic shows the terminal's current core focus rather than a filter from before;
    /// only the rebuild is skipped there.
    ///
    /// Args:
    ///     cx: View context used to rebuild cached rows and request a repaint.
    ///
    /// Returns:
    ///     Nothing; a broadcast about other scopes and an unchanged selection both rebuild nothing.
    pub(super) fn adopt_broadcast_core_filter(&mut self, cx: &mut Context<Self>) {
        let broadcast = self.backend.read(cx).core_filter().clone();
        // Nothing published and nothing retained: leave before paying for the scope's core list.
        if broadcast.is_empty() && self.sel_cores.is_empty() {
            return;
        }
        let available: Vec<CoreId> = self
            .scope_cores(self.backend.read(cx))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        if !crate::controls::apply_core_broadcast(&mut self.sel_cores, &broadcast, available) {
            return;
        }
        if self
            .effective_scope(self.backend.read(cx))
            .is_workspace_owned()
        {
            return;
        }
        self.rebuild_cache(cx);
        cx.notify();
    }

    /// Toggle every still-available core from one exchange section in the Classic filter.
    ///
    /// Empty means All before the click, so the first exchange selection becomes explicit. A
    /// fully selected exchange is removed without changing selections from other exchanges.
    /// Rendered ids that left this panel's group are ignored. Auto mode leaves the retained Classic
    /// selection and cache unchanged.
    ///
    /// Args:
    ///     exchange_cores: Core ids captured from one rendered exchange section.
    ///     cx: View context used to rebuild cached rows and request a repaint.
    ///
    /// Returns:
    ///     Nothing; a Classic change rebuilds once, while stale-only and Auto-owned calls are
    ///     no-ops.
    pub(super) fn toggle_exchange_cores(
        &mut self,
        exchange_cores: Vec<CoreId>,
        cx: &mut Context<Self>,
    ) {
        if self
            .effective_scope(self.backend.read(cx))
            .is_workspace_owned()
        {
            return;
        }
        let available = self
            .scope_cores(self.backend.read(cx))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        if crate::controls::toggle_exchange_cores(&mut self.sel_cores, &available, exchange_cores) {
            self.rebuild_cache(cx);
            cx.notify();
        }
    }

    /// Toggle the momentary IP reveal for one server.
    ///
    /// Revealing focuses the panel so the [`Context::on_blur`] handler re-masks the IP when focus
    /// later leaves the panel. Hiding is immediate.
    ///
    /// Args:
    ///     key: Server identity whose IP should be shown or hidden.
    ///     window: Host window used to move focus to the panel on reveal.
    ///     cx: View context used to repaint.
    ///
    /// Returns:
    ///     Nothing; the reveal set is transient and never persisted.
    pub(super) fn toggle_reveal(
        &mut self,
        key: ServerKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.revealed_ips.remove(&key) {
            self.revealed_ips.insert(key);
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    /// Begin inline renaming of one server, seeding the field with its current display name.
    ///
    /// Args:
    ///     key: Server identity being renamed. Only address servers persist a name.
    ///     current: Current display name used as the initial field value.
    ///     window: Host window used to build and focus the input state.
    ///     cx: View context used to create the input entity and subscribe to its commit events.
    ///
    /// Returns:
    ///     Nothing; the input entity and its subscription live until the edit commits or cancels.
    pub(super) fn start_rename(
        &mut self,
        key: ServerKey,
        current: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = cx.new(|cx| MoonInputState::new(window, cx).default_value(current));
        cx.subscribe(&state, move |this, state, event: &MoonInputEvent, cx| {
            if matches!(
                event,
                MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }
            ) {
                let value = state.read(cx).value().to_string();
                this.commit_rename(key, value, cx);
            }
        })
        .detach();
        self.edit_input = Some(state);
        self.editing = Some(key);
        cx.notify();
    }

    /// Commit or clear a server's custom name, then re-resolve display names.
    ///
    /// An empty name removes the custom entry so the default `Server N` ordinal returns. Only
    /// address servers persist; an unknown-endpoint edit simply closes.
    ///
    /// Args:
    ///     key: Server identity whose name is being committed.
    ///     text: New name from the input; trimmed, with empty meaning "reset to default".
    ///     cx: View context used to update the backend layout and rebuild caches.
    ///
    /// Returns:
    ///     Nothing; a persisted change marks the layout dirty for the shared save path.
    fn commit_rename(&mut self, key: ServerKey, text: String, cx: &mut Context<Self>) {
        if self.editing != Some(key) {
            return;
        }
        if let ServerKey::Address(address) = key {
            let ip = address.to_string();
            let text = text.trim().to_string();
            self.backend.update(cx, |b, _| {
                let changed = if text.is_empty() {
                    b.layout.core_server_names.remove(&ip).is_some()
                } else if b.layout.core_server_names.get(&ip) != Some(&text) {
                    b.layout.core_server_names.insert(ip, text);
                    true
                } else {
                    false
                };
                if changed {
                    b.layout_dirty = true;
                }
            });
        }
        self.editing = None;
        self.edit_input = None;
        self.rebuild_cache(cx);
        cx.notify();
    }

    /// Switch between grouped and flat presentations.
    ///
    /// Args:
    ///     mode: Requested presentation.
    ///     cx: View context used to repaint.
    ///
    /// Returns:
    ///     Nothing; data caches and visibility state are retained.
    pub(super) fn set_mode(&mut self, mode: CoreStatusMode, cx: &mut Context<Self>) {
        if self.mode != mode {
            self.mode = mode;
            cx.notify();
        }
    }
}
