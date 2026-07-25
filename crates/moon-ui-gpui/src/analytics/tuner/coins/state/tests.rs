use super::{CoinFilter, CoinLists, CoinsState, coin_token};
use moon_core::db::ReadFail;
use moon_core::db::analytics::GroupStat;
use moon_core::db::coin_lists::{CoinListEntries, CoinListRow, CoinListRows};

fn coin(name: &str) -> GroupStat {
    GroupStat {
        name: name.to_string(),
        ..Default::default()
    }
}

/// The two sides name a coin differently — the report writes the contract (`BTC_RP`),
/// the strategy field a bare token in free case (`mith`). Matching the raw strings
/// finds nothing, which renders as "the blacklist is empty" rather than as a bug.
#[test]
fn lists_match_report_coin_names() {
    let l = CoinLists::parse("ANC, GALA , mith\nbtcst", "1kFLOKI;ENA");
    assert!(l.black.contains(&coin_token("ANC_RP")));
    assert!(l.black.contains(&coin_token("btcst_1230")));
    assert!(l.white.contains(&coin_token("1KFLOKI")));
    assert_eq!(l.black.len(), 4, "separators split, blanks drop");
    assert!(CoinLists::parse("", "").is_empty());
}

/// `coins/state.rs:CoinLists::from_rows` must preserve both sides of the shared fallible
/// snapshot; dropping either iterator makes a successful reload erase that side before Save.
#[test]
fn snapshot_rows_supply_both_saved_list_sides() {
    let rows = CoinListRows {
        black: vec![CoinListRow {
            coin: "BTC".into(),
            entries: vec!["BTC".into()],
            core_uid: 1,
            core_name: "core".into(),
            since_ms: None,
            before_ms: None,
        }],
        white: vec![CoinListEntries {
            coin: "ETH".into(),
            entries: vec!["ETH".into()],
            core_uid: 1,
        }],
    };

    let lists = CoinLists::from_rows(&rows);

    assert_eq!(lists.black.into_iter().collect::<Vec<_>>(), vec!["BTC"]);
    assert_eq!(lists.white.into_iter().collect::<Vec<_>>(), vec!["ETH"]);
}

/// `coins/state.rs:CoinsState::adopt_if_ready` must leave state untouched on `ReadFail`;
/// replacing the error arm with `adopt(default)` erases an unsaved draft after a transient
/// strategies-database failure and makes the next Save destructive.
#[test]
fn failed_saved_list_read_preserves_the_draft() {
    let mut state = CoinsState::default();
    state.adopt(CoinLists::parse("BTC", ""));
    state.toggle(true, coin_token("ETH"));

    state.adopt_if_ready(Err(ReadFail::NotReady));

    assert!(state.saved.black.contains("BTC"));
    assert!(state.work.black.contains("ETH"));
    assert!(state.has_changes());
}

/// v1 is "the same trades with these lists applied": whitelist keeps, blacklist drops,
/// and one token expands to every contract of that coin the period holds.
#[test]
fn variants_apply_the_working_lists() {
    let universe = [coin("BTC_RP"), coin("BTC_1230"), coin("ETH"), coin("SOL")];
    let mut s = CoinsState::default();
    let p = s.plan(&universe);
    assert_eq!(p.variants.len(), 2, "v1 exists from the start");
    assert!(
        p.variants[1].is_empty(),
        "and restricts nothing until a list says so"
    );

    s.toggle(true, coin_token("BTC"));
    let p = s.plan(&universe);
    assert_eq!(p.variants.len(), 2);
    assert_eq!(
        p.variants[1].coins_out,
        vec!["BTC_1230".to_string(), "BTC_RP".to_string()],
        "one token drops every contract of that coin"
    );
    assert!(
        p.variants[1].coins_in.is_none(),
        "no whitelist → no restriction"
    );
    assert_eq!(p.bl, 1, "one coin reached, not two contracts");

    s.toggle(false, coin_token("ETH"));
    let p = s.plan(&universe);
    assert_eq!(p.variants[1].coins_in, Some(vec!["ETH".to_string()]));
    assert_eq!(p.wl, 1);
}

/// A token naming nothing that traded must not turn into an empty restriction that
/// scores every trade away.
/// A blacklist naming nothing that traded changes nothing — but a WHITELIST in the
/// same position keeps nothing, and must not collapse into "no whitelist" and score
/// the untouched fact as the plan.
#[test]
fn variants_handle_tokens_with_no_trades() {
    let mut s = CoinsState::default();
    s.toggle(true, "NOSUCHCOIN".into());
    let p = s.plan(&[coin("BTC_RP")]);
    assert_eq!(p.variants.len(), 2);
    assert!(p.variants[1].coins_out.is_empty());
    assert_eq!(p.bl, 0, "the label must not claim a coin it never reached");

    let mut s = CoinsState::default();
    s.toggle(false, "NOSUCHCOIN".into());
    let p = s.plan(&[coin("BTC_RP")]);
    assert_eq!(
        p.variants[1].coins_in,
        Some(Vec::new()),
        "an active whitelist that matched nothing keeps nothing"
    );
    assert!(!p.variants[1].is_empty(), "and is not the fact");
}

/// An edit made WHILE the read was in flight must survive the reply — otherwise the
/// click is silently rolled back by a request that started before it.
#[test]
fn adopt_keeps_an_edit_made_mid_flight() {
    let mut s = CoinsState::default();
    s.toggle(true, coin_token("ETH"));
    s.adopt(CoinLists::parse("BTC", ""));
    assert!(
        s.work.black.contains(&coin_token("ETH")),
        "the click survives"
    );
    assert!(
        s.saved.black.contains(&coin_token("BTC")),
        "baseline still lands"
    );
    assert!(s.has_changes());
}

/// "Changed" is the diff against what the strategies hold, so adopting a list must
/// leave nothing changed, and reverting must undo an edit exactly.
#[test]
fn changed_tracks_the_edit_not_the_saved_list() {
    let mut s = CoinsState::default();
    s.adopt(CoinLists::parse("BTC", ""));
    assert!(!s.has_changes(), "just loaded — nothing edited yet");
    assert!(!s.is_changed(&coin_token("BTC")), "saved is not 'changed'");
    assert!(!s.filter_available(CoinFilter::Changed));

    s.toggle(true, coin_token("ETH"));
    assert!(s.has_changes());
    assert!(s.is_changed(&coin_token("ETH")));
    assert!(!s.is_changed(&coin_token("BTC")));
    assert!(s.filter_available(CoinFilter::Changed));

    s.revert();
    assert!(!s.has_changes(), "revert returns to the saved lists");
    assert!(s.work.black.contains(&coin_token("BTC")));
}

/// A scope change must retire the generations, or a reply already in flight for the
/// PREVIOUS selection passes its sequence check, publishes that selection's lists, and
/// clears `dirty` — pinning the panel to foreign numbers permanently.
#[test]
fn invalidate_retires_in_flight_requests() {
    let mut s = CoinsState::default();
    s.adopt(CoinLists::parse("BTC", "ETH"));
    let (seq, kpi) = (s.seq, s.kpi_seq);
    s.dirty = false;
    s.invalidate();
    assert_ne!(s.seq, seq, "the table request generation must advance");
    assert_ne!(s.kpi_seq, kpi, "the KPI request generation must advance");
    assert!(s.needs_reload());
    assert!(s.work.is_empty() && s.saved.is_empty(), "lists retired too");
}

/// `coins/state.rs:CoinsState::mark_report_stale` must not add list resets or request generation
/// bumps from `invalidate`; the former erases unsaved edits and the latter starves long scans
/// while trades keep arriving.
#[test]
fn report_staleness_preserves_working_lists_and_picks() {
    let mut s = CoinsState::default();
    s.adopt(CoinLists::parse("BTC", ""));
    s.toggle(true, coin_token("ETH"));
    s.pick_coin("SOL".into(), false);
    let (seq, kpi, picked) = (s.seq, s.kpi_seq, s.picked_seq);

    s.mark_report_stale();

    assert!(s.has_changes(), "the Revert affordance must remain active");
    assert!(
        s.is_changed(&coin_token("ETH")),
        "the Changed view must retain the pending ETH edit"
    );
    s.pick_coin("SOL".into(), false);
    assert!(
        s.picked.is_empty(),
        "clicking the preserved sole pick must clear it"
    );
    assert_eq!(s.seq, seq);
    assert_eq!(s.kpi_seq, kpi);
    assert_eq!(s.picked_seq, picked);
    assert!(s.needs_reload());
}

/// `coins/state.rs:CoinPlan::build_rebased` must replay the request-start draft onto the fresh
/// saved baseline; replacing its rebased work with `fresh_saved` displays KPI for a different
/// list than the checked rows.
#[test]
fn refreshed_baseline_plan_keeps_the_request_start_draft() {
    let old_saved = CoinLists::parse("BTC", "");
    let old_work = CoinLists::parse("BTC, ETH", "");
    let fresh_saved = CoinLists::parse("BTC, SOL", "");
    let rebased = fresh_saved.duplicate().with_delta(&old_saved, &old_work);
    let plan = super::CoinPlan::build_rebased(
        &fresh_saved,
        &old_saved,
        &old_work,
        &[coin("BTC"), coin("ETH"), coin("SOL"), coin("DOGE")],
    );

    assert!(rebased.black.contains("ETH"));
    assert!(rebased.black.contains("SOL"));
    assert_eq!(
        plan.variants[1].coins_out,
        vec!["BTC".to_string(), "ETH".to_string(), "SOL".to_string()]
    );
}

/// Selecting nothing leaves no list to filter by; staying on the blacklist view would
/// render "no matches" for what is really "not known here".
#[test]
fn filter_falls_back_without_a_list() {
    let mut s = CoinsState {
        filter: CoinFilter::Black,
        ..Default::default()
    };
    s.adopt(CoinLists::parse("BTC", ""));
    s.settle_filter();
    assert!(s.filter == CoinFilter::Black, "a real list keeps the view");
    s.adopt(CoinLists::default());
    s.settle_filter();
    assert!(s.filter == CoinFilter::All, "no list falls back to All");
}

/// The click rule the strategy list already teaches, mirrored: plain click picks one and
/// un-picks it on a repeat, Ctrl accumulates. Getting this wrong makes the panel feel
/// like a different program from the list directly above it.
#[test]
fn picking_a_coin_follows_the_list_convention() {
    let mut s = CoinsState::default();
    s.pick_coin("BTC".into(), false);
    assert_eq!(s.picked.len(), 1);

    s.pick_coin("BTC".into(), false);
    assert!(s.picked.is_empty(), "clicking the sole pick clears it");

    s.pick_coin("BTC".into(), false);
    s.pick_coin("ETH".into(), true);
    assert_eq!(s.picked.len(), 2, "Ctrl accumulates");
    s.pick_coin("ETH".into(), true);
    assert_eq!(s.picked.len(), 1, "and Ctrl removes again");

    // A plain click after a multi-select collapses to just that coin.
    s.pick_coin("SOL".into(), true);
    s.pick_coin("DOGE".into(), false);
    assert_eq!(s.picked.len(), 1);
    assert!(s.picked.contains("DOGE"));
}

/// The share is scope-independent; what it COMES TO is not. Rounded up, so a non-zero
/// share always excludes something instead of silently doing nothing.
#[test]
fn trades_floor_is_a_share_of_the_busiest() {
    let mut s = CoinsState::default();
    assert_eq!(s.trades_floor(400), 0, "0% keeps everything");
    s.min_trades_pct = 10;
    assert_eq!(s.trades_floor(400), 40);
    assert_eq!(s.trades_floor(0), 0, "no coins, no threshold");
    s.min_trades_pct = 1;
    assert_eq!(s.trades_floor(40), 1, "rounds up, never down to a no-op");
}

/// A reload landing on top of an edit must REPLAY it, not union it in.
///
/// The union this replaced kept additions but resurrected every removal, because a
/// removal is the absence of a coin and a union cannot see one. An untick that arrived
/// while a reload was in flight came back ticked — and the next Save wrote it to the
/// strategy, undoing the user's deletion on the write path.
#[test]
fn a_reload_replays_the_edit_instead_of_unioning_it() {
    let mut s = CoinsState::default();
    s.adopt(CoinLists::parse("BTC, ETH", ""));
    s.toggle(true, coin_token("ETH")); // untick: remove ETH
    s.toggle(true, coin_token("SOL")); // tick: add SOL

    // The same baseline comes back from a reload that was already in flight.
    s.adopt(CoinLists::parse("BTC, ETH", ""));
    assert!(!s.work.black.contains("ETH"), "the removal must survive");
    assert!(s.work.black.contains("SOL"), "the addition must survive");
    assert!(
        s.work.black.contains("BTC"),
        "untouched coins come from the baseline"
    );

    // A coin the CORE added meanwhile is adopted — nothing in the edit spoke about it.
    s.adopt(CoinLists::parse("BTC, ETH, DOGE", ""));
    assert!(
        s.work.black.contains("DOGE"),
        "a new baseline coin is adopted"
    );
    assert!(!s.work.black.contains("ETH"), "the removal still holds");
}

/// The whitelist is edited by its own tick column and written by the same Save, so it
/// gets the identical treatment — a removal there is just as lossy.
#[test]
fn the_replay_covers_the_whitelist_too() {
    let mut s = CoinsState::default();
    s.adopt(CoinLists::parse("", "BTC, ETH"));
    s.toggle(false, coin_token("ETH"));
    s.adopt(CoinLists::parse("", "BTC, ETH"));
    assert!(!s.work.white.contains("ETH"));
    assert!(s.work.white.contains("BTC"));
}

/// After a scope change there is no edit to replay: `invalidate` empties both sets, so
/// the incoming baseline is adopted whole and the previous selection's picks cannot leak.
#[test]
fn a_scope_change_adopts_the_new_baseline_whole() {
    let mut s = CoinsState::default();
    s.adopt(CoinLists::parse("BTC, ETH", ""));
    s.toggle(true, coin_token("ETH"));
    s.invalidate();
    s.adopt(CoinLists::parse("SOL, DOGE", ""));
    let mut got: Vec<&str> = s.work.black.iter().map(String::as_str).collect();
    got.sort_unstable();
    assert_eq!(
        got,
        vec!["DOGE", "SOL"],
        "no trace of the previous scope's edit"
    );
    assert!(
        !s.has_changes(),
        "a freshly adopted baseline is not an edit"
    );
}
