//! Unit tests for folder-tree edits.

use super::{rebase, sendable, with_added, without};

/// The rewrite covers the folder and everything under it. The tree is submitted as the COMPLETE
/// desired set, so a path left un-rewritten is a folder the core is told to keep at its old name,
/// with the renamed one arriving beside it.
#[test]
fn a_rename_rewrites_the_folder_and_its_whole_subtree() {
    let tree = vec![
        "Research".to_string(),
        "Research/Deep".to_string(),
        "Live".to_string(),
    ];
    assert_eq!(
        rebase(&tree, "Research", "Archive"),
        vec!["Archive", "Archive/Deep", "Live"]
    );
}

/// "Under" is a segment boundary, never a character prefix. Rewriting a folder that merely starts
/// with the same letters would rename something nobody touched — and, the tree being complete,
/// delete the original by omission in the same breath.
#[test]
fn a_folder_sharing_a_prefix_is_left_alone() {
    let tree = vec!["Research".to_string(), "Research2".to_string()];
    assert_eq!(
        rebase(&tree, "Research", "Archive"),
        vec!["Archive", "Research2"]
    );
}

/// The core compares folder paths case-insensitively, so the rewrite must too: a tree that kept
/// `RESEARCH/Deep` while renaming `Research` would ask the core to hold one folder under two names,
/// and the one it dropped would be the operator's.
#[test]
fn the_match_ignores_case_while_the_new_spelling_is_kept() {
    let tree = vec!["research".to_string(), "RESEARCH/Deep".to_string()];
    assert_eq!(
        rebase(&tree, "Research", "Archive"),
        vec!["Archive", "Archive/Deep"]
    );
}

/// Renaming onto a name that already exists merges the two folders; the list must say so once.
#[test]
fn a_rename_onto_an_existing_folder_lists_it_once() {
    let tree = vec!["Old".to_string(), "New".to_string()];
    assert_eq!(rebase(&tree, "Old", "New"), vec!["New"]);
}

/// Nothing to rewrite leaves the tree exactly as it stands, including the case-only rename the
/// protocol states is not a distinct folder-tree change at all.
#[test]
fn a_tree_with_nothing_to_rebase_comes_back_unchanged() {
    let tree = vec!["Alpha".to_string(), "Alpha/Deep".to_string()];
    assert_eq!(rebase(&tree, "Alpha", "alpha"), tree);
    assert_eq!(rebase(&tree, "", "Beta"), tree);
    assert_eq!(rebase(&tree, "Missing", "X"), tree);
}

/// A move is the same rewrite with a longer destination — one operation, so a drag and a rename
/// cannot drift apart.
#[test]
fn a_move_reparents_the_subtree() {
    let tree = vec![
        "Live".to_string(),
        "Live/Deep".to_string(),
        "Box".to_string(),
    ];
    assert_eq!(
        rebase(&tree, "Live", "Box/Live"),
        vec!["Box/Live", "Box/Live/Deep", "Box"]
    );
}

/// Adding is idempotent, case-insensitively: the same folder asked for twice is one folder, and a
/// second spelling of it would ask the core to create it again.
#[test]
fn adding_a_folder_the_tree_already_holds_changes_nothing() {
    let tree = vec!["Research".to_string()];
    assert_eq!(
        with_added(&tree, "Research/New"),
        vec!["Research", "Research/New"]
    );
    assert_eq!(with_added(&tree, "research"), tree);
}

/// The mirror of moonproto's validator on the case this terminal actually holds: MoonBot allows a
/// `/` inside a folder name, the validator does not, and it refuses the WHOLE submission on one
/// such path — including the strategy moves bundled with it. A core reporting one of those cannot
/// have its folders edited at all, which is what `CoreFolders::editable` is for.
#[test]
fn a_moonbot_folder_name_containing_a_slash_is_not_sendable() {
    assert!(sendable(["Research", "Research/Deep"].into_iter()));
    assert!(!sendable(["EMA / ORGANIC"].into_iter()));
    // The parent moonproto's own state derives from that name by splitting it.
    assert!(!sendable(["EMA "].into_iter()));
    assert!(!sendable([" leading"].into_iter()));
}

/// The validator's remaining rules, so a submission cannot be refused for a reason this mirror does
/// not know about.
#[test]
fn quotes_control_characters_and_overlong_paths_are_not_sendable() {
    assert!(!sendable(["say \"no\""].into_iter()));
    assert!(!sendable(["two\nlines"].into_iter()));
    assert!(!sendable(["a/"].into_iter()));
    // 255 BYTES, not characters: a Cyrillic name half that long already exceeds it.
    let long = "я".repeat(128);
    assert!(!sendable([long.as_str()].into_iter()));
    let fits = "я".repeat(127);
    assert!(sendable([fits.as_str()].into_iter()));
    // An empty path is the root, which is always acceptable.
    assert!(sendable([""].into_iter()));
}

/// Removal is omission, and it takes the whole subtree with it — a descendant left in the desired
/// tree would ask the core to keep the folder it was just told to drop.
#[test]
fn removing_a_folder_takes_its_subtree_and_nothing_else() {
    let tree = vec![
        "Research".to_string(),
        "Research/Deep".to_string(),
        "Research2".to_string(),
        "Live".to_string(),
    ];
    assert_eq!(without(&tree, "Research"), vec!["Research2", "Live"]);
    assert_eq!(without(&tree, "Missing"), tree);
}

/// The two spellings of one folder fold together everywhere: a path stored with `\` names the same
/// folder as one with `/`, and the core's own compare ignores case. An edit that missed either
/// would look for a folder the core has under a name it does not.
#[test]
fn one_folder_spelled_two_ways_is_one_folder() {
    let tree = vec![r"Deep\Inner".to_string()];
    assert_eq!(without(&tree, "deep/inner"), Vec::<String>::new());
    assert_eq!(with_added(&tree, "DEEP/inner"), tree);
    assert_eq!(rebase(&tree, "deep", "Box"), vec!["Box/Inner"]);
}

/// A folder name whose lowercase form is LONGER than the name itself. `İ` (U+0130) lowercases to
/// two characters, so a rewrite that sliced the raw path at the folded prefix's byte length would
/// cut mid-character and panic — on a string this terminal received from a core.
#[test]
fn a_name_that_changes_length_when_folded_is_still_rebased() {
    let tree = vec!["İ".to_string(), "İ/Deep".to_string(), "Other".to_string()];
    assert_eq!(rebase(&tree, "İ", "Box"), vec!["Box", "Box/Deep", "Other"]);
    assert_eq!(without(&tree, "İ"), vec!["Other"]);
}
