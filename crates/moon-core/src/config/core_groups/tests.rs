//! Unit coverage for the saved core-group list's own algebra: sanitizing, unique naming, and
//! reordering. See the module docstring for the invariants these pin.

use super::*;

/// Build a group fixture with one member.
fn g(name: &str) -> CoreGroup {
    CoreGroup {
        name: name.to_string(),
        cores: vec![1],
    }
}

/// Named breakage (`sanitize_core_groups`): a future author simplifies the uniqueness rule back
/// to "drop the later duplicate" instead of renaming it. Consequence: a hand-edited
/// `settings.toml` holding `Scalpers` and `scalpers` silently loses one group, with its member
/// list, on the next launch.
#[test]
fn a_case_insensitive_duplicate_name_is_renamed_not_dropped() {
    let mut groups = vec![
        CoreGroup {
            name: "Scalpers".to_string(),
            cores: vec![1],
        },
        CoreGroup {
            name: "scalpers".to_string(),
            cores: vec![2],
        },
    ];

    let changed = sanitize_core_groups(&mut groups);

    assert!(changed, "renaming a collision must still report a change");
    assert_eq!(
        groups.len(),
        2,
        "a case-insensitive collision must be RENAMED, never dropped -- membership is not \
         recoverable from anywhere else"
    );
    assert_ne!(
        groups[0].name.to_lowercase(),
        groups[1].name.to_lowercase(),
        "the two names must no longer collide after sanitizing"
    );
    assert_eq!(
        groups[0].cores,
        vec![1],
        "the first group's members must survive untouched"
    );
    assert_eq!(
        groups[1].cores,
        vec![2],
        "the second group's members must survive untouched"
    );
}

/// Named breakage (`sanitize_core_groups`): dropping the `if !cores.contains(core)` guard would
/// push every member unconditionally, losing the de-duplication that keeps first occurrence (and
/// therefore save order) intact.
#[test]
fn duplicate_members_collapse_to_first_occurrence_preserving_order() {
    let mut groups = vec![CoreGroup {
        name: "Group".to_string(),
        cores: vec![5, 3, 5, 7, 3],
    }];

    sanitize_core_groups(&mut groups);

    assert_eq!(
        groups[0].cores,
        vec![5, 3, 7],
        "members must de-duplicate keeping FIRST occurrence, preserving save order"
    );
}

/// Named breakage (`sanitize_core_groups`): the name cap must count CHARACTERS, not bytes, or a
/// Cyrillic name is cut to roughly half the length an ASCII one gets, and can be cut mid-character.
#[test]
fn the_name_cap_counts_characters_not_bytes() {
    // Every char here is 2 UTF-8 bytes: a byte-based cap would corrupt this at half the
    // character count a byte cap of CORE_GROUP_NAME_MAX would imply.
    let exactly_at_cap = "\u{042B}".repeat(CORE_GROUP_NAME_MAX);
    let mut groups = vec![CoreGroup {
        name: exactly_at_cap.clone(),
        cores: vec![1],
    }];
    sanitize_core_groups(&mut groups);
    assert_eq!(
        groups[0].name, exactly_at_cap,
        "a name exactly at the character cap must survive whole"
    );

    let over_cap = "\u{042B}".repeat(CORE_GROUP_NAME_MAX + 10);
    let mut groups2 = vec![CoreGroup {
        name: over_cap,
        cores: vec![1],
    }];
    sanitize_core_groups(&mut groups2);
    assert_eq!(
        groups2[0].name.chars().count(),
        CORE_GROUP_NAME_MAX,
        "a name over the cap must be cut at CORE_GROUP_NAME_MAX CHARACTERS, not bytes"
    );
}

/// Named breakage (`unique_group_name`): dropping the base-shortening `room`/`stem` computation
/// would let a renamed name exceed the cap, and `sanitize_core_groups` truncating it back down
/// afterward would cut the ` (2)` suffix straight back off -- the rename would appear to do
/// nothing, since the collision it was meant to resolve reappears.
#[test]
fn unique_group_name_shortens_the_base_so_the_suffix_survives_the_cap() {
    let base = "A".repeat(CORE_GROUP_NAME_MAX);
    let existing = vec![base.clone()];

    let renamed = unique_group_name(&existing, &base);

    assert!(
        renamed.chars().count() <= CORE_GROUP_NAME_MAX,
        "a renamed group must not exceed the character cap: got {} chars",
        renamed.chars().count()
    );
    assert!(
        renamed.ends_with(" (2)"),
        "at the cap the numeric suffix must survive whole, not be truncated back off: got \
         {renamed:?}"
    );
    assert_ne!(
        renamed.to_lowercase(),
        base.to_lowercase(),
        "the rename must actually resolve the collision"
    );
}

/// Named breakage (`move_group`): swapping the two rotate directions moves the element the wrong
/// way -- a forward move rotating right (or vice versa) shuffles the WHOLE spanned range instead
/// of relocating exactly the one element the caller asked to move.
#[test]
fn move_group_moves_exactly_one_element_forward_and_backward() {
    let mut forward = vec![g("A"), g("B"), g("C"), g("D")];
    assert!(move_group(&mut forward, 0, 2));
    assert_eq!(
        forward.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(),
        vec!["B", "C", "A", "D"],
        "moving index 0 to index 2 must shift B and C left by one and place A at 2"
    );

    let mut backward = vec![g("A"), g("B"), g("C"), g("D")];
    assert!(move_group(&mut backward, 3, 1));
    assert_eq!(
        backward.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(),
        vec!["A", "D", "B", "C"],
        "moving index 3 to index 1 must shift B and C right by one and place D at 1"
    );
}

/// `move_group` refuses a same-position or out-of-range move rather than clamping it -- both
/// mean the caller's view of the list is stale.
#[test]
fn move_group_refuses_out_of_range_or_same_position_moves() {
    let mut groups = vec![g("A"), g("B")];

    assert!(!move_group(&mut groups, 0, 0));
    assert!(!move_group(&mut groups, 0, 5));
    assert!(!move_group(&mut groups, 5, 0));
    assert_eq!(
        groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(),
        vec!["A", "B"],
        "a refused move must leave the list untouched"
    );
}
