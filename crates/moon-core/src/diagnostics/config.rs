//! Shape of `cfg/diagnostics.toml` and the rules for reading it.
//!
//! Parsing is deliberately separate from the runtime state in [`super`]: this half is pure, takes a
//! string and an environment, and returns a value, so every rule below is testable without a
//! process-wide logger or a real file.
//!
//! Two rules are easy to get backwards and are stated here once:
//! - A broken file yields DEFAULTS, never a partial read. `serde(default)` fills missing keys, but a
//!   syntax error aborts the whole parse, and silently running with half a configuration is worse
//!   than running with none.
//! - The environment can only turn a switch ON. A scripted run sets `MOON_RENDER_DIAG=1` and must
//!   get the channel regardless of what the file says; the reverse — a file quietly disabling what a
//!   CI job asked for — would make a failed capture look like a passing one.

use serde::{Deserialize, Serialize};

/// Everything `diagnostics.toml` can say.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiagCfg {
    pub log: LogAreas,
    pub channels: Channels,
    pub limits: Limits,
}

/// `[log]` — areas whose debug tracing is admitted into the ordinary log.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogAreas {
    /// Balance repair on the live feed (`moon_core::feed::live`).
    pub balances: bool,
    /// Candle-cache merges (`moon_core::market::kline_cache`).
    pub kline_cache: bool,
    /// Market data sources (`moon_core::market::source`).
    pub market_sources: bool,
    /// Chart hit-testing (`moonterminal::panels::chart`): where a press landed, and against which
    /// rectangles. The one question the code cannot answer by reading — the chart draws in its own
    /// pass, and its coordinates pass through the presenter's scale.
    pub chart_input: bool,
    /// Keyboard shortcut dispatch (`moonterminal::hotkeys`): every key the window root sees, and
    /// what the bindings made of it. Answers the one question a silent hotkey raises and nothing
    /// else can — whether the keystroke was eaten before the terminal saw it, arrived and matched
    /// no binding, or resolved and then did nothing.
    pub hotkeys: bool,
    /// Extra directives in `RUST_LOG` syntax, appended after the area directives.
    pub filter: String,
}

/// `[channels]` — diagnostics that write their own file instead of the ordinary log.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Channels {
    pub render: bool,
    pub detect: bool,
    /// Order-pipeline SELECTOR, not a flag: `""` off, `"1"` everything, `"BTC"` or `"CORE/MARKET"`
    /// to narrow. Preserved verbatim from `MOON_ORDER_DIAG`; see `crate::order_diag`.
    pub orders: String,
    pub assets: bool,
    pub markets: bool,
    /// Coin-spelling SELECTOR, not a flag: `""` off, or a comma-separated list of coins
    /// (`"BONK,1000SATS"`) whose catalog spellings are dumped once per core. See
    /// `crate::coin_naming`.
    pub coin_naming: String,
}

/// `[limits]` — sizes and collapse windows, in effect whether or not anything above is on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Limits {
    pub log_ring_lines: usize,
    pub balance_repeat_window_sec: u64,
    pub market_trace_min_interval_ms: u64,
}

/// Lines the Log panel keeps in memory when the file says nothing.
pub const DEFAULT_RING_LINES: usize = 5000;
/// Floor for `log_ring_lines`. A ring of zero would make the Log panel permanently empty, which
/// reads as "the application stopped logging" rather than as a configuration mistake.
pub const MIN_RING_LINES: usize = 100;
/// Ceiling for `log_ring_lines`, about 150 MB of retained text at ~150 bytes per line. Past this a
/// typed extra digit costs more memory than the whole rest of the terminal.
pub const MAX_RING_LINES: usize = 1_000_000;
/// Default collapse window for repeated balance-repair requests.
pub const DEFAULT_BALANCE_WINDOW_SEC: u64 = 5;
/// Default floor between two market-trace lines about the same subject.
pub const DEFAULT_MARKET_FLOOR_MS: u64 = 1000;

impl Default for Limits {
    fn default() -> Self {
        Self {
            log_ring_lines: DEFAULT_RING_LINES,
            balance_repeat_window_sec: DEFAULT_BALANCE_WINDOW_SEC,
            market_trace_min_interval_ms: DEFAULT_MARKET_FLOOR_MS,
        }
    }
}

impl DiagCfg {
    /// Whether anything at all is switched on, which is what decides if the startup warning is
    /// worth printing. `filter` counts: a directive string is a switch like any other.
    pub fn any_active(&self) -> bool {
        let l = &self.log;
        let c = &self.channels;
        l.balances
            || l.kline_cache
            || l.market_sources
            || l.chart_input
            || l.hotkeys
            || !l.filter.trim().is_empty()
            || c.render
            || c.detect
            || c.assets
            || c.markets
            || !c.orders.trim().is_empty()
            || !c.coin_naming.trim().is_empty()
    }

    /// Human-readable list of active switches for the warning line, or `None` when all are off.
    ///
    /// The names match the file's keys rather than prose, so a reader can go straight from the
    /// warning to the line to edit.
    pub fn active_summary(&self) -> Option<String> {
        if !self.any_active() {
            return None;
        }
        let mut on: Vec<String> = Vec::new();
        let mut flag = |name: &str, v: bool| {
            if v {
                on.push(name.to_string());
            }
        };
        flag("log.balances", self.log.balances);
        flag("log.kline_cache", self.log.kline_cache);
        flag("log.market_sources", self.log.market_sources);
        flag("log.chart_input", self.log.chart_input);
        flag("log.hotkeys", self.log.hotkeys);
        flag("channels.render", self.channels.render);
        flag("channels.detect", self.channels.detect);
        flag("channels.assets", self.channels.assets);
        flag("channels.markets", self.channels.markets);
        // The string-valued switches carry their value: "orders" alone would not say which
        // market is being followed, and that is the whole content of the setting.
        let coin_naming = self.channels.coin_naming.trim();
        if !coin_naming.is_empty() {
            on.push(format!("channels.coin_naming={coin_naming}"));
        }
        let orders = self.channels.orders.trim();
        if !orders.is_empty() {
            on.push(format!("channels.orders={orders}"));
        }
        let filter = self.log.filter.trim();
        if !filter.is_empty() {
            on.push(format!("log.filter={filter}"));
        }
        Some(on.join(", "))
    }

    /// Clamp values that would otherwise break a surface rather than configure it.
    fn sanitize(&mut self) {
        self.limits.log_ring_lines = self
            .limits
            .log_ring_lines
            .clamp(MIN_RING_LINES, MAX_RING_LINES);
    }
}

/// Parse file text. `Err` carries the message shown once in the log; the caller then uses defaults.
pub fn parse(text: &str) -> Result<DiagCfg, toml::de::Error> {
    let mut cfg: DiagCfg = toml::from_str(text)?;
    cfg.sanitize();
    Ok(cfg)
}

/// One environment variable's effect, resolved through a lookup so the rules stay testable: a
/// process cannot set the same variable twice for two cases.
pub fn apply_env(cfg: &mut DiagCfg, get: impl Fn(&str) -> Option<String>) {
    // Presence alone enables, matching every one of these variables' existing behaviour: today
    // `MOON_RENDER_DIAG=0` enables the channel, and a run that relied on that must keep working.
    let on = |var: &str, field: &mut bool| {
        if get(var).is_some() {
            *field = true;
        }
    };
    on("MOON_DIAG_BALANCES", &mut cfg.log.balances);
    on("MOON_DIAG_KLINE_CACHE", &mut cfg.log.kline_cache);
    on("MOON_DIAG_MARKET_SOURCES", &mut cfg.log.market_sources);
    on("MOON_DIAG_CHART_INPUT", &mut cfg.log.chart_input);
    on("MOON_DIAG_HOTKEYS", &mut cfg.log.hotkeys);
    on("MOON_RENDER_DIAG", &mut cfg.channels.render);
    on("MOON_DETECT_DIAG", &mut cfg.channels.detect);
    on("MOON_ASSETS_DIAG", &mut cfg.channels.assets);
    on("MOON_MARKET_DIAG", &mut cfg.channels.markets);
    // `MOON_MARKET_DIAG` and `MOON_RENDER_DIAG` have always been alternatives for the market
    // channel (`market::source::market_diag_enabled`), so the render switch keeps enabling it.
    if get("MOON_RENDER_DIAG").is_some() {
        cfg.channels.markets = true;
    }
    // The order channel carries a value rather than a flag. An empty value means "follow
    // everything" in the existing selector, so it must not be confused with the unset case.
    if let Some(v) = get("MOON_ORDER_DIAG") {
        cfg.channels.orders = if v.is_empty() { "1".to_string() } else { v };
    }
    // A list of coins, so an empty value cannot mean "everything" the way the order selector's
    // does: dumping every market of every core is not an answer anyone can read.
    if let Some(v) = get("MOON_COIN_NAMING") {
        if !v.trim().is_empty() {
            cfg.channels.coin_naming = v;
        }
    }
}

/// Read the process environment. Separated from [`apply_env`] so tests never touch global state.
pub fn env_lookup(var: &str) -> Option<String> {
    std::env::var_os(var).map(|v| v.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests;
