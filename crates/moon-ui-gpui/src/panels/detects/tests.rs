//! Regression tests for detection presentation scoping.

use super::{detect_expired, detection_core_visible, detection_route_visible};

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
        .filter(|core| detection_core_visible(*core, &[22]))
        .collect();
    assert_eq!(selected, vec![22]);
    assert_eq!(retained, vec![11, 22, 11]);
    assert!(
        retained
            .iter()
            .all(|core| detection_core_visible(*core, &[11, 22]))
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
    assert!(render.contains("detection_core_visible(item.core, &visible_cores)"));
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
    let src = include_str!("mod.rs");
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
    let sig = src
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
