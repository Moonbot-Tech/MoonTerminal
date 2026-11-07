//! One entry of the AI-access page's change journal: who changed what, where, and how it ended.

use gpui::*;
use moon_core::station_api::mcp::{AuditChange, AuditEntry, CoreInfo, Scope};
use moon_ui::{MoonPalette, rgba_from, v_flex};
use rust_i18n::t;

use crate::design;

/// An action's name for the owner; one this build does not know stays as the Station sent it.
fn action_label(action: &str) -> String {
    match action {
        "strategy_edit" => t!("station_mcp.action.strategy_edit"),
        "strategy_create" => t!("station_mcp.action.strategy_create"),
        "strategy_delete" => t!("station_mcp.action.strategy_delete"),
        "trading" => t!("station_mcp.action.trading"),
        "autodetect" => t!("station_mcp.action.autodetect"),
        "settings_patch" => t!("station_mcp.action.settings_patch"),
        "order_place" => t!("station_mcp.action.order_place"),
        "order_cancel" => t!("station_mcp.action.order_cancel"),
        "order_move" => t!("station_mcp.action.order_move"),
        "position_close" => t!("station_mcp.action.position_close"),
        "panic_sell" => t!("station_mcp.action.panic_sell"),
        "leverage" => t!("station_mcp.action.leverage"),
        "hedge_mode" => t!("station_mcp.action.hedge_mode"),
        "runtime_restart" => t!("station_mcp.action.runtime_restart"),
        "access_change" => t!("station_mcp.action.access_change"),
        other => return other.to_owned(),
    }
    .to_string()
}

/// A state's name for the owner. Entries from before the journal kept its operation apart store
/// `admitted:<operation>`, so the id is dropped first; an unknown state stays as sent.
fn outcome_label(outcome: &str) -> String {
    let state = outcome.split(':').next().unwrap_or(outcome);
    match state {
        "admitted" => t!("station_mcp.outcome.admitted"),
        "queued_in_runtime" => t!("station_mcp.outcome.queued_in_runtime"),
        "submitted" => t!("station_mcp.outcome.submitted"),
        "confirmed" => t!("station_mcp.outcome.confirmed"),
        "adjusted" => t!("station_mcp.outcome.adjusted"),
        "superseded" => t!("station_mcp.outcome.superseded"),
        "rejected" => t!("station_mcp.outcome.rejected"),
        "unknown" => t!("station_mcp.outcome.unknown"),
        "already_applied" => t!("station_mcp.outcome.already_applied"),
        "applied" => t!("station_mcp.outcome.applied"),
        other => return other.to_owned(),
    }
    .to_string()
}

/// The entry's cores as the owner knows them, or by number once a core has left the Station.
fn core_names(entry: &AuditEntry, cores: &[CoreInfo]) -> String {
    entry
        .cores
        .iter()
        .map(|uid| {
            cores
                .iter()
                .find(|core| core.uid == *uid)
                .map_or_else(|| format!("#{uid}"), |core| core.name.clone())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A change as one line: `field: before → after`, or just the new value when there was no earlier
/// one to state.
fn change_line(change: &AuditChange) -> String {
    let after = change.after.as_deref().unwrap_or("—");
    match &change.before {
        Some(before) => format!("{}: {before} → {after}", change.field),
        None => format!("{}: {after}", change.field),
    }
}

/// A change of a client's permissions as one line. The Station stores `#uid` and protocol words so
/// the record does not depend on a language; here they become the owner's core name and words.
fn access_line(change: &AuditChange, cores: &[CoreInfo], word: impl Fn(&str) -> String) -> String {
    let core = change
        .field
        .strip_prefix('#')
        .and_then(|uid| uid.parse::<u64>().ok())
        .and_then(|uid| cores.iter().find(|core| core.uid == uid))
        .map_or_else(|| change.field.clone(), |core| core.name.clone());
    let words = |text: &str| text.split(", ").map(&word).collect::<Vec<_>>().join(", ");
    let after = change
        .after
        .as_deref()
        .map_or_else(|| "—".to_owned(), words);
    match change.before.as_deref() {
        Some(before) => format!("{core}: {} → {after}", words(before)),
        None => format!("{core}: {after}"),
    }
}

/// A permission's protocol word in the owner's language; an unknown one stays as sent.
fn scope_word(word: &str) -> String {
    match word {
        "read" => super::scope_short(Scope::Read),
        "strategies" => super::scope_short(Scope::Strategies),
        "trading" => super::scope_short(Scope::Trading),
        "settings" => super::scope_short(Scope::Settings),
        other => other.to_owned(),
    }
}

/// The entry's first line. Parts an older entry does not have are left out rather than blank.
fn headline(entry: &AuditEntry, cores: &[CoreInfo], action: &str, outcome: &str) -> String {
    let time = chrono::DateTime::from_timestamp_millis(entry.at_ms)
        .map_or_else(String::new, |time| {
            time.format("%Y-%m-%d %H:%M:%S UTC").to_string()
        });
    let what = match &entry.subject {
        Some(subject) => format!("{action} «{subject}»"),
        None => action.to_owned(),
    };
    [
        time,
        entry.client_name.clone(),
        core_names(entry, cores),
        what,
        outcome.to_owned(),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" · ")
}

/// One journal entry: its headline over the values it changed, muted.
pub(super) fn entry_el(entry: &AuditEntry, cores: &[CoreInfo], cx: &App) -> AnyElement {
    let muted = rgba_from(MoonPalette::active(cx).text_muted, 1.0);
    let title = headline(
        entry,
        cores,
        &action_label(&entry.action),
        &outcome_label(&entry.outcome),
    );
    let mut lines = entry
        .changes
        .iter()
        .map(|change| {
            if entry.action == "access_change" {
                access_line(change, cores, scope_word)
            } else {
                change_line(change)
            }
        })
        .collect::<Vec<_>>();
    if entry.changes_omitted > 0 {
        lines.push(t!("station_mcp.changes_more", count = entry.changes_omitted).to_string());
    }
    v_flex()
        .w_full()
        .min_w_0()
        .gap(design::ui_px(cx, 2.0))
        .child(div().child(title))
        .children(
            lines
                .into_iter()
                .map(|line| div().text_color(muted).child(line)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests;
