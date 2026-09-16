//! Missing-sound notice identity follows the catalog's shared matching key.

use super::{MissingSound, State, push};

/// Bypassing the shared key emits duplicate toasts for one missing file.
#[test]
fn sound_folder_aliases_share_one_notice() {
    let mut state = State::default();
    push(&mut state, MissingSound::Name("sounds/hook".into()));
    push(&mut state, MissingSound::Name("Hook.wav".into()));
    assert_eq!(state.queue.len(), 1);
    assert_eq!(
        state.queue.front(),
        Some(&MissingSound::Name("sounds/hook".into()))
    );
}
