// Explicit imports on purpose: `use super::*` would pull in the parent's `gpui::*`
// re-export, whose `test` shadows the built-in attribute and makes `#[test]` expand
// recursively ("recursion limit reached").
use super::{
    LineView, RefreshGate, aggregate, collect_coin_bases, exchange_chart_candidates,
    exchange_members_from, find_coin, selected_core_log_sig,
};
use crate::panels::log::{
    AGG_PER_CORE, LogSource, LogSourceItem, VIEW_LIMIT, exchange_membership_changed,
};
use moon_core::applog::LogLine;
use moon_core::session::CoreStore;
use std::collections::{HashMap, HashSet};

/// Builds a core selector entry for aggregate tests without involving UI state.
fn source(id: u64, display: &str) -> LogSourceItem {
    LogSourceItem {
        source: LogSource::Core(id),
        display: display.to_string(),
        file_label: display.to_string(),
    }
}

/// Builds one sortable core log row with a deterministic payload.
fn row(ts: &str, msg: String) -> LogLine {
    LogLine {
        ts: ts.to_string(),
        level: log::Level::Info,
        target: String::new(),
        msg,
    }
}

/// A multi-line message is flattened before its coin range is computed.
#[test]
fn multiline_message_flattens_and_keeps_the_coin_range_valid() {
    let line = LogLine::core(0, "SPK order failed\r\n  at retry 2".to_string());
    let known = HashSet::from(["SPK".to_string()]);
    let view = LineView::parse(&line, &known);

    assert!(
        !view.flat.contains('\n') && !view.flat.contains('\r'),
        "raw break survived into the log row: {:?}",
        view.flat
    );
    let (range, base) = view
        .coin
        .expect("SPK is a known base and should be detected");
    assert_eq!(&view.flat[range], "SPK");
    assert_eq!(base, "SPK");
}

/// `render.rs:RefreshGate::observe` removing the active guard would rebuild a hidden Log tab on
/// every backend revision instead of catching up once when the user opens it.
#[test]
fn refresh_gate_skips_hidden_revisions_and_catches_up_on_activation() {
    let mut gate = RefreshGate::default();

    assert!(!gate.observe(1), "a never-opened dock tab must stay idle");
    assert!(
        gate.set_active(true, 1),
        "the first activation must load the current tail"
    );
    assert!(
        !gate.observe(1),
        "an unchanged active source must not reload"
    );
    assert!(
        gate.observe(2),
        "an active source must schedule new rows for its next render"
    );
    assert!(
        !gate.observe(2),
        "the same pending revision must not schedule duplicate work"
    );
    assert!(
        gate.take_observed_reload(),
        "the next actual render must consume the scheduled reload"
    );
    gate.record_reload(2);
    assert!(!gate.set_active(false, 2));
    assert!(!gate.observe(3), "a hidden tab must ignore new revisions");
    assert!(
        gate.set_active(true, 3),
        "reactivation must catch up to revisions missed while hidden"
    );
}

/// `render.rs:RefreshGate::request_reload` becoming a no-op would leave a hidden panel at its
/// scrolled position after follow-tail resumes until another log revision happens to arrive.
#[test]
fn requested_reload_survives_deactivation_until_the_next_render() {
    let mut gate = RefreshGate::default();
    assert!(gate.set_active(true, 7));
    assert!(!gate.set_active(false, 7));

    gate.request_reload();

    assert!(
        gate.take_observed_reload(),
        "a hidden follow-tail resume must survive until the panel renders again"
    );
}

/// `render.rs:aggregate` changing the per-core tail iterator from newest-first to oldest-first
/// would show stale rows instead of the newest `VIEW_LIMIT` entries across all cores.
#[test]
fn aggregate_keeps_only_the_globally_newest_rows() {
    let sources = [source(1, "alpha"), source(2, "beta"), source(3, "gamma")];
    let mut store = CoreStore::default();
    for id in 1..=3 {
        store.ensure(id);
    }
    for local_ix in 0..AGG_PER_CORE {
        for (source_ix, id) in (1..=3).enumerate() {
            let global_ix = local_ix * 3 + source_ix;
            store
                .core_mut(id)
                .expect("test core must exist")
                .log
                .push_back(row(&format!("{global_ix:08}"), format!("row-{global_ix}")));
        }
    }

    let lines = aggregate(&store, &sources, None);

    assert_eq!(lines.len(), VIEW_LIMIT);
    let first_global = AGG_PER_CORE * sources.len() - VIEW_LIMIT;
    for (offset, line) in lines.iter().enumerate() {
        let global_ix = first_global + offset;
        assert_eq!(line.ts, format!("{global_ix:08}"));
        assert_eq!(line.msg, format!("row-{global_ix}"));
        assert_eq!(line.target, sources[global_ix % sources.len()].display);
    }
}

/// `render.rs:aggregate` reversing the heap's source-ordinal tie-break would change the stable
/// equal-timestamp cutoff and relabel which core's oldest rows remain visible.
#[test]
fn aggregate_preserves_source_and_row_order_for_equal_timestamps() {
    let sources = [source(1, "alpha"), source(2, "beta"), source(3, "gamma")];
    let mut store = CoreStore::default();
    for id in 1..=3 {
        store.ensure(id);
        let core = store.core_mut(id).expect("test core must exist");
        for row_ix in 0..AGG_PER_CORE {
            core.log
                .push_back(row("same-time", format!("{id}-{row_ix:04}")));
        }
    }

    let lines = aggregate(&store, &sources, None);

    assert_eq!(lines.len(), VIEW_LIMIT);
    let dropped_from_first = AGG_PER_CORE * sources.len() - VIEW_LIMIT;
    for (offset, line) in lines[..AGG_PER_CORE - dropped_from_first]
        .iter()
        .enumerate()
    {
        let row_ix = dropped_from_first + offset;
        assert_eq!(line.msg, format!("1-{row_ix:04}"));
        assert_eq!(line.target, "alpha");
    }
    for (source_ix, chunk) in lines[AGG_PER_CORE - dropped_from_first..]
        .chunks_exact(AGG_PER_CORE)
        .enumerate()
    {
        let id = source_ix + 2;
        for (row_ix, line) in chunk.iter().enumerate() {
            assert_eq!(line.msg, format!("{id}-{row_ix:04}"));
            assert_eq!(line.target, sources[id - 1].display);
        }
    }
}

/// `render.rs:exchange_members_from` dropping the live, group, or exact-exchange intersection
/// would mix disconnected, foreign-window, or foreign-exchange cores into a selected exchange.
#[test]
fn exchange_members_require_every_membership_constraint() {
    let configured = [(1, "desk"), (2, "desk"), (3, "other"), (4, "desk")];
    let live = HashSet::from([1, 2, 3]);
    let exchange_names = HashMap::from([
        (1, "Binance Futures".to_string()),
        (2, "ByBit Futures".to_string()),
        (3, "Binance Futures".to_string()),
        (4, "Binance Futures".to_string()),
    ]);

    let members = exchange_members_from(
        configured,
        &live,
        &exchange_names,
        "desk",
        "Binance Futures",
    );

    assert_eq!(members, HashSet::from([1]));
    assert_eq!(
        exchange_members_from(configured, &live, &exchange_names, "", "Binance Futures",),
        HashSet::from([1, 3]),
        "an empty panel group must include matching live cores from every group"
    );
}

/// `render.rs:selected_core_log_sig` omitting the core id would miss a same-revision membership
/// replacement, while folding every store entry would refresh for an unrelated exchange.
#[test]
fn exchange_signature_tracks_membership_but_ignores_foreign_revisions() {
    let mut store = CoreStore::default();
    for id in [1, 2, 9] {
        store.ensure(id);
        store.core_mut(id).expect("test core must exist").log_rev = 5;
    }
    let selected = HashSet::from([1]);
    let original = selected_core_log_sig(&store, &selected);

    store
        .core_mut(9)
        .expect("foreign test core must exist")
        .log_rev = 6;
    assert_eq!(
        selected_core_log_sig(&store, &selected),
        original,
        "a foreign exchange revision must not rebuild the selected source"
    );
    assert_ne!(
        selected_core_log_sig(&store, &HashSet::from([2])),
        original,
        "same-revision membership replacement must rebuild the selected source"
    );
}

/// `render.rs:exchange_chart_candidates` resolving the clicked row before the membership filter
/// would open a same-named core outside the selected exchange.
#[test]
fn exchange_chart_candidates_never_escape_membership() {
    let configured = [(1, "BinF1"), (2, "Other"), (3, "BinF2")];
    let members = HashSet::from([1, 3]);

    assert_eq!(
        exchange_chart_candidates(configured, &members, "Other"),
        vec![1, 3],
        "an out-of-exchange target must fall back only to selected members"
    );
    assert_eq!(
        exchange_chart_candidates(configured, &members, "BinF2"),
        vec![3],
        "an in-exchange target should narrow to its exact core"
    );
}

/// `render.rs:aggregate` ignoring its `allowed` membership would display rows from another
/// exchange while the dropdown still names the selected exchange.
#[test]
fn aggregate_excludes_sources_outside_selected_exchange() {
    let sources = [source(1, "alpha"), source(2, "beta"), source(3, "gamma")];
    let mut store = CoreStore::default();
    for (id, message) in [(1, "one"), (2, "foreign"), (3, "three")] {
        store.ensure(id);
        store
            .core_mut(id)
            .expect("test core must exist")
            .log
            .push_back(row("00000001", message.to_string()));
    }

    let lines = aggregate(&store, &sources, Some(&HashSet::from([1, 3])));

    assert_eq!(
        lines
            .iter()
            .map(|line| line.target.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha", "gamma"]
    );
    assert!(
        lines.iter().all(|line| line.msg != "foreign"),
        "a foreign exchange row leaked into the selected aggregate"
    );
}

/// `mod.rs:reload_rows` treating every exchange membership as unchanged would preserve departed
/// core rows in the paused buffer after a live membership swap.
#[test]
fn exchange_membership_changes_invalidate_paused_rows() {
    let original = HashSet::from([1, 2]);
    let replacement = HashSet::from([1, 3]);

    assert!(!exchange_membership_changed(
        Some(&original),
        Some(&original)
    ));
    assert!(exchange_membership_changed(
        Some(&original),
        Some(&replacement)
    ));
    assert!(
        exchange_membership_changed(None, Some(&replacement)),
        "the first exchange snapshot must not inherit rows from a previous source"
    );
    assert!(
        !exchange_membership_changed(Some(&original), None),
        "a non-exchange source relies on its normal source-change reset"
    );
}

/// The coin the core states at the head of its own line, as the panel must read it.
///
/// Both shapes come from real core logs, and both defeat every shape-based rule: `CLO` is a bare
/// three-letter name, `1kRATS` carries a lowercase letter.
#[test]
fn a_core_line_head_names_its_coin() {
    let known = HashSet::new();
    let short = "12:27:56  CLO<Short> : [1] (48190) Auto cancel buy order activated since 184 sec.";
    let long = "12:27:47  1kRATS: [1] (48186) Auto cancel buy order activated since 180 sec.";

    let (range, base) = find_coin(short, &known).expect("CLO стоит в голове строки");
    assert_eq!(&short[range], "CLO");
    assert_eq!(base, "CLO");

    let (range, base) = find_coin(long, &known).expect("1kRATS стоит в голове строки");
    assert_eq!(
        &long[range], "1kRATS",
        "строчная буква — часть имени монеты"
    );
    assert_eq!(base, "1kRATS");
}

/// A two-letter name is a real coin (`US`), and the slot bracket is what separates a coin head
/// from an ordinary prefixed sentence.
#[test]
fn only_the_slot_or_direction_tail_makes_a_head_a_coin() {
    let known = HashSet::new();

    let two = "00:00:02  US<Short> : [4] (155845) BUY Order SET!";
    assert_eq!(
        find_coin(two, &known).map(|(r, _)| &two[r]),
        Some("US"),
        "двухбуквенная монета — тоже монета"
    );

    let prefixed = "12:27:56  PumpDetection: NBIS Ask:199.39 dBTC: 0.16";
    assert_ne!(
        find_coin(prefixed, &known).map(|(_, base)| base),
        Some("PumpDetection".to_string()),
        "обычная строка с префиксом — не монета"
    );

    let sentence = "Starting new MoonShot market: USDT-AIO (strategy <MainShotMS>) Delay: 16";
    assert_eq!(
        find_coin(sentence, &known).map(|(_, base)| base),
        Some("AIO".to_string()),
        "без головы работает прежнее правило рыночной формы"
    );
}

/// A head coin joins the known set, so a later BARE mention of it lights up as well.
#[test]
fn a_head_coin_is_learned_for_later_bare_mentions() {
    let line = |msg: &str| LogLine {
        ts: "2026-08-03 12:27:56".into(),
        level: log::Level::Info,
        target: String::new(),
        msg: msg.to_string(),
    };
    let lines = [
        line("12:27:47  1kRATS: [1] (48186) Auto cancel buy order activated"),
        line("Srv: NewOrder worker created: Market=1kRATS CorrelationTag=<0>"),
    ];

    let known = collect_coin_bases(&lines);

    assert!(known.contains("1kRATS"), "голова строки учит имя монеты");
    let bare = &lines[1].msg;
    assert_eq!(
        find_coin(bare, &known).map(|(r, _)| &bare[r]),
        Some("1kRATS"),
        "выученное имя подсвечивается и в строке без головы"
    );
}
