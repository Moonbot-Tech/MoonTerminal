//! Execution-time workspace guards for strategy-tree controls.

/// Compile-time source used to ensure the checkbox producer retains its action guard.
const SRC: &str = include_str!("../moon.rs");

/// Removing the visibility guard from `tree/moon.rs` checkbox `on_change` would let a stale
/// callback stage a hidden core after switching the owning Auto workspace.
#[test]
fn stale_checkbox_callback_cannot_stage_a_hidden_core() {
    let start = SRC
        .find(".on_change(move |ch: &bool")
        .expect("strategy checkbox handler must exist");
    let handler = &SRC[start..];
    let staged_write = handler
        .find("let before = this.staged")
        .expect("strategy checkbox must still stage an edit");
    let guard = handler
        .find("strategy_core_is_visible(this.workspace_cores.as_deref(), key.0)")
        .expect("strategy checkbox must validate the current workspace at dispatch");

    assert!(
        guard < staged_write,
        "the workspace guard must execute before any retained staging mutation"
    );
}
