//! Regression coverage for currency grouping and stable provider selection.

use super::*;

const CORE_A: CoreId = 1;
const CORE_B: CoreId = 2;
const CORE_C: CoreId = 3;
const EXCHANGE: ExchangeId = ExchangeId::new(4);

/// Build the smallest identified core fixture with explicit currency and readiness.
fn core(id: CoreId, currency: Option<&str>, ready: bool) -> ElectionCore<'_> {
    ElectionCore {
        id,
        exchange: Some(EXCHANGE),
        currency,
        ready,
    }
}

/// Construct expected keys independently of currency normalization.
fn key(currency: Option<&str>) -> ProviderKey {
    ProviderKey {
        exchange: EXCHANGE,
        currency: currency.map(Box::from),
    }
}

/// Merging currencies would route the USDC core to an incomplete USDT tape.
#[test]
fn usdt_usdt_usdc_elects_two_providers() {
    let (providers, mapping) = elect(
        &[
            core(CORE_A, Some("USDT"), true),
            core(CORE_B, Some("USDT"), true),
            core(CORE_C, Some("USDC"), true),
        ],
        &HashMap::new(),
    );
    assert_eq!(
        providers,
        HashMap::from([(key(Some("USDT")), CORE_A), (key(Some("USDC")), CORE_C)])
    );
    assert_eq!(
        mapping,
        HashMap::from([(CORE_A, CORE_A), (CORE_B, CORE_A), (CORE_C, CORE_C)])
    );
}

/// Electing per core would duplicate full tapes on single-currency setups.
#[test]
fn same_currency_elects_one_provider() {
    let (providers, mapping) = elect(
        &[
            core(CORE_A, Some("USDT"), true),
            core(CORE_B, Some("USDT"), true),
        ],
        &HashMap::new(),
    );
    assert_eq!(providers, HashMap::from([(key(Some("USDT")), CORE_A)]));
    assert_eq!(mapping, HashMap::from([(CORE_A, CORE_A), (CORE_B, CORE_A)]));
}

/// Treating blank currencies as distinct would add tapes before CoreBase arrives.
#[test]
fn all_unknown_currency_elects_one_provider() {
    let (providers, mapping) = elect(
        &[core(CORE_A, None, true), core(CORE_B, Some("  "), true)],
        &HashMap::new(),
    );
    assert_eq!(providers, HashMap::from([(key(None), CORE_A)]));
    assert_eq!(mapping, HashMap::from([(CORE_A, CORE_A), (CORE_B, CORE_A)]));
}

/// Choosing alphabetical currency before USDT would misroute an unknown core.
#[test]
fn unknown_currency_joins_the_usdt_group() {
    let (providers, mapping) = elect(
        &[
            core(CORE_A, Some("USDC"), true),
            core(CORE_B, Some("USDT"), true),
            core(CORE_C, None, true),
        ],
        &HashMap::new(),
    );
    assert_eq!(providers.len(), 2);
    assert_eq!(
        mapping,
        HashMap::from([(CORE_A, CORE_A), (CORE_B, CORE_B), (CORE_C, CORE_B)])
    );
}

/// Input-order fallback would pick USDC instead of alphabetically earlier BTC.
#[test]
fn unknown_currency_joins_alphabetically_first_without_usdt() {
    let (providers, mapping) = elect(
        &[
            core(CORE_A, Some("USDC"), true),
            core(CORE_B, Some("BTC"), true),
            core(CORE_C, Some(" "), true),
        ],
        &HashMap::new(),
    );
    assert_eq!(providers.len(), 2);
    assert_eq!(mapping[&CORE_C], CORE_B);
}

/// Missing normalization would create two subscriptions for the same account currency.
#[test]
fn currency_is_trimmed_and_case_folded() {
    assert_eq!(normalize_currency("usdc "), Some(Box::from("USDC")));
    assert_eq!(normalize_currency("usdc "), normalize_currency("USDC"));
    assert_eq!(normalize_currency("  "), None);
    let (providers, mapping) = elect(
        &[
            core(CORE_A, Some("usdc "), true),
            core(CORE_B, Some("USDC"), true),
        ],
        &HashMap::new(),
    );
    assert_eq!(providers, HashMap::from([(key(Some("USDC")), CORE_A)]));
    assert_eq!(mapping[&CORE_B], CORE_A);
}

/// Looking up only the new key would flip a healthy provider when CoreBase arrives.
#[test]
fn ready_current_provider_kept_when_key_gains_currency() {
    let (providers, mapping) = elect(
        &[
            core(CORE_A, Some("USDT"), true),
            core(CORE_B, Some("USDT"), true),
        ],
        &HashMap::from([(key(None), CORE_B)]),
    );
    assert_eq!(providers, HashMap::from([(key(Some("USDT")), CORE_B)]));
    assert_eq!(mapping, HashMap::from([(CORE_A, CORE_B), (CORE_B, CORE_B)]));
}

/// Ignoring the DEX discriminator would merge independent market universes.
#[test]
fn different_dex_on_same_code_stays_separate() {
    let mut first = core(CORE_A, Some("USDT"), true);
    first.exchange = Some(ExchangeId::with_dex(4, "DEX_A"));
    let mut second = core(CORE_B, Some("USDT"), true);
    second.exchange = Some(ExchangeId::with_dex(4, "DEX_B"));
    let (providers, mapping) = elect(&[first, second], &HashMap::new());
    assert_eq!(providers.len(), 2);
    assert_eq!(mapping, HashMap::from([(CORE_A, CORE_A), (CORE_B, CORE_B)]));
}

/// Keeping a non-ready provider would bypass the required first-core fallback.
#[test]
fn not_ready_group_falls_back_to_first_core() {
    let (providers, mapping) = elect(
        &[
            core(CORE_B, Some("USDT"), false),
            core(CORE_A, Some("USDT"), false),
        ],
        &HashMap::from([(key(Some("USDT")), CORE_A)]),
    );
    assert_eq!(providers, HashMap::from([(key(Some("USDT")), CORE_B)]));
    assert_eq!(mapping, HashMap::from([(CORE_A, CORE_B), (CORE_B, CORE_B)]));
}

/// Keeping an unavailable provider would prevent failover to the first Ready core.
#[test]
fn first_ready_core_replaces_non_ready_current_provider() {
    let (providers, mapping) = elect(
        &[
            core(CORE_A, Some("USDT"), false),
            core(CORE_C, Some("USDT"), true),
            core(CORE_B, Some("USDT"), true),
        ],
        &HashMap::from([(key(Some("USDT")), CORE_A)]),
    );
    assert_eq!(providers, HashMap::from([(key(Some("USDT")), CORE_C)]));
    assert_eq!(mapping[&CORE_A], CORE_C);
}

/// Reusing a current provider from another group would cross currency boundaries.
#[test]
fn current_provider_must_belong_to_the_group() {
    let (providers, mapping) = elect(
        &[
            core(CORE_A, Some("USDT"), true),
            core(CORE_B, Some("USDC"), true),
        ],
        &HashMap::from([(key(Some("USDT")), CORE_B)]),
    );
    assert_eq!(
        providers,
        HashMap::from([(key(Some("USDT")), CORE_A), (key(Some("USDC")), CORE_B)])
    );
    assert_eq!(mapping[&CORE_A], CORE_A);
}

/// An unidentified core must neither get a provider nor supply a fallback currency.
#[test]
fn unidentified_cores_are_skipped() {
    let mut unidentified = core(CORE_A, Some("USDT"), true);
    unidentified.exchange = None;
    let (providers, mapping) = elect(&[unidentified, core(CORE_B, None, true)], &HashMap::new());
    assert_eq!(providers, HashMap::from([(key(None), CORE_B)]));
    assert_eq!(mapping, HashMap::from([(CORE_B, CORE_B)]));
}
