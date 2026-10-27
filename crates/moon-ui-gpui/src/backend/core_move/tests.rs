/// Same guard as `hotkeys::tests`: moon-core names the move's log target rooted at the BINARY and
/// cannot check it from its own side. A module move or a `[[bin]]` rename would mute every move
/// line under the `warn` baseline again while every gate stayed green.
#[test]
fn the_core_move_log_target_still_matches_this_module() {
    let target = moon_core::diagnostics::CORE_MOVE_TARGET;
    assert!(
        module_path!().starts_with(target),
        "the base filter raises {target:?}, but this module logs as {:?}",
        module_path!()
    );
}
