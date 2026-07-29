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
        if self.chart_server != Some(key) {
            self.chart_server = Some(key);
            cx.notify();
        }
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

    /// Toggle one core in the multi-select filter, or toggle the All item.
    ///
    /// `Some(id)` toggles one core. `None` clears a selection containing every non-empty scoped
    /// core, otherwise replacing it with that full set. Stale ids do not stand in for current cores.
    ///
    /// Args:
    ///     id: Core to toggle, or `None` for the All row.
    ///     cx: View context used to rebuild cached rows and request a repaint.
    ///
    /// Returns:
    ///     Nothing; the in-memory filter and cached rows are updated in place.
    pub(super) fn toggle_core(&mut self, id: Option<CoreId>, cx: &mut Context<Self>) {
        let all: HashSet<CoreId> = self
            .scope_cores(self.backend.read(cx))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        match id {
            None => crate::controls::toggle_all_core_selection(&mut self.sel_cores, all),
            Some(id) => {
                if !self.sel_cores.remove(&id) {
                    self.sel_cores.insert(id);
                }
            }
        }
        self.rebuild_cache(cx);
        cx.notify();
    }

    /// Toggle every still-available core from one clicked exchange section.
    ///
    /// Empty means All before the click, so the first exchange selection becomes explicit. A
    /// fully selected exchange is removed without changing selections from other exchanges.
    /// Rendered ids that left this panel's group are ignored.
    ///
    /// Args:
    ///     exchange_cores: Core ids captured from one rendered exchange section.
    ///     cx: View context used to rebuild cached rows and request a repaint.
    ///
    /// Returns:
    ///     Nothing; a changed selection rebuilds the cache once, while a stale-only batch is a
    ///     no-op.
    pub(super) fn toggle_exchange_cores(
        &mut self,
        exchange_cores: Vec<CoreId>,
        cx: &mut Context<Self>,
    ) {
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
