//! Connections tab, ported from egui `settings/connections.rs`. It renders market-data and core
//! order selectors above pending cores and a window-group-to-exchange-to-core tree. Group branches
//! provide activation, icon, window, picker, and add-core controls; exchange branches come from
//! each live core's own server identity; core leaves provide editable connection fields, feed
//! flags, color, delete, reconnect, and status controls. Edits update the draft, while live status
//! and reconnect requests use [`Backend`].
//!
//! This module owns per-row editor state through [`ConnRow`] and [`build_conn`], plus group
//! synchronization from servers. [`table`] owns core rows, columns, headers, feed controls, and
//! add/delete actions; [`tab`] owns pending/group/exchange branches, icon picking, selectors, and
//! tab assembly.

mod entries;
mod tab;
mod table;

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicU64, Ordering};

use gpui::*;
use moon_ui::{MoonColorPickerState, MoonInputEvent, MoonInputState};

use super::SettingsView;
use crate::Backend;
use moon_core::config::{GroupConfig, Secret, ServerConfig, ensure_server_group_configs};

pub(super) use entries::ConnEntry;

/// Monotonic source of per-row identity keys.
///
/// A draft `ServerConfig.id` cannot serve as one: `add_server` assigns `max(id) + 1` and
/// `delete_server` removes the row, so deleting the highest-id unsaved row and adding another
/// reissues the same id inside a single Settings session. A reissued key would let a replacement
/// row inherit the deleted row's element identity -- and, worse, its open popup state. This counter
/// only ever goes up, so a key names one row for as long as the process lives.
///
/// Saved rows do not need it for PERSISTED identity: `ServerConfig.uid` is issued by
/// `AppConfig.next_uid` and is never reused. This key is per-session UI identity, nothing more, and
/// is never written to the config.
static NEXT_ROW_KEY: AtomicU64 = AtomicU64::new(1);

/// Component state for one server row's text fields and color picker.
pub(super) struct ConnRow {
    /// Per-session identity for this row's elements and popup state. See [`NEXT_ROW_KEY`].
    pub(super) row_key: u64,
    /// Precomputed per-row element-id strings, built once here rather than with `format!` inside
    /// the row factory on every frame. See [`ConnRowIds`].
    pub(super) ids: ConnRowIds,
    name: Entity<MoonInputState>,
    key: Entity<MoonInputState>,
    group: Entity<MoonInputState>,
    /// AddToChart bundle name; empty delegates to the global setting.
    ///
    /// See `ServerConfig::chart_bundle`.
    bundle: Entity<MoonInputState>,
    color: Entity<MoonColorPickerState>,
}

/// The ten per-row element-id strings the row factory used to rebuild with `format!` on every
/// frame. Built once in [`build_conn`] and read from thereafter, so `server_row` allocates none of
/// them.
///
/// Keyed on row IDENTITY, not draft position: a saved row keys on its `ServerConfig.uid` (stable
/// for the row's whole configured life), an unsaved row on its [`ConnRow::row_key`] (`uid` is zero
/// for every pending row and cannot tell them apart -- see [`row_ident`]).
pub(super) struct ConnRowIds {
    pub(super) name: SharedString,
    pub(super) key: SharedString,
    pub(super) group: SharedString,
    pub(super) bundle: SharedString,
    pub(super) feed: SharedString,
    pub(super) proto: SharedString,
    pub(super) preset: SharedString,
    pub(super) act: SharedString,
    pub(super) win: SharedString,
    pub(super) del: SharedString,
    pub(super) rec: SharedString,
}

/// Build the identity fragment for one row's element IDs.
///
/// A saved row uses `u{uid}`; a pending row uses `s{row_key}` because draft `ServerConfig.id` can
/// be reissued after deletion, while neither `uid` nor `row_key` is reused.
///
/// Args:
///     uid: Persisted row identity, or zero for a pending row.
///     row_key: Monotonic per-session identity for the row.
///
/// Returns:
///     The saved-row or pending-row fragment used by all of the row's element IDs.
fn row_ident(uid: u64, row_key: u64) -> String {
    if uid != 0 {
        format!("u{uid}")
    } else {
        format!("s{row_key}")
    }
}

impl ConnRowIds {
    /// Build every stable element ID for one server-row editor.
    ///
    /// Args:
    ///     uid: Persisted row identity, or zero for a pending row.
    ///     row_key: Monotonic per-session identity for the row.
    ///
    /// Returns:
    ///     The precomputed IDs keyed by the row's stable identity fragment.
    fn build(uid: u64, row_key: u64) -> Self {
        let ident = row_ident(uid, row_key);
        Self {
            name: SharedString::from(format!("name-{ident}")),
            key: SharedString::from(format!("key-{ident}")),
            group: SharedString::from(format!("group-{ident}")),
            bundle: SharedString::from(format!("bundle-{ident}")),
            feed: SharedString::from(format!("feed-{ident}")),
            proto: SharedString::from(format!("proto-{ident}")),
            preset: SharedString::from(format!("preset-{ident}")),
            act: SharedString::from(format!("act-{ident}")),
            win: SharedString::from(format!("win-{ident}")),
            del: SharedString::from(format!("del-{ident}")),
            rec: SharedString::from(format!("rec-{ident}")),
        }
    }
}

/// Add missing group rows to a Settings preview without removing intermediate names.
///
/// Returns whether a row was inserted. Orphan removal stays at `AppConfig::save_impl`, so erasing
/// and retyping a group name cannot replace its Size, TP, SL, S-slot, or stop-market settings.
pub(super) fn sync_groups_from_servers(
    servers: &[ServerConfig],
    groups: &mut Vec<GroupConfig>,
) -> bool {
    ensure_server_group_configs(servers, groups)
}

/// Build a text input bound to a field of draft server `servers[i]`.
///
/// Focusing the field brings its row into view and records it as focused; blurring it, when this
/// is still the focused row, clears that record. `on_conn_visible_range` reads the record to blur a
/// field whose row has scrolled out of the mounted range, so a keystroke can never target an
/// invisible input.
///
/// Args:
///     window: Settings window used to create the input state.
///     cx: Settings context used to subscribe to input events.
///     i: Draft index of the server field.
///     row_key: Per-session identity of the owning row.
///     init: Initial field value.
///     get: Accessor for the draft field.
///     set: Mutator for the draft field.
///     sync_groups: Whether a change must synchronize draft group rows.
///
/// Returns:
///     Input state synchronized with the specified draft-server field.
fn conn_input(
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    i: usize,
    row_key: u64,
    init: String,
    get: fn(&ServerConfig) -> String,
    set: fn(&mut ServerConfig, String),
    sync_groups: bool,
) -> Entity<MoonInputState> {
    let st = cx.new(|cx| MoonInputState::new(window, cx).default_value(init));
    cx.subscribe(&st, move |this, emitter, ev: &MoonInputEvent, cx| {
        if matches!(ev, MoonInputEvent::Change) {
            let val = emitter.read(cx).value().to_string();
            this.backend.update(cx, |b, bcx| {
                if let Some(p) = b.preview.as_mut() {
                    if let Some(s) = p.servers.get_mut(i) {
                        if get(s) != val {
                            set(s, val);
                            if sync_groups {
                                sync_groups_from_servers(&p.servers, &mut p.groups);
                            }
                            bcx.notify();
                        }
                    }
                }
            });
            // This keystroke may have re-ranked the list under the field being typed into; the
            // next frame scrolls this row back into view before anything can evict it.
            this.conn_edit_pending = Some(row_key);
        } else if matches!(ev, MoonInputEvent::Focus) {
            this.focused_conn_row = Some(row_key);
            this.scroll_conn_row_into_view(row_key);
        } else if matches!(ev, MoonInputEvent::Blur) && this.focused_conn_row == Some(row_key) {
            this.focused_conn_row = None;
        }
    })
    .detach();
    st
}

/// Build a color picker bound to draft `servers[i].color`.
fn conn_color(
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    i: usize,
    init: [u8; 3],
) -> Entity<MoonColorPickerState> {
    super::draft_color(window, cx, init, move |p, c| {
        if let Some(s) = p.servers.get_mut(i) {
            if s.color != c {
                s.color = c;
                return true;
            }
        }
        false
    })
}

/// Build per-server editor state from draft servers.
///
/// Called from `SettingsView::new` and after adding or removing a server so subscriptions capture
/// current indices.
///
/// Args:
///     backend: Settings backend that owns the draft configuration.
///     window: Settings window used to create input and picker states.
///     cx: Settings context used to install change subscriptions.
///
/// Returns:
///     Editor state for every current draft server, keyed by its source index.
pub(super) fn build_conn(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> Vec<ConnRow> {
    let servers = {
        let b = backend.read(cx);
        b.preview.as_ref().unwrap_or(&b.config).servers.clone()
    };
    servers
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let row_key = NEXT_ROW_KEY.fetch_add(1, Ordering::Relaxed);
            ConnRow {
                row_key,
                ids: ConnRowIds::build(s.uid, row_key),
                name: conn_input(
                    window,
                    cx,
                    i,
                    row_key,
                    s.name.clone(),
                    |s| s.name.clone(),
                    |s, v| s.name = v,
                    false,
                ),
                // Treat the key as a password field, matching egui's `.password(true)`: mask its
                // characters and provide `mask_toggle` for optional visibility.
                key: {
                    let st = conn_input(
                        window,
                        cx,
                        i,
                        row_key,
                        s.key.expose().to_string(),
                        |s| s.key.expose().to_string(),
                        |s, v| {
                            // Typing or Ctrl+V into the field fills a row's transport mode the
                            // same way the Paste glyph does, and by the same once-only rule: this
                            // runs per KEYSTROKE, so anything that re-reads an already-set mode
                            // would rewrite it mid-edit. See `config::seeded_transport`.
                            s.transport = moon_core::config::seeded_transport(s.transport, &v);
                            s.key = Secret::new(v);
                            // A new key can point this row at a DIFFERENT Moonbot, where strategy
                            // ids mean something else, so the pinned id is void. The NAME survives
                            // and re-pins itself against the new host's list.
                            if let Some(manual) = s.manual_strategy.as_mut() {
                                manual.id = 0;
                            }
                        },
                        false,
                    );
                    st.update(cx, |st, c| st.set_masked(true, window, c));
                    st
                },
                group: conn_input(
                    window,
                    cx,
                    i,
                    row_key,
                    s.group.clone(),
                    |s| s.group.clone(),
                    |s, v| s.group = v,
                    true,
                ),
                bundle: conn_input(
                    window,
                    cx,
                    i,
                    row_key,
                    s.chart_bundle.clone(),
                    |s| s.chart_bundle.clone(),
                    |s, v| s.chart_bundle = v,
                    false,
                ),
                color: conn_color(window, cx, i, s.color),
            }
        })
        .collect()
}

impl SettingsView {
    /// Find the last flattened list position for a row key.
    ///
    /// Args:
    ///     row_key: Per-session identity of the draft row to locate.
    ///
    /// Returns:
    ///     The matching `CoreRow` position, or `None` when the row is not in the cached entries.
    fn conn_entry_index(&self, row_key: u64) -> Option<usize> {
        self.conn_entries.iter().position(|e| match e {
            ConnEntry::CoreRow { draft_index, .. } => self
                .conn
                .get(*draft_index)
                .is_some_and(|r| r.row_key == row_key),
            _ => false,
        })
    }

    /// Scroll a Connections row into view when its input gains keyboard focus.
    ///
    /// Args:
    ///     row_key: Per-session identity of the row to reveal.
    ///
    /// Returns:
    ///     Nothing; updates the virtual-list scroll target when the row is present.
    fn scroll_conn_row_into_view(&mut self, row_key: u64) {
        if let Some(pos) = self.conn_entry_index(row_key) {
            self.conn_scroll.scroll_to_item(pos, ScrollStrategy::Top);
        }
    }

    /// Keep the row the user is EDITING on screen.
    ///
    /// The list order is a pure function of the draft plus the sort mode, so a keystroke in a name
    /// or group field can re-rank the list and carry the very row being typed into out of the
    /// mounted range -- where [`Self::on_conn_visible_range`] would blur it and end the edit after
    /// one character.
    ///
    /// Keyed on the EDIT, deliberately, and not on a change in the list ORDER. Order is the wrong
    /// trigger: the entry sequence also moves for reasons that have nothing to do with the user's
    /// hands -- a venue resolving for some other core re-buckets it and shifts every index below --
    /// and `scroll_to_item` is non-strict, measured against the offset the user's own wheel has
    /// already produced this frame (`moon-gpui/src/elements/uniform_list.rs`). Order-triggered, an
    /// unrelated reshuffle therefore yanked the viewport back to a merely-focused row while the user
    /// was calmly scrolling somewhere else. Tied to the edit, nothing fires unless a field actually
    /// changed, and a row that is already visible costs nothing because the scroll is a no-op.
    ///
    /// Runs while `connections_tab` assembles the frame, BEFORE the list lays out, so the scroll is
    /// already applied by the time a visible range is computed.
    ///
    /// Returns:
    ///     Nothing; consumes the pending edit and queues that row's scroll when one is recorded.
    fn follow_edited_conn_row(&mut self) {
        if let Some(key) = self.conn_edit_pending.take() {
            self.scroll_conn_row_into_view(key);
        }
    }

    /// Handle a visible-range change from the Connections virtual list.
    ///
    /// On every visible-range change: blur a focused input whose row scrolled out of range, so a
    /// keystroke can never target an unmounted field; close the feed-flag menu, the transport-mode
    /// menu or the icon picker when the row or group heading that owns it left the range, so
    /// scrolling back cannot resurrect a controlled popup nothing dismissed.
    ///
    /// Args:
    ///     range: Currently mounted entry range.
    ///     window: Settings window used to blur an evicted input.
    ///     cx: Settings context used to request a repaint after state changes.
    ///
    /// Returns:
    ///     Nothing; evicts focused or open row-owned state outside the mounted range.
    pub(super) fn on_conn_visible_range(
        &mut self,
        range: std::ops::Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        let visible_at = |pos: Option<usize>| pos.is_some_and(|pos| range.contains(&pos));

        if let Some(key) = self.focused_conn_row {
            let visible = visible_at(self.conn_entry_index(key));
            if !visible {
                window.blur();
                self.focused_conn_row = None;
                changed = true;
            }
        }

        if let Some(key) = self.feed_open {
            let visible = visible_at(self.conn_entry_index(key));
            if !visible {
                self.feed_open = None;
                changed = true;
            }
        }

        if let Some(key) = self.proto_open {
            let visible = visible_at(self.conn_entry_index(key));
            if !visible {
                self.proto_open = None;
                changed = true;
            }
        }

        if let Some(key) = self.preset_open {
            let visible = visible_at(self.conn_entry_index(key));
            if !visible {
                self.preset_open = None;
                changed = true;
            }
        }

        if let Some(name) = self.picking.clone() {
            let visible =
                visible_at(self.conn_entries.iter().position(
                    |e| matches!(e, ConnEntry::GroupHeader { name: n, .. } if *n == name),
                ));
            if !visible {
                self.picking = None;
                changed = true;
            }
        }

        if changed {
            cx.notify();
        }
    }
}
