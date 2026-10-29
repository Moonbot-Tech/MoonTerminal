//! The closed-row totals statement of one report source, and the sliced read built on it.
//!
//! [`query_totals`](super::query_totals) runs one grouped statement per source. A caller that
//! needs the same totals for many slices of one filter -- the Mini App's period total, every
//! exchange, every core and every day -- used to repeat that statement per slice, evaluating the
//! heavy per-row money expressions once per slice a row falls into. [`query_totals_sliced`] reads
//! the rows once, sums each into every slice that admits it with [`SqliteSum`], and hands each
//! slice's groups to the same [`TotalsSink`] the grouped statement feeds, so every slice states
//! exactly what its own `query_totals` would have stated.

use std::collections::{BTreeMap, HashMap, HashSet};

mod slice_index;

use rusqlite::Connection;
use rusqlite::types::{FromSql, FromSqlError, Value, ValueRef};

use super::super::report_axis::ReportAxis;
use super::super::sql_sum::{GroupKey, SqliteSum, SumColumn};
use super::{
    CLOSEDATE, PeriodBasis, QuoteBreakdown, ReadResult, ReadSource, ReportFilter, ReportTotals,
    RowScope, StrategyMeta, ValuationMode, build_where, entry_spend_sql, profit_column, read_fail,
    read_sources_res, report_strategy_meta, source_partition, traded_volume_sql,
    with_valuation_fallback,
};

/// The closed-row totals statement of one physical source, in its two shapes.
pub(super) struct ClosedPass<'a> {
    /// Physical source the statement reads.
    src: &'a ReadSource,
    /// Quote-identity expression every group is keyed by.
    quote: String,
    /// `GROUP BY` clause over [`Self::quote`].
    group_by: String,
    /// Valuation joins, empty without a projection.
    joins: String,
    /// Row predicate.
    where_sql: String,
    /// Values bound by [`Self::where_sql`].
    params: Vec<Box<dyn rusqlite::types::ToSql>>,
    /// Summed columns after the quote key, in the order [`TotalsSink::add_group`] reads them.
    columns: Vec<SumColumn>,
    /// Whether the columns carry the six valuation-coverage sums.
    valued: bool,
    /// Counted entry-spend predicate. The distinct-core statement adds it to [`Self::where_sql`].
    counted: String,
}

impl<'a> ClosedPass<'a> {
    /// Build the statement exactly as [`query_totals_attempt`](super::query_totals_attempt)
    /// always has.
    ///
    /// Args:
    ///     src: Physical source and its discovered columns.
    ///     f: Complete Report filter.
    ///     include_valuation: Whether the historical mode may join the attached derived cache.
    ///     meta: Strategy metadata of this read, with its resolved name mask.
    pub(super) fn new(
        src: &'a ReadSource,
        f: &ReportFilter,
        include_valuation: bool,
        meta: &StrategyMeta,
    ) -> Self {
        let closed_scope = ReportFilter {
            rows: if src.cols.contains("closedate") {
                RowScope::Closed
            } else {
                RowScope::ClosedAndOpen
            },
            ..f.clone()
        };
        let (where_sql, params) = build_where(&closed_scope, &src.cols, meta);
        let (quote, group_by) = super::super::quote::trusted_quote_group("r", &src.cols);
        let valuation = super::super::valuation::projection(
            f.valuation,
            include_valuation,
            "r",
            &src.cols,
            source_partition(src),
        );
        let rate = valuation
            .as_ref()
            .map(|parts| parts.per_row.quote_rate.as_str());
        let mut columns = vec![profit_column(src), SumColumn::Count];
        if let Some(parts) = &valuation {
            columns.extend(parts.sum_columns());
        }
        columns.extend(traded_volume_sql(src, rate).sum_columns());
        let spend = entry_spend_sql(src, rate);
        let counted = spend.counted_predicate().to_string();
        columns.extend(spend.sum_columns());
        Self {
            src,
            quote,
            group_by,
            joins: valuation
                .as_ref()
                .map(|parts| parts.joins.clone())
                .unwrap_or_default(),
            where_sql,
            params,
            columns,
            valued: valuation.is_some(),
            counted,
        }
    }

    /// Run the grouped statement and feed every group to `sink`.
    pub(super) fn run_grouped(
        &self,
        conn: &Connection,
        sink: &mut TotalsSink,
    ) -> rusqlite::Result<()> {
        let sql = format!(
            "SELECT {}, {} FROM {} r{}{}{}",
            self.quote,
            self.select(SumColumn::aggregate_sql),
            self.src.table,
            self.joins,
            self.where_sql,
            self.group_by,
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(self.refs().as_slice())?;
        let width = 1 + self.columns.len();
        let mut values = Vec::with_capacity(width);
        while let Some(row) = rows.next()? {
            values.clear();
            for index in 0..width {
                values.push(row.get::<_, Value>(index)?);
            }
            sink.add_group(&values, self.valued)?;
        }
        Ok(())
    }

    /// The row-pass statement: quote key, core, close date, then every column's per-row value,
    /// over exactly the rows the grouped statement groups.
    fn row_sql(&self) -> String {
        let close = if self.src.cols.contains("closedate") {
            CLOSEDATE
        } else {
            "NULL"
        };
        format!(
            "SELECT {}, r.core_uid, {close}, {} FROM {} r{}{}",
            self.quote,
            self.select(SumColumn::row_sql),
            self.src.table,
            self.joins,
            self.where_sql,
        )
    }

    fn select(&self, shape: fn(&SumColumn) -> String) -> String {
        self.columns
            .iter()
            .map(shape)
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn refs(&self) -> Vec<&dyn rusqlite::types::ToSql> {
        self.params.iter().map(|param| param.as_ref()).collect()
    }

    /// Cores with at least one counted entry-spend row under this pass's own predicate.
    ///
    /// A separate statement from the grouped sums. `GROUP BY` quote would count a core once per
    /// quote, and summing those counts would double a core that traded two quotes. `DISTINCT`
    /// over the ungrouped counted rows does not.
    ///
    /// Args:
    ///     conn: Open report reader or snapshot.
    ///     cores: Set the caller unions across sources. A core already recorded is left as it is.
    pub(super) fn collect_counted_cores(
        &self,
        conn: &Connection,
        cores: &mut HashSet<i64>,
    ) -> rusqlite::Result<()> {
        let sql = format!(
            "SELECT DISTINCT r.core_uid FROM {} r{}{} AND ({})",
            self.src.table, self.joins, self.where_sql, self.counted,
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(self.refs().as_slice())?;
        while let Some(row) = rows.next()? {
            if let Value::Integer(core) = row.get::<_, Value>(0)? {
                cores.insert(core);
            }
        }
        Ok(())
    }

    /// Index of the counted entry-spend value in a row-pass value vector.
    ///
    /// [`TotalsSink::add_group`] sees the quote key at index 0, so its counted column is one past
    /// this index. The row pass omits that key.
    fn counted_value_index(&self) -> usize {
        counted_group_index(self.valued) - 1
    }
}

/// Index of the counted entry-spend column in a grouped values vector.
///
/// Layout: quote key, profit, count, six coverage columns when `valued`, five volume columns,
/// then the counted column.
fn counted_group_index(valued: bool) -> usize {
    3 + usize::from(valued) * 6 + 5
}

/// Everything the closed-row groups of one totals read add up to.
#[derive(Default)]
pub(super) struct TotalsSink {
    groups: Vec<(Option<i64>, f64, i64)>,
    volume_groups: Vec<(Option<i64>, i64, i64, f64, i64, f64)>,
    spend_groups: Vec<(Option<i64>, i64, f64, f64, i64, f64, f64)>,
    coverage: super::super::valuation::CoverageAggregate,
    /// The same coverage sums kept per group, so the finished breakdown can tell which of its
    /// currencies are completely valued; [`Self::coverage`] alone only knows the whole.
    coverage_groups: Vec<(Option<i64>, i64, i64, i64, f64, f64, i64)>,
}

impl TotalsSink {
    /// Add one group: the quote key, then the values of [`ClosedPass::columns`].
    ///
    /// Args:
    ///     values: One grouped row, as SQLite returned it or as [`SqliteSum`] restated it.
    ///     valued: Whether the six valuation-coverage columns follow the count.
    pub(super) fn add_group(&mut self, values: &[Value], valued: bool) -> rusqlite::Result<()> {
        let ordinal = super::super::quote::report_ordinal_from_value(&values[0]);
        self.groups
            .push((ordinal, get(values, 1)?, get(values, 2)?));
        if valued {
            let group = (
                ordinal,
                get(values, 3)?,
                get(values, 4)?,
                get(values, 5)?,
                get(values, 6)?,
                get(values, 7)?,
                get(values, 8)?,
            );
            self.coverage
                .add(group.1, group.2, group.3, group.4, group.5, group.6);
            self.coverage_groups.push(group);
        }
        let volume = 3 + usize::from(valued) * 6;
        let spend = counted_group_index(valued);
        self.volume_groups.push((
            ordinal,
            get(values, volume)?,
            get(values, volume + 1)?,
            get(values, volume + 2)?,
            get(values, volume + 3)?,
            get(values, volume + 4)?,
        ));
        self.spend_groups.push((
            ordinal,
            get(values, spend)?,
            get(values, spend + 1)?,
            get(values, spend + 2)?,
            get(values, spend + 3)?,
            get(values, spend + 4)?,
            get(values, spend + 5)?,
        ));
        Ok(())
    }

    /// The read's totals.
    ///
    /// Args:
    ///     valuation_present: Whether the selected mode could build a projection, so coverage is
    ///         published.
    ///     open_groups: The open pass's per-quote tally, empty for a closed-only read.
    ///     counted_cores: Distinct cores with a counted entry-spend row. Zero when the filter
    ///         named its cores explicitly, or when none contributed.
    pub(super) fn finish(
        self,
        valuation_present: bool,
        open_groups: Vec<(Option<i64>, f64, i64)>,
        counted_cores: u64,
    ) -> ReportTotals {
        let mut quotes = QuoteBreakdown::from_groups(self.groups.iter().copied())
            .with_traded_volume(super::super::TradedVolume::from_groups(
                self.volume_groups.iter().copied(),
            ))
            .with_entry_spend(super::super::EntrySpend::from_groups(
                self.spend_groups.iter().copied(),
            ));
        quotes.counted_cores = counted_cores;
        quotes.source_groups = Box::new(super::super::SourceGroups {
            profit: self.groups,
            coverage: self.coverage_groups,
            volume: self.volume_groups,
            spend: self.spend_groups,
        });
        // Publish coverage whenever the selected mode can build a projection: always for current
        // rates, and only with an attached cache for historical rates.
        ReportTotals {
            quotes: if valuation_present {
                quotes.with_valuation(self.coverage.finish())
            } else {
                quotes
            },
            open: super::super::OpenPositions::from_groups(open_groups),
        }
    }
}

/// Decode one value the way `Row::get` decodes the same column.
fn get<T: FromSql>(values: &[Value], index: usize) -> rusqlite::Result<T> {
    let value = &values[index];
    T::column_result(ValueRef::from(value)).map_err(|error| match error {
        FromSqlError::InvalidType => {
            rusqlite::Error::InvalidColumnType(index, String::new(), value.data_type())
        }
        other => {
            rusqlite::Error::FromSqlConversionFailure(index, value.data_type(), Box::new(other))
        }
    })
}

/// One slice of a totals read: the base filter narrowed to some cores and some period.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TotalsSlice {
    /// Cores the slice covers, as [`ReportFilter::core_uids`] states them; `None` keeps the base
    /// filter's own scope.
    pub core_uids: Option<Vec<u64>>,
    /// Lower bound, true-UTC seconds, inclusive.
    pub date_from: Option<i64>,
    /// Upper bound, true-UTC seconds, inclusive.
    pub date_to: Option<i64>,
}

impl TotalsSlice {
    /// The complete filter this slice stands for.
    fn filter(&self, base: &ReportFilter) -> ReportFilter {
        ReportFilter {
            core_uids: self
                .core_uids
                .clone()
                .unwrap_or_else(|| base.core_uids.clone()),
            date_from: self.date_from,
            date_to: self.date_to,
            ..base.clone()
        }
    }
}

/// Closed-row totals of many slices of one filter, each exactly what
/// [`query_totals`](super::query_totals) states for that slice's filter.
///
/// One row pass serves every slice when the base filter is a closed-row, close-date read and
/// every slice lies inside it: no slice core outside the base scope, no slice bound outside the
/// base period. A slice holding a sum [`SqliteSum`] cannot restate exactly -- a TEXT money cell,
/// an integer overflow, a total whose value could depend on the order SQLite fed its rows -- is
/// read by its own `query_totals`; so is every slice when the pass meets a row it cannot place
/// (a non-integer quote key or core, a close date it cannot compare exactly) or the clock offsets
/// move under it, and whenever the single pass does not apply at all. The answer never depends on
/// which path produced it.
///
/// Args:
///     conn: Open report reader or snapshot.
///     base: The filter every slice narrows.
///     slices: The slices, answered in this order.
///
/// Returns:
///     One totals per slice.
///
/// Errors:
///     Returns `Failed` when source discovery or any statement fails.
pub fn query_totals_sliced(
    conn: &Connection,
    base: &ReportFilter,
    slices: &[TotalsSlice],
) -> ReadResult<Vec<ReportTotals>> {
    if one_pass_serves(base, slices) {
        let meta = report_strategy_meta(conn, base)
            .map_err(|error| read_fail("reports: resolve strategy mask", error))?;
        let sources = read_sources_res(conn)?;
        let now = crate::util::now_unix_ms_i64().div_euclid(1_000);
        let sliced = with_valuation_fallback(
            conn,
            "reports: query_totals_sliced",
            "reports: query_totals_sliced native retry",
            |include_valuation| {
                sliced_attempt(conn, base, slices, &sources, &meta, include_valuation, now)
            },
        )?;
        if let Some(totals) = sliced {
            return totals
                .into_iter()
                .zip(slices)
                .map(|(totals, slice)| match totals {
                    Some(totals) => Ok(totals),
                    None => super::query_totals(conn, &slice.filter(base)),
                })
                .collect();
        }
    }
    slices
        .iter()
        .map(|slice| super::query_totals(conn, &slice.filter(base)))
        .collect()
}

/// Whether one row pass under `base` sees every row any slice admits.
fn one_pass_serves(base: &ReportFilter, slices: &[TotalsSlice]) -> bool {
    if base.rows != RowScope::Closed || base.period_basis != PeriodBasis::CloseDate {
        return false;
    }
    let scope: HashSet<u64> = base.core_uids.iter().copied().collect();
    slices.iter().all(|slice| {
        let cores_inside = match &slice.core_uids {
            None => true,
            Some(_) if scope.is_empty() => true,
            Some(cores) => !cores.is_empty() && cores.iter().all(|core| scope.contains(core)),
        };
        let from_inside = match (base.date_from, slice.date_from) {
            (None, _) => true,
            (Some(base), Some(from)) => from >= base,
            (Some(_), None) => false,
        };
        let to_inside = match (base.date_to, slice.date_to) {
            (None, _) => true,
            (Some(base), Some(to)) => to <= base,
            (Some(_), None) => false,
        };
        cores_inside && from_inside && to_inside
    })
}

/// SQLite's numeric comparison of a close date with an integer bound, where it is exact.
fn compare(close: &Value, bound: i64) -> Option<std::cmp::Ordering> {
    match *close {
        Value::Integer(close) => Some(close.cmp(&bound)),
        // Exact while the bound is an integer a double holds; any real bound is.
        Value::Real(close) if (bound as f64) as i64 == bound && bound.unsigned_abs() < 1 << 53 => {
            close.partial_cmp(&(bound as f64))
        }
        _ => None,
    }
}

/// Sum every slice from one row pass per source.
///
/// Returns:
///     Per slice, its totals or `None` when one of its sums cannot be restated exactly; `None`
///     overall when the pass met a row it cannot place, so the caller asks SQLite slice by slice.
fn sliced_attempt(
    conn: &Connection,
    base: &ReportFilter,
    slices: &[TotalsSlice],
    sources: &[ReadSource],
    meta: &StrategyMeta,
    include_valuation: bool,
    now: i64,
) -> rusqlite::Result<Option<Vec<Option<ReportTotals>>>> {
    let valuation_present = base.valuation == ValuationMode::Current || include_valuation;
    let passes = sources
        .iter()
        .map(|src| ClosedPass::new(src, base, include_valuation, meta))
        .collect::<Vec<_>>();
    let mut indices: HashMap<i64, slice_index::SliceIndex> = HashMap::new();
    let mut matching = Vec::new();
    // slice -> source -> quote key -> one fold per column
    let mut folds: Vec<Vec<BTreeMap<GroupKey, Vec<SqliteSum>>>> =
        vec![vec![BTreeMap::new(); passes.len()]; slices.len()];
    // A slice one of whose sums the fold refused; its own statement answers it instead.
    let mut refused = vec![false; slices.len()];
    // Same gate as the grouped read: only an empty core list has no cardinality of its own.
    let count_cores: Vec<bool> = slices
        .iter()
        .map(|slice| slice.filter(base).core_uids.is_empty())
        .collect();
    let mut counted_cores: Vec<HashSet<i64>> = vec![HashSet::new(); slices.len()];
    for (source, pass) in passes.iter().enumerate() {
        let has_close = pass.src.cols.contains("closedate");
        let mut stmt = conn.prepare(&pass.row_sql())?;
        let mut rows = stmt.query(pass.refs().as_slice())?;
        let width = pass.columns.len();
        let mut values = Vec::with_capacity(width);
        while let Some(row) = rows.next()? {
            let Some(key) = GroupKey::of(&row.get::<_, Value>(0)?) else {
                return Ok(None);
            };
            let Value::Integer(core) = row.get::<_, Value>(1)? else {
                return Ok(None);
            };
            let close = row.get::<_, Value>(2)?;
            values.clear();
            for index in 0..width {
                values.push(row.get::<_, Value>(3 + index)?);
            }
            let index = indices.entry(core).or_insert_with(|| {
                slice_index::SliceIndex::new(slices, core, core_offset(&base.axis, core, now))
            });
            if index
                .matching(has_close.then_some(&close), &mut matching)
                .is_none()
            {
                return Ok(None);
            }
            for &index in &matching {
                let slice_folds = &mut folds[index];
                let refused = &mut refused[index];
                if *refused {
                    continue;
                }
                let sums = slice_folds[source]
                    .entry(key)
                    .or_insert_with(|| vec![SqliteSum::default(); width]);
                let failed = sums
                    .iter_mut()
                    .zip(&values)
                    .any(|(sum, value)| sum.step(value).is_err());
                *refused = failed;
                // A refused slice is answered by its own statement, so its set is discarded.
                if !failed && count_cores[index] && row_counted(pass, &values) {
                    counted_cores[index].insert(core);
                }
            }
        }
    }
    // Each statement of the base pass read the clock on its own. Had any measured core's offset
    // moved between this read's instant and now -- a core with rows or one whose rows it shifted
    // out -- the base pass and the slices could disagree about its window. An unmeasured core is
    // the identity at every instant.
    let later = crate::util::now_unix_ms_i64().div_euclid(1_000);
    if base.axis.measured_cores().into_iter().any(|core| {
        core_offset(&base.axis, core as i64, now) != core_offset(&base.axis, core as i64, later)
    }) {
        return Ok(None);
    }
    let mut totals = Vec::with_capacity(folds.len());
    for ((slice_folds, refused), cores) in folds.into_iter().zip(refused).zip(counted_cores) {
        totals.push(if refused {
            None
        } else {
            finish_slice(&passes, slice_folds, valuation_present, cores.len() as u64)?
        });
    }
    Ok(Some(totals))
}

/// Whether this row-pass value vector is a counted entry-spend row.
///
/// The row SQL selects `CASE WHEN counted THEN 1 ELSE 0 END`, an integer. A real or a missing
/// column is not that flag.
fn row_counted(pass: &ClosedPass<'_>, values: &[Value]) -> bool {
    matches!(
        values.get(pass.counted_value_index()),
        Some(Value::Integer(n)) if *n > 0
    )
}

/// One slice's totals from its folds, or `None` when one of its sums cannot be restated.
fn finish_slice(
    passes: &[ClosedPass<'_>],
    slice_folds: Vec<BTreeMap<GroupKey, Vec<SqliteSum>>>,
    valuation_present: bool,
    counted_cores: u64,
) -> rusqlite::Result<Option<ReportTotals>> {
    let mut sink = TotalsSink::default();
    // Sources in discovery order and keys ascending: the order the grouped statements return
    // their groups, which the floating-point folds in `TotalsSink` depend on.
    for (pass, groups) in passes.iter().zip(slice_folds) {
        for (key, sums) in groups {
            let mut group = Vec::with_capacity(1 + sums.len());
            group.push(key.value());
            for (column, sum) in pass.columns.iter().zip(&sums) {
                match column.finish(sum) {
                    Ok(value) => group.push(value),
                    Err(_) => return Ok(None),
                }
            }
            sink.add_group(&group, pass.valued)?;
        }
    }
    Ok(Some(sink.finish(
        valuation_present,
        Vec::new(),
        counted_cores,
    )))
}

/// The offset `append_row_scope` shifts this core's bounds by: its measured offset at `now`, or
/// the identity for a core never measured.
fn core_offset(axis: &ReportAxis, core: i64, now: i64) -> i32 {
    axis.groups(&[core as u64], now)
        .first()
        .map_or(0, |(offset, _)| *offset)
}

#[cfg(test)]
mod tests;
