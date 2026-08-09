//! Execution-time workspace guards for strategy clipboard and drag actions.

use super::action_cores_visible;

/// `tree/dnd.rs:action_cores_visible` accepting a hidden source or target would allow a stale
/// Paste/Drop callback to create or move strategies on a core hidden by an Auto workspace switch.
#[test]
fn stale_drag_or_paste_cannot_act_on_hidden_cores() {
    assert!(action_cores_visible(Some(&[22]), [22]));
    assert!(!action_cores_visible(Some(&[22]), [11]));
    assert!(!action_cores_visible(Some(&[22]), [11, 22]));
    assert!(
        action_cores_visible(None, [11, 22]),
        "Classic keeps cross-core drag behavior"
    );
}
