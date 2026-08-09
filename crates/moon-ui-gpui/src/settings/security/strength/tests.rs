//! Unit tests for the provisional encryption-password estimator.
//!
//! Explicit imports, never `use super::*`: the Settings tree re-exports `gpui::*`, whose own `test`
//! shadows the built-in attribute and makes `#[test]` expand recursively (see CONTRIBUTING).

use super::{Issue, Level, MIN_BITS, MIN_LEN, estimate};

/// An empty field must read as "nothing typed", not as a rejected password: the meter and the
/// error line both key off this, and reporting a real complaint on an untouched field is how the
/// section ends up permanently red before the user has done anything.
#[test]
fn an_untouched_field_is_empty_rather_than_wrong() {
    let verdict = estimate("");
    assert_eq!(verdict.level, Level::Empty);
    assert_eq!(verdict.bits, 0.0);
    assert!(!verdict.accepted());
}

/// Length is the one property variety cannot substitute for. `aA1!aA1!` covers all four classes
/// and would pass a naive class-counting rule, which is exactly the rule this floor replaces.
#[test]
fn full_class_variety_does_not_buy_a_short_password_in() {
    let verdict = estimate("aA1!aA1!");
    assert!(verdict.bits > MIN_BITS, "the naive score would have passed");
    assert_eq!(verdict.issue, Some(Issue::TooShort));
    assert!(!verdict.accepted());
}

/// A repeated run is one guess, not one guess per character. Without the discount, `MIN_LEN`
/// identical characters score like a random string of the same length.
#[test]
fn a_repeated_run_is_not_paid_for_per_character() {
    let verdict = estimate("aaaaaaaaaaaaaaaa");
    assert!(!verdict.accepted());
    assert_eq!(verdict.issue, Some(Issue::Repeats));
    assert!(
        verdict.bits < MIN_BITS,
        "16 identical characters scored {} bits",
        verdict.bits
    );
}

/// Same argument for a walked alphabet or keypad run.
#[test]
fn an_ascending_run_is_not_paid_for_per_character() {
    let verdict = estimate("abcdefghijklmn");
    assert!(!verdict.accepted());
    assert_eq!(verdict.issue, Some(Issue::Sequence));
}

/// A known chunk is guessed whole, so decorating it must not rescue it. This is the case a pure
/// entropy formula always gets wrong: `Moonbot2026!` looks like 12 mixed-class characters.
#[test]
fn decoration_does_not_rescue_a_known_chunk() {
    let verdict = estimate("Moonbot2026!");
    assert!(verdict.bits < MIN_BITS, "scored {} bits", verdict.bits);
    assert_eq!(verdict.issue, Some(Issue::Common));
    assert!(!verdict.accepted());
    // Case must not be an escape hatch: the chunk list is matched case-insensitively.
    assert_eq!(estimate("MOONBOT2026!").issue, Some(Issue::Common));
}

/// The floor has to admit passwords real people can produce, or it teaches users to disable the
/// feature. A long mixed passphrase with no run and no known chunk must pass.
#[test]
fn an_ordinary_strong_passphrase_is_accepted() {
    for candidate in ["Vk7#pLmq2Zr!", "corner-lamp-Whisk3r", "Xq4!nvBz9Ttw"] {
        let verdict = estimate(candidate);
        assert!(
            verdict.accepted(),
            "{candidate} was rejected: {:?} at {} bits",
            verdict.issue,
            verdict.bits
        );
        assert!(matches!(verdict.level, Level::Good | Level::Strong));
    }
}

/// One class stays reportable even when nothing repeats, so the message can say what to change
/// instead of only that it is not good enough.
#[test]
fn a_long_single_class_password_names_its_class_problem() {
    let verdict = estimate("qhvnrtmklspd");
    assert!(!verdict.accepted());
    assert_eq!(verdict.issue, Some(Issue::OneClass));
}

/// Appending a genuinely new character can never make a password weaker. A discount applied to
/// the wrong character (counting the FIRST character of a run rather than its continuations)
/// breaks exactly this and nothing else notices.
#[test]
fn extending_a_password_never_lowers_the_estimate() {
    let base = "Vk7#pLmq";
    let mut previous = estimate(base).bits;
    for suffix in ["2", "2Z", "2Zr", "2Zr!", "2Zr!w"] {
        let bits = estimate(&format!("{base}{suffix}")).bits;
        assert!(
            bits >= previous - f32::EPSILON,
            "{base}{suffix} scored {bits} after {previous}"
        );
        previous = bits;
    }
}

/// The meter must stay inside the component's range for both a trivial and an extreme password;
/// `MoonProgress` clamps internally, so an out-of-range value would silently pin the bar instead
/// of showing anything wrong.
#[test]
fn the_meter_percentage_stays_within_range() {
    let extreme = "Vk7#pLmq2Zr!".repeat(8);
    for candidate in ["", "a", "aaaaaaaaaaaa", extreme.as_str()] {
        let percent = estimate(candidate).percent();
        assert!(
            (0.0..=100.0).contains(&percent),
            "{candidate:?} produced {percent}"
        );
    }
}

/// The bar is read before the text under it, so a rejected password must never be painted in a
/// band that means "fine". `aA1!aA1!` clears the entropy floor on charset variety alone and is
/// still refused for length — the case where a naive `bits → colour` mapping shows green over a
/// field Save rejects.
#[test]
fn a_rejected_password_is_never_shown_in_a_good_band() {
    for candidate in ["aA1!aA1!", "aA1!", "Ab1!Ab1!Ab"] {
        let verdict = estimate(candidate);
        if !verdict.accepted() {
            assert!(
                matches!(verdict.level, Level::Weak | Level::Fair | Level::Empty),
                "{candidate} was rejected ({:?}) but shown as {:?}",
                verdict.issue,
                verdict.level
            );
        }
    }
}

/// A known chunk must lower the score, not veto the password: a long passphrase that merely
/// CONTAINS "root" is genuinely strong, and refusing it teaches users the rule is arbitrary.
/// The chunk is allowed to reject only when it leaves too little behind — which is what the score
/// already measures.
#[test]
fn a_known_chunk_rejects_only_when_it_dominates() {
    let dominated = estimate("Rootroot12");
    assert!(!dominated.accepted());
    assert_eq!(dominated.issue, Some(Issue::Common));

    let surrounded = estimate("MyLongSecureRootPassphrase99!");
    assert!(
        surrounded.accepted(),
        "a 29-character passphrase was rejected over one embedded chunk: {:?} at {} bits",
        surrounded.issue,
        surrounded.bits
    );
}

/// This product's users type Russian. Classifying letters by script instead of by case made a
/// Cyrillic passphrase a single-class password and told the user to add a capital it already had,
/// with no way to satisfy the rule without switching keyboard layout.
#[test]
fn a_cyrillic_passphrase_satisfies_the_two_class_rule() {
    let verdict = estimate("МоёОченьДлинноеСлово");
    assert!(
        verdict.accepted(),
        "rejected: {:?} at {} bits",
        verdict.issue,
        verdict.bits
    );
    // Lower case alone is still one class, exactly as it is for Latin.
    assert_eq!(
        estimate("моёоченьдлинноеслово").issue,
        Some(Issue::OneClass)
    );
}

/// The exported floor and the verdict must agree. A future edit that raises `MIN_BITS` without
/// touching acceptance would otherwise leave the meter saying "Good" for a rejected password.
#[test]
fn acceptance_agrees_with_the_published_floor() {
    let verdict = estimate("Vk7#pLmq2Zr!");
    assert!(verdict.accepted());
    assert!(verdict.bits >= MIN_BITS);
    assert!("Vk7#pLmq2Zr!".chars().count() >= MIN_LEN);
}
