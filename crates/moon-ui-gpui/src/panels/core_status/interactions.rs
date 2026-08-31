//! Core Status user-interaction handlers: chart-span and server selection, tree expansion, the core
//! multi-select filter, inline server rename, sort, and presentation mode. Split out of `mod.rs` as
//! the mutation half of the panel, distinct from its render-cache pipeline and its rendering.

use std::collections::HashSet;
use std::net::IpAddr;

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonInputEvent, MoonInputState, MoonPalette,
    MoonWindowExt as _, h_flex,
};

use super::by_ip_header::ByIpDragAnchor;
use super::by_ip_widths::{ByIpCol, MAX_COL_W, MIN_COL_W};
use super::model::ServerKey;
use super::{ChartWindow, CoreStatusMode, CoreStatusView};
use crate::design;
use moon_core::feed::ConnStatus;
use moon_core::session::CoreId;
use moon_core::session::core_update::CoreUpdatePhase;
use rust_i18n::t;

impl CoreStatusView {
    /// Record where a By IP header divider drag began.
    ///
    /// Called once per drag, from the handle's `on_drag` constructor, which GPUI runs a single time
    /// after the drag threshold is crossed. The anchor is in LOGICAL (pre-shrink) width space — see
    /// [`Self::drag_by_ip_col`] for why that matters.
    ///
    /// Args:
    ///     anchor: The column, the pointer x, and the column's logical width at the grab.
    ///
    /// Returns:
    ///     Nothing; no repaint, because nothing has moved yet.
    pub(super) fn begin_by_ip_resize(&mut self, anchor: ByIpDragAnchor) {
        self.by_ip_drag = Some(anchor);
    }

    /// Apply a live By IP divider drag: the anchored width plus the pointer's travel since the grab.
    ///
    /// Deliberately NOT `pointer_x - cell_origin_x`, which is how MoonUI's data table does it. The
    /// By IP header puts a `flex_1` spacer between IP and CPU, so every column right of it is
    /// right-anchored: widening one moves its OWN left edge left by the same amount, and the next
    /// event measures against the moved origin. That loop triples the sensitivity per frame. An
    /// anchor captured once at the grab is immune to relayout.
    ///
    /// The anchor is the LOGICAL width, so on a shrunk panel the column tracks the pointer at the
    /// shrink factor's speed rather than 1:1. That is the correct trade: anchoring on the PAINTED
    /// width would write back an already-scaled value, the resolver would scale it a second time,
    /// and the column would jump narrower on the first pixel of the drag.
    ///
    /// Args:
    ///     col: Column whose divider is being dragged.
    ///     pointer_x: Current pointer x, in window pixels.
    ///     cx: View context; the width bag's observer persists and repaints.
    ///
    /// Returns:
    ///     Nothing. A drag for a different column than the live anchor, or with no anchor at all, is
    ///     ignored rather than guessed at.
    pub(super) fn drag_by_ip_col(&mut self, col: ByIpCol, pointer_x: f32, cx: &mut Context<Self>) {
        let Some(anchor) = self.by_ip_drag else {
            return;
        };
        if anchor.col != col {
            return;
        }
        let width = (anchor.width + (pointer_x - anchor.mouse_x)).clamp(MIN_COL_W, MAX_COL_W);
        self.by_ip_col_widths.update(cx, |state, cx| {
            if state.column_widths.get(col.key()).copied() == Some(width) {
                return;
            }
            state.set_column_width(col.key(), width);
            cx.notify();
        });
    }

    /// Restore automatic width for one By IP column, or for every one of them.
    ///
    /// Mirrors the `MoonDataTable` divider gesture so the two views answer the same input: a plain
    /// double-click drops this column back to its design width, Shift+double-click drops all of them
    /// (the toolbar button's equivalent, and the only route to it in a detached window — see
    /// `toolbar_buttons`).
    ///
    /// An already-clear bag changes nothing and must NOT notify: the observer would otherwise arm
    /// `layout_dirty` and schedule a layout write for a double-click that did nothing.
    ///
    /// Args:
    ///     col: Column to reset when `all` is false.
    ///     all: Reset every column instead of just `col`.
    ///     cx: View context; the width bag's observer persists and repaints.
    ///
    /// Returns:
    ///     Nothing.
    pub(super) fn reset_by_ip_col(&mut self, col: ByIpCol, all: bool, cx: &mut Context<Self>) {
        self.by_ip_col_widths.update(cx, |state, cx| {
            let changed = if all {
                let had_any = !state.column_widths.is_empty();
                state.column_widths.clear();
                had_any
            } else {
                state.column_widths.remove(col.key()).is_some()
            };
            if changed {
                cx.notify();
            }
        });
    }

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
        let next =
            super::ordering::restore_flat_sort(Some(moon_core::config::TableSortPreference {
                column: key.to_string(),
                ascending,
            }));
        if self.flat_sort != next {
            self.flat_sort = next;
            let preference = self.flat_sort.as_ref().map(|(column, ascending)| {
                moon_core::config::TableSortPreference {
                    column: column.clone(),
                    ascending: *ascending,
                }
            });
            crate::persistence::table_persist::set_sort(
                &self.backend,
                &self.widths_id,
                preference,
                cx,
            );
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
        let preference =
            (self.group_sort != (super::ordering::GroupSortField::Name, true)).then(|| {
                moon_core::config::TableSortPreference {
                    column: self.group_sort.0.key().to_string(),
                    ascending: self.group_sort.1,
                }
            });
        let id = crate::persistence::table_persist::ctx_id("core-status-by-ip", self.detached);
        crate::persistence::table_persist::set_sort(&self.backend, &id, preference, cx);
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

    /// Hide or show the whole By-IP address column.
    ///
    /// One state for the whole column, not one per server: a fleet of servers is masked or shown in
    /// a single click, and the panel does not have to hold — or leak — which rows a user happened to
    /// open. The state is transient and never persisted, so it lasts only as long as this panel.
    ///
    /// Deliberately NOT tied to focus. An earlier per-row reveal was cleared by the panel's blur,
    /// and a docked panel loses focus on nearly any click, so an address vanished as fast as it
    /// appeared.
    ///
    /// Args:
    ///     cx: View context used to repaint.
    ///
    /// Returns:
    ///     Nothing; the mask flag is transient and never persisted.
    pub(super) fn toggle_ip_mask(&mut self, cx: &mut Context<Self>) {
        self.ip_masked = !self.ip_masked;
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

    /// Switch the Core Status presentation.
    ///
    /// The choice is remembered per host context, so this panel reopens in the same mode after a
    /// dock rebuild or a restart; a docked tab and a detached window keep their own selections.
    ///
    /// Args:
    ///     mode: Requested presentation.
    ///     cx: View context used to repaint.
    ///
    /// Returns:
    ///     Nothing; the changed mode is persisted while data caches and visibility state are retained.
    pub(super) fn set_mode(&mut self, mode: CoreStatusMode, cx: &mut Context<Self>) {
        if self.mode != mode {
            self.mode = mode;
            // Inside the change gate on purpose: re-selecting the current mode writes nothing and
            // cannot arm a layout flush, matching the width and sort maps beside it.
            crate::persistence::table_persist::set_core_status_mode(
                &self.backend,
                &super::mode_ctx_id(self.detached),
                mode.code(),
                cx,
            );
            cx.notify();
        }
    }

    /// Best-effort preview of what a fleet-wide bulk update is about to enqueue, read from public
    /// session data rather than the private `SessionManager::eligible` gate it mirrors.
    ///
    /// Deliberately fleet-wide, over the whole store rather than this panel's own scope: the
    /// engine's `cores_behind` and `fleet_newest_version` are already store-wide so that two
    /// differently scoped panels never disagree, and a bulk action started from either one must
    /// preview the same fleet it is about to enqueue against.
    ///
    /// Args:
    ///     only_behind: Whether to preview `cores_behind()` or every core the enqueue gate would
    ///         currently accept.
    ///     cx: View context used to read the backend snapshot.
    ///
    /// Returns:
    ///     `(core count, distinct lane/server count)` for the confirm question. An exact match to
    ///     the engine's own gate is not the point -- wording the question honestly before the
    ///     press is.
    fn fleet_update_preview(&self, only_behind: bool, cx: &Context<Self>) -> (usize, usize) {
        let b = self.backend.read(cx);
        let store = b.session.store();
        let mut lanes: HashSet<IpAddr> = HashSet::new();
        let count = if only_behind {
            let ids = b.session.cores_behind();
            for id in &ids {
                if let Some(address) = store
                    .core(*id)
                    .and_then(|data| data.endpoint)
                    .map(|endpoint| endpoint.address)
                {
                    lanes.insert(address);
                }
            }
            ids.len()
        } else {
            let mut n = 0usize;
            for (id, data) in store.cores() {
                if data.status != ConnStatus::Ready || data.server_version.is_none() {
                    continue;
                }
                let Some(endpoint) = data.endpoint else {
                    continue;
                };
                let in_flight = matches!(
                    b.session.core_update_phase(id),
                    Some(phase) if !matches!(phase, CoreUpdatePhase::Done(_))
                );
                if in_flight {
                    continue;
                }
                n += 1;
                lanes.insert(endpoint.address);
            }
            n
        };
        (count, lanes.len())
    }

    /// Open the ONE confirm a fleet-wide bulk update gets, naming the core and lane counts before
    /// the press that fills the whole per-IP queue.
    ///
    /// Args:
    ///     only_behind: Forwarded to `update_fleet` on confirmation: only cores behind the fleet's
    ///         newest build, or every core the enqueue gate accepts.
    ///     window: Window that owns the unique dialog.
    ///     cx: View context used to read the preview counts and build the dialog.
    ///
    /// Returns:
    ///     Nothing; only the Yes button reaches `update_fleet`, and it closes the dialog either way.
    pub(super) fn confirm_fleet_update(
        &mut self,
        only_behind: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (core_count, lane_count) = self.fleet_update_preview(only_behind, cx);
        let backend = self.backend.clone();
        window.open_unique_moon_dialog(
            "core-status-fleet-update-confirm",
            cx,
            move |dialog, _window, cx| {
                let p = MoonPalette::active(cx);
                let confirm_backend = backend.clone();
                let question = t!(
                    "core_update.confirm.q",
                    cores = core_count,
                    servers = lane_count
                )
                .to_string();
                dialog
                    .w(px(360.0))
                    .close_button(true)
                    .overlay(true)
                    .overlay_closable(true)
                    .bg(rgb(p.shell_high))
                    .border_color(rgb(p.border))
                    .rounded(design::r_container(cx))
                    .text_color(rgb(p.text))
                    .header(
                        div()
                            .w_full()
                            .py_2()
                            .border_b_1()
                            .border_color(rgb(p.border))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("core_update.confirm.title").to_string()),
                    )
                    .content(move |content, _window, cx| {
                        let p = MoonPalette::active(cx);
                        content.child(
                            div()
                                .font_family(design::mono())
                                .text_size(design::t_body(cx))
                                .text_color(rgb(p.text))
                                .child(question.clone()),
                        )
                    })
                    .footer(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .justify_end()
                            .child(
                                MoonButton::new("core-status-fleet-confirm-no")
                                    .outline()
                                    .size(MoonButtonSize::Action)
                                    .label(format!("  {}  ", t!("dialogs.no")))
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                    })
                                    .render(),
                            )
                            .child(
                                MoonButton::new("core-status-fleet-confirm-yes")
                                    .size(MoonButtonSize::Action)
                                    .variant(MoonButtonVariant::Danger)
                                    .label(format!("  {}  ", t!("dialogs.yes")))
                                    .on_click(move |_, window, cx| {
                                        crate::controls::core_update::update_fleet(
                                            &confirm_backend,
                                            only_behind,
                                            cx,
                                        );
                                        window.close_dialog(cx);
                                    })
                                    .render(),
                            ),
                    )
            },
        );
    }
}
