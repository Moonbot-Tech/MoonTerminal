//! `--fixture [name]`: run the real application against a committed chart bench.
//!
//! The bench replaces the two variable halves of a chart — the core that serves candles and the
//! report replica that serves closed trades — with committed databases, so a run always draws the
//! same day. It exists for work whose result can only be judged by eye: trade markers, order lines
//! and drawing figures all sit on top of price, and comparing two versions of them on live data
//! compares two different markets.
//!
//! What the flag does, in the order it must happen:
//!   1. copies the fixture to a throwaway data root and points every path at it — BEFORE the
//!      diagnostics file, the logger, or the config are read, or the application would already
//!      have opened the developer's real databases;
//!   2. configures one synthetic core (no network, no key) carrying the fixture's market, so the
//!      chart has a core to belong to and the market catalog knows the name;
//!   3. asks the synthetic feed for a single window with a single chart, which its `AddToChart`
//!      stream then opens on that market.
//!
//! Candles come from the bench through `moon_core::market::source`; closed trades need no special
//! path at all, because the chart already reads them from `reports.sqlite` — and that file is now
//! the fixture's copy.

use moon_core::fixture::ChartFixture;

/// Command-line flag that turns this process into a bench run.
const FLAG: &str = "--fixture";

/// Fixture used when the flag carries no name.
const DEFAULT_FIXTURE: &str = "chart-ace";

/// Name requested on the command line, if this process is a bench run.
///
/// Accepts `--fixture` alone and `--fixture <name>`; a following argument that starts with `-` is
/// another flag, not a name.
///
/// Args:
///     args: Process arguments, including the executable at position zero.
///
/// Returns:
///     The requested fixture name, or `None` for a normal run.
pub(crate) fn requested<I>(args: I) -> Option<String>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg != FLAG {
            continue;
        }
        return Some(match args.next() {
            Some(name) if !name.starts_with('-') && !name.trim().is_empty() => name,
            _ => DEFAULT_FIXTURE.to_string(),
        });
    }
    None
}

/// Prepare the bench and configure the synthetic core that will carry it.
///
/// Call FIRST in startup, before anything resolves a data path.
///
/// Args:
///     args: Process arguments, including the executable at position zero.
///
/// Returns:
///     The opened bench, or `None` for a normal run.
///
/// Errors:
///     Propagates a missing, incomplete or unreadable fixture. A bench run that silently fell back
///     to the real data root would write into the developer's own databases.
pub(crate) fn bootstrap<I>(args: I) -> anyhow::Result<Option<&'static ChartFixture>>
where
    I: IntoIterator<Item = String>,
{
    let Some(name) = requested(args) else {
        return Ok(None);
    };
    // A note left by a PREVIOUS failed run would otherwise be read as describing this one.
    let note = std::env::temp_dir().join("moonterminal-fixture-error.log");
    let _ = std::fs::remove_file(&note);
    let fixture = moon_core::fixture::prepare(&name).inspect_err(|error| {
        // This runs before the logger, the panic hook and the window exist, and the binary is a
        // GUI subsystem one — an error returned from here would otherwise end the process with no
        // window, no log line and nothing on screen at all. Leave a file that says what happened.
        let note = std::env::temp_dir().join("moonterminal-fixture-error.log");
        let _ = std::fs::write(&note, format!("--fixture {name}: {error}\n"));
        eprintln!("--fixture {name}: {error} (см. {})", note.display());
    })?;
    for (key, value) in environment_for(fixture.market(), fixture.last_price()) {
        // SAFETY: `bootstrap` is the first statement of `startup::run`, which runs on the main
        // thread before the logger, the config, GPUI, or any background worker exists. Nothing
        // else can be reading the environment concurrently, which is the exact condition
        // `set_var` requires. The variables are the documented plaintext/synthetic knobs, and
        // every reader of them runs later in this same startup.
        unsafe { std::env::set_var(key, value) };
    }
    Ok(Some(fixture))
}

/// Environment the synthetic core needs to carry `market`.
///
/// Separated from the process mutation so the exact configuration is testable: the market name has
/// to reach BOTH the plaintext core (which decides the core's default market) and the synthetic
/// feed (which decides what the market catalog contains), and a mismatch between the two opens a
/// chart on a market the bench has no candles for.
fn environment_for(market: &str, last_price: f32) -> [(&'static str, String); 11] {
    [
        ("MOON_CONFIG_PLAINTEXT", "1".to_string()),
        ("MOON_CONFIG_PLAINTEXT_SYNTHETIC", "1".to_string()),
        ("MOON_CONFIG_PLAINTEXT_NAME", "FIXTURE".to_string()),
        ("MOON_CONFIG_PLAINTEXT_MARKET", market.to_string()),
        ("MOON_SYNTH_MARKET_NAMES", market.to_string()),
        // Anchor the synthetic ticks — and with them the order book, the header ticker and the
        // price labels — on the bench's own last close. The default 100 per market would sit
        // three orders of magnitude above these candles and drag the Y fit off them.
        ("MOON_SYNTH_START_PRICE", format!("{last_price}")),
        // A book with real extent: 100 levels a side, 0.05% apart, so it spans about ±5% instead
        // of the default ±0.5% — which on a chart scaled to a whole trading day is one line.
        ("MOON_SYNTH_DEPTH", "100".to_string()),
        ("MOON_SYNTH_BOOK_STEP_PCT", "0.05".to_string()),
        // One window, one chart: the bench is looked at, not stress-tested.
        ("MOON_STRESS_WINDOWS", "1".to_string()),
        ("MOON_STRESS_CHARTS", "1".to_string()),
        // Open the core's default market on Main by itself. This is the diagnostics auto-open
        // that already exists for render measurements — the bench needs exactly the same thing,
        // and a second way to open a chart would be a second thing to keep working.
        ("MOON_RENDER_DIAG_OPEN_FIRST_MARKET", "1".to_string()),
    ]
}

#[cfg(test)]
mod tests;
