//! What the composed directive string actually does, checked against a real `env_filter::Filter`.
//!
//! These assert the LIBRARY's behaviour, not the string's shape. The whole scheme rests on
//! longest-prefix-wins, which is a property of `env_filter` rather than of this module: if a future
//! version resolved directives first-match-wins instead, every area switch would be shadowed by the
//! base filter and silently stop working, and a test comparing strings would still pass.

use super::*;
use crate::diagnostics::config::DiagCfg;

/// Build `spec` the way production does — `try_parse`, not the error-tolerant `parse`.
///
/// The distinction is the whole point: `parse` swallows a spec error and quietly yields an
/// `error`-only filter, so an oracle built on it would agree with a broken spec instead of
/// catching it. Anything `compose` produces must parse cleanly, and this asserts that.
fn built(spec: &str) -> env_filter::Filter {
    env_filter::Builder::new()
        .try_parse(spec)
        .unwrap_or_else(|e| panic!("compose produced a spec that does not parse: {spec:?} — {e}"))
        .build()
}

/// Whether `spec` admits a record at `level` from `target`.
fn allows(spec: &str, target: &str, level: log::Level) -> bool {
    built(spec).enabled(
        &log::Metadata::builder()
            .level(level)
            .target(target)
            .build(),
    )
}

/// The maximum level `spec` can emit, which is what `log::set_max_level` is given.
fn ceiling(spec: &str) -> log::LevelFilter {
    built(spec).filter()
}

fn with_balances() -> DiagCfg {
    let mut cfg = DiagCfg::default();
    cfg.log.balances = true;
    cfg
}

#[test]
fn the_default_base_admits_info_and_stops_below_it() {
    let spec = compose(&DiagCfg::default(), None);
    assert!(allows(&spec, "moon_core::feed::live", log::Level::Info));
    assert!(!allows(&spec, "moon_core::feed::live", log::Level::Debug));
}

#[test]
fn an_area_raises_its_own_subtree_and_nothing_else() {
    let spec = compose(&with_balances(), None);
    assert!(
        allows(&spec, "moon_core::feed::live", log::Level::Debug),
        "the area directive must beat the broader `moon_core=info` in the base — this is the \
         longest-prefix rule the whole scheme depends on; spec={spec}"
    );
    assert!(
        !allows(&spec, "moon_core::market::kline_cache", log::Level::Debug),
        "turning on one area must not raise the level for another"
    );
    assert!(
        !allows(&spec, "moon_core::db", log::Level::Debug),
        "turning on one area must not raise the rest of the crate"
    );
}

#[test]
fn an_area_covers_the_modules_below_it() {
    // The balance trace is emitted from `feed::live` AND `feed::live::commands`; an area that
    // covered only the exact module would silence one of its own three call sites.
    let spec = compose(&with_balances(), None);
    assert!(allows(
        &spec,
        "moon_core::feed::live::commands",
        log::Level::Debug
    ));
}

#[test]
fn every_area_switch_reaches_the_module_that_emits_its_trace() {
    let mut cfg = DiagCfg::default();
    cfg.log.kline_cache = true;
    cfg.log.market_sources = true;
    let spec = compose(&cfg, None);
    assert!(allows(
        &spec,
        "moon_core::market::kline_cache",
        log::Level::Debug
    ));
    assert!(allows(
        &spec,
        "moon_core::market::source::read",
        log::Level::Debug
    ));
}

#[test]
fn everything_off_leaves_the_ceiling_where_it_was() {
    assert_eq!(
        ceiling(&compose(&DiagCfg::default(), None)),
        log::LevelFilter::Info,
        "with no area on, `log::max_level` must stay at Info so every debug! site short-circuits \
         inside the macro and never evaluates its arguments — that is what makes 'off' free"
    );
}

#[test]
fn switching_an_area_on_raises_the_ceiling() {
    assert_eq!(
        ceiling(&compose(&with_balances(), None)),
        log::LevelFilter::Debug,
        "without this the macro's own max_level check drops the record before the filter is ever \
         consulted, and the switch would appear to do nothing"
    );
}

#[test]
fn an_area_cannot_be_defeated_by_the_environment_base() {
    // Same module named by both, so the directives have equal length and only insertion order can
    // separate them. The switch has to win, matching the rule that a switch may only turn tracing ON.
    let spec = compose(&with_balances(), Some("moon_core::feed::live=error"));
    assert!(allows(&spec, "moon_core::feed::live", log::Level::Debug));
}

#[test]
fn an_explicit_base_replaces_the_default_rather_than_joining_it() {
    let spec = compose(&DiagCfg::default(), Some("warn"));
    assert!(!allows(&spec, "moon_core", log::Level::Info));
    assert!(allows(&spec, "moon_core", log::Level::Warn));
}

#[test]
fn the_escape_hatch_is_appended_and_wins() {
    let mut cfg = DiagCfg::default();
    cfg.log.filter = "moon_core::db=debug".to_string();
    let spec = compose(&cfg, None);
    assert!(allows(&spec, "moon_core::db", log::Level::Debug));
    assert!(!allows(&spec, "moon_core::feed", log::Level::Debug));
}

#[test]
fn a_slash_in_the_escape_hatch_is_refused_rather_than_passed_through() {
    // One slash makes the text after it a body match applied to the WHOLE spec, so every record in
    // the application becomes conditional on it. Two make `env_filter` discard every directive and
    // substitute a lone `error`, muting the terminal. Both come from one plausible typo in a
    // hand-edited file, so the field is dropped instead.
    let mut cfg = DiagCfg::default();
    cfg.log.filter = "moon_core::db=debug/oops".to_string();
    let spec = compose(&cfg, None);
    assert_eq!(spec, DEFAULT_BASE_FILTER, "the field must be ignored whole");
    assert!(super::filter_is_rejected(&cfg.log.filter));

    cfg.log.filter = "a/b/c".to_string();
    let spec = compose(&cfg, None);
    assert!(
        allows(&spec, "moon_core", log::Level::Info),
        "the base must survive a filter that would otherwise mute everything below error"
    );
}

#[test]
fn a_directive_typo_in_the_escape_hatch_does_not_take_the_rest_with_it() {
    // `dbg` is not a level. The parser does not drop just this directive — it abandons the whole
    // spec, so appending it unchecked would delete the base AND every area switch and leave the
    // terminal at `error`. `built()` panics on an unparsable spec, so reaching the assertions at
    // all is half the check.
    let mut cfg = with_balances();
    cfg.log.filter = "moon_core::db=dbg".to_string();
    assert!(
        super::filter_is_rejected(&cfg.log.filter),
        "a fragment that fails to parse must be recognised as rejected, not only one with a slash"
    );
    let spec = compose(&cfg, None);
    assert!(allows(&spec, "moon_core::feed::live", log::Level::Debug));
    assert!(allows(&spec, "moon_core", log::Level::Info));
}

#[test]
fn a_valid_escape_hatch_is_still_accepted() {
    let mut cfg = DiagCfg::default();
    cfg.log.filter = "moon_core::db=debug".to_string();
    assert!(
        !super::filter_is_rejected(&cfg.log.filter),
        "the validation must not be so eager that the feature stops working"
    );
}

#[test]
fn a_rejected_filter_does_not_take_the_area_switches_with_it() {
    let mut cfg = with_balances();
    cfg.log.filter = "junk/with/slashes".to_string();
    let spec = compose(&cfg, None);
    assert!(
        allows(&spec, "moon_core::feed::live", log::Level::Debug),
        "a bad escape hatch must not disable a switch the user also turned on"
    );
}

#[test]
fn an_empty_escape_hatch_adds_no_stray_separator() {
    let spec = compose(&DiagCfg::default(), None);
    assert_eq!(spec, DEFAULT_BASE_FILTER);
    assert!(!spec.ends_with(','), "a trailing comma parses as an empty directive");
}

#[test]
fn every_directive_names_a_target_the_terminal_can_actually_emit() {
    // The defect this guards: `moon-ui-gpui` builds ONE binary, `moonterminal`, and declares no
    // `[lib]`, so `module_path!()` — the target `log` stamps on a record — roots every UI line at
    // `moonterminal`. The shipped filter said `moon_ui_gpui=info`, which matched no record ever
    // emitted; the whole UI sat at the baseline `warn` while the spec read as though it were
    // traced, and the chart's own diagnostic switch was inert for the same reason. Nothing failed,
    // which is exactly why it survived: a directive naming a target that does not exist is
    // indistinguishable from one that is simply quiet.
    let spec = compose(&DiagCfg::default(), None);
    assert!(
        allows(&spec, "moonterminal::panels::chart::trade", log::Level::Info),
        "the manual-order trail must reach the log at info: {spec}"
    );
    assert!(
        !allows(&spec, "moon_ui_gpui::panels::chart", log::Level::Info),
        "the dead crate name must not come back: {spec}"
    );
    // The GPUI directive is narrower than it looks and is left that way on purpose: `moon-gpui`
    // renames its lib to `gpui`, so this prefix reaches the sibling platform packages and not GPUI
    // core. Asserted so the doc explaining it cannot drift away from what the spec does.
    assert!(
        allows(&spec, "moon_gpui_windows::platform", log::Level::Info),
        "the platform layer's info records must survive: {spec}"
    );
    assert!(
        !allows(&spec, "gpui::window", log::Level::Info),
        "GPUI core is NOT covered by `moon_gpui`; widening it is a separate decision: {spec}"
    );

    let mut cfg = DiagCfg::default();
    cfg.log.chart_input = true;
    let spec = compose(&cfg, None);
    assert!(
        allows(&spec, "moonterminal::panels::chart::trade", log::Level::Debug),
        "log.chart_input must raise the chart subtree it names: {spec}"
    );
    assert!(
        !allows(&spec, "moonterminal::shell::actions", log::Level::Debug),
        "and only that subtree: {spec}"
    );
}

#[test]
fn the_hotkey_area_raises_dispatch_tracing_and_nothing_else() {
    let mut cfg = DiagCfg::default();
    cfg.log.hotkeys = true;
    let spec = compose(&cfg, None);
    assert!(
        allows(&spec, "moonterminal::hotkeys", log::Level::Debug),
        "log.hotkeys must raise the module that resolves bindings: {spec}"
    );
    assert!(
        !allows(&spec, "moonterminal::panels::chart", log::Level::Debug),
        "and must not drag the chart channel on with it: {spec}"
    );
    assert!(
        !allows(&compose(&DiagCfg::default(), None), "moonterminal::hotkeys", log::Level::Debug),
        "off by default, like every other area"
    );
}
