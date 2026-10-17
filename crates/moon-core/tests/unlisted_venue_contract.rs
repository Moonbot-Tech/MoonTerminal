//! Source guards for early venue refusals that would hide an unlisted core's recorded tape.

use std::path::{Path, PathBuf};

/// Locate sibling crates so the binary-only tuner can participate in the contract.
fn crates_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned()
}

/// Normalize whitespace and line comments before checking small forbidden code patterns.
fn code(relative: &str) -> String {
    let path = crates_root().join(relative);
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    assert!(
        source.len() > 500,
        "{} is unexpectedly short",
        path.display()
    );
    source
        .lines()
        .map(|line| line.split("//").next().unwrap_or_default())
        .flat_map(str::chars)
        .filter(|ch| !ch.is_whitespace())
        .collect()
}

/// Reintroducing the optional-venue shortcut would turn an addressable tuner row into NoAddress.
#[test]
fn tuner_does_not_require_a_directory_venue() {
    let fetch = code("moon-ui-gpui/src/analytics/tuner/ticks/fetch.rs");
    assert!(
        !fetch.contains("address.venue?"),
        "the core address works without a REST venue"
    );
    assert!(
        fetch.contains("replay_tick_value("),
        "tuner must retain shared tick valuation"
    );
    let mini = code("moon-tg/src/mini_app/charts.rs");
    let window = code("moon-ui-gpui/src/trade_window/mod.rs");
    for (name, source) in [("Mini App", mini), ("trade window", window)] {
        assert!(
            source.contains(".replay_tick_value("),
            "{name} must share core-aware tick valuation"
        );
    }
}

/// Reintroducing UnknownVenue into replay code would restore a refusal before the core tick stage.
#[test]
fn replay_has_no_unknown_venue_empty_outcome() {
    let root = crates_root();
    let replay = root.join("moon-core/src/market/trade_replay");
    let mut pending = vec![root];
    let mut scanned = 0;
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "tests") {
                continue;
            }
            for entry in std::fs::read_dir(path).unwrap() {
                pending.push(entry.unwrap().path());
            }
        } else if path.extension().is_some_and(|extension| extension == "rs")
            && path.file_name().is_some_and(|name| name != "tests.rs")
        {
            let source = std::fs::read_to_string(&path).unwrap();
            scanned += 1;
            assert!(
                !source.contains("TradeReplayEmpty::UnknownVenue")
                    && (!path.starts_with(&replay) || !source.contains("UnknownVenue")),
                "{}: core ticks need no directory venue",
                path.display()
            );
        }
    }
    assert!(scanned > 0, "the source tree must not be empty");
}

/// A venue-none capture skip would discard the only durable history source for an unlisted core.
#[test]
fn close_capture_keeps_unlisted_exchanges() {
    let lifecycle = code("moon-core/src/session/lifecycle.rs");
    let capture = lifecycle
        .split("fncapture_closed_trade(")
        .nth(1)
        .expect("capture function")
        .split("fn")
        .next()
        .unwrap();
    for pattern in [
        "venue.is_none()",
        "address.venue?",
        "letSome(venue)=address.venueelse",
    ] {
        assert!(
            !capture.contains(pattern),
            "capture must preserve exchange identity without a directory route: {pattern}"
        );
    }
    assert!(
        capture.contains("worker::capture("),
        "capture scan must reach the actual enqueue"
    );
}

/// Returning directly from a missing-venue guard would strand held prints before stage_and_stamp.
#[test]
fn replay_unlisted_branch_reaches_the_tick_stage() {
    let serve = code("moon-core/src/market/trade_replay/worker/serve.rs");
    for pattern in [
        "request.address.venue?",
        "ifrequest.address.venue.is_none(){return",
        "letSome(venue)=request.address.venueelse{return",
    ] {
        assert!(
            !serve.contains(pattern),
            "unlisted requests must reach held/core ticks: {pattern}"
        );
    }
    assert!(
        serve.contains("stage_and_stamp(None,"),
        "unlisted tick stage must remain reachable"
    );
    // The catch-up retention-final branch is intentionally allowed; its edge is tested behaviorally.
}
