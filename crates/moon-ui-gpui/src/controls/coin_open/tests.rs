//! The one thing about this module that a type cannot state.

/// The completion callback is handed back DEFERRED, and only when the open was accepted.
///
/// `open` is called from a click listener, so the caller's entity is leased for the duration of
/// that call. A callback invoked synchronously cannot touch it: GPUI panics with "cannot update …
/// while it is already being updated" (`moon-gpui/src/app/entity_map.rs`), which is what a crowd
/// detect card did on 09.09.2026 when its coin resolved to exactly one core — the picker path
/// survived only because its callback arrives from a later click of its own.
///
/// The guard is pinned with it, because the two failures are opposite and both silent: call it
/// synchronously and the terminal dies; call it outside `if done` and a coin no core trades has its
/// card dismissed by a click that did nothing.
///
/// Mutation: call `opened(app)` directly at either site, or hoist either call out of its guard.
/// Neither shows up in a type, a flag test, or a reading of the diff — the first lives in the
/// distance between two stack frames, the second in a card that vanishes for no reason.
#[test]
fn the_completion_callback_is_deferred_and_guarded() {
    let src = include_str!("../coin_open.rs");
    let body = src
        .split("pub(crate) fn open(")
        .nth(1)
        .expect("the opener must exist");

    // Two handovers, one per arm, and each of them immediately inside its own `if done`. The file
    // holds four `if done {` in all — the inner pair guards `notify` inside the backend update — so
    // the guard is checked at each handover rather than by counting them.
    const HANDOVER: &str = "app.defer(move |app| opened(app));";
    let mut handovers = 0;
    let mut rest = body;
    while let Some(at) = rest.find(HANDOVER) {
        // The nearest thing before the handover must be its own guard, not a closing brace:
        // measured in braces rather than in characters, so an indentation change cannot fake it.
        let before = &rest[..at];
        assert!(
            before.rfind("if done {") > before.rfind('}'),
            "a handover left its `if done` guard, so a click that opened nothing reports success"
        );
        handovers += 1;
        rest = &rest[at + HANDOVER.len()..];
    }
    assert_eq!(
        handovers, 2,
        "both the direct open and the picker must hand the callback back deferred"
    );
    assert!(
        !body.contains(
            "    opened(app);
"
        ),
        "a synchronous callback reaches for an entity this call still holds leased"
    );
}
