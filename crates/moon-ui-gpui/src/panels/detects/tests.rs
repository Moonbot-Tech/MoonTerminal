//! Regression tests for detection presentation scoping.

use super::cards::{self, strategy_chip_text};
use super::crowd::crowd_chip_text;
use super::rules::{
    crowd_card_yields, detect_expired, detection_core_visible, detection_route_visible,
    empty_feed_text,
};
use crate::workspace::scope_marker::ScopeMarker;
use moon_core::config::WorkspaceMode;
use rust_i18n::t;

/// Body of `DetectsPanel::ingest`, the subject of the source-shape assertions below. Its closing
/// brace is the first at method indentation.
fn ingest_body() -> &'static str {
    include_str!("mod.rs")
        .split("fn ingest(")
        .nth(1)
        .and_then(|tail| tail.split("\n    }").next())
        .expect("Detects ingest must exist")
}

/// `detects/mod.rs:ingest` filtering by the effective Auto core would advance every cursor while
/// dropping hidden cards, so returning to Overview could neither reveal nor replay those detects.
#[test]
fn presentation_scope_keeps_hidden_detection_cards_retained() {
    let retained = vec![11, 22, 11];

    let selected: Vec<u64> = retained
        .iter()
        .copied()
        .filter(|core| detection_core_visible(Some(*core), &[22]))
        .collect();
    assert_eq!(selected, vec![22]);
    assert_eq!(retained, vec![11, 22, 11]);
    assert!(
        retained
            .iter()
            .all(|core| detection_core_visible(Some(*core), &[11, 22]))
    );

    let src = include_str!("mod.rs");
    let ingest = ingest_body();
    assert!(ingest.contains(".filter(|s| s.group == self.group)"));
    assert!(!ingest.contains("effective_workspace_scope"));

    let render = src
        .split("impl Render for DetectsPanel")
        .nth(1)
        .expect("Detects render must exist");
    assert!(render.contains("effective_workspace_scope"));
    assert!(render.contains("detection_core_visible(item.core(), &visible_cores)"));
}

/// The AddToChart setting gates BOTH ends. Ingestion skips a chart-routed row while the setting is
/// off, so a disabled feed pays for none of the snapshot work; presentation repeats the rule, so
/// switching the setting off clears cards taken in while it was on instead of leaving them for the
/// rest of their `KeepAlert`.
///
/// Mutation: drop either gate. Dropping the ingest one makes the setting cost a market snapshot per
/// chart detect even when nobody wants the cards; dropping the presentation one leaves stale cards
/// on screen for up to a minute after the operator turns the setting off.
#[test]
fn add_to_chart_cards_are_gated_at_ingest_and_at_presentation() {
    // An ordinary detect never depends on the setting; a chart-routed one always does.
    assert!(detection_route_visible(0, false));
    assert!(detection_route_visible(0, true));
    assert!(!detection_route_visible(3, false));
    assert!(detection_route_visible(3, true));

    let src = include_str!("mod.rs");
    let ingest = ingest_body();
    assert!(ingest.contains("det.add_to_chart > 0 && !show_add_to_chart"));
    // The sound gate stays SEPARATE from the routing gate: a chart-routed detect is an ordinary
    // detect and still needs `SoundAlert` or an alert firing to earn a card.
    assert!(ingest.contains("if !det.sound_alert && !det.is_alert {"));

    let render = src
        .split("impl Render for DetectsPanel")
        .nth(1)
        .expect("Detects render must exist");
    assert!(render.contains("detection_route_visible(item.add_to_chart, cfg.show_add_to_chart)"));
}

/// The setting must work in BOTH directions. Ingestion drops chart-routed rows while it is off and
/// the cursor moves on regardless, so the pass that sees it come on replays each core's ring; the
/// signature carries the setting so every panel of the group takes that pass, not only the one
/// whose checkbox was clicked.
///
/// Mutation: delete the cursor reset, or seed the signature with `0`. Ticking the box would then
/// leave the feed unchanged until an unrelated detect happened to fire.
#[test]
fn turning_the_setting_on_replays_the_ring_for_every_panel_of_the_group() {
    let ingest = ingest_body();
    let reset = ingest
        .find("if show_add_to_chart {")
        .expect("the pass that sees the setting come on must be recognizable");
    let clear = ingest.find("self.last_seq.clear();").expect("ring replay");
    assert!(reset < clear, "the ring is replayed outside that pass");
    // Only the cursors are rewound. Dropping the retained cards too would destroy a long-lived one
    // whose row has already left its core's ring, and the replay could not rebuild it.
    assert!(
        !ingest.contains("self.items.clear();"),
        "the replay must not throw away cards it cannot rebuild"
    );
    // The falling edge drops what it hides, so invisible cards stop holding slots in the queue.
    assert!(ingest.contains("self.items.retain(|it| it.add_to_chart == 0);"));
    // A replay appends rows older than cards already held, so the queue is re-ordered by birth
    // before the trim; stable sorting keeps same-instant detects in ingest order.
    assert!(ingest.contains("if replayed {"));
    assert!(ingest.contains("sort_by(|a, b| a.born_ms.total_cmp(&b.born_ms))"));
    // Only the newest row per market survives collection: an older one would be overwritten in
    // place anyway, after paying for a market snapshot of its own.
    assert!(ingest.contains("if newest_of_market.insert(det.market.as_str()) {"));

    // Free function: its body ends at the first unindented brace. Scanning to end-of-file instead
    // would match the render gate and pass with no setting in the signature at all.
    let sig = include_str!("rules.rs")
        .split("fn detects_sig(")
        .nth(1)
        .and_then(|tail| tail.split("\n}").next())
        .expect("Detects signature must exist");
    assert!(sig.contains("shows_add_to_chart"));
    // The setting rides as its own value. Folded into the revision hash as `31 * flag + rev`, a
    // core advancing by exactly 31 in the same flush would cancel the flip out.
    assert!(sig.contains("(revs, b.detects_view.shows_add_to_chart(group))"));
}

/// A replayed ring must not pay for what it is about to throw away: a row already past its
/// `KeepAlert` is skipped BEFORE `detect_snapshot`, which reads the kline cache and a day of trades
/// per accepted row. Cursors are empty whenever a panel is built or the setting comes on, so the
/// whole per-core ring — thousands of rows — walks through this loop.
///
/// Mutation: move the check below the snapshot, or drop it. Opening the panel after hours of
/// uptime would then block the UI thread on one cache read per expired detect.
#[test]
fn expired_rows_are_dropped_before_paying_for_a_snapshot() {
    // One rule for the queue and for ingestion: a card is gone exactly when its KeepAlert is up.
    assert!(detect_expired(1_000.0, 0.0, 1_000.0));
    assert!(detect_expired(1_500.0, 0.0, 1_000.0));
    assert!(!detect_expired(999.0, 0.0, 1_000.0));
    // A core whose clock runs ahead of ours must not have its detects dropped on arrival.
    assert!(!detect_expired(0.0, 500.0, 1_000.0));

    let ingest = ingest_body();
    let expiry = ingest
        .find("detect_expired(now_ms, det.time_ms, ttl)")
        .expect("expired rows must be recognized");
    // A row already held at the same instant is a replay of a card that exists: it keeps the chart
    // frozen at detection time, and pays for no market read.
    let held = ingest
        .find("it.born_ms == det.time_ms")
        .expect("a row already held must be recognized");
    let snapshot = ingest
        .find("detect_snapshot(")
        .expect("snapshot capture must remain");
    assert!(held < snapshot, "a replayed card is re-frozen and re-read");
    assert!(
        expiry < snapshot,
        "an expired row still pays for a snapshot"
    );
}

/// Detect cards must validate Main/Compare authority before removing their retained card.
///
/// Mutation: move either `retain` call before its authorized request. A stale card click would
/// disappear and navigate to a core hidden by the current rail selection.
#[test]
fn stale_detect_navigation_is_rejected_before_card_removal() {
    let source = include_str!("mod.rs");
    for (method, authority) in [
        ("fn open(&mut self", "open_on_main_if_authorized"),
        ("fn open_compare(&mut self", "open_compare_if_authorized"),
    ] {
        let body = source
            .split(method)
            .nth(1)
            .expect("Detect navigation method must exist");
        let guard = body.find(authority).expect("workspace guard must exist");
        let removal = body.find("self.items").expect("card removal must remain");
        assert!(
            guard < removal,
            "{method} removes a stale card before authority"
        );
    }
}

/// The strategy chip names the firer, and an alert firing names ITSELF rather than leaving the
/// slot the user configured empty.
///
/// Mutation: make the empty arm return the alert label unconditionally. The third case reddens —
/// a detect whose card exists but whose strategy did not name it would then be mislabelled an
/// alert. `DetectRow.strat_name` already substitutes `strat <id>` for an unnamed strategy, so an
/// empty name reaching here means no strategy at all.
#[test]
fn strategy_chip_names_the_firer_or_the_alert() {
    let alert = "Алерт";
    assert_eq!(
        strategy_chip_text("EMA_scalp", false, alert),
        Some("EMA_scalp")
    );
    assert_eq!(strategy_chip_text("", true, alert), Some(alert));
    assert_eq!(strategy_chip_text("", false, alert), None);
    // A strategy named with blanks alone names nothing; an alert still says what it is.
    assert_eq!(strategy_chip_text("   ", false, alert), None);
    assert_eq!(strategy_chip_text("   ", true, alert), Some(alert));
    // Surrounding blanks never reach the card as leading indentation.
    assert_eq!(strategy_chip_text("  Waves  ", false, alert), Some("Waves"));
    // A named strategy is itself even on an alert firing: both can be true at once.
    assert_eq!(strategy_chip_text("Waves", true, alert), Some("Waves"));
}

/// Turning a card's chart off must hand its width to the text, and a lone edge must not reserve
/// space for a neighbour that is not there.
///
/// Mutation: subtract the chart zone unconditionally in `medium_col_name_w`. The first assertion
/// reddens — that is the version that cut names to a third of a card standing half empty.
#[test]
fn name_budget_follows_the_space_a_card_actually_has() {
    let mut cfg = moon_core::config::DetectSizeCfg {
        w: 210,
        chart: moon_core::config::DetectChart::Candles,
        ..Default::default()
    };
    let with_chart = cards::medium_col_name_w(&cfg);
    cfg.chart = moon_core::config::DetectChart::None;
    let without_chart = cards::medium_col_name_w(&cfg);
    assert!(
        without_chart > with_chart * 1.5,
        "a chart-less card must spend the freed zone on text: {without_chart} vs {with_chart}"
    );
    // Roughly five more characters at the caption step is what the chart costs the name.
    assert!(
        with_chart > 60.0,
        "a 210px card with a chart still owes the name a readable run, got {with_chart}"
    );

    // A cluster alone in its row keeps the whole width; one facing a neighbour splits it.
    assert_eq!(cards::side_name_w(100.0, false), 100.0);
    assert_eq!(cards::side_name_w(100.0, true), 50.0);
    // Never below the floor, however narrow the card is configured.
    assert!(cards::side_name_w(1.0, true) >= 24.0);
}

/// `detects/mod.rs:empty_feed_text` must check `available == 0` before `retained > 0`.
///
/// Mutation: swap the first two branches. A card retained across a disconnected core would claim a
/// preset hid it instead of explaining that no core in the group can currently detect.
#[test]
fn empty_feed_no_available_cores_outrank_retained_cards() {
    let hidden_marker = ScopeMarker::new(Some(WorkspaceMode::Classic), 0, 3);

    assert_eq!(
        empty_feed_text(&hidden_marker, 1, 0),
        t!("detects.empty_no_cores")
    );
}

/// `detects/mod.rs:empty_feed_text` must apply `scope_empty_text` only when cards are retained.
///
/// Mutation: wrap all branches in `scope_empty_text`. An empty feed that never received a detect
/// would incorrectly say that the preset hid every core instead of saying no detects have fired.
#[test]
fn empty_feed_all_hidden_preset_does_not_rewrite_an_unretained_feed() {
    let hidden_marker = ScopeMarker::new(Some(WorkspaceMode::Classic), 0, 3);

    assert_eq!(empty_feed_text(&hidden_marker, 0, 3), t!("detects.empty"));
}

/// `detects/mod.rs:empty_feed_text` must preserve the filtered sentence for a partial preset.
///
/// Mutation: replace the retained branch with `detects.empty`. A retained detect hidden by only
/// part of the preset would lose the explanation that the active scope is withholding it.
#[test]
fn empty_feed_partial_preset_with_retained_cards_reports_filtered_state() {
    let partial_marker = ScopeMarker::new(Some(WorkspaceMode::Classic), 1, 3);

    assert_eq!(
        empty_feed_text(&partial_marker, 1, 3),
        t!("detects.empty_filtered")
    );
}

/// A crowd detection has no core, so no display preset can address it — and one that hid it would
/// send the reader widening a preset that does not mention the crowd at all.
///
/// Mutation: make the rule fall through to `visible.contains`. Every crowd card then disappears the
/// moment a group scopes itself to one core, with nothing on screen saying why.
#[test]
fn a_crowd_card_belongs_to_every_scope() {
    assert!(detection_core_visible(None, &[]));
    assert!(detection_core_visible(None, &[11, 22]));
    // And a core card is still judged by the scope it belongs to.
    assert!(!detection_core_visible(Some(11), &[22]));
}

/// The card states the two figures the rule was actually read against, in the crowd feature's own
/// money format: always two decimals, so the card and the table it came from say the same number
/// the same way.
#[test]
fn a_crowd_card_states_what_fired_it() {
    assert_eq!(crowd_chip_text(999.5, 42), "+999.50 · 42");
    // EXACT past a thousand, where the tables compact. A card is the evidence for a threshold, and
    // "+1.20K" beside a line drawn at 1200 prints a figure that does not clear the line it crossed.
    assert_eq!(crowd_chip_text(1234.5, 7), "+1234.50 · 7");
    assert_eq!(crowd_chip_text(1204.0, 3), "+1204.00 · 3");
    // The sign is written out: the same chip position on another card can hold a loss.
    assert_eq!(crowd_chip_text(0.0, 10), "+0.00 · 10");
}

/// A crowd card lives the window it was computed from, until somebody says otherwise.
///
/// Mutation: raise the default and the feed keeps pointing at a minute that has gone; drop it to
/// zero and the card is pruned on the pass that ingests it, so the rule appears to do nothing.
#[test]
fn a_crowd_card_lives_one_window_by_default() {
    let cards = crate::chart_tabs::crowd_cards(&moon_core::config::layout::WindowLayout::default());
    assert_eq!(cards.keep_ms(), moon_core::crowd::minute::WINDOW_MS as f64);
    // And the feed follows the market rather than holding the first five it ever saw.
    assert!(cards.evict);

    let born = 1_000_000.0;
    assert!(!detect_expired(
        born + cards.keep_ms() - 1.0,
        born,
        cards.keep_ms()
    ));
    assert!(detect_expired(
        born + cards.keep_ms(),
        born,
        cards.keep_ms()
    ));
}

/// A coin a core has already reported is not announced a second time by the crowd.
///
/// The two land together more often than not — the crowd is trading it because something is
/// happening there, which is also why a strategy fired — and the core's card is the one with a
/// strategy, an exchange and a chart of its own behind it.
///
/// Mutation: drop the yield and a busy coin takes three seats in a 48-card feed, one per core plus
/// the crowd's, all saying the same thing.
#[test]
fn the_crowd_yields_a_coin_a_core_already_reported() {
    // What the cards CARRY: `MarketLabel::identity`, which is the core's own canonic where the
    // catalog has one. Bybit's `1kBONKPERP` and Binance's spelling of it both resolve to the same
    // string here, which is the whole reason the comparison is made on this field and not on a
    // fold of the name.
    let cored: std::collections::HashSet<&str> = ["BONKPERP", "AAVE"].into_iter().collect();

    assert!(crowd_card_yields("BONKPERP", &cored));
    assert!(crowd_card_yields("AAVE", &cored));

    // A different coin is a different coin, however close it looks on screen.
    assert!(!crowd_card_yields("NIULAI", &cored));
    // `1000SATS` carries a thousand that is part of its real ticker, not a multiplier; nothing here
    // is allowed to fold it into `SATS`, and the catalog is what keeps them apart.
    assert!(!crowd_card_yields(
        "SATS",
        &["1000SATS"].into_iter().collect()
    ));
    // Nothing on screen, nothing to yield to.
    assert!(!crowd_card_yields(
        "AAVE",
        &std::collections::HashSet::new()
    ));
}

/// The identity a card carries is the CROSS-EXCHANGE one, and it is not the same question as
/// "does this core's own coin list name it".
///
/// Mutation: build the card's `identity` from `match_key` instead. Everything still compiles and
/// every same-named coin still de-duplicates — but the multiplier spellings the field exists for
/// stop matching, which is exactly the case that is invisible without a catalog in front of you.
#[test]
fn the_card_carries_the_cross_exchange_key_and_not_the_local_one() {
    let src = include_str!("mod.rs");
    assert!(
        src.contains("identity: label.identity(),"),
        "a core card must freeze the catalog's cross-exchange identity"
    );
    assert!(
        src.contains(".map(|it| it.identity.as_str())"),
        "the duplicate rule must compare the identity the card froze"
    );
    let crowd = include_str!("crowd.rs");
    assert!(
        crowd.contains(
            ".market_label(hit.core, &hit.market)\n                            .identity()"
        ),
        "a crowd card must borrow its identity from the market it borrowed its chart from"
    );
}

/// A price move is drawn when there IS one, whoever the card came from.
///
/// The chart, the venue and the deltas come out of ONE snapshot of ONE market, so a card that draws
/// the picture and hides the numbers summarising it is inconsistent about a single set of facts.
/// The one pair that needs a gate is the deltas: `detect_snapshot` returns them at `0.0` when the
/// market has no retained history, and `delta_chip` renders that as a measured `0.00%`.
///
/// Mutation: gate on the card's ORIGIN again and every crowd card loses its deltas even when the
/// market behind it supplied them; gate on the market NAME and a borrowed market with no history
/// prints a flat day it measured from nothing.
#[test]
fn a_price_move_is_drawn_only_when_there_is_one() {
    use moon_core::config::DetectField;

    for field in [DetectField::Delta24h, DetectField::Delta1h] {
        assert!(
            super::crowd::field_applies(field, true),
            "{field:?} was hidden with history behind it"
        );
        assert!(
            !super::crowd::field_applies(field, false),
            "{field:?} was drawn from nothing"
        );
    }
    // Everything else answers for its own absence and is never gated here — the exchange chip
    // returns None without a venue, the badge without a strategy kind.
    for field in [
        DetectField::Coin,
        DetectField::Time,
        DetectField::Badge,
        DetectField::Core,
        DetectField::Exchange,
        DetectField::ExchangeKind,
        DetectField::Strategy,
    ] {
        assert!(
            super::crowd::field_applies(field, false),
            "{field:?} was gated on price history"
        );
    }
}
