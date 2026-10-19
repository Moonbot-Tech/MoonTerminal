//! Synthetic parser, identity and single-venue selection regressions without live IO.

use super::*;

/// Nameless identities still route held tape but cannot manufacture a filename label.
#[test]
fn nameless_identity_line_still_yields_venue() {
    let text = "core 42 identity: exchange_code=1 -> ExchangeId { code: 1, dex: 2 }";
    assert_eq!(
        core_venues_unique(text).get(&42),
        Some(&Some("1:00000002".into()))
    );
    assert!(core_labels(text).is_empty());
}

/// A renamed historical ID must not retain gate eligibility through a valid interval.
#[test]
fn renamed_strategy_is_param_fallback() {
    let conn = versions();
    conn.execute_batch("UPDATE strategies SET name='STRAT_RENAMED';")
        .unwrap();
    for at in [100, 300] {
        assert!(matches!(
            parameters(&conn, 10, 1, at),
            Some(Parameters::ParamFallback)
        ));
    }
}

/// Neutral started lines share the same task-prefix grammar as subsequent chatter.
fn started(clock: &str, strategy: &str, id: u64) -> String {
    task(clock, "XALPHA", &format!("Task {id} started; USDT-XALPHA (strategy <{strategy}>) UseAsk: 1.2345  BUY 1.2 (signal price -2%)")).replace("(678)", &format!("({id})"))
}

/// Overlapping unbound candidates must not let chatter erase an already started outcome.
#[test]
fn chatter_never_downgrades_task_started() {
    let text = detect_line("15:20:22.000", "STRAT_A")
        + &detect_line("15:20:22.010", "STRAT_B")
        + &task("15:20:22.020", "XALPHA", "HistoryDataLoaded: YES")
        + &started("15:20:22.030", "STRAT_A", 678)
        + &task("15:20:22.040", "XALPHA", "EMAFilter: YES");
    let p = parse_core_log("CORE_A", "2026-01-02", &text);
    assert_eq!(p.records[0].outcome, Outcome::TaskStarted);
    assert_eq!(p.records[1].outcome, Outcome::NoFollowUp);
    assert_eq!(p.records[1].task_id, None);
}

/// Chatter for an owned task cannot bind a different overlapping same-coin strategy.
#[test]
fn chatter_does_not_bind_foreign_task_id() {
    let text = detect_line("15:20:22.000", "STRAT_A")
        + &detect_line("15:20:22.010", "STRAT_B")
        + &started("15:20:22.020", "STRAT_A", 678)
        + &started("15:20:22.030", "STRAT_B", 679)
        + &task("15:20:22.040", "XALPHA", "HistoryDataLoaded: YES");
    let p = parse_core_log("CORE_A", "2026-01-02", &text);
    assert_eq!(
        p.records.iter().map(|r| r.task_id).collect::<Vec<_>>(),
        [Some(678), Some(679)]
    );
    assert!(p.records.iter().all(|r| r.outcome == Outcome::TaskStarted));
}

/// A late refusal remains a parsed line without replacing the first terminal decision.
#[test]
fn filtercheck_after_task_started_keeps_first_terminal() {
    let text = detect_line("15:20:22.000", "STRAT_A")
        + &started("15:20:22.020", "STRAT_A", 678)
        + &task("15:20:22.030", "XALPHA", "FilterCheck Before Buy: refused");
    let p = parse_core_log("CORE_A", "2026-01-02", &text);
    assert_eq!(p.records[0].outcome, Outcome::TaskStarted);
    assert_eq!(p.stats.lines, 3);
    assert_eq!(p.stats.unparsed, 0);
}

/// C2 must try the next market when the first contains coverage but no prints.
#[test]
fn c2_tape_for_skips_empty_market() {
    let pairs = vec![
        ("1:00000000".into(), "XALPHA".into()),
        ("1:00000000".into(), "USDT-XALPHA".into()),
    ];
    let span = Coverage::one((0, 100));
    let mut reads = Vec::new();
    let (ticks, _) =
        super::super::tape_for_deal_with("1:00000000", "XALPHA", &pairs, &span, |_, m, _| {
            reads.push(m.to_string());
            let ticks = if m == "XALPHA" {
                Vec::new()
            } else {
                vec![crate::db::tuner::ticks::tests::tick(
                    50,
                    1.0,
                    crate::feed::types::Side::Buy,
                )]
            };
            (ticks, span.clone())
        })
        .unwrap();
    assert_eq!(reads, ["XALPHA", "USDT-XALPHA"]);
    assert_eq!(ticks.len(), 1);
}

/// Emit the documented neutral detect shape, including real TSV separators.
fn detect_line(clock: &str, strategy: &str) -> String {
    format!(
        "{clock}\tINFO\t\t15:20:22   DropsDetection: <{strategy}> USDT-XALPHA  DailyVol: 322m PriceIsLow: false xPriceDelta: 2.9\n"
    )
}

/// A Signal quotes the detect and must never add another record.
fn signal(clock: &str, verdict: &str) -> String {
    format!(
        "{clock}\tINFO\t\t Signal USDT-XALPHA Ask:1.2345 dBTC: 0.1 AutoStart: {verdict} Autodetect ON; Autodetected [ DropsDetection: <STRAT_A> USDT-XALPHA DailyVol: 322m PriceIsLow: false xPriceDelta: 2.9 ] (strategy <STRAT_A>)\n"
    )
}

/// Neutral task prefixes exercise both short annotation and whitespace before the colon.
fn task(clock: &str, prefix: &str, body: &str) -> String {
    format!("{clock}\tINFO\t\t15:20:22  {prefix}: [1] (678) {body}\n")
}

/// Regressions in task parsing must not lose absolute BUY prices or relative BUY variants.
#[test]
fn parses_detect_signal_and_task_into_one_record() {
    for (prefix, buy, expected_price, expected_pct) in [
        ("XALPHA", "1.2098 (signal price -2%)", Some(1.2098), -2.0),
        (
            "XALPHA<Short> ",
            "+0.3% (strategy <STRAT_A>) price)",
            None,
            0.3,
        ),
    ] {
        let text = detect_line("15:20:22.034", "STRAT_A")
            + &signal("15:20:22.034", "TRUE")
            + &task(
                "15:20:22.065",
                prefix,
                &format!(
                    "Task 678 started; USDT-XALPHA (strategy <STRAT_A>) UseAsk: 1.2345 CurAsk: 1.2345  BUY {buy}"
                ),
            );
        let parsed = parse_core_log("CORE_A", "2026-01-02", &text);
        assert_eq!(parsed.records.len(), 1);
        let row = &parsed.records[0];
        assert_eq!(row.outcome, Outcome::TaskStarted);
        assert_eq!(row.ask, Some(1.2345));
        assert_eq!(row.use_ask, Some(1.2345));
        assert_eq!(row.buy_price, expected_price);
        assert_eq!(row.signal_pct, Some(expected_pct));
        assert_eq!(row.task_ms.unwrap() - row.detect_ms, 31);
        assert_eq!(row.x_price_delta, 2.9);
        assert_eq!(row.daily_vol_m, Some(322.0));
        assert!(!row.price_is_low);
        assert_eq!(parsed.stats.unparsed, 0);
    }
}

/// Counting quoted payloads would double the population and hide an unmatched Signal.
#[test]
fn signal_quote_of_detect_is_not_a_second_detect() {
    let text = detect_line("15:20:22.034", "STRAT_A")
        + &signal("15:20:22.034", "TRUE")
        + &signal("15:20:24.000", "TRUE");
    let parsed = parse_core_log("CORE_A", "2026-01-02", &text);
    assert_eq!(parsed.records.len(), 1);
    assert_eq!(parsed.stats.detects, 1);
    assert_eq!(parsed.stats.signals_unpaired, 1);
    assert_eq!(parsed.stats.lines, 3);
}

/// Both higher-than and less-than filter names survive without leaking the strategy name.
#[test]
fn autostart_false_records_filter_name() {
    for (comparison, filter) in [
        ("higher", "BV_SV_FilterRatioMax"),
        ("less", "BV_SV_FilterRatio"),
    ] {
        let verdict =
            format!("FALSE BV/SV: 1.2 is {comparison} then strategy <STRAT_A> {filter} (1.1)");
        let parsed = parse_core_log(
            "CORE_A",
            "2026-01-02",
            &(detect_line("15:20:22.034", "STRAT_A") + &signal("15:20:22.034", &verdict)),
        );
        assert_eq!(
            parsed.records[0].outcome,
            Outcome::AutoStartFalse(filter.into())
        );
        assert_eq!(parsed.records[0].ask, Some(1.2345));
        assert_eq!(parsed.stats.unparsed, 0);
    }
}

/// A refusal must follow its bound task ID; another task's refusal cannot steal the detect.
#[test]
fn filtercheck_before_buy_is_refusal() {
    let base = detect_line("15:20:22.034", "STRAT_A")
        + &signal("15:20:22.034", "TRUE")
        + &task("15:20:22.040", "XALPHA ", "HistoryDataLoaded: YES");
    let wrong = task(
        "15:20:22.050",
        "XALPHA ",
        "FilterCheck Before Buy: market no longer meets the conditions.",
    )
    .replace("(678)", "(679)");
    let parsed = parse_core_log("CORE_A", "2026-01-02", &(base.clone() + &wrong));
    assert_eq!(parsed.records[0].outcome, Outcome::NoFollowUp);
    let parsed = parse_core_log(
        "CORE_A",
        "2026-01-02",
        &(base
            + &wrong
            + &task(
                "15:20:22.060",
                "XALPHA ",
                "FilterCheck Before Buy: market no longer meets the conditions.",
            )),
    );
    assert_eq!(parsed.records[0].outcome, Outcome::FilterCheckRefused);
    assert_eq!(parsed.stats.unparsed, 1);
}

/// Exact strategies on overlapping coins must retain distinct task ownership.
#[test]
fn two_overlapping_same_coin_strategies_pair_correctly() {
    let text=detect_line("15:20:22.000","STRAT_A")+&detect_line("15:20:22.010","STRAT_B")
        +&signal("15:20:22.020","TRUE").replace("STRAT_A","STRAT_B")
        +&signal("15:20:22.030","TRUE")
        +&task("15:20:22.040","XALPHA","Task 678 started; USDT-XALPHA (strategy <STRAT_A>) UseAsk: 1.2345  BUY 1.2 (signal price -2%)")
        +&task("15:20:22.050","XALPHA","Task 679 started; USDT-XALPHA (strategy <STRAT_B>) UseAsk: 1.2345  BUY 1.2 (signal price -2%)").replace("(678)","(679)");
    let parsed = parse_core_log("CORE_A", "2026-01-02", &text);
    assert_eq!(
        parsed
            .records
            .iter()
            .map(|r| (r.task_id, r.outcome.clone()))
            .collect::<Vec<_>>(),
        [
            (Some(678), Outcome::TaskStarted),
            (Some(679), Outcome::TaskStarted)
        ]
    );
    assert_eq!(parsed.stats.signals_unpaired, 0);
}

/// A malformed ASK still consumes its Signal; a later Signal must enrich the other detect.
#[test]
fn repeated_same_strategy_detects_consume_distinct_signals() {
    let text = detect_line("15:20:22.000", "STRAT_A")
        + &detect_line("15:20:22.100", "STRAT_A")
        + &signal("15:20:22.010", "TRUE").replace("Ask:1.2345", "Ask:invalid")
        + &signal("15:20:22.110", "TRUE");
    let parsed = parse_core_log("CORE_A", "2026-01-02", &text);
    assert_eq!(parsed.records.len(), 2);
    assert_eq!(parsed.records[0].ask, None);
    assert_eq!(parsed.records[1].ask, Some(1.2345));
    assert_eq!(parsed.stats.signals_unpaired, 0);
    assert_eq!(parsed.stats.unparsed, 1);
    let tied = detect_line("15:20:22.000", "STRAT_A")
        + &detect_line("15:20:22.000", "STRAT_A")
        + &signal("15:20:22.010", "TRUE");
    let tied = parse_core_log("CORE_A", "2026-01-02", &tied);
    assert_eq!(tied.stats.signals_unpaired, 1);
    assert!(
        tied.records
            .iter()
            .all(|row| row.ask.is_none() && row.outcome == Outcome::Ambiguous)
    );
}

/// A quoted Signal without a standalone detect must remain an unmatched line, not a record.
#[test]
fn unmatched_quoted_signal_is_counted_not_paired() {
    let parsed = parse_core_log("CORE_A", "2026-01-02", &signal("15:20:22.000", "TRUE"));
    assert_eq!(parsed.stats.signals_unpaired, 1);
    assert_eq!(parsed.stats.detects, 0);
    assert!(parsed.records.is_empty());
}

/// Daily rotation must preserve same-label task association across UTC midnight.
#[test]
fn followup_across_day_boundary_binds() {
    let first = detect_line("23:59:59.900", "STRAT_A");
    let second = signal("00:00:00.010", "TRUE")
        + &task(
            "00:00:00.020",
            "XALPHA",
            "Task 678 started; USDT-XALPHA (strategy <STRAT_A>) UseAsk: 1.2345  BUY 1.2 (signal price -2%)",
        );
    let parsed = parse_core_files("CORE_A", &[("2026-01-02", &first), ("2026-01-03", &second)]);
    assert_eq!(parsed.records.len(), 1);
    assert_eq!(parsed.records[0].outcome, Outcome::TaskStarted);
    assert_eq!(parsed.records[0].task_id, Some(678));
    assert_eq!(
        parsed.records[0].task_ms.unwrap() - parsed.records[0].detect_ms,
        120
    );
    assert_eq!(parsed.stats.signals_unpaired, 0);
}

/// A refusal carrying no task ownership must never become an attributed FilterCheck refusal.
#[test]
fn filtercheck_without_task_id_is_ambiguous() {
    let text = detect_line("15:20:22.000", "STRAT_A")
        + "15:20:22.100 INFO XALPHA: FilterCheck Before Buy: market no longer meets the conditions.\n";
    let parsed = parse_core_log("CORE_A", "2026-01-02", &text);
    assert_eq!(parsed.records[0].outcome, Outcome::Ambiguous);
    let text = detect_line("15:20:22.000", "STRAT_A")
        + &task(
            "15:20:22.100",
            "XALPHA",
            "FilterCheck Before Buy: market no longer meets the conditions.",
        );
    assert_eq!(
        parse_core_log("CORE_A", "2026-01-02", &text).records[0].outcome,
        Outcome::Ambiguous
    );
}

/// A small version database isolates interval boundaries from process-global strategy readers.
fn versions() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE strategies(core_uid INTEGER,strategy_id INTEGER,name TEXT); INSERT INTO strategies VALUES(1,10,'STRAT_A'); CREATE TABLE strategy_versions(core_uid INTEGER,strategy_id INTEGER,valid_from INTEGER,valid_to INTEGER,raw_json TEXT); INSERT INTO strategy_versions VALUES(1,10,100,200,'{\"StrategyName\":\"STRAT_A\",\"Short\":false,\"DropsPriceDelta\":2.0}'),(1,10,300,NULL,'{\"StrategyName\":\"STRAT_A\",\"Short\":false,\"DropsPriceDelta\":3.0}');").unwrap();
    conn
}

/// An interval gap and the exclusive end must not inherit a nearby parameter version.
#[test]
fn param_version_must_contain_detect_ms() {
    let conn = versions();
    for at in [100, 199, 300] {
        let Some(Parameters::Long { params, from_dump }) = parameters(&conn, 10, 1, at) else {
            panic!("valid interval refused")
        };
        assert_eq!(params.price_delta_pct, if at < 200 { 2.0 } else { 3.0 });
        assert!(from_dump[3]);
        assert!(!from_dump[0]);
    }
    for at in [200, 299] {
        assert!(matches!(
            parameters(&conn, 10, 1, at),
            Some(Parameters::ParamFallback)
        ));
    }
    conn.execute_batch("INSERT INTO strategy_versions VALUES(1,11,1,NULL,'{\"StrategyName\":\"STRAT_A\",\"Short\":false}');").unwrap();
    assert!(matches!(
        parameters(&conn, 10, 1, 100),
        Some(Parameters::ParamFallback)
    ));
    assert_eq!(
        join_strategy(&conn, 1, "STRAT_A").unwrap(),
        Join::ParamFallback
    );
    conn.execute_batch("DELETE FROM strategy_versions WHERE strategy_id=11; UPDATE strategies SET name='STRAT_RENAMED';").unwrap();
    assert_eq!(join_strategy(&conn, 1, "STRAT_A").unwrap(), Join::Exact(10));
}

/// A future dump and a side-changing history must both stay outside the calibration cohort.
#[test]
fn future_version_is_fallback_not_dump() {
    let conn = versions();
    assert!(matches!(
        parameters(&conn, 10, 1, 99),
        Some(Parameters::ParamFallback)
    ));
    conn.execute_batch("UPDATE strategy_versions SET raw_json='{\"StrategyName\":\"STRAT_A\",\"Short\":true}' WHERE valid_from=300;").unwrap();
    assert!(matches!(
        parameters(&conn, 10, 1, 150),
        Some(Parameters::ParamFallback)
    ));
}

/// A parameter switch, rather than the logged factual low flag, controls supported eligibility.
#[test]
fn price_is_low_strategy_excluded() {
    let conn = versions();
    assert!(matches!(
        parameters(&conn, 10, 1, 100),
        Some(Parameters::Long { .. })
    ));
    conn.execute_batch("UPDATE strategy_versions SET raw_json='{\"StrategyName\":\"STRAT_A\",\"DropsPriceIsLow\":true}';").unwrap();
    assert!(matches!(
        parameters(&conn, 10, 1, 100),
        Some(Parameters::PriceIsLowOn)
    ));
}

/// A late task and a mismatching named strategy must leave the detect without an outcome.
#[test]
fn detect_without_followup_is_no_followup() {
    for (clock, strategy) in [
        ("15:20:27.034", "STRAT_A"),
        ("15:20:27.035", "STRAT_A"),
        ("15:20:22.065", "(2) STRAT_A"),
    ] {
        let text = detect_line("15:20:22.034", "STRAT_A")
            + &task(
                clock,
                "XALPHA",
                &format!(
                    "Task 678 started; USDT-XALPHA (strategy <{strategy}>) UseAsk: 1.2345 CurAsk: 1.2345  BUY 1.2098 (signal price -2%)"
                ),
            );
        let parsed = parse_core_log("CORE_A", "2026-01-02", &text);
        assert_eq!(
            parsed.records[0].outcome,
            if clock == "15:20:27.034" {
                Outcome::TaskStarted
            } else {
                Outcome::NoFollowUp
            }
        );
    }
    let parsed = parse_core_log(
        "CORE_A",
        "2026-01-02",
        &detect_line("15:20:22.034", "STRAT_A"),
    );
    assert_eq!(parsed.records[0].outcome, Outcome::NoFollowUp);
}

/// Stripping too early conflates distinct strategies; multiple matches must be refused.
#[test]
fn duplicate_prefix_kept_then_stripped_only_on_miss() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE strategies(core_uid INTEGER, strategy_id INTEGER, name TEXT); CREATE TABLE strategy_versions(core_uid INTEGER,strategy_id INTEGER,raw_json TEXT); INSERT INTO strategies VALUES(1, 10, 'STRAT_A'), (1, 11, '(2) STRAT_A'), (2, 12, '(2) STRAT_A');").unwrap();
    assert_eq!(
        join_strategy(&conn, 1, "(2) STRAT_A").unwrap(),
        Join::Exact(11)
    );
    assert_eq!(
        join_strategy(&conn, 1, "(3) STRAT_A").unwrap(),
        Join::PrefixStripped(10)
    );
    assert_eq!(join_strategy(&conn, 3, "STRAT_A").unwrap(), Join::Missing);
    conn.execute_batch("INSERT INTO strategies VALUES(1, 13, '(2) STRAT_A');")
        .unwrap();
    assert_eq!(
        join_strategy(&conn, 1, "(2) STRAT_A").unwrap(),
        Join::Ambiguous
    );
    conn.execute_batch("INSERT INTO strategies VALUES(1, 14, 'STRAT_A');")
        .unwrap();
    assert_eq!(
        join_strategy(&conn, 1, "(3) STRAT_A").unwrap(),
        Join::Ambiguous
    );
    let parsed = parse_core_log(
        "CORE_A",
        "2026-01-02",
        &detect_line("15:20:22.034", "(2) STRAT_A"),
    );
    assert_eq!(parsed.records[0].strategy, "(2) STRAT_A");
}

/// Large backward clocks are stragglers; small interleaved steps keep the file's UTC date.
#[test]
fn backward_clock_line_is_counted_unparsed_not_rolled() {
    let text = "23:59:59.999\tINFO\t\tunrelated\n".to_string()
        + &detect_line("00:00:00.034", "STRAT_A")
        + &detect_line("00:00:00.030", "STRAT_A");
    let parsed = parse_core_log("CORE_A", "1970-01-01", &text);
    assert!(parsed.records.is_empty());
    assert_eq!(parsed.stats.unparsed, 2);
    let interleaved = parse_core_log(
        "CORE_A",
        "1970-01-01",
        &(detect_line("15:20:22.100", "STRAT_A") + &detect_line("15:20:22.000", "STRAT_A")),
    );
    assert_eq!(
        interleaved
            .records
            .iter()
            .map(|d| d.detect_ms)
            .collect::<Vec<_>>(),
        [55_222_100, 55_222_000]
    );
    assert_eq!(interleaved.stats.unparsed, 0);
    let parsed = parse_core_log(
        "CORE_A",
        "1970-01-02",
        &detect_line("15:20:22.034", "STRAT_A"),
    );
    assert_eq!(parsed.records[0].detect_ms, 86_400_000 + 55_222_034);
}

/// A missing Ask cannot let a later Signal overwrite a consumed refusal.
#[test]
fn signal_without_ask_still_consumed_once() {
    let text = detect_line("15:20:22.000", "STRAT_A")
        + &signal("15:20:22.010", "FALSE BV_SV_FilterRatioMax")
            .replace("Ask:1.2345", "Ask:invalid")
        + &signal("15:20:22.020", "TRUE");
    let parsed = parse_core_log("CORE_A", "2026-01-02", &text);
    assert_eq!(parsed.records[0].ask, None);
    assert_eq!(
        parsed.records[0].outcome,
        Outcome::AutoStartFalse("BV_SV_FilterRatioMax".into())
    );
    assert_eq!(parsed.stats.signals_unpaired, 1);
}

/// Invalid verdicts must not masquerade as a quiet, successfully admitted signal.
#[test]
fn autostart_false_without_filter_is_ambiguous() {
    for verdict in ["FALSE", "UNKNOWN"] {
        let parsed = parse_core_log(
            "CORE_A",
            "2026-01-02",
            &(detect_line("15:20:22.000", "STRAT_A") + &signal("15:20:22.010", verdict)),
        );
        assert_eq!(parsed.records[0].outcome, Outcome::Ambiguous);
        assert_eq!(parsed.stats.unparsed, 1);
    }
}

/// Repeated announcements cannot rescue a UID whose venue identity conflicts.
#[test]
fn uid_with_two_venues_refused() {
    let a =
        "core 42 \u{ab}CORE_A\u{bb} identity: exchange_code=1 -> ExchangeId { code: 1, dex: 0 }\n";
    let b =
        "core 42 \u{ab}CORE_A\u{bb} identity: exchange_code=2 -> ExchangeId { code: 2, dex: 0 }\n";
    assert_eq!(
        core_venues_unique(a).get(&42),
        Some(&Some("1:00000000".into()))
    );
    assert_eq!(
        core_venues_unique(&(a.to_string() + b + a)).get(&42),
        Some(&None)
    );
}

/// Forwarded core payloads in app logs must not double the detect population.
#[test]
fn core_label_app_is_counted() {
    let text =
        "core 42 \u{ab}app\u{bb} identity: exchange_code=1 -> ExchangeId { code: 1, dex: 0 }\n"
            .to_string()
            + &detect_line("15:20:22.000", "STRAT_A");
    let (records, stats, app, failed) =
        parse_log_snapshot(vec![("2026-01-02".into(), "app".into(), text)], 0);
    assert_eq!(stats.detects, 0);
    assert_eq!(failed, 0);
    assert!(records.is_empty());
    assert_eq!(core_labels(&app).get("app"), Some(&42));
}

/// Fully recorded quiet markets stay coverage-qualified even without a print.
#[test]
fn quiet_coin_full_coverage_is_full_not_none() {
    let span = Coverage::one((0, 100));
    let pairs = vec![("1:00000000".into(), "XALPHA".into())];
    let (ticks, covered) = tape_for_coin_with("1:00000000", "XALPHA", &pairs, &span, |_, _, _| {
        (Vec::new(), span.clone())
    })
    .unwrap();
    assert!(ticks.is_empty());
    assert!(covered.covers(&span));
}

/// The filename key must use the writer's sanitizer while sharing identity venue parsing.
#[test]
fn core_label_matches_sanitize_label_of_identity_name() {
    let text =
        "core 42 \u{ab}CORE_A /\u{bb} identity: exchange_code=1 -> ExchangeId { code: 1, dex: 2 }";
    let labels = core_labels(text);
    assert_eq!(
        labels.get(&crate::applog::sanitize_label("CORE_A /")),
        Some(&42)
    );
    assert_eq!(
        identity_lines(text).next(),
        Some((42, Some("CORE_A /".into()), "1:00000002".into()))
    );
}

/// A sanitize collision must stay refused even after the first UID announces itself again.
#[test]
fn ambiguous_core_label_refused() {
    let a = "core 42 \u{ab}CORE_A /\u{bb} identity: exchange_code=1 -> ExchangeId { code: 1, dex: 0 }\n";
    let b = "core 43 \u{ab}CORE_A ?\u{bb} identity: exchange_code=1 -> ExchangeId { code: 1, dex: 0 }\n";
    assert_eq!(core_labels(&(a.to_string() + a)).len(), 1);
    assert!(core_labels(&(a.to_string() + b + a)).is_empty());
}

/// Invalid detect data must raise accounting rather than silently shrink the population.
#[test]
fn unparsed_lines_are_counted() {
    let text = detect_line("15:20:22.034", "STRAT_A")
        .replace("xPriceDelta: 2.9", "xPriceDelta: invalid")
        + "invalid DropsDetection: <STRAT_A>\n15:20:23.000\tINFO\t\tunrelated\n";
    let parsed = parse_core_log("CORE_A", "2026-01-02", &text);
    assert!(parsed.records.is_empty());
    assert_eq!(parsed.stats.unparsed, 2);
    assert_eq!(parsed.stats.lines, 3);
}

/// A second venue with usable prints cannot rescue a missing tape on the selected venue.
#[test]
fn tape_for_coin_never_merges_venues() {
    let pairs = vec![
        ("2:00000000".into(), "USDT-XALPHA".into()),
        ("1:00000000".into(), "USDT-XALPHA".into()),
    ];
    let span = Coverage::one((0, 100));
    let mut reads = Vec::new();
    let answer = tape_for_coin_with("1:00000000", "XALPHA", &pairs, &span, |venue, _, _| {
        reads.push(venue.to_string());
        (Vec::new(), span.clone())
    });
    let (ticks, covered) = answer.unwrap();
    assert!(ticks.is_empty());
    assert!(covered.covers(&span));
    assert_eq!(reads, ["1:00000000"]);
}

/// Frozen legacy selection cases pin fallback order, clipping, sorting and coverage ownership.
#[test]
fn tape_for_refactor_matches_previous_selection() {
    let pairs = vec![
        ("2:00000000".into(), "USDT-XALPHA".into()),
        ("1:00000000".into(), "XALPHA".into()),
        ("1:00000000".into(), "USDT-XALPHA".into()),
    ];
    let span = Coverage::one((10, 20));
    let ticks = [21, 20, 9, 10]
        .map(|at| crate::db::tuner::ticks::tests::tick(at, 1.2345, crate::feed::types::Side::Buy))
        .to_vec();
    let mut queried = Vec::new();
    let (selected, covered) =
        tape_for_coin_with("1:00000000", "XALPHA", &pairs, &span, |venue, market, _| {
            queried.push((venue.to_string(), market.to_string()));
            if market == "XALPHA" {
                (Vec::new(), Coverage::none())
            } else {
                (ticks.clone(), Coverage::one((10, 15)))
            }
        })
        .unwrap();
    assert_eq!(queried, pairs[1..]);
    assert_eq!(
        selected
            .iter()
            .map(|t| t.time_ms as i64)
            .collect::<Vec<_>>(),
        [10, 20]
    );
    assert_eq!(covered.spans(), &[(10, 15)]);
    let mut deal = crate::db::tuner::ticks::tests::deal();
    deal.coin = "XALPHA".into();
    // The legacy deal rule admits the bare catalog coin, never the log-only spelling.
    assert_eq!(
        super::super::tape_pairs_for_deal(&deal, "1:00000000", &pairs),
        vec![("1:00000000".to_string(), "XALPHA".to_string())]
    );
    // Empty inventories exercise the deal wrapper without starting process-global readers.
    assert!(super::super::tape_for(&deal, "1:00000000", &[], &span).is_none());
}
