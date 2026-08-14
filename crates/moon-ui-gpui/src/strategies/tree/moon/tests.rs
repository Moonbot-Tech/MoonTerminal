//! Execution-time workspace guards for strategy-tree controls.

use moon_core::feed::ExchangeId;
use moon_core::venue::CoreVenue;

use super::id_exchange;

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

/// Build a venue with independently controlled identity and display metadata.
fn venue(code: u8, id_dex: u32, dex: &str, reported: &str) -> CoreVenue {
    CoreVenue {
        id: ExchangeId { code, dex: id_dex },
        dex: dex.to_string(),
        reported: reported.to_string(),
    }
}

/// `tree/moon.rs:id_exchange`: deriving the node ID from a reported caption would reset expansion
/// when only wire spelling changes and could merge two HIP-3 exchanges that share a caption.
#[test]
fn exchange_node_ids_follow_identity_and_have_one_unknown_value() {
    let first = venue(9, 17, "alpha", "First caption");
    let renamed = venue(9, 17, "beta", "Second caption");
    let other_code = venue(10, 17, "alpha", "First caption");
    let other_dex = venue(9, 18, "alpha", "First caption");

    assert_eq!(id_exchange(Some(&first)), id_exchange(Some(&renamed)));
    assert_ne!(id_exchange(Some(&first)), id_exchange(Some(&other_code)));
    assert_ne!(id_exchange(Some(&first)), id_exchange(Some(&other_dex)));
    assert_eq!(id_exchange(None), id_exchange(None));
    assert_eq!(id_exchange(None).as_ref(), "x:unknown");
}
