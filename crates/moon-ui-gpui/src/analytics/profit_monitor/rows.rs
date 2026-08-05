//! Pure row grouping for the desktop Profit Monitor.

use std::collections::HashMap;

use moon_core::db::analytics::{ProfitMonitorCore, ProfitMonitorSummary};
use moon_core::session::CoreId;

/// User-selected grouping axis for the monitor table.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum GroupMode {
    /// Keep one row per report core in canonical core order.
    #[default]
    Core,
    /// Merge every core reporting the same exchange name.
    Exchange,
}

impl GroupMode {
    /// Restore a persisted group id.
    ///
    /// Args:
    ///     id: Stable layout value.
    ///
    /// Returns:
    ///     Matching mode, or `None` for unknown future values.
    pub(super) fn from_id(id: &str) -> Option<Self> {
        match id {
            "core" => Some(Self::Core),
            "exchange" => Some(Self::Exchange),
            _ => None,
        }
    }

    /// Return the stable layout id.
    ///
    /// Returns:
    ///     `core` or `exchange`.
    pub(super) fn id(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Exchange => "exchange",
        }
    }
}

/// Cached non-database context that can regroup an existing report snapshot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct LiveContext {
    /// Last known nonblank exchange names keyed by core.
    pub(super) exchange_names: HashMap<CoreId, String>,
    /// Current configured names keyed by core.
    pub(super) core_names: HashMap<CoreId, String>,
    /// Configured cores in canonical display order.
    pub(super) core_order: Vec<CoreId>,
}

/// One displayed row after the selected grouping axis has merged per-core data.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct MonitorRow {
    /// Visible group label.
    pub(super) name: String,
    /// Projected profit.
    pub(super) profit: f64,
    /// Closed-trade count.
    pub(super) trades: i64,
    /// Profitable-trade count.
    pub(super) wins: i64,
    /// Additive positive-spend total.
    pub(super) positive_spent: f64,
    /// Count contributing to `positive_spent`.
    pub(super) positive_orders: i64,
    /// Core identity used only for canonical Core-mode ordering.
    pub(super) primary_core: CoreId,
}

impl MonitorRow {
    /// Merge one report-core aggregate into this displayed row.
    ///
    /// Args:
    ///     source: Per-core additive values from the database snapshot.
    fn push(&mut self, source: &ProfitMonitorCore) {
        self.profit += source.profit;
        self.trades += source.trades;
        self.wins += source.wins;
        self.positive_spent += source.positive_spent;
        self.positive_orders += source.positive_orders;
    }

    /// Return the exact win percentage after grouping.
    ///
    /// Returns:
    ///     Profitable trades divided by all trades, or zero for an empty row.
    pub(super) fn win_rate(&self) -> f64 {
        if self.trades == 0 {
            0.0
        } else {
            self.wins as f64 * 100.0 / self.trades as f64
        }
    }

    /// Return the exact average positive order spend after grouping.
    ///
    /// Returns:
    ///     Positive spend divided by its contributing count, or zero when none exists.
    pub(super) fn average_order(&self) -> f64 {
        if self.positive_orders == 0 {
            0.0
        } else {
            self.positive_spent / self.positive_orders as f64
        }
    }
}

/// Localized fallback labels used by the pure grouping pass.
pub(super) struct RowLabels<'a> {
    /// Prefix used when no usable core name exists.
    pub(super) core: &'a str,
    /// One shared explicit label for missing exchange metadata.
    pub(super) unknown_exchange: &'a str,
}

/// Group per-core report aggregates into visible Core or Exchange rows.
///
/// Args:
///     summary: Per-core additive database payload.
///     live: Current labels and canonical core order.
///     mode: Selected grouping axis.
///     labels: Localized fallback labels.
///
/// Returns:
///     Exact merged rows in canonical Core order or descending profit order for aggregate modes.
pub(super) fn grouped_rows(
    summary: &ProfitMonitorSummary,
    live: &LiveContext,
    mode: GroupMode,
    labels: RowLabels<'_>,
) -> Vec<MonitorRow> {
    let mut grouped = HashMap::<String, MonitorRow>::new();
    for source in &summary.cores {
        let core = source.core_uid;
        let configured = live
            .core_names
            .get(&core)
            .map(String::as_str)
            .filter(|name| !name.trim().is_empty());
        let report = (!source.report_name.trim().is_empty()).then_some(source.report_name.as_str());
        let (key, name) = match mode {
            GroupMode::Core => {
                let name = configured
                    .or(report)
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{} {core}", labels.core));
                (format!("core:{core}"), name)
            }
            GroupMode::Exchange => {
                let name = live
                    .exchange_names
                    .get(&core)
                    .cloned()
                    .unwrap_or_else(|| labels.unknown_exchange.to_string());
                (format!("exchange:{}", name.to_lowercase()), name)
            }
        };
        grouped
            .entry(key)
            .or_insert_with(|| MonitorRow {
                name,
                primary_core: core,
                ..MonitorRow::default()
            })
            .push(source);
    }

    let mut rows = grouped.into_values().collect::<Vec<_>>();
    if mode == GroupMode::Core {
        let rank = live
            .core_order
            .iter()
            .enumerate()
            .map(|(index, core)| (*core, index))
            .collect::<HashMap<_, _>>();
        rows.sort_by_key(|row| {
            (
                rank.get(&row.primary_core).copied().unwrap_or(usize::MAX),
                row.primary_core,
            )
        });
    } else {
        rows.sort_by(|a, b| {
            b.profit
                .total_cmp(&a.profit)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
    }
    rows
}
