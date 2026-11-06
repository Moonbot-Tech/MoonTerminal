//! Retrieval regressions: exact parameter lookup, bounded context and immutable source citations.

use super::*;

/// Task guidance must come from the pinned source and stop before the next unrelated workflow.
#[test]
fn task_context_is_a_revision_pinned_section_with_explicit_absence() {
    let snapshot = Snapshot {
        revision: "b".repeat(40),
        documents: BTreeMap::from([(
            "reference/station-mcp-guide.md".into(),
            "# Guide\n## Strategies\nKeep exact values.\n## Orders\nDifferent workflow.".into(),
        )]),
        ..Default::default()
    };
    let context = snapshot.context("Strategies");
    assert_eq!(context["text"], "## Strategies\nKeep exact values.");
    assert_eq!(context["first_line"], 2);
    assert_eq!(context["knowledge"]["revision"], "b".repeat(40));
    assert_eq!(snapshot.context("Diagnosis")["available"], false);
}

/// Exercise the actual public source separately from deterministic offline regression tests.
#[test]
#[ignore = "requires public GitHub access"]
fn live_public_snapshot_loads_as_one_complete_revision() {
    let agent = ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(0)
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .new_agent();
    let (_stop, receiver) = mpsc::channel();
    let snapshot = refresh(&agent, "", &receiver).unwrap().unwrap();
    assert!(
        snapshot
            .documents
            .contains_key("reference/settings/index.tsv")
    );
    assert!(
        snapshot
            .documents
            .contains_key("concepts/08-testing-debugging-and-analytics.md")
    );
    assert!(
        !snapshot.search("OrderSize").unwrap()["hits"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn exact_parameter_index_wins_and_context_links_keep_the_source_revision() {
    let snapshot=Snapshot { revision:"a".repeat(40),checked_ms:1,documents:BTreeMap::from([
        ("reference/intro.md".into(),"An OrderSize example\nUse the account quote currency.".into()),
        ("reference/params/index.tsv".into(),"OrderSize\tMain\tDouble\t100\tparams.md\tAmount in trading currency\nOther\tMain\tBool".into()),
    ]),last_error:Some("refresh failed".into()) };
    let result = snapshot.search("OrderSize").expect("search");
    assert_eq!(result["hits"][0]["path"], "reference/params/index.tsv");
    assert!(
        result["hits"][0]["url"]
            .as_str()
            .expect("citation")
            .contains(&format!("/blob/{}/", "a".repeat(40)))
    );
    assert_eq!(result["knowledge"]["refresh_error"], "refresh failed");
    let page = snapshot.read("reference/intro.md", 1, 50).expect("page");
    assert!(
        page["text"]
            .as_str()
            .expect("text")
            .contains("account quote currency")
    );
    assert!(snapshot.read("../../mcp-auth.json", 1, 10).is_err());
}

#[test]
fn public_snapshot_cannot_fetch_private_materials_or_url_traversal() {
    for path in [
        "materials/private.md",
        "reference/../secrets.md",
        "reference/name?token.md",
        "reference/%2e%2e/file.md",
        "reference/file\\name.md",
    ] {
        assert!(!published_document(path), "{path}");
    }
}
