use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// A normal launch must not become a bench run: the flag is the only trigger.
#[test]
fn absent_flag_is_a_normal_run() {
    assert_eq!(requested(args(&["moonterminal.exe"])), None);
    assert_eq!(
        requested(args(&["moonterminal.exe", "--debug-script", "chart-smoke"])),
        None
    );
}

/// The bare flag opens the default bench, so the common case needs no name.
#[test]
fn bare_flag_selects_the_default_fixture() {
    assert_eq!(
        requested(args(&["moonterminal.exe", "--fixture"])).as_deref(),
        Some(DEFAULT_FIXTURE)
    );
}

/// A name after the flag selects that bench.
#[test]
fn name_after_the_flag_is_used() {
    assert_eq!(
        requested(args(&["moonterminal.exe", "--fixture", "chart-btc"])).as_deref(),
        Some("chart-btc")
    );
}

/// A following FLAG is not a fixture name: `--fixture --debug-script x` must open the default
/// bench and leave the other flag alone, not look for a fixture called `--debug-script`.
#[test]
fn following_flag_is_not_taken_as_a_name() {
    assert_eq!(
        requested(args(&["moonterminal.exe", "--fixture", "--debug-script"])).as_deref(),
        Some(DEFAULT_FIXTURE)
    );
}

/// Both consumers of the market name must receive the SAME name: the plaintext core decides which
/// market the core defaults to, the synthetic feed decides which markets exist at all, and a
/// disagreement opens a chart the bench has no candles for.
#[test]
fn market_reaches_both_the_core_and_the_feed() {
    let env = environment_for("ACEUSDT", 0.19538);
    let get = |key: &str| {
        env.iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
            .unwrap_or_default()
    };
    assert_eq!(get("MOON_CONFIG_PLAINTEXT_MARKET"), "ACEUSDT");
    assert_eq!(get("MOON_SYNTH_MARKET_NAMES"), "ACEUSDT");
    assert_eq!(get("MOON_CONFIG_PLAINTEXT_SYNTHETIC"), "1");
    // A synthetic core with no key must still be accepted, which is what plaintext mode grants.
    assert_eq!(get("MOON_CONFIG_PLAINTEXT"), "1");
    // Without the auto-open the bench starts with an empty Main and shows nothing at all.
    assert_eq!(get("MOON_RENDER_DIAG_OPEN_FIRST_MARKET"), "1");
    // The synthetic stream must start from the bench's own price: its default is 100 per market,
    // which drags the order book, the ticker and the Y fit three orders of magnitude off these
    // candles and leaves the chart looking empty.
    assert_eq!(get("MOON_SYNTH_START_PRICE"), "0.19538");
}
