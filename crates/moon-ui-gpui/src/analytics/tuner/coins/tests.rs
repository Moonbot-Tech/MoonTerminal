use super::{CoinsBody, coins_body, no_match_text, plural_key};

/// Without a strategy the table asks for one, whatever the reads hold — it never falls back
/// to listing every coin of every strategy.
#[test]
fn no_strategy_shows_the_empty_state() {
    assert_eq!(coins_body(false, true, 40), CoinsBody::NoStrategy);
    assert_eq!(coins_body(false, false, 0), CoinsBody::NoStrategy);
    assert_eq!(coins_body(true, false, 0), CoinsBody::Note);
    assert_eq!(coins_body(true, true, 0), CoinsBody::NoMatch);
    assert_eq!(coins_body(true, true, 3), CoinsBody::Rows);
}

/// An empty table caused by the trade threshold names that rule; otherwise it is a plain
/// "nothing matched".
#[test]
fn an_all_hidden_table_names_the_hide_rule() {
    let _locale = crate::test_locale::force("en");
    let hidden = no_match_text(7);
    assert!(
        hidden.contains('7') && !hidden.contains("analytics."),
        "{hidden}"
    );
    assert_ne!(hidden, no_match_text(0));
}

/// Count-dependent messages pick the form by the count, Slavic three-way.
#[test]
fn count_messages_pick_their_plural_form() {
    let _locale = crate::test_locale::force("ru");
    let base = "analytics.coins.all_hidden";
    for (n, form) in [
        (1, "one"),
        (3, "few"),
        (5, "many"),
        (11, "many"),
        (21, "one"),
        (24, "few"),
    ] {
        assert_eq!(plural_key(base, n), format!("{base}_{form}"), "n={n}");
    }
    assert!(no_match_text(1).contains("монета"), "{}", no_match_text(1));
    assert!(no_match_text(3).contains("монеты"));
    assert!(no_match_text(5).contains("монет "));
    drop(_locale);
    let _locale = crate::test_locale::force("en");
    assert_eq!(plural_key(base, 1), format!("{base}_one"));
    assert_eq!(plural_key(base, 2), format!("{base}_many"));
    assert!(no_match_text(1).contains("1 coin is"));
}

/// The Auto heading says what a finished pick found instead of the dash that reads as
/// "never ran"; before a pick it keeps the dash.
#[test]
fn the_auto_heading_names_a_finished_empty_verdict() {
    use super::auto_idle_sub;
    use super::state::PickState;
    use moon_core::db::metrics::Tally;
    use moon_core::db::tuner::coin_pick::{CoinPick, NoiseReason, PickVerdict};
    let _locale = crate::test_locale::force("en");
    let done = |verdict| PickState::Done {
        excluded: Default::default(),
        adopted: false,
        awaiting_save: false,
        pick: std::sync::Arc::new(CoinPick {
            black: Vec::new(),
            white: Vec::new(),
            train_n: 70,
            holdout_n: 30,
            fact_holdout: Tally::default(),
            manual_holdout: Tally::default(),
            pick_holdout: Tally::default(),
            verdict,
        }),
        lists_rev: 0,
        report_gen: 0,
        holdout_pct: 30,
    };
    assert_eq!(auto_idle_sub(&PickState::Idle), "—");
    let nothing = auto_idle_sub(&done(PickVerdict::NothingFound));
    let few = auto_idle_sub(&done(PickVerdict::TooFewTrades));
    for word in [&nothing, &few] {
        assert!(word != "—" && !word.contains("analytics."), "{word}");
    }
    assert_ne!(nothing, few);
    assert_eq!(
        auto_idle_sub(&done(PickVerdict::Noise(NoiseReason::FewHoldout))),
        few
    );
}
