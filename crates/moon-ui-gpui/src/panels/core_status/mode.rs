//! Core Status presentation modes and persisted log table state.

use super::CoreStatusView;
use crate::Backend;
use gpui::*;
use moon_ui::MoonDataTableState;

/// Chart X-axis span selectable in the detached window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ChartWindow {
    /// Last five minutes.
    #[default]
    Min5,
    /// Last hour.
    Hour1,
}

impl ChartWindow {
    /// Number of seconds (and points at 1 Hz) the window spans.
    pub(super) fn secs(self) -> usize {
        match self {
            Self::Min5 => 300,
            Self::Hour1 => 3600,
        }
    }

    /// Localization key for the span label (`5 мин` / `1 ч`).
    pub(super) fn label_key(self) -> &'static str {
        match self {
            Self::Min5 => "core_status.chart_5m",
            Self::Hour1 => "core_status.chart_1h",
        }
    }
}

/// Available Core Status presentations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CoreStatusMode {
    /// Expandable server rows grouped by endpoint address.
    ByIp,
    /// Existing one-row-per-core telemetry table.
    Flat,
    /// What the cores themselves currently report as confirmed problems.
    Problems,
    /// Recorded warning episodes from the database, newest first.
    Warnings,
    /// The update-history log, merged with attempts still in flight.
    Updates,
}

/// How many recent warning episodes the Warnings list shows.
pub(super) const WARN_LIST_LIMIT: usize = 500;

/// Position of the dead separator cell in the mode strip: after the three LIVE views (By-IP, Flat,
/// Problems) and before the two HISTORY views (Warnings, Updates).
///
/// Named because it is load-bearing in two places that must agree — the item list and the
/// click-index match. A literal in both is how a separator quietly becomes a mode.
pub(super) const MODE_DIVIDER_INDEX: usize = 3;

/// Unscaled width of that separator cell. Wide enough to read as a gap with a rule in it, narrow
/// enough not to read as a missing button.
pub(super) const MODE_DIVIDER_WIDTH: f32 = 13.0;

/// How many recent update-history rows the Updates list shows, after scope filtering. Matches
/// `HISTORY_CAP` in `crates/moon-core/src/session/core_update.rs`: the backing history itself
/// never holds more than this, so the cap only ever bites when scoping filters less than the
/// whole retained log.
pub(super) const UPDATE_LIST_LIMIT: usize = 2_000;

impl Default for CoreStatusMode {
    /// Return the server-by-IP presentation a panel opens on when nothing was ever remembered.
    fn default() -> Self {
        Self::ByIp
    }
}

impl CoreStatusMode {
    /// Stable machine code written to `layout.toml`.
    ///
    /// Never localized and never derived from the tab caption: the captions come from
    /// `core_status.mode.*` and change with the locale, while this is the persistence contract and
    /// must not. Kebab-case matches `WorkspaceMode::code` in `moon-core`.
    ///
    /// Returns:
    ///     The stable, non-localized persistence code for this presentation.
    pub(super) const fn code(self) -> &'static str {
        match self {
            Self::ByIp => "by-ip",
            Self::Flat => "flat",
            Self::Problems => "problems",
            Self::Warnings => "warnings",
            Self::Updates => "updates",
        }
    }

    /// Resolve a persisted code without letting a hand edit change what the panel does.
    ///
    /// Leading and trailing whitespace is ignored. Anything unknown — an empty value, a typo, or a
    /// code a newer build wrote — yields the first-run default rather than an error, so a single bad
    /// entry costs one remembered mode and never the window layout around it.
    ///
    /// Args:
    ///     code: Persisted machine code, potentially hand-edited.
    ///
    /// Returns:
    ///     The matching presentation, or By IP for an empty or unrecognized code.
    pub(super) fn from_code(code: &str) -> Self {
        match code.trim() {
            "flat" => Self::Flat,
            "problems" => Self::Problems,
            "warnings" => Self::Warnings,
            "updates" => Self::Updates,
            _ => Self::default(),
        }
    }
}

/// Context-qualified storage id for the Core Status presentation choice.
///
/// A panel-level choice rather than a property of a table, so it takes its own base and never
/// shares `core-status-table`'s. The `:dock`/`:win` split is what lets a docked tab and a detached
/// window remember different modes; a detached window therefore opens on whatever `:win` last held,
/// and is deliberately NOT seeded from the docked panel it was torn off.
///
/// Args:
///     detached: Whether this panel instance is hosted in a detached window.
///
/// Returns:
///     The context-qualified persistence key for the panel's presentation mode.
pub(super) fn mode_ctx_id(detached: bool) -> String {
    crate::persistence::table_persist::ctx_id("core-status-mode", detached)
}

/// Build a log table state with its dragged column order restored, and keep later drags.
///
/// Widths stay in memory for these logs. Only the order is written, under `base` plus the
/// panel's `:dock` or `:win` suffix.
///
/// Args:
///     cx: Panel context that owns the new state.
///     backend: Shared backend whose layout holds the order.
///     base: Unqualified table id (`core-status-problems`, and the warnings and updates siblings).
///     detached: Whether this panel is a detached window.
///     keys: Column ids in source order, used to drop removed ids and append new ones.
///
/// Returns:
///     The table state, already observing itself for order changes.
pub(super) fn ordered_log_state(
    cx: &mut Context<CoreStatusView>,
    backend: &Entity<Backend>,
    base: &str,
    detached: bool,
    keys: &[&str],
) -> Entity<MoonDataTableState> {
    let id = crate::persistence::table_persist::ctx_id(base, detached);
    let order = crate::persistence::table_persist::restored_order(backend.read(cx), &id, keys);
    let state = cx.new(|_| {
        let mut table = MoonDataTableState::new();
        table.column_order = order;
        table
    });
    cx.observe(&state, move |this, state, cx| {
        crate::persistence::table_persist::persist_order(
            &this.backend,
            &id,
            &state,
            this.order_seen.entry(id.clone()).or_default(),
            cx,
        );
    })
    .detach();
    state
}
