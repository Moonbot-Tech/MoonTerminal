//! What a MoonBot key can and cannot say about the transport mode.

use super::{TransportVersion, seeded_transport, transport_from_key};

/// A key is the seed for the mode, not a requirement for having one: an empty field and a
/// mistyped key must both come back as "nothing to seed" rather than as `V0`, or every core
/// without a readable key would be pinned to V0 behind the user's back.
#[test]
fn an_unreadable_key_names_no_mode() {
    for key in ["", "   ", "not base64!", "aGVsbG8="] {
        assert_eq!(
            transport_from_key(key),
            None,
            "key {key:?} carries no MoonProto network block"
        );
    }
}

/// The labels are MoonBot's, shown verbatim in the dropdown and in its own Moon Proto radio.
/// Renaming them here would make the two screens disagree about which mode a core is on.
#[test]
fn the_labels_match_moonbots_radio() {
    assert_eq!(
        TransportVersion::ALL.map(TransportVersion::label),
        ["V0", "V1", "V2"]
    );
}

/// The mapping into MoonProto's own type is what actually selects the wire format; swapping two
/// arms produces a connection that fails silently, with no local error to read.
#[test]
fn each_mode_maps_to_its_moonproto_counterpart() {
    for (ours, theirs) in [
        (TransportVersion::V0, moonproto::TransportMode::V0),
        (TransportVersion::V1, moonproto::TransportMode::V1),
        (TransportVersion::V2, moonproto::TransportMode::V2),
    ] {
        assert_eq!(moonproto::TransportMode::from(ours), theirs);
        assert_eq!(TransportVersion::from(theirs), ours);
    }
}

/// A mode already set is the user's and must survive re-reading the key. The scenario the field
/// exists for is a core whose switch moved WITHOUT a new key, so the key still claims the old
/// mode; letting it speak again would undo the fix, and the key field emits a change per
/// keystroke, so it would speak on every one of them.
#[test]
fn a_set_mode_is_never_reseeded_from_the_key() {
    for key in ["", "not-a-key", "whatever-the-key-says"] {
        assert_eq!(
            seeded_transport(Some(TransportVersion::V1), key),
            Some(TransportVersion::V1),
            "key {key:?} must not overwrite a mode the terminal already has"
        );
    }
}

/// The other half: while nothing is set, the key is the only thing that can answer -- that is how
/// a core being added, and an older config on its first load, get a mode at all.
#[test]
fn an_unset_mode_is_taken_from_the_key() {
    assert_eq!(
        seeded_transport(None, "not-a-key"),
        transport_from_key("not-a-key"),
        "with nothing stored the key decides, whatever it says"
    );
    assert_eq!(
        seeded_transport(None, ""),
        None,
        "an empty key names nothing"
    );
}
