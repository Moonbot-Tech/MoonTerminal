use super::*;

/// Directory guesses must not replace the core's exchange identity or display caption.
#[test]
fn venue_caps_unlisted_source_keeps_identity_without_a_directory_venue() {
    let source = source(42);
    let address = source.replay_address(42).expect("fixture is addressable");
    assert_eq!(address.venue, None);
    assert_eq!(address.exchange_key, EXCHANGE_KEY);
    assert_eq!(crate::venue::venue(CODE), None);
    let identity = crate::venue::CoreVenue::identify(CODE, "", Some(CAPTION));
    assert_eq!(identity.reported, "EX-XX");
}
