//! Adding a switch to a file the user already has, without disturbing anything in it.

use super::*;
use crate::diagnostics::config::{DiagCfg, parse};
use crate::diagnostics::template;

/// A file as an older release would have written it, then edited by hand: one switch turned on, one
/// comment of the user's own, a whole section that did not exist yet, and one key missing from a
/// section that did.
const OLD_AND_EDITED: &str = "\
# my own note: turned this on while chasing the phantom balances
[log]
balances = true
kline_cache = false
filter = \"\"

[channels]
render = false
detect = false
orders = \"GateF/BTC\"
assets = false
markets = false
";

#[test]
fn a_complete_file_is_left_alone() {
    assert!(
        merge_missing(&template::render()).is_none(),
        "rewriting an already-complete file would churn its mtime and wake the watcher every start"
    );
}

#[test]
fn merging_is_idempotent() {
    let (once, _) = merge_missing(OLD_AND_EDITED).expect("the sample is missing keys");
    assert!(
        merge_missing(&once).is_none(),
        "a second start must not keep appending"
    );
}

#[test]
fn a_file_that_does_not_parse_is_never_written() {
    assert!(
        merge_missing("[log]\nbalances = ").is_none(),
        "a file we cannot read is one we must not edit, or a single typo costs the user their \
         whole configuration"
    );
}

#[test]
fn the_missing_key_arrives_with_its_documentation() {
    let (merged, added) = merge_missing(OLD_AND_EDITED).expect("keys are missing");
    assert!(
        added.contains(&"log.market_sources".to_string()),
        "{added:?}"
    );
    assert!(
        merged.contains("market_sources = false"),
        "the key itself must be written"
    );
    assert!(
        merged.contains("# ENV: MOON_DIAG_MARKET_SOURCES=1"),
        "a key without its comment is exactly the discoverability problem this file exists to \
         solve; merged=\n{merged}"
    );
}

#[test]
fn a_section_introduced_later_is_added_whole() {
    let (merged, added) = merge_missing(OLD_AND_EDITED).expect("keys are missing");
    assert!(
        added.contains(&"limits.log_ring_lines".to_string()),
        "{added:?}"
    );
    let cfg = parse(&merged).expect("the merged file must parse");
    assert_eq!(cfg.limits, DiagCfg::default().limits);
    assert!(
        merged.contains("[limits]"),
        "the keys must land under a real section header, not at the document root where nothing \
         would read them"
    );
}

#[test]
fn the_users_own_edits_and_comments_survive() {
    let (merged, _) = merge_missing(OLD_AND_EDITED).expect("keys are missing");
    assert!(
        merged.contains("# my own note: turned this on while chasing the phantom balances"),
        "merged=\n{merged}"
    );
    let cfg = parse(&merged).expect("the merged file must parse");
    assert!(cfg.log.balances, "a value the user set must not be reset");
    assert_eq!(
        cfg.channels.orders, "GateF/BTC",
        "a selector the user typed must not be reset"
    );
}

#[test]
fn nothing_the_merge_does_not_recognise_is_removed() {
    let text = format!("{OLD_AND_EDITED}\n[mine]\nkeep = 42\n");
    let (merged, _) = merge_missing(&text).expect("keys are missing");
    assert!(
        merged.contains("[mine]") && merged.contains("keep = 42"),
        "an unknown key may belong to a newer build or be the user's own note; deleting it would \
         make downgrading destructive. merged=\n{merged}"
    );
}

#[test]
fn a_section_name_the_user_turned_into_a_value_is_not_clobbered() {
    // `log = 1` is a mistake, but it is THEIR mistake and their text; replacing it with our table
    // would silently delete what they typed instead of letting them see the parse error.
    let (merged, _) = merge_missing("log = 1\n").expect("other sections are still missing");
    assert!(merged.contains("log = 1"));
}
