//! Connections tab, ported from egui `settings/connections.rs`. It renders market-data and core
//! order selectors above pending cores and a window-group-to-exchange-to-core tree. Group branches
//! provide activation, icon, window, picker, and add-core controls; exchange branches come from
//! each live core's own server identity; core leaves provide editable connection fields, feed
//! flags, color, delete, reconnect, and status controls. Edits update the draft, while live status
//! and reconnect requests use [`Backend`].
//!
//! This module owns per-row editor state through [`ConnRow`] and [`build_conn`], plus group
//! synchronization from servers. [`columns`] owns the ONE column specification the header and the
//! core rows are both built from; [`table`] owns core rows, headers, feed controls, and add/delete
//! actions; [`tab`] owns pending/group/exchange branches, icon picking, selectors, and tab
//! assembly. [`endpoints`] derives endpoint text and duplicate peers from every draft key.

mod columns;
mod cores;
mod endpoints;
mod entries;
mod page;
mod tab;
mod table;

#[cfg(test)]
mod tests;

// The same group → exchange → core tree, reused by Settings → Storage's "Data by core" list.
pub(in crate::settings) use columns::{CONN_INDENT_MARGIN, CONN_INDENT_PAD, CONN_TABLE_INSET};
pub(in crate::settings) use entries::{EntryLabels, flatten_entries};
pub(in crate::settings) use tab::{
    ServerRowMeta, group_count, group_icon, group_name, group_pill, sorted_group_rows,
    subsection_header_row,
};
pub(in crate::settings) use table::conn_row_h_value;

use std::sync::atomic::{AtomicU64, Ordering};

use gpui::*;
use moon_ui::{MoonColorPickerState, MoonInputEvent, MoonInputState};
use rust_i18n::t;

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
    name: Entity<MoonInputState>,
    key: Entity<MoonInputState>,
    /// Hand-typed endpoint override; empty follows the key, whose endpoint is the placeholder.
    ///
    /// See `ServerConfig::endpoint_override` and [`address_placeholder`].
    address: Entity<MoonInputState>,
    group: Entity<MoonInputState>,
    /// AddToChart bundle name; empty delegates to the global setting.
    ///
    /// See `ServerConfig::chart_bundle`.
    bundle: Entity<MoonInputState>,
    color: Entity<MoonColorPickerState>,
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
/// Args:
///     window: Settings window used to create the input state.
///     cx: Settings context used to subscribe to input events.
///     i: Draft index of the server field.
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
    init: String,
    get: fn(&ServerConfig) -> String,
    set: fn(&mut ServerConfig, String),
    sync_groups: bool,
) -> Entity<MoonInputState> {
    // The placeholders are set where the core card draws these fields, so they follow a live
    // locale switch and the key's endpoint without a subscription of their own.
    let st = cx.new(|cx| MoonInputState::new(window, cx).default_value(init));
    cx.subscribe(&st, move |this, emitter, ev: &MoonInputEvent, cx| {
        if matches!(ev, MoonInputEvent::Change) {
            let val = emitter.read(cx).value().to_string();
            this.backend.update(cx, |b, bcx| {
                if let Some(p) = b.preview.as_mut()
                    && let Some(s) = p.servers.get_mut(i)
                    && get(s) != val
                {
                    set(s, val);
                    if sync_groups {
                        sync_groups_from_servers(&p.servers, &mut p.groups);
                    }
                    bcx.notify();
                }
            });
            // Key replacements can leave settings_sig unchanged; refresh endpoint cells anyway.
            cx.notify();
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
        if let Some(s) = p.servers.get_mut(i)
            && s.color != c
        {
            s.color = c;
            return true;
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
                name: conn_input(
                    window,
                    cx,
                    i,
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
                address: conn_input(
                    window,
                    cx,
                    i,
                    s.endpoint_override.clone(),
                    |s| s.endpoint_override.clone(),
                    set_endpoint_override,
                    false,
                ),
                group: conn_input(
                    window,
                    cx,
                    i,
                    s.group.clone(),
                    // This state is never drawn — the card picks the group from a menu — and
                    // `move_selected_to_group` only mirrors the choice into it. An empty value is
                    // the unnamed group of cores stored before a name was required.
                    |s| s.group.clone(),
                    |s, v| s.group = v,
                    true,
                ),
                bundle: conn_input(
                    window,
                    cx,
                    i,
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

/// Write the address field into the draft, dropping the station tick on ANY change.
///
/// The tick belongs to the address it was given for: replacing, editing or clearing the text must
/// not hand the new address — a LAN one, say — to the station unasked. The cloud glyph turns grey
/// at once, so re-ticking a corrected address is one visible click.
///
/// Args:
///     s: The draft row.
///     v: The field's new text.
pub(super) fn set_endpoint_override(s: &mut ServerConfig, v: String) {
    if s.endpoint_override != v {
        s.endpoint_to_station = false;
    }
    s.endpoint_override = v;
}

/// The address field's placeholder for a row whose key is `key`: the key's own endpoint, or a hint
/// that the address comes from the key while there is no readable key yet.
///
/// The core card computes it from the key field on every render, so it follows typing, the Paste
/// glyph's `set_value` (which emits no Change) and a live locale switch alike.
pub(super) fn address_placeholder(key: &str) -> String {
    endpoints::key_placeholder(key).unwrap_or_else(|| t!("conn.address_ph").to_string())
}
