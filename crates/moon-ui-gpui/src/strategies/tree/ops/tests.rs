use super::*;
use moon_core::feed::{SchemaField, SchemaFieldUi, SchemaKind, SchemaSection, StrategyRow};

fn row(id: u64, name: &str, path: &str, checked: bool) -> StrategyRow {
    StrategyRow {
        id,
        name: name.to_string(),
        kind: "Long".to_string(),
        kind_ordinal: 1,
        folder_path: path.to_string(),
        checked,
        is_short: false,
        fields: vec![
            (STRATEGY_NAME_FIELD.to_string(), name.to_string()),
            ("Amount".to_string(), "10".to_string()),
        ],
    }
}

fn field(name: &str, default: Option<&str>) -> SchemaField {
    SchemaField {
        name: name.to_string(),
        type_name: "String".to_string(),
        ui: SchemaFieldUi::Edit,
        picklist: vec![],
        default: default.map(str::to_string),
    }
}

fn kind() -> SchemaKind {
    SchemaKind {
        ordinal: 1,
        name: "Long".to_string(),
        sections: vec![SchemaSection {
            title: "main".to_string(),
            fields: vec![
                field(STRATEGY_NAME_FIELD, None),
                field("Amount", Some("100")),
                field("Spread", Some("0.5")),
            ],
        }],
    }
}

#[test]
fn split_and_join_roundtrip() {
    assert_eq!(split_path("a/b\\c"), vec!["a", "b", "c"]);
    assert_eq!(split_path("/a//b/"), vec!["a", "b"]);
    assert_eq!(split_path(""), Vec::<String>::new());
    assert_eq!(join_path(&split_path("a/b")), "a/b");
}

/// A folder NAME may contain `/`. MoonProto flattens placement into one unescaped string and
/// MoonBot allows a slash inside a name, so `"EMA / ORGANIC WAVE STRUCTURE STRATEGIES LLM"` is a
/// single folder there; `path_segments` splits only at a slash with no whitespace beside it.
///
/// BREAKAGE: `ops.rs:path_segments` is "simplified" back to
/// `path.split(['/', '\\']).filter(|s| !s.is_empty())` — the scan reads like pointless ceremony.
/// The tree would then show a folder chain MoonBot does not have, and every folder rename, move
/// and delete would address a folder that exists nowhere. Under that edit the real path below
/// yields THREE segments instead of two, two of them carrying an edge space, so both the equality
/// and the `trim()` assertion fail.
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
    // slash. Across a live 53-core set that count is 91 under the old rule and 0 under this one.
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

#[test]
fn rows_under_includes_nested() {
    let rows = vec![
        row(1, "s1", "a/b", false),
        row(2, "s2", "a/b/c", false),
        row(3, "s3", "a/x", false),
    ];
    let under = rows_under(&rows, &split_path("a/b"));
    let ids: Vec<u64> = under.iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![1, 2]);
}

#[test]
fn all_off_rule() {
    let on = row(1, "s1", "a", true);
    let off = row(2, "s2", "a", false);
    assert!(all_off(&[&off]));
    assert!(!all_off(&[&off, &on]));
    assert!(all_off(&[]));
}

#[test]
fn new_strategy_uses_defaults_and_name() {
    let ns = new_strategy(&kind(), "My Strat", "folder/x");
    assert_eq!(ns.kind_ordinal, 1);
    assert_eq!(ns.folder_path, "folder/x");
    // The name is stored in the StrategyName field.
    let name = ns.fields.iter().find(|(n, _)| n == STRATEGY_NAME_FIELD);
    assert_eq!(
        name,
        Some(&(STRATEGY_NAME_FIELD.to_string(), "My Strat".to_string()))
    );
    // Schema defaults are preserved.
    let amount = ns.fields.iter().find(|(n, _)| n == "Amount");
    assert_eq!(amount.map(|(_, v)| v.as_str()), Some("100"));
    let spread = ns.fields.iter().find(|(n, _)| n == "Spread");
    assert_eq!(spread.map(|(_, v)| v.as_str()), Some("0.5"));
}

#[test]
fn unique_name_suffixing() {
    let mut taken = HashSet::new();
    assert_eq!(unique_name(&taken, "S"), "S");
    taken.insert("S".to_string());
    assert_eq!(unique_name(&taken, "S"), "S (2)");
    taken.insert("S (2)".to_string());
    assert_eq!(unique_name(&taken, "S"), "S (3)");
    // Copying a copy strips old suffixes to the base instead of producing `(copy) (copy)`.
    taken.insert("S (copy)".to_string());
    assert_eq!(unique_name(&taken, "S (copy)"), "S (3)");
    taken.insert("S (3)".to_string());
    assert_eq!(unique_name(&taken, "S (3)"), "S (4)");
    // An occupied `(copy) (copy)` name is uniquified from its base instead of extending the suffix.
    taken.insert("S (copy) (copy)".to_string());
    assert_eq!(unique_name(&taken, "S (copy) (copy)"), "S (4)");
    // Parentheses unrelated to copying remain part of the name.
    taken.insert("Grid (v2 beta)".to_string());
    assert_eq!(unique_name(&taken, "Grid (v2 beta)"), "Grid (v2 beta) (2)");
}

#[test]
fn clip_text_roundtrip() {
    let clip = vec![ClipItem {
        kind_ordinal: 7,
        kind: "MoonShot".to_string(),
        name: "S (2)".to_string(),
        rel_path: split_path("fld/sub"),
        fields: vec![
            (STRATEGY_NAME_FIELD.to_string(), "S (2)".to_string()),
            ("Formula".to_string(), "a\nb\\c".to_string()),
        ],
    }];
    let text = clip_to_text(&clip);
    assert_eq!(clip_from_text(&text), Some(clip));
    assert_eq!(clip_from_text("случайный текст"), None);
}

#[test]
fn copy_rows_flattens_to_target() {
    // A multi-selection from different folders has empty relative paths, so paste puts all in target.
    let rows = vec![
        row(1, "a", "grpA/p1", false),
        row(2, "b", "grpB/sub/p2", false),
    ];
    let refs: Vec<&StrategyRow> = rows.iter().collect();
    let clip = copy_rows(&refs);
    assert!(clip.iter().all(|c| c.rel_path.is_empty()));
    // `paste_plan` therefore places both directly in the target folder.
    let plan = paste_plan(&clip, &split_path("dest"), &HashSet::new());
    assert!(plan.iter().all(|n| n.folder_path == "dest"));
}

#[test]
fn copy_folder_keeps_folder_name() {
    let rows = vec![
        row(1, "a", "parent/fld", false),
        row(2, "b", "parent/fld/sub", false),
        row(3, "c", "other", false),
    ];
    let clip = copy_folder(&rows, &split_path("parent/fld"));
    // Relative to parent, the `fld` segment is preserved.
    let rels: Vec<Vec<String>> = clip.iter().map(|c| c.rel_path.clone()).collect();
    assert!(rels.contains(&split_path("fld")));
    assert!(rels.contains(&split_path("fld/sub")));
    assert_eq!(clip.len(), 2);
}

#[test]
fn paste_plan_rebases_and_dedups() {
    let clip = vec![
        ClipItem {
            kind_ordinal: 1,
            kind: "Long".to_string(),
            name: "S".to_string(),
            rel_path: vec![],
            fields: vec![(STRATEGY_NAME_FIELD.to_string(), "S".to_string())],
        },
        ClipItem {
            kind_ordinal: 1,
            kind: "Long".to_string(),
            name: "S".to_string(),
            rel_path: split_path("sub"),
            fields: vec![(STRATEGY_NAME_FIELD.to_string(), "S".to_string())],
        },
    ];
    let mut taken = HashSet::new();
    taken.insert("S".to_string());
    let plan = paste_plan(&clip, &split_path("dest"), &taken);
    assert_eq!(plan[0].folder_path, "dest");
    assert_eq!(plan[1].folder_path, "dest/sub");
    // Both names are unique and do not collide with each other.
    let n0 = plan[0]
        .fields
        .iter()
        .find(|(n, _)| n == STRATEGY_NAME_FIELD)
        .unwrap()
        .1
        .clone();
    let n1 = plan[1]
        .fields
        .iter()
        .find(|(n, _)| n == STRATEGY_NAME_FIELD)
        .unwrap()
        .1
        .clone();
    assert_eq!(n0, "S (2)");
    assert_eq!(n1, "S (3)");
    assert_ne!(n0, n1);
}

#[test]
fn rename_folder_rewrites_matching_only() {
    let rows = vec![
        row(1, "a", "a/old", false),
        row(2, "b", "a/old/sub", false),
        row(3, "c", "a/keep", false),
    ];
    let edits = rename_folder(&rows, &split_path("a/old"), "new");
    assert_eq!(edits.len(), 2);
    assert!(edits.contains(&(1, "a/new".to_string())));
    assert!(edits.contains(&(2, "a/new/sub".to_string())));
}

#[test]
fn move_folder_keeps_name_and_guards_self() {
    let rows = vec![
        row(1, "a", "src/fld", false),
        row(2, "b", "src/fld/sub", false),
        row(3, "c", "other", false),
    ];
    let edits = tree_ops_move_folder(&rows, "src/fld", "dest");
    assert!(edits.contains(&(1, "dest/fld".to_string())));
    assert!(edits.contains(&(2, "dest/fld/sub".to_string())));
    assert_eq!(edits.len(), 2);
    // Moving into itself or a descendant is a no-op.
    assert!(tree_ops_move_folder(&rows, "src/fld", "src/fld/sub").is_empty());
}

fn tree_ops_move_folder(rows: &[StrategyRow], folder: &str, target: &str) -> Vec<(u64, String)> {
    move_folder(rows, &split_path(folder), &split_path(target))
}

#[test]
fn move_to_flattens_to_target() {
    // Moving a multi-selection from different folders puts every row directly in the target.
    let rows = vec![
        row(1, "a", "src/p1", false),
        row(2, "b", "other/grp/p2", false),
    ];
    let refs: Vec<&StrategyRow> = rows.iter().collect();
    let edits = move_to(&refs, &split_path("dest"));
    assert!(edits.contains(&(1, "dest".to_string())));
    assert!(edits.contains(&(2, "dest".to_string())));
}
