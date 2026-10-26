use super::{
    CoinFilter, CoinLists, CoinsState, PickCfg, PickStaleCause, PickState, SuggestedLists,
    coin_token, parse_min_trades,
};
use moon_core::config::CoinsAxisLayout;
use moon_core::config::TableSortPreference;
use moon_core::db::ReadFail;
use moon_core::db::analytics::GroupStat;
use moon_core::db::coin_lists::{CoinListEntries, CoinListRow, CoinListRows};
use moon_core::db::metrics::Tally;
use moon_core::db::tuner::VarStats;
use moon_core::db::tuner::coin_pick::{CoinPick, CoinSuggestion, PickVerdict};

/// Removing exclusion filtering would put a coin the user crossed out into a saved list.
#[test]
fn exclusions_toggle_per_list_and_expire_with_the_pick() {
    let mut s = CoinsState::default();
    let mut pick = pick_of(&["XALPHA", "XBETA"]);
    pick.white = pick.black.clone();
    let seq = s.begin_pick();
    s.land_pick(seq, s.lists_rev, s.report_gen, 30, Ok(pick));
    s.toggle_exclude(true, "XALPHA".into());
    assert!(s.excluded(true, "XALPHA"));
    assert!(!s.excluded(false, "XALPHA"));
    s.toggle_exclude(true, "XALPHA".into());
    assert!(!s.excluded(true, "XALPHA"));
    s.toggle_exclude(true, "XALPHA".into());
    s.toggle_exclude(false, "XBETA".into());
    s.adopt_pick();
    assert_eq!(s.work.black, ["XBETA".to_string()].into());
    assert_eq!(s.work.white, ["XALPHA".to_string()].into());
    s.begin_pick();
    assert!(!s.excluded(true, "XALPHA"));
    assert!(!s.excluded(false, "XBETA"));
}

/// All excluded white suggestions must not turn a restrictive saved whitelist into an empty one.
#[test]
fn excluding_every_white_coin_preserves_the_working_whitelist() {
    let mut s = CoinsState::default();
    s.adopt(CoinLists::parse("", "XORIGINAL"));
    let mut pick = pick_of(&[]);
    pick.white = pick_of(&["XBETA"]).black;
    let seq = s.begin_pick();
    s.land_pick(seq, s.lists_rev, s.report_gen, 30, Ok(pick));
    s.toggle_exclude(false, "XBETA".into());
    s.adopt_pick();
    assert_eq!(s.work.white, ["XORIGINAL".to_string()].into());
    assert!(!s.has_changes());
}

/// Reverting a table tick must preserve an unapplied verdict, while reverting an adoption retires it.
#[test]
fn revert_preserves_unadopted_results_but_retires_adopted_results() {
    let mut s = CoinsState::default();
    let seq = s.begin_pick();
    s.land_pick(seq, s.lists_rev, s.report_gen, 30, Ok(pick_of(&["XBETA"])));
    s.toggle(true, "XMANUAL".into());
    s.revert();
    assert!(matches!(s.pick, PickState::Done { adopted: false, .. }));
    assert_eq!(s.suggested().unwrap().black, vec!["XBETA"]);
    assert!(!s.has_changes());
    s.adopt_pick();
    s.revert();
    assert_eq!(s.pick, PickState::Idle);
    assert!(!s.has_changes());
}

/// A no-op add needs visible feedback; only resolving a real edit should return to settings.
#[test]
fn no_op_adoption_survives_until_a_real_edit_is_saved() {
    let mut s = CoinsState::default();
    s.adopt(CoinLists::parse("XBETA", ""));
    let seq = s.begin_pick();
    s.land_pick(seq, s.lists_rev, s.report_gen, 30, Ok(pick_of(&["XBETA"])));
    s.adopt_pick();
    assert!(!s.retire_saved_pick());
    assert!(matches!(s.pick, PickState::Done { adopted: true, .. }));
    s.add_tokens(true, "XMANUAL");
    assert!(!s.retire_saved_pick(), "pending edits must remain visible");
    s.adopt(s.work.duplicate());
    assert!(s.retire_saved_pick());
    assert_eq!(s.pick, PickState::Idle);
}

/// The header counts changed coins once even when both their list memberships change.
#[test]
fn changed_badge_counts_unique_coins_and_revert_clears_the_card_edit() {
    let mut s = CoinsState::default();
    s.adopt(CoinLists::parse("XALPHA", ""));
    s.toggle(true, "XALPHA".into());
    s.toggle(false, "XALPHA".into());
    s.toggle(true, "XBETA".into());
    assert_eq!(s.changed_count(), 2);
    s.revert();
    assert_eq!(s.changed_count(), 0);
    assert_eq!(s.work.black, ["XALPHA".to_string()].into());
    assert!(s.work.white.is_empty());
}

/// A backend-free host for the same window-bound action used by the card.
struct PickActionProbe {
    state: std::rc::Rc<std::cell::RefCell<CoinsState>>,
    requested: std::rc::Rc<std::cell::Cell<bool>>,
}

impl gpui::Render for PickActionProbe {
    /// Use the real card and adoption; the injected dialog callback records its request.
    fn render(
        &mut self,
        _: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use std::rc::Rc;
        let add = {
            let state = self.state.clone();
            Rc::new(move |_: &mut gpui::Window, _: &mut gpui::App| state.borrow_mut().adopt_pick())
        };
        let save = {
            let state = self.state.clone();
            let requested = self.requested.clone();
            Rc::new(move |_: &mut gpui::Window, _: &mut gpui::App| {
                assert!(state.borrow().work.black.contains("XBETA"));
                assert!(state.borrow().has_changes());
                requested.set(true);
            })
        };
        let acts = super::super::auto::AutoCardActs {
            on_pick: Rc::new(|_, _| {}),
            on_reset: Rc::new(|_, _| {}),
            on_one_by_one: Rc::new(|_, _| {}),
            on_add: add,
            on_save: save,
            on_revert: Rc::new(|_, _| {}),
            on_exclude: Rc::new(|_, _, _, _| {}),
            on_black: Rc::new(|_, _, _| {}),
            on_white: Rc::new(|_, _, _| {}),
        };
        super::super::auto::auto_card_el(
            &super::super::auto::AutoCardModel::new(&self.state.borrow(), true, false, true),
            None,
            &acts,
            moon_ui::MoonPalette::active(cx),
            cx,
        )
    }
}

/// Swapping save and adoption would request an empty dialog; dropping save would restore the extra click.
#[gpui::test]
fn add_and_save_adopts_before_requesting_the_existing_dialog(cx: &mut gpui::TestAppContext) {
    use gpui::px;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    let _locale = crate::test_locale::force("en");
    cx.update(moon_ui::init);
    cx.update(|cx| {
        moon_ui::MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(
                moon_core::config::UiThemeMode::Light,
                1.0,
            ),
            cx,
        )
    });
    let state = Rc::new(RefCell::new(CoinsState::default()));
    let seq = state.borrow_mut().begin_pick();
    state
        .borrow_mut()
        .land_pick(seq, 0, 0, 30, Ok(pick_of(&["XBETA"])));
    let requested = Rc::new(Cell::new(false));
    let window = cx.open_window(gpui::size(px(470.0), px(600.0)), |_, _| PickActionProbe {
        state: state.clone(),
        requested: requested.clone(),
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    for _ in 0..3 {
        visual.update(|window, app| {
            let _ = window.draw(app);
        });
    }
    let button = visual
        .debug_bounds("an-pick-add-save")
        .expect("primary add and save");
    visual.simulate_click(button.center(), gpui::Modifiers::none());
    assert!(requested.get(), "existing save dialog callback requested");
    assert!(matches!(
        state.borrow().pick,
        PickState::Done { adopted: true, .. }
    ));
}

/// Toggling a mask-covered coin must neither add a redundant literal nor dirty either list.
#[test]
fn toggling_a_mask_covered_coin_leaves_both_lists_unchanged() {
    for black_side in [false, true] {
        let mut state = CoinsState::default();
        state.work.black.insert("XAL*".into());
        state.work.white.insert("XAL*".into());
        state.saved.black = state.work.black.clone();
        state.saved.white = state.work.white.clone();
        let rev = state.lists_rev;
        state.toggle(black_side, "XALPHA".into());
        state.toggle(black_side, "XALPHA".into());
        assert!(!state.has_changes());
        assert_eq!(state.lists_rev, rev);
        assert!(!state.work.black.contains("XALPHA"));
        assert!(!state.work.white.contains("XALPHA"));
    }
}

fn coin(name: &str) -> GroupStat {
    GroupStat {
        name: name.to_string(),
        ..Default::default()
    }
}

/// Manual and Auto must expand the same masks the fitter applies, including all contracts.
#[test]
fn manual_masks_expand_by_prefix_or_suffix() {
    let universe = [
        coin("YPERP"),
        coin("XPERP_RP"),
        coin("YBETA"),
        coin("XALPHA"),
    ];
    let mut state = CoinsState::default();
    state.adopt(CoinLists::parse("X*", "*PERP"));
    let plan = state.plan(&universe);
    assert_eq!(
        plan.variants[1].coins_in,
        Some(vec!["XPERP_RP".into(), "YPERP".into()])
    );
    assert_eq!(plan.variants[1].coins_out, vec!["XALPHA", "XPERP_RP"]);
    assert_eq!((plan.bl, plan.wl), (2, 2));
    state.suggestion = Some(SuggestedLists {
        black: vec!["YPERP".into()],
        white: Vec::new(),
    });
    let plan = state.plan(&universe);
    assert_eq!(plan.variants[2].coins_in, plan.variants[1].coins_in);
    assert_eq!(
        plan.variants[2].coins_out,
        vec!["XALPHA", "XPERP_RP", "YPERP"]
    );
    state.adopt(CoinLists::parse("X*", "*X*"));
    let plan = state.plan(&universe);
    assert_eq!(plan.variants[1].coins_in, Some(Vec::new()));
    assert_eq!(plan.variants[1].coins_out, vec!["XALPHA", "XPERP_RP"]);
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
        None,
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

/// A config without the block opens on the axis defaults; an out-of-range holdout is clamped
/// rather than handed to the pick.
#[test]
fn coin_settings_restore_defaults_and_clamp_the_holdout() {
    let fresh = CoinsState::load(None, None);
    assert_eq!(fresh.min_trades, 0);
    assert!(fresh.hide_zero);
    assert_eq!(
        fresh.pick_cfg,
        PickCfg {
            min_trades: 5,
            holdout_pct: 30,
            black: true,
            white: false,
        }
    );

    let saved = CoinsAxisLayout {
        min_trades: Some(7),
        hide_zero: Some(false),
        pick_holdout_pct: Some(95),
        pick_white: Some(true),
        ..CoinsAxisLayout::default()
    };
    let s = CoinsState::load(None, Some(&saved));
    assert_eq!(s.min_trades, 7);
    assert!(!s.hide_zero);
    assert_eq!(
        s.pick_cfg.holdout_pct, 50,
        "95 clamps to the top of 10..=50"
    );
    assert_eq!(
        s.pick_cfg.min_trades, 5,
        "a missing field keeps its default"
    );
    assert!(s.pick_cfg.black && s.pick_cfg.white);

    let back = CoinsState::load(None, Some(&s.to_layout()));
    assert_eq!(
        back.to_layout(),
        s.to_layout(),
        "what is written loads back the same"
    );
}

/// A paste in any separator adds upper-cased base tokens, and an adopt landing afterwards
/// replays the addition instead of dropping it.
#[test]
fn pasted_coins_are_added_and_survive_a_reload() {
    let mut s = CoinsState::default();
    s.adopt(CoinLists::parse("XOLD", ""));
    let rev = s.lists_rev;
    assert_eq!(s.add_tokens(true, "xaaa, XBBB_RP,,xccc"), 3);
    assert!(s.lists_rev != rev, "an edit bumps the list revision");
    for token in ["XAAA", "XBBB", "XCCC", "XOLD"] {
        assert!(s.work.black.contains(token), "{token} must be listed");
    }
    assert_eq!(s.add_tokens(true, "XAAA"), 0, "already listed adds nothing");

    s.adopt(CoinLists::parse("XOLD, XNEW", ""));
    for token in ["XAAA", "XBBB", "XCCC", "XOLD", "XNEW"] {
        assert!(
            s.work.black.contains(token),
            "{token} must survive the replay"
        );
    }
}

/// Clear empties the working list only; Revert restores the saved one.
#[test]
fn clear_then_revert_restores_saved() {
    let mut s = CoinsState::default();
    s.adopt(CoinLists::parse("XAAA, XBBB", "XCCC"));
    s.clear(true);
    assert!(s.work.black.is_empty());
    assert_eq!(s.work.white.len(), 1, "the other side is untouched");
    assert!(s.has_changes());
    s.revert();
    assert_eq!(s.work.black, s.saved.black);
    assert!(!s.has_changes());
}

/// The pending delta names each side's additions and removals, sorted.
#[test]
fn pending_delta_is_signed_per_side_and_sorted() {
    let mut s = CoinsState::default();
    s.adopt(CoinLists::parse("XAAA, XBBB", "XCCC"));
    s.add_tokens(true, "XZZZ, XDDD");
    s.remove_token(true, "XBBB");
    s.remove_token(false, "XCCC");
    s.add_tokens(false, "XEEE");
    let d = s.pending_delta();
    assert_eq!(d.added_bl, ["XDDD", "XZZZ"]);
    assert_eq!(d.removed_bl, ["XBBB"]);
    assert_eq!(d.added_wl, ["XEEE"]);
    assert_eq!(d.removed_wl, ["XCCC"]);
    s.restore_token(true, "XBBB");
    assert!(
        s.pending_delta().removed_bl.is_empty(),
        "undo takes the removal back"
    );
}

/// A scope change retires a running pick: the card goes Idle and its reply cannot land.
#[test]
fn invalidate_drops_a_running_pick() {
    let mut s = CoinsState::default();
    s.pick = PickState::Running { seq: s.pick_seq };
    let before = s.pick_seq;
    s.invalidate();
    assert_eq!(s.pick, PickState::Idle);
    assert!(
        s.pick_seq != before,
        "the in-flight reply's seq no longer matches"
    );
}

/// A material report append must let an in-flight pick land and retain its original baseline.
///
/// Mutation: `drop_pick` while the pick is `Running`. The report commits every few
/// seconds, so a pick slower than that interval would never land and the card would
/// fall back to the settings form — the same symptom as dropping a finished pick.
#[test]
fn a_report_refresh_lets_an_in_flight_pick_land_stale() {
    let mut s = CoinsState::default();
    let seq = s.begin_pick();
    let lists_rev = s.lists_rev;
    let report_gen = s.report_gen;
    let seq_before = s.pick_seq;
    s.mark_report_stale();
    assert_eq!(s.pick, PickState::Running { seq });
    assert_eq!(
        s.pick_seq, seq_before,
        "the reply must still be allowed to land"
    );
    assert!(s.land_pick(seq, lists_rev, report_gen, 30, Ok(pick_of(&["XAAA"]))));
    s.sample_snapshot = super::super::super::freshness::SampleSnapshot::from_count(100, 0);
    s.kpi.apply(Ok(vec![VarStats {
        n: 105,
        ..Default::default()
    }]));
    assert!(s.pick_stale(), "the reply scored the previous report");
    assert_eq!(s.adopt_suggestion(true), 1);
    assert!(s.work.black.contains("XAAA"));
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

/// `coins/state.rs:CoinsState::load` must restore a valid key and translate ascending to `desc`.
///
/// Mutation: copy `ascending` directly or skip the saved value. The coin table would reopen with
/// the wrong row direction/arrow, and the exact stored tuple assertion reddens.
#[test]
fn coin_sort_load_restores_valid_key_and_direction() {
    let state = CoinsState::load(
        Some(TableSortPreference {
            column: "analytics.col.profit".to_string(),
            ascending: true,
        }),
        None,
    );
    assert_eq!(
        state.sort,
        Some(("analytics.col.profit".to_string(), false))
    );
}

/// `coins/state.rs:CoinsState::load` must retain Trades-descending for absent or unknown keys.
///
/// Mutation: accept a retired string. The header could show no matching arrow while rows sort on a
/// key no longer rendered, and the second assertion reddens.
#[test]
fn coin_sort_load_keeps_historical_default_for_unknown_key() {
    let expected = CoinsState::default().sort;
    assert_eq!(CoinsState::load(None, None).sort, expected);
    assert_eq!(
        CoinsState::load(
            Some(TableSortPreference {
                column: "retired".to_string(),
                ascending: true,
            }),
            None
        )
        .sort,
        expected
    );
}

/// The "fewer trades than" input takes a whole number (blank = 0) and refuses anything else,
/// so the caller keeps the previous threshold.
#[test]
fn parse_min_trades_rejects_non_numbers_and_negatives() {
    assert_eq!(parse_min_trades("12"), Some(12));
    assert_eq!(parse_min_trades(" 3 "), Some(3));
    assert_eq!(parse_min_trades(""), Some(0));
    assert_eq!(parse_min_trades("-1"), None);
    assert_eq!(parse_min_trades("abc"), None);
    assert_eq!(parse_min_trades("2.5"), None);
}

fn pick_of(black: &[&str]) -> CoinPick {
    CoinPick {
        black: black
            .iter()
            .map(|t| CoinSuggestion {
                token: t.to_string(),
                n: 10,
                profit: -50.0,
            })
            .collect(),
        white: Vec::new(),
        train_n: 70,
        holdout_n: 30,
        fact_holdout: Tally::default(),
        manual_holdout: Tally::default(),
        pick_holdout: Tally::default(),
        verdict: PickVerdict::Useful,
    }
}

/// Startup and absent baselines never warn; an adopted batch warns only after material growth.
#[test]
fn startup_and_batch_picks_use_a_real_sample_baseline() {
    let mut s = CoinsState {
        report_end: 100,
        ..Default::default()
    };
    s.kpi.apply(Ok(vec![VarStats {
        n: 100,
        ..Default::default()
    }]));
    s.mark_report_stale();
    assert_eq!(s.pick_stale_cause(), None, "startup has no pick");

    s.adopt_batch_pick(std::sync::Arc::new(pick_of(&[])), 30);
    s.mark_report_stale();
    assert_eq!(
        s.pick_stale_cause(),
        None,
        "adoption must establish the 100-trade baseline"
    );
    s.kpi.apply(Ok(vec![VarStats {
        n: 101,
        ..Default::default()
    }]));
    assert_eq!(s.pick_stale_cause(), None, "one new trade is immaterial");
    s.kpi.apply(Ok(vec![VarStats {
        n: 105,
        ..Default::default()
    }]));
    assert_eq!(s.pick_stale_cause(), Some(PickStaleCause::Report));

    s.drop_pick();
    assert_eq!(s.pick_stale_cause(), None);
    s.pick = PickState::Done {
        excluded: Default::default(),
        adopted: false,
        awaiting_save: false,
        pick: std::sync::Arc::new(pick_of(&[])),
        lists_rev: s.lists_rev,
        report_gen: 0,
        holdout_pct: 30,
    };
    assert_eq!(
        s.pick_stale_cause(),
        None,
        "an absent baseline is not evidence of new trades"
    );
}

/// No pick: Fact and Manual only. A finished pick adds the Auto column, scored on the
/// working blacklist plus the suggested one.
#[test]
fn the_auto_variant_adds_the_suggestion_to_the_manual_lists() {
    let universe = [coin("XAAA"), coin("XBBB_RP"), coin("XCCC")];
    let work = CoinLists::parse("XAAA", "");
    let plan = super::CoinPlan::build(&work, None, &universe);
    assert_eq!(plan.variants.len(), 2);
    assert_eq!(plan.auto, None);

    let sug = SuggestedLists {
        black: vec!["XBBB".to_string()],
        white: Vec::new(),
    };
    let plan = super::CoinPlan::build(&work, Some(&sug), &universe);
    assert_eq!(plan.variants.len(), 3);
    assert_eq!(plan.variants[1].coins_out, ["XAAA"]);
    assert_eq!(plan.variants[2].coins_out, ["XAAA", "XBBB_RP"]);
    assert_eq!(plan.auto, Some((2, 0)));
}

/// A reply whose generation was retired (Stop, a new Pick) is dropped, not published.
#[test]
fn a_stale_pick_reply_is_dropped() {
    let mut s = CoinsState::default();
    let first = s.begin_pick();
    let second = s.begin_pick();
    assert!(!s.land_pick(first, s.lists_rev, s.report_gen, 30, Ok(pick_of(&["XAAA"]))));
    assert_eq!(s.pick, PickState::Running { seq: second });
    assert!(s.suggested().is_none());
    assert!(s.land_pick(
        second,
        s.lists_rev,
        s.report_gen,
        30,
        Ok(pick_of(&["XAAA"]))
    ));
    assert_eq!(
        s.suggested().map(|x| x.black.clone()),
        Some(vec!["XAAA".to_string()])
    );
}

/// A list edit while the pick runs: the reply still lands, but the pick is flagged stale.
/// Adopting the pick's own suggestion afterwards is not such an edit.
#[test]
fn a_list_edit_during_the_pick_marks_it_stale() {
    let mut s = CoinsState::default();
    let seq = s.begin_pick();
    let launched = s.lists_rev;
    s.add_tokens(true, "XZZZ");
    assert!(s.land_pick(seq, launched, s.report_gen, 30, Ok(pick_of(&["XAAA"]))));
    match &s.pick {
        PickState::Done { lists_rev, .. } => assert_ne!(*lists_rev, s.lists_rev),
        other => panic!("expected Done, got {other:?}"),
    }
    assert!(s.pick_stale());

    let mut fresh = CoinsState::default();
    let seq = fresh.begin_pick();
    let rev = fresh.lists_rev;
    assert!(fresh.land_pick(seq, rev, fresh.report_gen, 30, Ok(pick_of(&["XAAA"]))));
    assert_eq!(fresh.adopt_suggestion(true), 1);
    assert!(fresh.work.black.contains("XAAA"));
    assert!(
        !fresh.pick_stale(),
        "adopting the pick's own answer is no stale edit"
    );
}

/// A material report append must keep a finished pick and mark it stale against its baseline.
///
/// Mutation: call `drop_pick` for `PickState::Done`. The next report commit would clear the
/// verdict, the Suggested view and the table Auto column while the KPI Auto column kept that
/// pick's numbers, and Add to blacklist would have nothing left to apply. A scope change
/// still drops the pick, and the KPI column with it.
#[test]
fn a_report_refresh_keeps_a_finished_pick_and_marks_it_stale() {
    let mut s = CoinsState::default();
    let seq = s.begin_pick();
    let rev = s.lists_rev;
    assert!(s.land_pick(seq, rev, s.report_gen, 30, Ok(pick_of(&["XAAA"]))));
    s.sample_snapshot = super::super::super::freshness::SampleSnapshot::from_count(100, 0);
    let pick_rev = s.pick_rev;
    s.filter = CoinFilter::Suggested;
    s.kpi.apply(Ok(vec![
        VarStats {
            n: 105,
            ..VarStats::default()
        };
        3
    ]));
    s.kpi_auto = Some((2, 0));

    s.mark_report_stale();

    assert!(
        matches!(s.pick, PickState::Done { .. }),
        "the verdict must stay on screen"
    );
    assert!(s.pick_stale(), "the card must show the stale note");
    assert_eq!(
        s.pick_rev, pick_rev,
        "the table Auto column must keep its tags"
    );
    assert_eq!(
        s.suggested().map(|x| x.black.clone()),
        Some(vec!["XAAA".to_string()])
    );
    assert!(
        s.filter == CoinFilter::Suggested,
        "the Suggested view must stay reachable"
    );
    assert_eq!(
        s.kpi.data().map(|stats| stats.len()),
        Some(3),
        "the KPI Auto column must keep the same pick"
    );
    assert_eq!(s.kpi_auto, Some((2, 0)));
    assert_eq!(
        s.adopt_suggestion(true),
        1,
        "Add to blacklist still applies"
    );
    assert!(s.work.black.contains("XAAA"));
    assert!(
        s.pick_stale(),
        "adopting the suggestion does not make an old verdict current"
    );

    s.invalidate();
    assert_eq!(
        s.pick,
        PickState::Idle,
        "a scope change still drops the pick"
    );
    assert!(s.suggested().is_none());
    assert_eq!(s.kpi_auto, None);
    assert_eq!(
        s.kpi.data().map(|stats| stats.len()),
        Some(2),
        "dropping the pick must drop the KPI Auto column too"
    );
}

/// `coins/state.rs:CoinsState::pick_stale_cause` must name the input that moved.
///
/// Mutation: always return `Lists`, or return `Report` when both bindings moved.
/// A report refresh would tell the user they edited the lists, and a list edit made
/// while new trades also arrived would hide that edit behind the trades note.
#[test]
fn pick_stale_cause_names_the_input_that_moved() {
    let finished = || {
        let mut s = CoinsState::default();
        let seq = s.begin_pick();
        assert!(s.land_pick(seq, s.lists_rev, s.report_gen, 30, Ok(pick_of(&["XAAA"]))));
        s.sample_snapshot = super::super::super::freshness::SampleSnapshot::from_count(100, 0);
        s.kpi.apply(Ok(vec![VarStats {
            n: 105,
            ..Default::default()
        }]));
        s
    };

    let mut report = finished();
    report.mark_report_stale();
    assert_eq!(
        report.pick_stale_cause(),
        Some(PickStaleCause::Report),
        "a report refresh with the lists untouched names the report"
    );

    let mut lists = finished();
    lists.add_tokens(true, "XZZZ");
    assert_eq!(
        lists.pick_stale_cause(),
        Some(PickStaleCause::Lists),
        "a list edit with the report untouched names the lists"
    );

    let mut both = finished();
    both.add_tokens(true, "XZZZ");
    both.mark_report_stale();
    assert_eq!(
        both.pick_stale_cause(),
        Some(PickStaleCause::Lists),
        "when both moved, the user's own edit is the note"
    );
}

/// "Add to whitelist" applies what the Auto column scored: the suggested whitelist REPLACES
/// the working one, while the blacklist side still adds.
#[test]
fn adopting_a_suggested_whitelist_replaces_the_working_one() {
    let mut s = CoinsState::default();
    s.add_tokens(false, "XAAA,XBBB,XCCC");
    s.add_tokens(true, "XDDD");
    let seq = s.begin_pick();
    let mut pick = pick_of(&["XEEE"]);
    pick.white = vec![CoinSuggestion {
        token: "XAAA".to_string(),
        n: 40,
        profit: 120.0,
    }];
    assert!(s.land_pick(seq, s.lists_rev, s.report_gen, 30, Ok(pick)));
    assert_eq!(s.adopt_suggestion(false), 0, "XAAA was already listed");
    assert_eq!(s.work.white, ["XAAA".to_string()].into_iter().collect());
    assert_eq!(s.adopt_suggestion(true), 1);
    assert!(s.work.black.contains("XDDD") && s.work.black.contains("XEEE"));
    assert!(!s.pick_stale());
}

/// An empty answer is a finished pick, but it suggests nothing: no Auto variant.
#[test]
fn an_empty_pick_suggests_nothing() {
    let mut s = CoinsState::default();
    let seq = s.begin_pick();
    let rev = s.pick_rev;
    assert!(s.land_pick(seq, s.lists_rev, s.report_gen, 30, Ok(pick_of(&[]))));
    assert!(matches!(s.pick, PickState::Done { .. }));
    assert!(s.suggested().is_none());
    assert_eq!(s.pick_rev, rev);
}

/// The verdict names the holdout the pick RAN with, not the setting moved since.
#[test]
fn a_finished_pick_keeps_its_own_holdout() {
    let mut s = CoinsState::default();
    let seq = s.begin_pick();
    assert!(s.land_pick(seq, s.lists_rev, s.report_gen, 25, Ok(pick_of(&["XAAA"]))));
    s.pick_cfg.holdout_pct = 40;
    assert!(matches!(
        s.pick,
        PickState::Done {
            holdout_pct: 25,
            ..
        }
    ));
}

/// A pasted name longer than any coin is rejected, not drawn as a field-wide chip.
#[test]
fn an_overlong_token_is_rejected() {
    let mut s = CoinsState::default();
    let long = "X".repeat(super::MAX_TOKEN_LEN + 1);
    let ok = "Y".repeat(super::MAX_TOKEN_LEN);
    assert_eq!(s.add_tokens(true, &format!("{long},{ok}")), 1);
    assert!(s.work.black.contains(&ok));
}

/// The headings count only the listed coins the selected strategies' own trades hit: a list
/// built across every core names coins this strategy never traded, and counting them would
/// label the Manual column with a restriction its numbers were not scored under.
#[test]
fn the_headings_count_only_coins_this_strategy_traded() {
    let universe = [coin("XAAA"), coin("XBBB_RP"), coin("XCCC"), coin("XDDD")];
    let work = CoinLists::parse("XAAA, XBBB, XCCC", "");
    let sug = SuggestedLists {
        black: vec!["XDDD".to_string()],
        white: Vec::new(),
    };
    let plan = super::CoinPlan::build(&work, Some(&sug), &universe);
    assert_eq!(plan.bl, 3, "the scan still excludes every reached coin");
    let traded = |name: &str, n: i64| GroupStat {
        name: name.to_string(),
        n,
        ..Default::default()
    };
    let scoped = [traded("XBBB_RP", 4), traded("XCCC", 0), traded("XDDD", 2)];
    assert_eq!(plan.traded_reach(Some(&scoped)), (1, 0, Some((2, 0))));
    assert_eq!(
        plan.traded_reach(None),
        (3, 0, Some((4, 0))),
        "without the strategy's coins the universe-wide counts stand"
    );
}
