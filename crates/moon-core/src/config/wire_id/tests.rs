//! What this adapter has to hold: an id above `i64::MAX` must survive a TOML round trip, an id
//! below it must still be written as the bare integer every existing `settings.toml` holds, and
//! no stored value of any shape may cost the file.
//!
//! Written through `toml::to_string_pretty`, not `to_string`: that is what the shipping writer
//! calls (`toml_io::save` <- `store::write_settings`), and a guard that checks the other one
//! proves nothing about the file the application actually produces.

use serde::{Deserialize, Serialize};

/// Stand-in for the two fields that carry a core-issued id into `settings.toml`
/// (`ServerMeta::default_alert_strategy` and `ManualStratState::id`), with the same attribute.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Holder {
    #[serde(default, with = "crate::config::wire_id")]
    id: u64,
}

/// The bug this module exists for: `toml` refuses a `u64` above `i64::MAX`, and one such id used
/// to abort the whole `settings.toml` write.
#[test]
fn an_id_above_the_toml_integer_ceiling_survives_a_round_trip() {
    for id in [i64::MAX as u64, i64::MAX as u64 + 1, u64::MAX] {
        let text =
            toml::to_string_pretty(&Holder { id }).expect("id above i64::MAX must serialize");
        assert_eq!(
            toml::from_str::<Holder>(&text).expect("its own output must parse"),
            Holder { id },
            "round trip through {text:?}"
        );
    }
}

/// The compatibility half: everything an existing file holds keeps the shape it already has, so
/// a save does not rewrite every user's ids into a form older builds cannot read.
#[test]
fn an_id_that_fits_stays_a_bare_integer() {
    let id = 7_394_783_480_262_116_308;
    let text = toml::to_string_pretty(&Holder { id }).expect("serialize");
    assert_eq!(text.trim(), format!("id = {id}"));
    assert_eq!(
        toml::from_str::<Holder>(&text).expect("parse"),
        Holder { id }
    );
}

/// Reading has to accept both shapes, or a file written by this build stops loading — and a file
/// written by an older one has to keep loading.
#[test]
fn both_stored_shapes_read_back_the_same_id() {
    let id = u64::MAX;
    assert_eq!(
        toml::from_str::<Holder>(&format!("id = \"{id}\"")).expect("string form"),
        Holder { id }
    );
    assert_eq!(
        toml::from_str::<Holder>("id = 42").expect("integer form"),
        Holder { id: 42 }
    );
}

/// A value that is not an id resolves to "no strategy" instead of rejecting the file: one bad
/// number must not quarantine `settings.toml` and take every unrelated setting with it.
#[test]
fn a_value_that_is_not_an_id_reads_as_none() {
    for text in [
        "id = -5",
        "id = \"\"",
        "id = \"7e18\"",
        "id = \"18446744073709551616\"",
        "id = 1.5",
        "id = true",
        // An array, a table and a datetime land on `Other(IgnoredAny)`. Enumerated because
        // serde's own answer to them is `invalid type` — an error on the WHOLE file, which is
        // exactly what this fallback exists to prevent.
        "id = [1, 2]",
        "id = { a = 1 }",
        "id = 1979-05-27T07:32:00Z",
    ] {
        assert_eq!(
            toml::from_str::<Holder>(text).expect("must not reject the file"),
            Holder { id: 0 },
            "{text}"
        );
    }
}

/// An absent field keeps its serde default, which is what an older file without the field has.
#[test]
fn an_absent_id_stays_zero() {
    assert_eq!(
        toml::from_str::<Holder>("").expect("parse"),
        Holder { id: 0 }
    );
}

/// The constraint this whole module answers, kept executable: a bare `u64` above `i64::MAX` is
/// REFUSED by `toml` — as `out-of-range value for u64 type`, the message that appeared beside the
/// Save button. Only the refusal is asserted, not its wording: the claim is that the ceiling is
/// real, and a `toml` release that rephrases its error must not turn this red.
#[test]
fn a_bare_u64_above_the_ceiling_is_still_refused_by_toml() {
    #[derive(Serialize)]
    struct Bare {
        id: u64,
    }

    toml::to_string_pretty(&Bare { id: u64::MAX })
        .expect_err("toml integers are i64, so a bare u64 field must still refuse this");
}

/// Persisted `u64` fields in `source` that carry neither the adapter nor the exemption marker.
///
/// Split out of the sweep below so the sweep's own ability to go red is provable against a
/// fixture: a guard nobody can watch fail is a guard nobody knows is still working.
fn undecided_u64_fields(source: &str) -> Vec<String> {
    // `CoreId` is the crate's alias for a terminal uid, and the doc at `ServerConfig::id` already
    // names it; a field declared through it must not slip past for want of the token.
    const U64_TOKENS: [&str; 2] = ["u64", "CoreId"];
    // Items, not fields: `pub const`, `pub fn` and friends share the `pub` prefix and are
    // persisted by nothing.
    const ITEM_KEYWORDS: [&str; 9] = [
        "const ", "fn ", "type ", "struct ", "enum ", "mod ", "use ", "static ", "trait ",
    ];

    let is_field_head = |line: &str| {
        (line.starts_with("pub ") || line.starts_with("pub("))
            && line.contains(':')
            && !ITEM_KEYWORDS.iter().any(|kw| line.contains(kw))
    };

    let lines: Vec<&str> = source.lines().collect();
    let mut undecided = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let head = line.trim();
        if !is_field_head(head) {
            continue;
        }
        // rustfmt wraps a long declaration onto its own lines, so the field is every line up to
        // the one that closes it. Matching the first line alone would skip such a field in
        // silence. A continuation is only ever more type, so anything else ends the search.
        let mut field = String::from(head);
        let mut j = i;
        while !lines[j].trim_end().ends_with(',') && j + 1 < lines.len() {
            let next = lines[j + 1].trim();
            if next.is_empty()
                || next.starts_with("//")
                || next.starts_with("#[")
                || next.starts_with('}')
            {
                break;
            }
            field.push(' ');
            field.push_str(next);
            j += 1;
        }
        if !U64_TOKENS.iter().any(|token| field.contains(token)) {
            continue;
        }
        // The block above THIS field, bounded below by the previous field or by the line that
        // opens the struct. Bounded, so a decision on the field ABOVE is never read as this
        // field's — the false green a fixed-size window gives. Bounded THAT way rather than by a
        // run of `//`/`#[` lines, because a blank line or an attribute rustfmt wrapped across
        // lines would cut such a run short and report an adapted field as undecided — a guard
        // that fails the build for nothing gets deleted, and then guards nothing at all.
        let block_start = lines[..i]
            .iter()
            .rposition(|l| {
                let l = l.trim();
                is_field_head(l) || l.ends_with('{') || l.ends_with('}')
            })
            .map_or(0, |p| p + 1);
        let decided = lines[block_start..i]
            .iter()
            .any(|l| l.contains("crate::config::wire_id") || l.contains("wire-id-exempt"));
        if !decided {
            undecided.push(field);
        }
    }
    undecided
}

/// Two fields carry a core-issued id today. The third one added without a decision would re-arm an
/// application-wide save failure, and a module doc saying so would not survive the refactor that
/// added it — so the source itself is swept, the way `config::uid_counter` sweeps for the traits
/// that must never appear on it.
///
/// Scope is the files holding the structs that reach `settings.toml`, `servers.enc` and
/// `layout.toml` — the last one added because the ceiling would take that whole file down the
/// same way, and because under-scoping this list is how `core_groups.rs` stayed invisible. Every
/// `u64` field there is either adapted or carries the exemption marker, so adding one is a
/// deliberate act rather than an omission nobody sees until a user cannot save.
#[test]
fn every_persisted_u64_field_is_adapted_or_marked_exempt() {
    const SOURCES: [(&str, &str); 4] = [
        ("config/schema.rs", include_str!("../schema.rs")),
        ("config/servers.rs", include_str!("../servers.rs")),
        ("config/core_groups.rs", include_str!("../core_groups.rs")),
        ("config/layout.rs", include_str!("../layout.rs")),
    ];

    for (name, source) in SOURCES {
        let undecided = undecided_u64_fields(source);
        assert!(
            undecided.is_empty(),
            "{name}: {undecided:?} persist a u64 with neither the `wire_id` adapter nor a `wire-id-exempt` marker; a core-issued id there breaks every settings save"
        );
    }
}

/// The sweep's own contract, because a source scan that silently matches nothing looks exactly
/// like a clean tree. The middle field is the case a fixed lookback window gets wrong: it is
/// undecided, and the only nearby decision belongs to the field ABOVE it.
#[test]
fn the_sweep_sees_an_undecided_field_next_to_a_decided_one() {
    const FIXTURE: &str = r#"
pub struct Persisted {
    /// Adapted.
    #[serde(default, with = "crate::config::wire_id")]
    pub adapted: u64,
    /// Undecided, one line under an adapted neighbour.
    pub undecided: u64,
    /// A restricted visibility is still a persisted field.
    pub(crate) restricted: u64,
    /// Declared through the crate's uid alias.
    pub aliased: CoreId,
    /// Adapted through an attribute rustfmt wrapped, with a blank line in the block.

    #[serde(
        default,
        rename = "wrapped_attr",
        with = "crate::config::wire_id"
    )]
    pub wrapped_attribute: u64,
    // wire-id-exempt: terminal-issued, never a core id.
    pub exempt: u64,
    /// Wrapped by rustfmt, and undecided.
    pub wrapped:
        HashMap<String, u64>,
    /// Not a u64 at all.
    pub name: String,
    pub const NOT_A_FIELD: u64 = 1;
}
"#;

    assert_eq!(
        undecided_u64_fields(FIXTURE),
        vec![
            "pub undecided: u64,".to_string(),
            "pub(crate) restricted: u64,".to_string(),
            "pub aliased: CoreId,".to_string(),
            "pub wrapped: HashMap<String, u64>,".to_string(),
        ]
    );
}
