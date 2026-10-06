use super::*;

/// SQL fragments used by the per-row Report reader, plus the rate the Analytics source projects.
///
/// A row-level reader wants the applied rate and its provenance beside the converted profit, so a
/// user can check one trade rather than trust a total.
///
/// [`rate`](Self::rate) has a SECOND consumer: the Analytics unified source projects it as
/// `quote_rate`, so an aggregate can convert a figure built from raw quantities and prices, which
/// no valuation column replaces. That is sound because `rate` reads the `v` value join alone,
/// which [`CoverageSql::joins`] already carries — but it holds ONLY for `rate`.
/// [`quote_rate`](Self::quote_rate) removes only the numeric-profit coverage gate for consumers
/// that reconstruct their own money amount, such as Report volume; a historical non-identity row
/// still needs the prepared-value join to identify its cached rate.
/// [`source`](Self::source) additionally needs the `ra` provenance join and must not follow it
/// into an aggregate.
pub(crate) struct PerRowSql {
    /// The `v` prepared-value join followed by the `ra` ready-rate provenance join.
    ///
    /// The joins are complete and dependency-ordered because `ra` reads
    /// `v.rate_minute_utc`; unresolved searches are not reader joins at all.
    pub joins: String,
    /// USDT paid for one quote unit, `1.0` on an identity row.
    pub rate: String,
    /// USDT paid for one quote unit without requiring a numeric profit value.
    ///
    /// Consumers valuing another independently reconstructed amount use this expression instead
    /// of inheriting profit coverage from [`Self::rate`].
    pub quote_rate: String,
    /// Human-readable provenance of that rate, NULL while the row is uncovered.
    pub source: String,
}

/// SQL fragments for aggregate valuation coverage and per-row Report values.
pub(crate) struct CoverageSql {
    /// The aggregate reader's input-matching prepared-value join.
    pub joins: String,
    /// One for every row carrying a known quote identity.
    pub eligible: String,
    /// One only for identity-USDT or an input-matching prepared value.
    pub valued: String,
    /// Historical routing never exposes a terminal unavailable state, so this is always zero.
    pub unavailable: String,
    /// Complete-row USDT profit expression, NULL while uncovered.
    ///
    /// Also the row-level converted profit: ONE expression, so a row and the total it belongs to
    /// can never disagree about the same number.
    pub profit_usdt: String,
    /// Complete-row USDT spend expression, NULL when absent or uncovered.
    pub spent_usdt: String,
    /// Row-level projections and the joins that back them.
    pub per_row: PerRowSql,
}

impl CoverageSql {
    /// Build the six grouped aggregate columns shared by Report and Analytics totals.
    ///
    /// Returns:
    ///     SQL expressions ordered for [`CoverageAggregate::add_row`].
    pub(crate) fn aggregate_columns(&self) -> String {
        self.sum_columns()
            .iter()
            .map(crate::db::sql_sum::SumColumn::aggregate_sql)
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The same six columns described once, for the grouped and the row-pass shape alike.
    ///
    /// Returns:
    ///     Columns ordered for [`CoverageAggregate::add_row`].
    pub(in crate::db) fn sum_columns(&self) -> Vec<crate::db::sql_sum::SumColumn> {
        use crate::db::sql_sum::{SumColumn, SumZero};
        vec![
            SumColumn::sum(
                format!("CASE WHEN {} THEN 1 ELSE 0 END", self.eligible),
                SumZero::Integer,
            ),
            SumColumn::sum(
                format!("CASE WHEN {} THEN 1 ELSE 0 END", self.valued),
                SumZero::Integer,
            ),
            SumColumn::sum(
                format!("CASE WHEN {} THEN 1 ELSE 0 END", self.unavailable),
                SumZero::Integer,
            ),
            SumColumn::sum(self.profit_usdt.clone(), SumZero::Real),
            SumColumn::sum(self.spent_usdt.clone(), SumZero::Real),
            SumColumn::sum(
                format!(
                    "CASE WHEN {} AND ({}) IS NOT NULL THEN 1 ELSE 0 END",
                    self.valued, self.spent_usdt
                ),
                SumZero::Integer,
            ),
        ]
    }
}

/// Mutable aggregation of coverage rows across typed and legacy report sources.
#[derive(Default)]
pub(crate) struct CoverageAggregate {
    eligible: i64,
    valued: i64,
    unavailable: i64,
    profit_usdt: f64,
    spent_usdt: f64,
    spent_rows: i64,
}

impl CoverageAggregate {
    /// Add one physical source's coverage aggregate.
    ///
    /// Args:
    ///     eligible: Known-quote rows.
    ///     valued: Identity or prepared rows.
    ///     unavailable: Terminal unavailable rows; always zero for historical routing.
    ///     profit_usdt: Sum over valued rows only.
    ///     spent_usdt: Sum over valued rows carrying numeric spend.
    ///     spent_rows: Valued rows carrying numeric spend.
    pub(crate) fn add(
        &mut self,
        eligible: i64,
        valued: i64,
        unavailable: i64,
        profit_usdt: f64,
        spent_usdt: f64,
        spent_rows: i64,
    ) {
        self.eligible += eligible;
        self.valued += valued;
        self.unavailable += unavailable;
        self.profit_usdt += profit_usdt;
        self.spent_usdt += spent_usdt;
        self.spent_rows += spent_rows;
    }

    /// Decode and add the shared six-column coverage suffix from one grouped SQLite row.
    ///
    /// Args:
    ///     row: Current grouped aggregate row.
    ///     offset: Index of the first coverage column.
    ///
    /// Returns:
    ///     SQLite success after every typed aggregate was decoded.
    pub(crate) fn add_row(&mut self, row: &rusqlite::Row, offset: usize) -> rusqlite::Result<()> {
        self.add(
            row.get(offset)?,
            row.get(offset + 1)?,
            row.get(offset + 2)?,
            row.get(offset + 3)?,
            row.get(offset + 4)?,
            row.get(offset + 5)?,
        );
        Ok(())
    }

    /// Finish complete-only public coverage without exposing a partial monetary sum.
    ///
    /// Returns:
    ///     Coverage whose USDT total exists only when every eligible row is valued.
    pub(crate) fn finish(self) -> crate::db::ValuationCoverage {
        let complete = self.eligible == self.valued && self.unavailable == 0;
        crate::db::ValuationCoverage {
            eligible_orders: self.eligible,
            valued_orders: self.valued,
            unavailable_orders: self.unavailable,
            usdt: complete.then_some(crate::db::UsdtTotal {
                profit: self.profit_usdt,
                spent: (self.spent_rows == self.valued).then_some(self.spent_usdt),
            }),
        }
    }
}

/// The row-level guards both conversions share.
pub(in crate::db) struct SourcePredicates {
    /// One for a row whose quote currency is a trusted persisted ordinal.
    pub quote_known: String,
    /// One for a row whose profit is numeric.
    pub numeric_profit: String,
    /// The row's spend, or NULL when absent or non-numeric.
    pub spent_value: String,
}

/// Build the guards that decide which rows either conversion may touch at all.
///
/// Shared by both SQL builders on purpose. The trusted-ordinal contract and the storage-class
/// checks are what "eligible" MEANS, and stating them twice is how the two modes would come to
/// disagree about which rows count — silently, with both still compiling and both still passing.
///
/// Args:
///     alias: Qualified report-row alias used by the caller.
///     columns: Discovered physical source columns.
///
/// Returns:
///     Predicates that name only columns the source actually has.
pub(in crate::db) fn source_predicates(
    alias: &str,
    columns: &std::collections::HashSet<String>,
) -> SourcePredicates {
    // SQLite resolves every column reference at prepare time, unreachable arms included, so an
    // absent column collapses to a constant rather than being named.
    SourcePredicates {
        quote_known: if columns.contains("basecurrency") {
            // The effective expression already rejects every non-integer storage class, so the
            // range check alone decides whether the identity is one this build knows.
            format!(
                "({quote}) BETWEEN 0 AND 20",
                quote = crate::db::quote::effective_ordinal_expr(alias, columns)
            )
        } else {
            "0".to_string()
        },
        numeric_profit: if columns.contains("profitbtc") {
            format!("typeof({alias}.profitbtc) IN ('integer','real')")
        } else {
            "0".to_string()
        },
        // The type check reads the STORED cell, while the value taken from it is the settled one:
        // a COIN-M liquidation stores its amount in a unit that is not the row's currency, and
        // valuing the stored number would convert the wrong quantity at the right rate.
        spent_value: if columns.contains("spentbtc") {
            format!(
                "CASE WHEN typeof({alias}.spentbtc) IN ('integer','real') THEN {settled} END",
                settled = crate::db::quote::settled_amount_expr(alias, columns, "spentbtc")
            )
        } else {
            "NULL".to_string()
        },
    }
}

/// Build input-matching valuation SQL for one physical report source.
///
/// Args:
///     alias: Qualified report-row alias used by the caller.
///     columns: Discovered physical source columns.
///     source: Typed or legacy source partition.
///
/// Returns:
///     Coverage fragments; sources without stable identity retain identity-USDT coverage only.
pub(crate) fn coverage_sql(
    alias: &str,
    columns: &std::collections::HashSet<String>,
    source: TradeSource,
) -> CoverageSql {
    let id_column = match source {
        TradeSource::Typed if columns.contains("newrecid") => Some("newrecid"),
        TradeSource::Legacy if columns.contains("db_id") => Some("db_id"),
        TradeSource::Typed | TradeSource::Legacy => None,
    };
    let has_closedate = columns.contains("closedate");
    let has_quote = columns.contains("basecurrency");
    let has_profit = columns.contains("profitbtc");
    // Every quote reference below goes through this ONE expression: the cache is keyed by quote
    // ordinal, so a join that matched the raw column while the projection valued the effective one
    // would look up the rate of a currency the row is not denominated in.
    let quote = crate::db::quote::effective_ordinal_expr(alias, columns);
    let SourcePredicates {
        quote_known,
        numeric_profit,
        spent_value,
    } = source_predicates(alias, columns);
    let date_valid = if has_closedate {
        format!("typeof({alias}.closedate)='integer' AND {alias}.closedate>0")
    } else {
        "0".to_string()
    };
    let input_match = if let Some(id_column) =
        id_column.filter(|_| columns.contains("core_uid") && has_required_trade_inputs(columns))
    {
        format!(
            "v.source_kind={source_kind}
             AND typeof({alias}.core_uid)='integer'
             AND typeof({alias}.{id_column})='integer'
             AND v.core_uid={alias}.core_uid AND v.row_id={alias}.{id_column}
             AND v.algorithm_version={algorithm_version}
             AND v.closedate={alias}.closedate AND v.quote_ordinal=({quote})
             AND v.profit_quote={settled_profit} AND v.spent_quote IS {spent_value}",
            source_kind = source.code(),
            algorithm_version = ALGORITHM_VERSION,
            settled_profit = crate::db::quote::settled_amount_expr(alias, columns, "profitbtc"),
        )
    } else {
        "0".to_string()
    };
    let eligible = format!("({quote_known})");
    let identity = if has_quote {
        format!(
            "({eligible} AND {numeric_profit} AND ({quote})={usdt})",
            usdt = crate::db::QuoteCurrency::usdt().ordinal()
        )
    } else {
        "0".to_string()
    };
    let prepared =
        format!("({eligible} AND {numeric_profit} AND {date_valid} AND v.row_id IS NOT NULL)");
    let valued = format!("({identity} OR {prepared})");
    // Known quotes remain pending until a market observation becomes available; historical
    // routing no longer has a terminal unavailable state.
    let unavailable = "0".to_string();
    let profit_usdt = if has_profit {
        format!(
            "CASE WHEN {identity} THEN {settled}
                  WHEN {prepared} THEN v.profit_usdt END",
            settled = crate::db::quote::settled_amount_expr(alias, columns, "profitbtc")
        )
    } else {
        "NULL".to_string()
    };
    let spent_usdt = format!(
        "CASE WHEN {identity} THEN {spent_value}
              WHEN {prepared} THEN v.spent_usdt END"
    );
    // An identity row stores no `rates` row at all — `resolve_rate_batch` synthesizes the USDT
    // identity in memory — so every per-row expression must answer for it before consulting the
    // cache. Treating a NULL provider as "not valued" would blank the column on most trades.
    let rate = format!("CASE WHEN {identity} THEN 1.0 WHEN {prepared} THEN v.rate_usdt END");
    let rate_identity = if has_quote {
        format!(
            "({eligible} AND ({quote})={usdt})",
            usdt = crate::db::QuoteCurrency::usdt().ordinal()
        )
    } else {
        "0".to_string()
    };
    let rate_prepared = format!("({eligible} AND {date_valid} AND v.row_id IS NOT NULL)");
    let quote_rate =
        format!("CASE WHEN {rate_identity} THEN 1.0 WHEN {rate_prepared} THEN v.rate_usdt END");
    // Without a quote column there is nothing to join on, and the source expression must not name
    // `ra`: SQLite resolves every column reference at prepare time, unreachable arms included.
    let (rate_join, source) = if has_quote {
        (
            format!(
                " LEFT JOIN valuation.rates ra
                    ON ra.algorithm_version={algorithm_version}
                   AND ra.quote_ordinal=({quote})
                   AND ra.minute_utc=v.rate_minute_utc",
                algorithm_version = ALGORITHM_VERSION,
            ),
            format!(
                "CASE WHEN {identity} THEN 'identity'
                      WHEN {prepared} THEN COALESCE(
                          ra.provider || ' ' || ra.symbol
                          || CASE ra.orientation WHEN {inverse} THEN ' inv' ELSE '' END
                          || CASE WHEN ra.leg2_provider IS NOT NULL THEN
                              ' -> ' || ra.leg2_provider || ' ' || ra.leg2_symbol
                              || CASE ra.leg2_orientation WHEN {inverse} THEN ' inv' ELSE '' END
                             ELSE '' END
                          || CASE WHEN ra.resolved_minute_utc>ra.minute_utc THEN
                              ' +' || ((ra.resolved_minute_utc-ra.minute_utc)/60) || 'm'
                             ELSE '' END,
                          'cached') END",
                inverse = RateOrientation::Inverse.code(),
            ),
        )
    } else {
        (String::new(), "NULL".to_string())
    };
    let value_join = format!(" LEFT JOIN valuation.trade_values v ON {input_match}");
    CoverageSql {
        joins: value_join.clone(),
        eligible,
        valued,
        unavailable,
        per_row: PerRowSql {
            // `rate_join` resolves `v.rate_minute_utc`, so it can only follow the value join.
            joins: format!("{value_join}{rate_join}"),
            rate,
            quote_rate,
            source,
        },
        profit_usdt,
        spent_usdt,
    }
}

/// Build valuation SQL for one physical report source under the requested mode.
///
/// The one seam between the two conversions. Every reader goes through here, so a mode cannot
/// reach one query path and miss another — which would show a row and the total it belongs to
/// converted by different rules.
///
/// Only the historical mode can be withheld. Its rows live in the attached derived cache, so a
/// detached or corrupt cache leaves nothing to project; current rates live in memory and stay
/// available whatever the cache is doing, which is also why they need no DETACH-and-retry path.
///
/// Args:
///     mode: Historical per-trade rates, or the latest known rates.
///     attached: Whether the derived valuation cache may be joined.
///     alias: Qualified report-row alias used by the caller.
///     columns: Discovered physical source columns.
///     source: Typed or legacy source partition.
///
/// Returns:
///     Coverage fragments in the shape both modes share, or `None` when the historical cache is
///     unavailable and the caller must fall back to native money.
pub(in crate::db) fn projection(
    mode: ValuationMode,
    attached: bool,
    alias: &str,
    columns: &std::collections::HashSet<String>,
    source: TradeSource,
) -> Option<CoverageSql> {
    match mode {
        ValuationMode::Historical => attached.then(|| coverage_sql(alias, columns, source)),
        ValuationMode::Current => {
            // One snapshot AND one clock for every projection of this read batch — see `RatePin`.
            let (rates, now_ms) = current::current_rates_at();
            Some(current_rate_sql(alias, columns, &rates, now_ms))
        }
    }
}

/// Report columns a trade must carry before it can be valued at all.
///
/// Shared with the Report window's synthetic-column table, so the columns that OFFER a conversion
/// and the gate that decides a row can be converted cannot drift into disagreeing — the visible
/// symptom of which would be a column that is permanently blank rather than absent.
pub(crate) const REQUIRED_TRADE_INPUTS: &[&str] = &["closedate", "basecurrency", "profitbtc"];

/// Check whether one report source carries every mandatory valuation input column.
///
/// Args:
///     columns: Discovered physical source columns.
///
/// Returns:
///     Whether close time, quote identity, and native profit are available.
pub(crate) fn has_required_trade_inputs(columns: &std::collections::HashSet<String>) -> bool {
    REQUIRED_TRADE_INPUTS
        .iter()
        .all(|column| columns.contains(*column))
}
