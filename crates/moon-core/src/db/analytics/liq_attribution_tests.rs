use super::effective_sid_expr;
use std::collections::HashSet;

fn cols(list: &[&str]) -> HashSet<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

fn full() -> HashSet<String> {
    cols(&[
        "strategyid",
        "core_uid",
        "channelname",
        "signaltype",
        "comment",
    ])
}

/// Off, or with no strategy database attached, the expression is the bare column — the
/// panel must behave exactly as it did before the feature existed.
#[test]
fn without_attribution_it_is_the_plain_column() {
    assert_eq!(effective_sid_expr("r", &full(), false), "r.\"strategyid\"");
    assert_eq!(effective_sid_expr("r", &full(), false), "r.\"strategyid\"");
}

/// `core_uid` is named by the correlated subquery, so a source without it must fall back:
/// naming it anyway makes the branch fail to PREPARE, and the whole window dies the moment
/// the switch is turned on.
#[test]
fn a_source_without_core_uid_falls_back() {
    let no_core = cols(&["strategyid", "channelname", "signaltype"]);
    assert_eq!(effective_sid_expr("r", &no_core, true), "r.\"strategyid\"");
}

/// The whole trimmed value is tried BEFORE the bracket-cut one, so a strategy whose own
/// name contains a bracket matches itself rather than a different strategy that happens to
/// be named after its prefix.
#[test]
fn the_whole_name_is_preferred_over_the_cut_one() {
    let e = effective_sid_expr("r", &full(), true);
    let whole = e
        .find("st.name = trim(COALESCE")
        .expect("whole-name lookup");
    let cut = e.find("st.name = trim(substr").expect("cut-name lookup");
    assert!(whole < cut, "the exact match must be tried first: {e}");
    // The obvious spelling — `IN (whole, cut) ORDER BY name = whole DESC` — compiles and
    // reads well, and SQLite refuses it: a correlated reference is not allowed in a
    // subquery's ORDER BY. It passed every string assertion and died at runtime.
    assert!(!e.contains("ORDER BY"), "{e}");
}

/// A source that cannot answer must not be made to guess: without `channelname` there is
/// no way to know a row is a liquidation, and without a name column no way to say whose.
#[test]
fn a_source_missing_its_inputs_falls_back() {
    let no_channel = cols(&["strategyid", "core_uid", "signaltype"]);
    assert_eq!(
        effective_sid_expr("r", &no_channel, true),
        "r.\"strategyid\""
    );
    let no_name = cols(&["strategyid", "core_uid", "channelname"]);
    assert_eq!(effective_sid_expr("r", &no_name, true), "r.\"strategyid\"");
}

/// The detection is an EXACT match. A substring test picks up 15 696 rows against 319 real
/// ones on the production database, because a strategy is named `Liquidations_Short_…`
/// and that name sits in `channelname` on every trade it makes.
#[test]
fn detection_is_exact_never_a_substring() {
    let e = effective_sid_expr("r", &full(), true);
    assert!(e.contains("= 'LIQUIDATION'"), "{e}");
    assert!(
        !e.contains("LIKE"),
        "a LIKE here would swallow a strategy's own name: {e}"
    );
}

/// The correlated subquery MUST name the outer row's core explicitly. Unqualified
/// `core_uid` binds to `strat.strategies` instead, and the lookup then matches a strategy
/// of that name on ANY core — silently attributing the loss to someone else's copy.
#[test]
fn the_subquery_qualifies_the_outer_core() {
    let e = effective_sid_expr("r", &full(), true);
    assert!(e.contains("st.core_uid = r.\"core_uid\""), "{e}");
    assert!(
        e.contains("st.deleted = 0"),
        "a deleted strategy is not an owner: {e}"
    );
}

/// An unmatched name yields 0, which is "Manual" — where the row already was. Measured on
/// the real database: 28 of 319 stay there (a deleted strategy, or no parseable name).
#[test]
fn an_unknown_name_stays_manual() {
    let e = effective_sid_expr("r", &full(), true);
    assert!(
        e.contains("), 0)"),
        "the lookup must COALESCE to 0, not NULL: {e}"
    );
}

/// `signaltype` is preferred and `comment` is the fallback — measured at 288/319 and
/// 304/319 respectively, so neither alone covers the set.
#[test]
fn the_name_comes_from_signaltype_then_comment() {
    let e = effective_sid_expr("r", &full(), true);
    let sig = e.find("signaltype").expect("signaltype used");
    let com = e.find("comment").expect("comment used");
    assert!(sig < com, "signaltype must be tried first: {e}");
    assert!(
        e.contains("instr("),
        "the name is cut at the first bracket: {e}"
    );
}
