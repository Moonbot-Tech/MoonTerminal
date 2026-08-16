//! Turning the `[log]` areas into one `env_filter` directive string.
//!
//! The whole scheme rests on one property of `env_filter`: `Builder::build` sorts directives by the
//! length of their module prefix and `enabled` walks them in reverse, so **the longest matching
//! prefix wins** regardless of the order they were written in. That is what lets an area be
//! APPENDED to a base filter that already covers its crate — `moon_core::feed::live=debug` after
//! `moon_core=info` raises exactly that subtree and nothing else. Were matching first-wins instead,
//! every area directive would be shadowed by the base and silently do nothing.
//!
//! Equal-length names fall back to insertion order (the sort is stable) and reverse iteration then
//! picks the last, so an area also wins against a directive naming the very same module. This is
//! why an area switch cannot be defeated by `RUST_LOG`, matching the rule that the environment and
//! the file may only ever turn tracing ON.

use super::config::DiagCfg;

/// Filter used when `RUST_LOG` says nothing. Must stay byte-identical to what the terminal shipped
/// before diagnostics moved into a file, or a normal launch changes how much it logs.
pub const DEFAULT_BASE_FILTER: &str = "warn,moon_ui_gpui=info,moon_gpui=info,moon_core=info";

/// Module prefix carrying balance-repair tracing (`feed::live` and its children).
const BALANCES_TARGET: &str = "moon_core::feed::live";
/// Module prefix carrying candle-cache merge tracing.
const KLINE_CACHE_TARGET: &str = "moon_core::market::kline_cache";
/// Module prefix carrying market-source tracing, including cache-prefix reads and native backfill.
const MARKET_SOURCES_TARGET: &str = "moon_core::market::source";

/// The directive string for `cfg`, taking the base from `RUST_LOG` when it is set.
pub fn filter_string(cfg: &DiagCfg) -> String {
    let base = std::env::var("RUST_LOG")
        .ok()
        .filter(|s| !s.trim().is_empty());
    compose(cfg, base.as_deref())
}

/// [`filter_string`] with the base supplied explicitly, so the composition is testable without
/// touching the process environment.
pub fn compose(cfg: &DiagCfg, base: Option<&str>) -> String {
    let mut out = base.unwrap_or(DEFAULT_BASE_FILTER).trim().to_string();
    let mut area = |on: bool, target: &str| {
        if on {
            if !out.is_empty() {
                out.push(',');
            }
            out.push_str(target);
            out.push_str("=debug");
        }
    };
    area(cfg.log.balances, BALANCES_TARGET);
    area(cfg.log.kline_cache, KLINE_CACHE_TARGET);
    area(cfg.log.market_sources, MARKET_SOURCES_TARGET);
    // Appended last so a hand-written directive can raise something an area already covers; it is
    // the escape hatch, and an escape hatch that loses to the presets is not one.
    let extra = cfg.log.filter.trim();
    if !extra.is_empty() && !filter_is_rejected(extra) {
        if !out.is_empty() {
            out.push(',');
        }
        out.push_str(extra);
    }
    out
}

/// Whether a hand-written `log.filter` will be ignored by [`compose`].
///
/// Validated ON ITS OWN, before being appended, because a bad fragment does not fail locally:
/// `env_filter` abandons the ENTIRE spec on any parse error, taking the base and every area
/// directive with it, and `build()` then substitutes a lone `error` directive. One typo in this
/// field would therefore mute the terminal down to errors while the areas still read as active.
/// Checking the fragment separately keeps the blast radius to the fragment.
///
/// A `/` is refused even though it parses. In `RUST_LOG` syntax it introduces a match on the
/// message TEXT that applies to the whole spec rather than to the directive it follows, so it would
/// silently make every record in the application — errors included — conditional on the user's
/// text. Nobody adding one module to a debug list meant that.
///
/// [`compose`] calls this rather than restating the rule, so the two cannot disagree about what is
/// accepted — which is exactly the drift a separate predicate would invite.
pub fn filter_is_rejected(filter: &str) -> bool {
    filter_rejection(filter).is_some()
}

/// Why a `log.filter` was ignored, or `None` when it was accepted.
///
/// The REASON, not just the verdict, because the two causes call for opposite corrections and a
/// message naming the wrong one sends the reader looking for a slash they never typed.
pub fn filter_rejection(filter: &str) -> Option<String> {
    let trimmed = filter.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains('/') {
        return Some(
            "«/» в синтаксисе RUST_LOG задаёт отбор по тексту сообщения для ВСЕГО лога, \
             а не для одной директивы"
                .to_string(),
        );
    }
    match env_filter::Builder::new().try_parse(trimmed) {
        Ok(_) => None,
        Err(e) => Some(format!("не разбирается как набор директив ({e})")),
    }
}

#[cfg(test)]
mod tests;
