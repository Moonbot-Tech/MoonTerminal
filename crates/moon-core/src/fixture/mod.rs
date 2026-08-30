//! Committed chart bench: a frozen day of candles and closed trades the application can open
//! without a core, a network, or a live exchange.
//!
//! The problem it solves: everything the chart draws on top of price — trade markers, order lines,
//! drawing figures — can only be judged by eye, and on a live core the market differs between two
//! runs, so "before" and "after" are never the same picture. The bench replaces the variable half
//! with a fixed one: the same candles and the same closed trades every time.
//!
//! Layout: `fixtures/<name>/` holds `reports.sqlite` (the typed replica schema, so
//! `db::query_chart_trade_history` reads it unchanged) and `klines.sqlite` (the `chunks` schema of
//! [`crate::market::kline_cache`]). Both are committed and anonymised; see `docs-internal/FIXTURES.md`.
//!
//! Why a COPY into a temporary directory: the running application writes to its data root — layout,
//! docks, logs, the report writer's own WAL. Pointing the data root at the working tree would leave
//! the fixture modified after every run and the repository dirty. [`prepare`] copies first and
//! points [`crate::config::paths`] at the copy.
//!
//! This module opens no worker threads and installs nothing global except the data-root override
//! and [`ACTIVE`]. That is deliberate: the caller sets environment variables right after it, and
//! `set_var` is only sound while the process is still single-threaded.

use std::path::{Path, PathBuf};

mod figures;

use crate::market::candles::{ChartCandle, resample};

/// Directory holding the committed fixture sets inside the working tree.
const FIXTURES_DIR: &str = "fixtures";

/// Database file names copied into the throwaway data root.
const DB_FILES: [&str; 2] = ["reports.sqlite", "klines.sqlite"];

/// Sidecars SQLite may leave beside a database; a stale one would be replayed onto the fresh copy.
const DB_SIDECARS: [&str; 2] = ["-wal", "-shm"];

/// Milliseconds in one day, the unit candle chunks are keyed by.
const DAY_MS: i64 = 86_400_000;

/// Bytes per packed candle row: `u32` offset within the day plus five `f32` fields.
const ROW_BYTES: usize = 24;

/// How far the bench's newest data is placed BEFORE the current moment.
///
/// The chart follows the live edge, so landing the last trade exactly on "now" would put it under
/// the right margin. A few minutes of clearance puts the closing trades on screen instead.
const HEAD_ROOM_SECS: i64 = 300;

/// The one bench installed for this process, if any.
static ACTIVE: std::sync::OnceLock<ChartFixture> = std::sync::OnceLock::new();

/// Price fields of one served window: the last close and the low/high the Y fit needs.
pub type WindowPrices = (Option<f32>, Option<(f32, f32)>);

/// Repacked candle rows grouped by their destination chunk, then by in-day offset.
type RegroupedChunks = std::collections::BTreeMap<
    (String, String, u32, i64),
    std::collections::BTreeMap<u32, RowValues>,
>;

/// One packed candle row without its leading offset: five `f32` fields.
type RowValues = [u8; ROW_BYTES - 4];

/// One window already served, kept so a repeat read costs no database round trip.
///
/// The chart re-reads on ordinary panning while the revision only moves when the window crosses a
/// timeframe bucket. Without this, every one of those reads returned "nothing changed" WITH empty
/// price fields, and the caller stores those unconditionally — which wiped the chart's Y reference
/// and its last price on the very next frame.
#[derive(Clone, Copy)]
struct Served {
    revision: u64,
    last_price: Option<f32>,
    price_range: Option<(f32, f32)>,
}

/// An opened chart bench: where its copy lives and what it can answer.
pub struct ChartFixture {
    /// Throwaway data root this run writes into.
    root: PathBuf,
    /// Exchange key the candle chunks are stored under, as `code:dex`.
    exchange: String,
    /// The single market the bench carries.
    market: String,
    /// Newest close in the bench, for anything that needs a plausible live price.
    last_price: f32,
    /// Seconds every timestamp was moved by, for the startup log.
    shift_secs: i64,
    /// Drawing tools laid onto the bench, one per tool the build knows.
    figures: usize,
    /// Most recently served window; see [`Served`].
    served: std::sync::Mutex<Option<Served>>,
}

impl ChartFixture {
    /// The market this bench can draw, e.g. `ACEUSDT`.
    pub fn market(&self) -> &str {
        &self.market
    }

    /// Exchange key the candle chunks are stored under.
    pub fn exchange(&self) -> &str {
        &self.exchange
    }

    /// Throwaway data root, for logging and for tests that inspect what was copied.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Newest close in the bench: the price a synthetic tick stream should start from, so the
    /// ticker, the order book and the chart do not disagree by orders of magnitude.
    pub fn last_price(&self) -> f32 {
        self.last_price
    }

    /// Seconds every timestamp was moved forward when the bench was prepared.
    pub fn shift_secs(&self) -> i64 {
        self.shift_secs
    }

    /// Whether `market` is the one the bench carries, ignoring case as the catalog does.
    pub fn covers(&self, market: &str) -> bool {
        self.market.eq_ignore_ascii_case(market)
    }

    /// Write the one line that says a bench is installed and what it holds.
    ///
    /// Called by the application AFTER its logger exists — `prepare` runs before it, so a line
    /// emitted there goes nowhere. It lives in this crate rather than at the call site because the
    /// default filter carries `moon_core` records into the log file and the shell crate's do not
    /// reach it.
    pub fn announce(&self) {
        log::info!(
            "стенд {}: биржа {}, сдвиг {} с, цена {}, фигур {}, данные в {}",
            self.market,
            self.exchange,
            self.shift_secs,
            self.last_price,
            self.figures,
            self.root.display()
        );
    }

    /// Price fields of an already-served window, when its revision still matches.
    pub fn served_window(&self, revision: u64) -> Option<WindowPrices> {
        let served = self.served.lock().unwrap_or_else(|e| e.into_inner());
        served
            .filter(|s| s.revision == revision)
            .map(|s| (s.last_price, s.price_range))
    }

    /// Record the window just served so a repeat read can answer from memory.
    pub fn remember_window(
        &self,
        revision: u64,
        last_price: Option<f32>,
        price_range: Option<(f32, f32)>,
    ) {
        let mut served = self.served.lock().unwrap_or_else(|e| e.into_inner());
        *served = Some(Served {
            revision,
            last_price,
            price_range,
        });
    }

    /// Candles for one timeframe over `[from_ms, to_ms]`, oldest first.
    ///
    /// The bench stores only the two kinds the recorder produces (1 and 5 minutes); coarser
    /// timeframes are resampled from the finest base that divides them, exactly as the live chart
    /// resamples a coarser panel from the core's finer series.
    ///
    /// Args:
    ///     cache: Cache already opened on the bench's copy by the application.
    ///     tf_ms: Requested series timeframe in milliseconds.
    ///     from_ms: Inclusive lower bound on candle open time.
    ///     to_ms: Inclusive upper bound on candle open time.
    ///
    /// Returns:
    ///     The series, empty when the bench holds nothing in that range.
    pub fn candles(
        &self,
        cache: &crate::market::kline_cache::KlineCache,
        tf_ms: i64,
        from_ms: i64,
        to_ms: i64,
    ) -> Vec<ChartCandle> {
        let tf_ms = tf_ms.max(60_000);
        let base_kind = base_kind_for(tf_ms);
        let base_tf_ms = base_kind as i64 * 60_000;
        // Align to the REQUESTED timeframe, not the base one: a 30-minute bar assembled from the
        // six 5-minute rows that survive a mid-bucket cut opens at the wrong price and reports a
        // truncated high and low. Aligning to the base would leave exactly that cut in place.
        let aligned_from = from_ms.div_euclid(tf_ms) * tf_ms;
        let base = cache
            .read_range(&self.exchange, &self.market, base_kind, aligned_from, to_ms)
            .unwrap_or_default();
        if base.is_empty() || tf_ms <= base_tf_ms {
            return base;
        }
        let mut out = Vec::new();
        resample(&base, tf_ms, &mut out);
        out
    }
}

/// Finest stored kind, in minutes, that divides `tf_ms`.
///
/// The bench stores kinds 1 and 5. Anything from 5 minutes up is built from the 5-minute base,
/// which covers 15 days instead of the 1-minute base's 2 — depth matters more than resolution once
/// a bar is wider than the base.
fn base_kind_for(tf_ms: i64) -> u32 {
    if tf_ms < 300_000 { 1 } else { 5 }
}

/// Copy the named fixture to a private directory and make it this process's data root.
///
/// Installs the result as the process-wide [`active`] bench. Call once, before anything resolves a
/// data path, and before the process starts a second thread.
///
/// Args:
///     name: Fixture directory name under `fixtures/`, e.g. `chart-ace`.
///
/// Returns:
///     The opened bench.
///
/// Errors:
///     The name is not a plain directory name, the fixture cannot be located or is incomplete, a
///     copy fails, the data-root override was already installed, or the databases are unusable.
pub fn prepare(name: &str) -> anyhow::Result<&'static ChartFixture> {
    validate_name(name)?;
    let source = locate(name).ok_or_else(|| {
        anyhow::anyhow!(
            "фикстура {name:?} не найдена: положите её в {FIXTURES_DIR}/{name} рядом с exe, \
             в корне репозитория или укажите MOON_FIXTURE_DIR"
        )
    })?;
    let root = std::env::temp_dir().join(format!(
        "moonterminal-fixture-{name}-{}",
        std::process::id()
    ));
    // A leftover directory from a previous run that reused this pid would mix two states. Its
    // failure is reported rather than swallowed: continuing would silently replay the old state.
    if root.exists() {
        std::fs::remove_dir_all(&root).map_err(|e| {
            anyhow::anyhow!("не удалил прошлый каталог стенда {}: {e}", root.display())
        })?;
    }
    std::fs::create_dir_all(&root)?;
    // The override goes in BEFORE the copy so the database directory comes from `paths::db_dir`
    // rather than from a second copy of the platform layout rule.
    if !crate::config::paths::set_data_dir_override(root.clone()) {
        anyhow::bail!("каталог данных уже переопределён — фикстура запрошена слишком поздно");
    }
    let db_dir = crate::config::paths::db_dir();
    for file in DB_FILES {
        let from = source.join(file);
        if !from.exists() {
            anyhow::bail!("фикстура {name:?} неполна: нет {}", from.display());
        }
        let to = db_dir.join(file);
        // A sidecar left in the destination would be replayed onto the database we just copied.
        for suffix in DB_SIDECARS {
            let _ = std::fs::remove_file(format!("{}{suffix}", to.display()));
        }
        std::fs::copy(&from, &to)?;
    }
    let shift_secs = relocate_to_now(&db_dir)?;
    let (exchange, market) = describe(&db_dir.join("klines.sqlite"))?;
    let last_price = newest_close(&db_dir.join("klines.sqlite"))?;
    // Drawing tools are seeded AFTER the shift, from the window the shifted trades occupy, so
    // their nodes need no shifting of their own and cannot drift away from the candles.
    //
    // `MOON_FIXTURE_FIGURES=0` leaves them out. That switch is what makes the bench able to
    // ANSWER a question rather than only pose it: a render measurement taken with figures and
    // without them isolates their cost, and the two runs are otherwise byte-identical.
    let figures = if std::env::var("MOON_FIXTURE_FIGURES").as_deref() == Ok("0") {
        0
    } else {
        seed_figures(&db_dir.join("reports.sqlite"), &market)?
    };
    let fixture = ChartFixture {
        root,
        exchange,
        market,
        last_price,
        shift_secs,
        figures,
        served: std::sync::Mutex::new(None),
    };
    ACTIVE
        .set(fixture)
        .map_err(|_| anyhow::anyhow!("фикстура уже установлена"))?;
    Ok(ACTIVE.get().expect("fixture just installed"))
}

/// The bench installed for this process, or `None` in a normal run.
pub fn active() -> Option<&'static ChartFixture> {
    ACTIVE.get()
}

/// Reject anything that is not a plain directory name.
///
/// The name arrives from the command line and is joined into a path that is then REMOVED
/// recursively; `--fixture ..\..\something` must not be able to point that removal at a directory
/// outside the temporary root.
fn validate_name(name: &str) -> anyhow::Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && name != "."
        && name != ".."
        && !name.contains("..");
    if ok {
        Ok(())
    } else {
        anyhow::bail!("недопустимое имя фикстуры {name:?}: только буквы, цифры, -, _ и .")
    }
}

/// Move the copied bench forward so its newest trade sits just before the current moment.
///
/// A chart follows the live edge: it asks for a window around *now*, and a bench frozen on a past
/// date answers every one of those reads with nothing, which on screen reads as an empty chart
/// rather than as a bench that needs scrolling.
///
/// The shift is applied to the DATA, in seconds, so nothing about the application's follow logic,
/// its report query, or its Y fit has to know a bench exists. Candles are repacked rather than
/// re-keyed by whole days: a whole-day shift would preserve the fixture's time of day, and the
/// picture would then depend on the hour the bench was started — most trades still in the future
/// early in the UTC day, the whole day already over late in it.
///
/// Candles NEWER than the current moment are dropped: the fixture's candles run further than its
/// trades, and after the shift that surplus would be drawn to the right of "now".
///
/// Args:
///     db_dir: Directory holding the copied `reports.sqlite` and `klines.sqlite`.
///
/// Returns:
///     Seconds added to every timestamp.
///
/// Errors:
///     Propagates SQLite failures and an empty fixture; an unshifted bench renders empty.
fn relocate_to_now(db_dir: &Path) -> anyhow::Result<i64> {
    let reports = rusqlite::Connection::open(db_dir.join("reports.sqlite"))?;
    let last_trade: Option<i64> =
        reports.query_row("SELECT MAX(closedate) FROM orders_rep", [], |r| r.get(0))?;
    let Some(last_trade) = last_trade.filter(|t| *t > 0) else {
        anyhow::bail!("в reports.sqlite фикстуры нет ни одной закрытой сделки");
    };
    let now_secs = crate::util::now_unix_ms_i64() / 1_000;
    let shift_secs = now_secs - HEAD_ROOM_SECS - last_trade;

    // Each column is shifted only where it actually carries a time: a zero is a sentinel for "no
    // such leg", and moving it would date an open leg to 1970 plus the shift.
    reports.execute(
        "UPDATE orders_rep SET
             buydate     = CASE WHEN buydate     > 0 THEN buydate     + ?1 ELSE buydate     END,
             sellsetdate = CASE WHEN sellsetdate > 0 THEN sellsetdate + ?1 ELSE sellsetdate END,
             closedate   = CASE WHEN closedate   > 0 THEN closedate   + ?1 ELSE closedate   END",
        [shift_secs],
    )?;

    shift_candles(
        &db_dir.join("klines.sqlite"),
        shift_secs * 1_000,
        now_secs * 1_000,
    )?;
    Ok(shift_secs)
}

/// Shift every packed candle by `shift_ms`, dropping rows that would land after `now_ms`.
///
/// Rows store an offset WITHIN their day, so an arbitrary shift has to unpack, move, regroup by
/// the new day and repack. The whole rewrite runs in ONE transaction: a failure between the delete
/// and the insert would leave a copy that renders an empty chart instead of failing loudly.
fn shift_candles(klines: &Path, shift_ms: i64, now_ms: i64) -> anyhow::Result<()> {
    let mut conn = rusqlite::Connection::open(klines)?;
    let rows: Vec<(String, String, u32, i64, Vec<u8>)> = {
        // Ordered on purpose: the repacked blobs must come out time-sorted, because `resample`
        // assumes it and unordered input silently produces duplicate buckets.
        let mut stmt =
            conn.prepare("SELECT exchange, market, kind, day, rows FROM chunks ORDER BY day")?;
        let mapped = stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?;
        mapped.collect::<Result<_, _>>()?
    };

    // Regroup into the days the shifted rows belong to; a shift that is not a whole day moves the
    // tail of one day into the next.
    // Keyed by in-day offset rather than appended: a shift moves the tail of one day into the
    // next, so rows from two source days land in one destination day and only the map keeps them
    // in time order.
    let mut regrouped: RegroupedChunks = std::collections::BTreeMap::new();
    for (exchange, market, kind, day, blob) in rows {
        for chunk in blob.chunks_exact(ROW_BYTES) {
            let offset = u32::from_le_bytes(chunk[0..4].try_into().expect("4 bytes"));
            let shifted = day * DAY_MS + offset as i64 + shift_ms;
            if shifted > now_ms {
                continue;
            }
            let new_day = shifted.div_euclid(DAY_MS);
            let new_offset = (shifted - new_day * DAY_MS) as u32;
            let mut values: RowValues = [0u8; ROW_BYTES - 4];
            values.copy_from_slice(&chunk[4..ROW_BYTES]);
            regrouped
                .entry((exchange.clone(), market.clone(), kind, new_day))
                .or_default()
                .insert(new_offset, values);
        }
    }

    let tx = conn.transaction()?;
    tx.execute("DELETE FROM chunks", [])?;
    {
        let mut insert = tx.prepare(
            "INSERT INTO chunks(exchange, market, kind, day, rows, updated_ms)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for ((exchange, market, kind, day), rows) in &regrouped {
            let mut blob = Vec::with_capacity(rows.len() * ROW_BYTES);
            for (offset, values) in rows {
                blob.extend_from_slice(&offset.to_le_bytes());
                blob.extend_from_slice(values);
            }
            insert.execute(rusqlite::params![exchange, market, kind, day, blob, now_ms])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Lay one figure of every drawing tool over the window the bench's trades occupy.
///
/// The window comes from the trades rather than from the candles: the tools are there to be judged
/// against the markers, and a set spread over the whole candle history would put most of them off
/// screen.
///
/// Args:
///     reports: The copied report replica, already shifted.
///     market: Market the bench carries.
///
/// Returns:
///     How many figures were written.
///
/// Errors:
///     Propagates SQLite failures reading the window.
fn seed_figures(reports: &Path, market: &str) -> anyhow::Result<usize> {
    let conn = rusqlite::Connection::open(reports)?;
    let (core_uid, from_secs, to_secs, low, high): (i64, i64, i64, f64, f64) = conn.query_row(
        "SELECT MIN(core_uid), MIN(buydate), MAX(closedate),
                MIN(MIN(buyprice, sellprice)), MAX(MAX(buyprice, sellprice))
         FROM orders_rep WHERE buydate > 0",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )?;
    if high <= low || !high.is_finite() || !low.is_finite() {
        anyhow::bail!("в фикстуре вырожденный ценовой диапазон сделок");
    }
    let window = figures::SeedWindow {
        from_ms: from_secs as f64 * 1_000.0,
        to_ms: to_secs as f64 * 1_000.0,
        low,
        high,
    };
    Ok(figures::seed(core_uid as u64, market, window))
}

/// Read the single exchange/market pair the candle database carries.
///
/// The pair is DATA, not a constant: a second fixture for another coin must not require a code
/// change, and a bench whose candles and trades disagree about the market is worth failing on.
fn describe(klines: &Path) -> anyhow::Result<(String, String)> {
    let conn = rusqlite::Connection::open(klines)?;
    let mut stmt = conn.prepare("SELECT DISTINCT exchange, market FROM chunks")?;
    let rows: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    match rows.as_slice() {
        [one] => Ok(one.clone()),
        [] => anyhow::bail!("в klines.sqlite фикстуры нет ни одной свечи"),
        many => anyhow::bail!(
            "фикстура держит {} пар биржа/рынок, ожидается ровно одна",
            many.len()
        ),
    }
}

/// Close of the newest candle in the bench.
///
/// Read from the packed blob of the newest chunk rather than kept as a constant, so a regenerated
/// fixture cannot disagree with the number the synthetic feed is started from.
fn newest_close(klines: &Path) -> anyhow::Result<f32> {
    let conn = rusqlite::Connection::open(klines)?;
    let blob: Vec<u8> = conn.query_row(
        "SELECT rows FROM chunks ORDER BY day DESC, kind ASC LIMIT 1",
        [],
        |r| r.get(0),
    )?;
    // By the largest in-day offset rather than by position: "last in the blob" is only the newest
    // row while the blob happens to be sorted, and that is an assumption about data, not a fact.
    let newest = blob
        .chunks_exact(ROW_BYTES)
        .max_by_key(|row| u32::from_le_bytes(row[0..4].try_into().expect("4 bytes")))
        .ok_or_else(|| anyhow::anyhow!("пустой чанк свечей в фикстуре"))?;
    let close = f32::from_le_bytes(newest[16..20].try_into().expect("4 bytes"));
    if close.is_finite() && close > 0.0 {
        Ok(close)
    } else {
        anyhow::bail!("в фикстуре некорректная цена закрытия {close}")
    }
}

/// Find the fixture directory: explicit override, beside the executable, then up the working tree.
///
/// The walk upward exists because a development build runs from `target/<triple>/debug`, four
/// levels below the `fixtures/` directory it is committed in.
fn locate(name: &str) -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("MOON_FIXTURE_DIR") {
        let dir = PathBuf::from(dir);
        let candidate = if dir.ends_with(name) {
            dir
        } else {
            dir.join(name)
        };
        return candidate.is_dir().then_some(candidate);
    }
    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent()?.to_path_buf();
    loop {
        let candidate = dir.join(FIXTURES_DIR).join(name);
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests;
