use super::*;

/// How many minutes a discovered quote-ordinal set is reused before the report is re-scanned.
///
/// The SET of currencies traded changes on the scale of a user adding a core, so re-deriving it
/// once per pass would pay a `DISTINCT` scan over the whole replica to learn nothing.
const CURRENT_SCAN_MINUTES: i64 = 10;

/// How many minutes pass between two refreshes of the gathered rates.
///
/// Half the freshness window, so a rate is replaced well before [`crate::db::valuation::FRESHNESS_MS`] can retire
/// it and one failed pass still cannot expire one. A shorter interval adds provider traffic without
/// improving the ten-minute freshness contract and creates more opportunities for visible changes
/// that make every open Report host and the Analytics window requery.
pub(in crate::db::valuation::worker) const CURRENT_REFRESH_MINUTES: i64 = 5;

/// One in-flight current-rate refresh pass.
///
/// Rates and permanent misses persist ACROSS passes: a refresh pass whose provider call failed
/// should keep serving the last good number until the freshness cutoff retires it, rather than
/// blanking every figure on one bad request.
#[derive(Default)]
pub(in crate::db::valuation::worker) struct CurrentRateState {
    /// Minute the most recent pass was armed in, used by the five-minute refresh throttle.
    pub(in crate::db::valuation::worker) minute: Option<i64>,
    /// Ordinals still to resolve in this pass, popped from the back.
    pub(in crate::db::valuation::worker) pending: Vec<i64>,
    /// Publish healthy currencies incrementally after any provider failure in this pass.
    pub(in crate::db::valuation::worker) had_outage: bool,
    /// Next reverse queue offset, advanced on failure without discarding the currency.
    pub(in crate::db::valuation::worker) next_offset: usize,
    /// Latest known rate per quote ordinal.
    pub(in crate::db::valuation::worker) rates: BTreeMap<i64, crate::db::valuation::CurrentRate>,
    /// Ordinals whose direct and inverse routes were all last classified as permanently absent.
    pub(in crate::db::valuation::worker) missing: BTreeSet<i64>,
    /// Minute the ordinal set was last discovered in.
    pub(in crate::db::valuation::worker) scanned: Option<i64>,
    /// Quote ordinals the report actually uses.
    pub(in crate::db::valuation::worker) ordinals: Vec<i64>,
    /// The most recently stored rates and permanent misses, used as the render-equivalence
    /// baseline for the next snapshot.
    ///
    /// Held to answer one question: does this snapshot render differently from the last stored
    /// one? A stored snapshot that caused no wake is render-equivalent to what the surfaces drew;
    /// its timestamps may move even when the rendered figures do not.
    pub(in crate::db::valuation::worker) published: Option<(
        BTreeMap<i64, crate::db::valuation::CurrentRate>,
        BTreeSet<i64>,
    )>,
}

impl CurrentRateState {
    /// Requeue healthy currencies ahead of unresolved retries so their rates stay fresh.
    pub(in crate::db::valuation::worker) fn rearm(&mut self, minute: i64) {
        self.minute = Some(minute);
        let mut added = self
            .ordinals
            .iter()
            .copied()
            .filter(|ordinal| !self.pending.contains(ordinal))
            .collect::<Vec<_>>();
        if !added.is_empty() {
            self.next_offset = self.pending.len();
        }
        added.append(&mut self.pending);
        self.pending = added;
    }

    /// Abandon the pass in flight because current-rate mode is disabled.
    ///
    /// The gathered rates are kept: switching the mode back on should reuse whatever is still
    /// inside the freshness window instead of blanking every figure until the next pass completes.
    pub(in crate::db::valuation::worker) fn stand_down(&mut self) {
        self.minute = None;
        self.pending.clear();
        self.had_outage = false;
        self.next_offset = 0;
    }

    /// Whether this snapshot would render differently from the last published one.
    ///
    /// A raw `fetched_at_ms` comparison is deliberately NOT the answer: it moves on every pass and
    /// feeds only the freshness cutoff, so a pegged quote whose price came back identical would
    /// otherwise cost a requery for nothing. What DOES matter is which quotes that cutoff still
    /// admits at `now_ms`, because a quote outside it converts nothing and renders as uncovered.
    /// The two differ whenever a pass outlives the window it is refreshing: each currency may cost
    /// four sequential provider routes, and a transient failure adds the stage's backoff on top, so
    /// a rate published before the pass began can expire for readers while the pass is still
    /// running. Without the freshness comparison, finishing that pass with unchanged prices would
    /// withhold the bump and leave the screen reading "uncovered" over a snapshot that covers it.
    ///
    /// Args:
    ///     now_ms: Instant the freshness cutoff is judged at.
    ///
    /// Returns:
    ///     `true` when a rate, its provenance, the permanently-missing set, or the set of quotes
    ///     still inside the freshness window changed.
    pub(in crate::db::valuation::worker) fn renders_differently(&self, now_ms: i64) -> bool {
        let Some((rates, missing)) = &self.published else {
            return true;
        };
        // One traversal answers both halves: a differing key set shows up as a `None` lookup,
        // and a rate that crossed the cutoff since the last publication differs in freshness while
        // matching in price.
        *missing != self.missing
            || rates.len() != self.rates.len()
            || self.rates.iter().any(|(ordinal, rate)| {
                rates.get(ordinal).is_none_or(|previous| {
                    !rate.renders_same(previous)
                        || rate.is_fresh(now_ms) != previous.is_fresh(now_ms)
                })
            })
    }

    /// Drop every gathered rate that has aged past the freshness window.
    ///
    /// Args:
    ///     now_ms: Current wall clock in Unix milliseconds.
    ///
    /// Returns:
    ///     Whether anything was dropped, so the caller can republish.
    pub(in crate::db::valuation::worker) fn expire_stale(&mut self, now_ms: i64) -> bool {
        let before = self.rates.len();
        self.rates.retain(|_, rate| rate.is_fresh(now_ms));
        self.rates.len() != before
    }

    /// When the oldest gathered rate stops counting as current.
    ///
    /// Returns:
    ///     The earliest expiry instant, or `None` while nothing is held.
    pub(in crate::db::valuation::worker) fn next_expiry_ms(&self) -> Option<i64> {
        self.rates
            .values()
            .map(|rate| {
                rate.fetched_at_ms
                    .saturating_add(crate::db::valuation::FRESHNESS_MS)
            })
            .min()
    }

    /// Publish the gathered snapshot, and wake the surfaces only when it reads differently.
    ///
    /// The snapshot itself is stored unconditionally, refreshed timestamps included: the SQL
    /// builders judge freshness against what is stored, so withholding an unchanged snapshot would
    /// expire rates the worker had in fact just re-fetched.
    ///
    /// The generation bump is separate, and conditional, because it is the half that costs the
    /// user something: every open Report host and the Analytics window requery on it, and asking
    /// them to redraw the same numbers is flicker with nothing behind it. An expiry DOES move the
    /// figures — the retired quote falls back to uncovered — and `renders_differently` sees that
    /// as a shrunken map, so the cutoff still reaches the screen on schedule.
    ///
    /// Args:
    ///     generation: Monotonic valuation publication counter.
    ///     dirty: Coalescing UI wake edge.
    ///     now_ms: Instant the freshness cutoff is judged at.
    pub(in crate::db::valuation::worker) fn publish_snapshot(
        &mut self,
        generation: &AtomicU64,
        dirty: &AtomicBool,
        now_ms: i64,
    ) {
        let changed = self.renders_differently(now_ms);
        crate::db::valuation::publish_current_rates(crate::db::valuation::CurrentRates::new(
            self.rates.clone(),
            self.missing.clone(),
        ));
        self.published = Some((self.rates.clone(), self.missing.clone()));
        if changed {
            // Through the DATA generation, not the health revision: this is a new set of numbers
            // to render, and the health counter deliberately never triggers a requery.
            publish(generation, dirty);
        }
    }
}

/// Discover which quote currencies the report actually contains.
///
/// Bounded by construction: the persisted ordinal contract is 0..=20, so this returns at most
/// twenty-one values however large the replica is.
///
/// COST: no index covers `basecurrency` on either source, so this is a full scan of the replica —
/// on a large one, tens to hundreds of milliseconds. It is bounded by the caller's
/// [`CURRENT_SCAN_MINUTES`] throttle and by the demand flag, so it runs at most once every ten
/// minutes and only while current-rate mode is enabled; it also runs on the worker
/// thread against its own reader, never on the UI thread. Deliberately NOT worth an index: that
/// would cost write amplification on every replicated trade to serve an opt-in feature.
///
/// The scan reads the EFFECTIVE ordinal, so a COIN-M row contributes BTC and the worker fetches the
/// rate that row is actually denominated in. That expression carries its own storage-class guard;
/// the Rust-side decode below stays as the readers' own contract, not as a second filter.
///
/// Args:
///     conn: Open report reader.
///
/// Returns:
///     Distinct known quote ordinals other than identity USDT, which needs no rate.
///
/// Errors:
///     Returns a classified report read failure when source discovery or the scan fails.
pub(in crate::db::valuation::worker) fn report_quote_ordinals(
    conn: &Connection,
) -> ReadResult<Vec<i64>> {
    const CTX: &str = "valuation: quote scan";
    let mut found = BTreeSet::new();
    for src in crate::db::read_sources_res(conn)? {
        if !src.cols.contains("basecurrency") {
            continue;
        }
        let sql = format!(
            "SELECT DISTINCT ({quote}) FROM {} r",
            src.table,
            quote = crate::db::quote::effective_ordinal_expr("r", &src.cols)
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|error| crate::db::read_fail::read_fail(CTX, error))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, rusqlite::types::Value>(0))
            .map_err(|error| crate::db::read_fail::read_fail(CTX, error))?;
        for raw in rows {
            let raw = raw.map_err(|error| crate::db::read_fail::read_fail(CTX, error))?;
            // The readers' own decode: a placeholder, a REAL, a TEXT or a future ordinal resolves
            // to no currency at all. Identity USDT is excluded separately — it needs no rate.
            let Some(currency) = crate::db::QuoteCurrency::from_report_value(&raw) else {
                continue;
            };
            let rusqlite::types::Value::Integer(ordinal) = raw else {
                continue;
            };
            if currency != crate::db::QuoteCurrency::usdt() {
                found.insert(ordinal);
            }
        }
    }
    Ok(found.into_iter().collect())
}

/// Whether a fresh refresh pass may be armed.
///
/// Split out of [`refresh_current_rates`] because both operands are UNIX SECONDS — the unit
/// [`current_minute_utc`] hands back — while the interval beside it is written in minutes. Keeping
/// the conversion in one named place with its own test is what stops the two from being compared
/// directly, which silently turns the whole throttle into "every minute".
///
/// A pass still working through its queue is never re-armed on top of: a slow provider must not
/// have its sweep restarted from the beginning underneath it.
///
/// Args:
///     minute: Current UTC minute, in Unix seconds.
///     armed: Minute the pass in flight was armed at, in Unix seconds.
///     pending_empty: Whether the previous pass has drained.
///
/// Returns:
///     `true` when a new pass should be armed for this minute.
pub(in crate::db::valuation::worker) fn refresh_is_due(
    minute: i64,
    armed: Option<i64>,
    pending_empty: bool,
) -> bool {
    pending_empty && minutes_elapsed(minute, armed, CURRENT_REFRESH_MINUTES)
}

/// Whether at least `minutes` have passed since `since`.
///
/// Both instants are UNIX SECONDS — the unit [`current_minute_utc`] hands back — while every
/// interval beside them is written in minutes. This is the one place the two units meet.
///
/// Args:
///     now: Current UTC minute, in Unix seconds.
///     since: Instant to measure from, in Unix seconds; `None` counts as elapsed.
///     minutes: Interval to clear.
///
/// Returns:
///     `true` when the interval has passed, or nothing has happened yet.
fn minutes_elapsed(now: i64, since: Option<i64>, minutes: i64) -> bool {
    since.is_none_or(|since| now - since >= minutes * 60)
}

/// Resolve one quote currency's latest rate, at most one per turn.
///
/// One currency per turn on purpose. A cold pass over K currencies costs up to K x 4 sequential
/// provider routes at a fifteen-second timeout each, and doing them in one turn would park the
/// reconciliation and outbox stages behind minutes of network wait.
///
/// The newest eligible minute is the previous one because the current candle has not closed. A
/// bounded lookback admits the latest known closed price when a sparse market had no trade in that
/// exact minute; historical valuation keeps its separate exact-minute resolver.
///
/// Args:
///     source: Spot candle boundary.
///     generation: Monotonic valuation publication counter.
///     dirty: Coalescing UI wake edge.
///     state: Pass state carried across turns.
///
/// Returns:
///     The stage outcome: awaiting the report replica, drained, or still carrying queued currencies.
///
/// Errors:
///     Returns a provider fault on a transient failure, leaving the currency queued for retry.
pub(in crate::db::valuation::worker) fn refresh_current_rates(
    source: &dyn SpotRateSource,
    generation: &AtomicU64,
    dirty: &AtomicBool,
    state: &mut CurrentRateState,
) -> Result<StageTurn, FaultCause> {
    let minute = current_minute_utc();
    if refresh_is_due(
        minute,
        state.minute,
        state.pending.is_empty() || state.had_outage,
    ) {
        if minutes_elapsed(minute, state.scanned, CURRENT_SCAN_MINUTES) {
            let conn = match crate::db::open_reader() {
                Ok(conn) => conn,
                Err(ReadFail::NotReady) => return Ok(StageTurn::AwaitingReplica),
                Err(error) => return Err(report_fault(error)),
            };
            state.ordinals = report_quote_ordinals(&conn).map_err(report_fault)?;
            state.scanned = Some(minute);
        }
        state.rearm(minute);
    }
    resolve_next_rate(source, generation, dirty, state, minute)
}

/// Resolve the next queued currency of an already-scoped pass.
///
/// Split from [`refresh_current_rates`] so the expensive half — how many provider calls one turn
/// costs, and when the pass stops asking — can be exercised without opening the report replica.
///
/// Args:
///     source: Spot candle boundary.
///     generation: Monotonic valuation publication counter.
///     dirty: Coalescing UI wake edge.
///     state: Pass state carried across turns.
///     minute: Minute this pass belongs to.
///
/// Returns:
///     Whether more currencies remain in this pass.
///
/// Errors:
///     Returns a provider fault on a transient failure, leaving the currency queued for retry.
pub(in crate::db::valuation::worker) fn resolve_next_rate(
    source: &dyn SpotRateSource,
    generation: &AtomicU64,
    dirty: &AtomicBool,
    state: &mut CurrentRateState,
    minute: i64,
) -> Result<StageTurn, FaultCause> {
    if state.pending.is_empty() {
        return Ok(StageTurn::Drained);
    }
    let index = state.pending.len() - 1 - state.next_offset % state.pending.len();
    let ordinal = state.pending[index];
    let ticker = crate::db::QuoteCurrency::from_report_ordinal(ordinal)
        .map(|currency| currency.ticker())
        .unwrap_or("UNKNOWN");
    let (oldest_minute, newest_minute) = current_rate_window(minute);
    match resolve_latest_rate(source, ordinal, ticker, oldest_minute, newest_minute) {
        Ok(rate) => {
            state.missing.remove(&ordinal);
            state.rates.insert(
                ordinal,
                crate::db::valuation::CurrentRate {
                    rate_usdt: rate.rate_usdt,
                    provider: rate.provider,
                    symbol: rate.symbol,
                    fetched_at_ms: now_unix_ms_i64(),
                },
            );
        }
        Err(FetchFailure::Missing) => {
            // Every route is permanently absent for this currency. Drop any rate it once had, so a
            // figure cannot keep rendering from a price the exchange has since delisted.
            state.rates.remove(&ordinal);
            state.missing.insert(ordinal);
        }
        // Retain the failed currency but advance the cursor so independent currencies can run
        // after the stage backoff. Published partial progress keeps their rates available.
        Err(FetchFailure::Unavailable(message) | FetchFailure::Transient(message)) => {
            state.next_offset = (state.next_offset + 1) % state.pending.len();
            state.had_outage = true;
            if !state.rates.is_empty() {
                state.publish_snapshot(generation, dirty, now_unix_ms_i64());
            }
            return Err(FaultCause::new(FailureKind::Provider, message));
        }
    }
    state.pending.remove(index);
    state.next_offset = 0;
    if state.had_outage {
        state.publish_snapshot(generation, dirty, now_unix_ms_i64());
    }
    if state.pending.is_empty() {
        state.had_outage = false;
        state.publish_snapshot(generation, dirty, now_unix_ms_i64());
        Ok(StageTurn::Drained)
    } else {
        Ok(StageTurn::Ran { more: true })
    }
}

/// Bound one current-rate lookup to fully closed candles inside the freshness window.
///
/// Args:
///     minute: Current UTC minute start in Unix seconds.
///
/// Returns:
///     Inclusive oldest and newest eligible candle-open minutes. The range contains ten one-minute
///     opens when [`crate::db::valuation::FRESHNESS_MS`] is ten minutes.
fn current_rate_window(minute: i64) -> (i64, i64) {
    (
        minute.saturating_sub(crate::db::valuation::FRESHNESS_MS.div_euclid(1_000)),
        minute.saturating_sub(60),
    )
}
