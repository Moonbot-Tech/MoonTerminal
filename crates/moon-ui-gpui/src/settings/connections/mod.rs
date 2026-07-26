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

mod tab;
mod table;

#[cfg(test)]
mod tests;

use gpui::*;
use moon_ui::{MoonColorPickerState, MoonInputEvent, MoonInputState};

use super::SettingsView;
use crate::Backend;
use moon_core::config::{GroupConfig, Secret, ServerConfig, ensure_server_group_configs};

/// Component state for one server row's text fields and color picker.
pub(super) struct ConnRow {
    name: Entity<MoonInputState>,
    key: Entity<MoonInputState>,
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
fn conn_input(
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    i: usize,
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
        .map(|(i, s)| ConnRow {
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
                    |s, v| s.key = Secret::new(v),
                    false,
                );
                st.update(cx, |st, c| st.set_masked(true, window, c));
                st
            },
            group: conn_input(
                window,
                cx,
                i,
                s.group.clone(),
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
        })
        .collect()
}
