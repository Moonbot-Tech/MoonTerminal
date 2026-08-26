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

/// Filter used when `RUST_LOG` says nothing.
///
/// The terminal's own directive names `moonterminal`, NOT the package. `moon-ui-gpui` declares no
/// `[lib]` and one `[[bin]] name = "moonterminal"`, so `module_path!()` — which is what `log` uses
/// as a record's target — roots every UI line at `moonterminal`. The directive shipped here read
/// `moon_ui_gpui=info` and therefore matched nothing at all: from the day it was written until
/// 2026-08-26 every `info!` in the terminal was dropped by the baseline `warn`, silently, while the
/// filter read as though the UI were traced.
///
/// `moon_gpui` below has the same disease and is left alone deliberately. `moon-gpui` declares
/// `[lib] name = "gpui"`, so GPUI's own records are targeted `gpui::…` and this directive misses
/// them; it reaches only the sibling packages that kept a default lib name (`moon_gpui_windows::…`,
/// which is where the window and present errors come from). Widening it would raise a crate whose
/// volume at `info` nobody here has measured — a separate change, on purpose.
///
/// Raised for `panels::chart` alone rather than the whole binary: that subtree is where the money
/// paths log — the manual order, its refusals, the shot — and it is a handful of event-driven
/// lines, whereas the binary at large has never had its volume at `info` measured even once.
pub const DEFAULT_BASE_FILTER: &str =
    "warn,moonterminal::panels::chart=info,moon_gpui=info,moon_core=info";

/// Module prefix carrying balance-repair tracing (`feed::live` and its children).
const BALANCES_TARGET: &str = "moon_core::feed::live";
/// Module prefix carrying candle-cache merge tracing.
const KLINE_CACHE_TARGET: &str = "moon_core::market::kline_cache";
/// Module prefix carrying market-source tracing, including cache-prefix reads and native backfill.
const MARKET_SOURCES_TARGET: &str = "moon_core::market::source";
/// Module prefix carrying chart hit-test and manual-order tracing.
///
/// Rooted at the BINARY's name for the reason [`DEFAULT_BASE_FILTER`] explains; spelled
/// `moon_ui_gpui` until 2026-08-26, which made this switch inert however it was set.
///
/// Public so the terminal can check this string against its own `module_path!()` instead of
/// carrying a second copy of it. Nothing in THIS crate can verify the prefix — the modules it names
/// live in the binary — so a test on the far side is the only thing that can catch the rename that
/// would make it inert again.
pub const CHART_INPUT_TARGET: &str = "moonterminal::panels::chart";

/// Module prefix carrying hotkey dispatch tracing.
///
/// Public for the same reason as [`CHART_INPUT_TARGET`]: the modules it names live in the binary,
/// so only a test over there can hold it against a real `module_path!()`.
pub const HOTKEYS_TARGET: &str = "moonterminal::hotkeys";

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
    area(cfg.log.chart_input, CHART_INPUT_TARGET);
    area(cfg.log.hotkeys, HOTKEYS_TARGET);
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
