//! Separator rule and round trip of the folder-path helpers.

use super::*;

#[test]
fn split_and_join_roundtrip() {
    assert_eq!(split_path("a/b\\c"), vec!["a", "b", "c"]);
    assert_eq!(split_path("/a//b/"), vec!["a", "b"]);
    assert_eq!(split_path(""), Vec::<String>::new());
    assert_eq!(join_path(&split_path("a/b")), "a/b");
}

/// A slash surrounded by folder-name whitespace remains part of that segment.
///
/// Plausible edit this catches: replacing `strategy_path.rs:path_segments` with an unconditional split
/// produces three segments for the real path below, making tree operations address folders that
/// MoonBot does not have. The owning function documents the complete separator rule.
#[test]
fn a_slash_with_whitespace_beside_it_belongs_to_the_folder_name() {
    let real = "EMA / ORGANIC WAVE STRUCTURE STRATEGIES LLM/RELATIVE STRENGTH LLM";
    let parts = split_path(real);
    assert_eq!(
        parts,
        vec![
            "EMA / ORGANIC WAVE STRUCTURE STRATEGIES LLM",
            "RELATIVE STRENGTH LLM"
        ]
    );
    // The fingerprint of a cut made inside a name: the segment keeps the space that surrounded the
    // slash. Across this user's live core set that count is 91 under the old rule and 0 under this.
    assert!(parts.iter().all(|s| s.trim() == s));
    // A canonical path round-trips; `join_path` is the inverse for that shape alone.
    assert_eq!(join_path(&parts), real);

    // The one-sided grey zone: live data holds none, so pin the intent rather than discover it.
    assert_eq!(split_path("a/ b"), vec!["a/ b"]);
    assert_eq!(split_path("a /b"), vec!["a /b"]);
    assert_eq!(split_path("a/b"), vec!["a", "b"]);
    // The edges obey the same rule, a missing neighbour counting as non-whitespace.
    assert_eq!(split_path("/a"), vec!["a"]);
    assert_eq!(split_path("/ a"), vec!["/ a"]);
    // `\` is a separator on the same terms as `/`; pinned so the two cannot silently diverge.
    assert_eq!(split_path("a\\ b"), vec!["a\\ b"]);
    assert_eq!(split_path("a\\b"), vec!["a", "b"]);
}
