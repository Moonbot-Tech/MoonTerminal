//! Launch-verifier tests.

use super::{constant_time_eq, hash, verify};

/// The basic contract: the password that was set gets in, anything else does not.
#[test]
fn the_password_that_was_set_is_the_one_that_opens_it() {
    let verifier = hash("1234").unwrap();
    assert!(verify(&verifier, "1234"));
    assert!(!verify(&verifier, "1235"));
    assert!(!verify(&verifier, ""));
    assert!(!verify(&verifier, "1234 "));
}

/// A short password is explicitly supported — the whole point of this latch is that the user may
/// choose four characters. A length rule sneaking in here would silently lock people out.
#[test]
fn a_four_character_password_is_accepted() {
    let verifier = hash("ab1!").unwrap();
    assert!(verify(&verifier, "ab1!"));
}

/// Two verifiers for the same password must differ, or the file would reveal that two users chose
/// the same one, and a precomputed table would cover both.
#[test]
fn the_same_password_hashes_differently_every_time() {
    assert_ne!(hash("1234").unwrap(), hash("1234").unwrap());
}

/// A damaged verifier must refuse entry rather than error out into a code path that treats "cannot
/// check" as "no password set". Every one of these is a way the string can arrive broken.
#[test]
fn a_malformed_verifier_refuses_entry() {
    for broken in [
        "",
        "argon2id",
        "argon2id$m=16384,t=2,p=1",
        "argon2id$m=16384,t=2,p=1$notbase64!!$AAAA",
        "argon2id$m=16384$AAAA$AAAA",
        "argon2id$m=16384,t=2,p=1$$",
        "scrypt$m=16384,t=2,p=1$AAAA$AAAA",
        // Costs above the ceiling must be refused, not attempted: this string is user-editable in
        // the sense that anyone who can write the file can write it.
        "argon2id$m=4294967295,t=2,p=1$AAAA$AAAA",
    ] {
        assert!(
            !verify(broken, "1234"),
            "accepted broken verifier {broken:?}"
        );
    }
}

/// The comparison must not depend on where the difference is.
#[test]
fn the_comparison_covers_length_and_content() {
    assert!(constant_time_eq(b"abc", b"abc"));
    assert!(!constant_time_eq(b"abc", b"abd"));
    assert!(!constant_time_eq(b"abc", b"ab"));
    assert!(!constant_time_eq(b"", b"a"));
    assert!(constant_time_eq(b"", b""));
}
