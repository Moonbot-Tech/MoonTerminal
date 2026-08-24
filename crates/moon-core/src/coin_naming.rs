//! Diagnostic channel for how a COIN is spelled on each exchange (`channels.coin_naming` in
//! `cfg/diagnostics.toml`), built to answer one question no amount of reading can settle: what does
//! a core's catalog actually hold for a coin that carries a MULTIPLIER.
//!
//! The terminal is about to need a cross-exchange identity for a coin — the arbitrage column
//! borrows quotes between exchanges, the comparison tab looks for the same coin elsewhere — and
//! both stumble on the same thing. Bybit spells `1000BONKPERP` as the coin `1kBONKPERP`, Binance
//! spells its own as something else, and `1000SATS` is not a multiplier at all but the coin's real
//! ticker. No rule over the NAME can tell those apart.
//!
//! The catalog carries fields that can: `market_currency_canonic` is the contract-free wallet
//! identity (`BONKPERP`, `AAVE`), and `leading1000` / `k1000` sit right beside it and are read by
//! the protocol but used by nothing — here or in moonproto. Their meaning is inferable from their
//! names and nowhere documented, so this channel prints them verbatim, per core, for the coins
//! asked about. What they turn out to hold decides how the identity is built; guessing it would put
//! a fabricated rule at the centre of two features.
//!
//! Set `channels.coin_naming` to a comma-separated list of coins — `"BONK,PEPE,1000SATS,BTC,AAVE"`
//! — or to a single one. Each entry matches case-insensitively as a SUBSTRING against every
//! spelling the catalog holds, so `BONK` finds `1000BONKUSDT` and `1kBONKPERP` alike. Lines are
//! appended to `logs/coin_naming.log` beside the application's own logs.
//!
//! A core is swept ONCE while the selector stands: the answer is a property of the catalog, not an
//! event, so a second pass would both bury the one core whose spelling differs and re-scan every
//! market universe on a tick that runs several times a second. A core whose catalog has not arrived
//! yet produces nothing and stays pending, so the sweep lands as soon as it does. Changing the
//! selector starts a fresh one.

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::OnceLock;

/// Most markets read per core per coin entry.
///
/// A coin token matches a handful of contracts on any one exchange; a cap keeps a two-letter
/// selector typed by mistake (`BT`) from writing the whole universe into the file.
pub const MARKETS_PER_CORE: usize = 12;

/// Whether the channel is on at all.
pub fn enabled() -> bool {
    crate::diagnostics::coin_naming_on()
}

/// The coins to look for on `core`, or `None` when there is nothing to do.
///
/// `None` covers both "the channel is off" and "this core has already been written under this
/// selector" — the caller's whole gate, so a swept core costs one atomic load and a set lookup
/// rather than a scan of its market universe on every reconciliation.
pub fn queries_for(core: u64) -> Option<Vec<String>> {
    let selector = crate::diagnostics::with_coin_naming_selector(str::to_string)?;
    if swept().lock().unwrap_or_else(|e| e.into_inner()).done(&selector, core) {
        return None;
    }
    let queries: Vec<String> = selector
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect();
    (!queries.is_empty()).then_some(queries)
}

/// Whether any selector entry matches one of a market's spellings.
///
/// Every spelling is offered, because which one carries the coin is exactly what is unknown: the
/// market is keyed by `bn_market_name` while the coin may only be readable from `market_name` or
/// from a catalog token. Matching a narrower set here than the core's own search used would drop
/// the very rows that disagree — the rows this channel exists to show.
fn follows_setting(setting: &str, spellings: &[&str]) -> bool {
    setting
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .any(|entry| {
            let needle = entry.to_ascii_uppercase();
            spellings
                .iter()
                .any(|s| s.to_ascii_uppercase().contains(&needle))
        })
}

/// Cores already written, and the selector they were written under.
struct Swept {
    selector: String,
    cores: HashSet<u64>,
}

impl Swept {
    /// Whether this core is finished, forgetting everything when the selector changed.
    ///
    /// Editing the selector in the live file is how a second coin is asked about, and a sweep that
    /// kept its memory across that edit would answer the new question with silence.
    fn done(&mut self, selector: &str, core: u64) -> bool {
        if self.selector != selector {
            self.selector = selector.to_string();
            self.cores.clear();
        }
        self.cores.contains(&core)
    }

    fn finish(&mut self, selector: &str, core: u64) {
        if self.selector == selector {
            self.cores.insert(core);
        }
    }
}

fn swept() -> &'static Mutex<Swept> {
    static SWEPT: OnceLock<Mutex<Swept>> = OnceLock::new();
    SWEPT.get_or_init(|| {
        Mutex::new(Swept {
            selector: String::new(),
            cores: HashSet::new(),
        })
    })
}

/// One market's catalog spellings, as the fields stand on the wire.
///
/// Deliberately a record of RAW fields rather than an interpretation: the whole point is to see
/// what the core sends, including an empty string or a zero, which is itself an answer about a
/// field nothing documents.
#[derive(Clone, Debug, Default)]
pub struct CatalogNaming {
    /// `bn_market_name` — the key the terminal addresses this market by.
    pub key: String,
    /// `market_name`, which is a different field and may spell the multiplier differently.
    pub name: String,
    pub classic: String,
    pub currency: String,
    pub canonic: String,
    pub long: String,
    pub base: String,
    pub leading1000: String,
    pub k1000: i32,
}

impl CatalogNaming {
    /// Every spelling a selector entry may match, in the order they are printed.
    fn spellings(&self) -> [&str; 6] {
        [
            self.key.as_str(),
            self.name.as_str(),
            self.classic.as_str(),
            self.currency.as_str(),
            self.canonic.as_str(),
            self.long.as_str(),
        ]
    }
}

/// Write one core's whole table, and mark the core finished if it went to disk.
///
/// One file open for the core rather than one per row: the first sweep of a terminal watching two
/// hundred cores runs on the main thread, and a syscall per row there is a visible stall.
///
/// The core is marked finished only on a successful write, so an unwritable `logs/` — an antivirus
/// holding the folder, a sync client mid-rename — costs a retry on the next tick instead of losing
/// the core's row until the selector is edited.
///
/// Args:
///     core: Core the rows were read from.
///     core_name: Server name as the user sees it.
///     venue: Exchange caption, for reading the table without resolving core ids.
///     rows: Every market read, before the selector is applied.
pub fn record_core(core: u64, core_name: &str, venue: &str, rows: &[CatalogNaming]) {
    let Some(selector) = crate::diagnostics::with_coin_naming_selector(str::to_string) else {
        return;
    };
    let stamp = crate::util::time::now_unix_ms_i64();
    let lines: Vec<String> = rows
        .iter()
        .filter(|row| follows_setting(&selector, &row.spellings()))
        .map(|row| {
            format!(
                "{stamp} core={core} ({core_name} · {venue}) key={key} name={name} \
                 classic={classic} currency={currency} canonic={canonic} long={long} \
                 base={base} leading1000='{leading}' k1000={k1000}",
                key = row.key,
                name = row.name,
                classic = row.classic,
                currency = row.currency,
                canonic = row.canonic,
                long = row.long,
                base = row.base,
                leading = row.leading1000,
                k1000 = row.k1000,
            )
        })
        .collect();
    // An empty table is not a finished core: the catalog arrives after the connection, so the
    // first ticks of a run legitimately see nothing and the sweep has to come back.
    if lines.is_empty() {
        return;
    }
    if crate::diagnostics::channel_lines("coin_naming.log", &lines) {
        swept()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .finish(&selector, core);
    }
}

#[cfg(test)]
mod tests;
