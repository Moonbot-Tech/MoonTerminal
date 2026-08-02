//! What a settings save does to windows and chart persistence.
//!
//! A synthetic desk stands in for the real workspace: three groups, five cores, and a chart tab of
//! every kind — dedicated, shared, named bundle, custom multi-market, and one with a compare
//! anchor. Each test edits the server set the way the Settings window does and asserts the two
//! things the user actually sees: whether their chart tabs survive (a window rebuild wipes every
//! ordinary tab, since only Main's settings and detached tabs come back from `charts.json`), and
//! what is left in persistence afterwards.

// NOT `use super::*`: the parent imports `gpui::*`, whose `test` macro shadows `#[test]`.
use super::{CoreEntry, core_delta, prune_orphaned_chart_specs};
use crate::persistence::chart_persist::ChartTabSpec;
use moon_core::config::ChartBucket;

/// One core of the synthetic desk.
fn core(uid: u64, group: &str, bundle: &str) -> CoreEntry {
    CoreEntry {
        uid,
        group: group.to_string(),
        bundle: bundle.to_string(),
    }
}

/// The desk before any edit: G1 holds cores 1 and 2 (2 lives in the "scalp" bundle), G2 holds 3
/// and 4, G3 holds 5.
fn desk() -> Vec<CoreEntry> {
    vec![
        core(1, "G1", ""),
        core(2, "G1", "scalp"),
        core(3, "G2", ""),
        core(4, "G2", "scalp"),
        core(5, "G3", ""),
    ]
}

/// Chart tabs spread across that desk, one of every kind that persistence can hold.
fn chart_tabs() -> Vec<ChartTabSpec> {
    let mut main_g1 = ChartTabSpec::new("G1".to_string(), 0, ChartBucket::Shared);
    main_g1.scale = Some(1.5);
    let dedicated_1 = ChartTabSpec::new("G1".to_string(), 1, ChartBucket::Core(1));
    let dedicated_2 = ChartTabSpec::new("G1".to_string(), 2, ChartBucket::Core(2));
    let bundle_g1 = ChartTabSpec::new("G1".to_string(), 3, ChartBucket::Bundle("scalp".into()));
    let mut custom_g1 = ChartTabSpec::new("G1".to_string(), 100_000, ChartBucket::Shared);
    custom_g1.custom_coins = Some(vec![
        (1, "BTCUSDT".to_string()),
        (2, "ETHUSDT".to_string()),
        (5, "SOLUSDT".to_string()),
    ]);
    custom_g1.compare_anchor = Some((2, "ETHUSDT".to_string()));
    custom_g1.compare_orderbook_only = true;
    let dedicated_3 = ChartTabSpec::new("G2".to_string(), 1, ChartBucket::Core(3));
    let shared_g2 = ChartTabSpec::new("G2".to_string(), 2, ChartBucket::Shared);
    let dedicated_5 = ChartTabSpec::new("G3".to_string(), 1, ChartBucket::Core(5));
    vec![
        main_g1,
        dedicated_1,
        dedicated_2,
        bundle_g1,
        custom_g1,
        dedicated_3,
        shared_g2,
        dedicated_5,
    ]
}

/// Tab identities surviving a prune, as `(group, num)`.
fn surviving(specs: &[ChartTabSpec]) -> Vec<(String, u32)> {
    specs
        .iter()
        .map(|s| (s.group.clone(), s.num))
        .collect::<Vec<_>>()
}

/// Adding a server — the operation this whole path exists for — must not rebuild the windows, so
/// no chart tab is lost. Regression target: the old bundle signature compared whole vectors, so a
/// longer vector read as a bundle change and every addition wiped the tabs.
#[test]
fn adding_a_server_keeps_every_window_and_chart_tab() {
    let before = desk();
    let mut into_existing_group = before.clone();
    into_existing_group.push(core(6, "G2", ""));
    let mut into_new_group = before.clone();
    into_new_group.push(core(7, "G4", ""));
    let mut into_existing_bundle = before.clone();
    into_existing_bundle.push(core(8, "G1", "scalp"));

    for after in [into_existing_group, into_new_group, into_existing_bundle] {
        let delta = core_delta(&before, &after);
        assert_eq!(delta.added.len(), 1);
        assert!(delta.removed.is_empty());
        assert!(delta.moved.is_empty());
        assert!(delta.rebundled.is_empty());
        assert!(
            !delta.needs_window_rebuild(false),
            "adding a server must not rebuild windows: {delta:?}"
        );
        // The windows survive, so their tabs do; persistence must be untouched as well.
        let mut specs = chart_tabs();
        let before_tabs = surviving(&specs);
        assert!(!prune_orphaned_chart_specs(
            &mut specs,
            &delta.moved,
            &delta.removed
        ));
        assert_eq!(surviving(&specs), before_tabs);
        assert!(specs.iter().all(|s| {
            s.custom_coins
                .as_ref()
                .is_none_or(|coins| coins.len() == 3 && s.compare_anchor.is_some())
        }));
    }
}

/// Removing a server MUST rebuild: `ChartTabs::ingest` only appends stacks for live sessions and
/// never retires one, so the departed core's tab would keep a dead `CoreId` in the live window.
#[test]
fn removing_a_server_rebuilds_and_sweeps_only_its_own_charts() {
    let before = desk();
    let after: Vec<CoreEntry> = before.iter().filter(|c| c.uid != 2).cloned().collect();

    let delta = core_delta(&before, &after);
    assert_eq!(delta.removed, vec![2]);
    assert!(delta.needs_window_rebuild(false));

    let mut specs = chart_tabs();
    // A removal is group-agnostic, unlike a move: the SAME core's tab under a different group must
    // go too. Without this record the test would pass for an implementation that ANDed `removed`
    // with a group match.
    specs.push(ChartTabSpec::new("G3".to_string(), 9, ChartBucket::Core(2)));
    assert!(prune_orphaned_chart_specs(
        &mut specs,
        &delta.moved,
        &delta.removed
    ));

    // Both of core 2's dedicated tabs are gone, in G1 and in G3; every other tab survives. The
    // "scalp" bundle tab of G1 stays even though core 2 was its only G1 member — a bundle spec is
    // keyed by name, not by core (see `removing_a_bundles_last_core_leaves_the_bundle_tab`).
    assert_eq!(
        surviving(&specs),
        vec![
            ("G1".to_string(), 0),
            ("G1".to_string(), 1),
            ("G1".to_string(), 3),
            ("G1".to_string(), 100_000),
            ("G2".to_string(), 1),
            ("G2".to_string(), 2),
            ("G3".to_string(), 1),
        ]
    );
    // The custom tab keeps the coins of the cores that remain, and loses its anchor on core 2.
    let custom = specs
        .iter()
        .find(|s| s.num == 100_000)
        .expect("custom tab survives");
    assert_eq!(
        custom.custom_coins.as_deref(),
        Some([(1, "BTCUSDT".to_string()), (5, "SOLUSDT".to_string())].as_slice())
    );
    assert_eq!(custom.compare_anchor, None);
    // Comparison mode goes with the anchor: a tab left comparing against nothing is not a state
    // the chart can render.
    assert!(!custom.compare_orderbook_only);
}

/// Removing the last core of a bundle leaves the bundle tab behind — it is keyed by name, not by
/// core, and nothing in the prune claims otherwise. Documented so the empty tab is a known state
/// rather than a surprise.
#[test]
fn removing_a_bundles_last_core_leaves_the_bundle_tab() {
    let before = desk();
    let after: Vec<CoreEntry> = before
        .iter()
        .filter(|c| c.bundle != "scalp")
        .cloned()
        .collect();

    let delta = core_delta(&before, &after);
    assert_eq!(delta.removed, vec![2, 4]);

    let mut specs = chart_tabs();
    let bundle_tabs = |specs: &[ChartTabSpec]| {
        specs
            .iter()
            .filter(|s| matches!(s.bucket(), ChartBucket::Bundle(name) if name == "scalp"))
            .count()
    };
    assert_eq!(bundle_tabs(&specs), 1);
    prune_orphaned_chart_specs(&mut specs, &delta.moved, &delta.removed);
    assert_eq!(
        bundle_tabs(&specs),
        1,
        "a bundle tab is keyed by name, so it outlives every core that fed it — the tab opens empty
         instead of disappearing, and only the user can remove it"
    );
}

/// Moving a core between groups rebuilds, and the prune must follow the core out of its OLD group
/// only — the same core's records under other groups are none of its business.
#[test]
fn moving_a_core_sweeps_the_old_group_and_spares_the_new_one() {
    let before = desk();
    let after: Vec<CoreEntry> = before
        .iter()
        .map(|c| {
            if c.uid == 1 {
                core(1, "G2", "")
            } else {
                c.clone()
            }
        })
        .collect();

    let delta = core_delta(&before, &after);
    assert_eq!(delta.moved, vec![(1, "G1".to_string())]);
    assert!(delta.removed.is_empty());
    assert!(delta.needs_window_rebuild(false));

    let mut specs = chart_tabs();
    // A tab core 1 already owns in its NEW group must survive the move.
    specs.push(ChartTabSpec::new("G2".to_string(), 3, ChartBucket::Core(1)));
    prune_orphaned_chart_specs(&mut specs, &delta.moved, &delta.removed);

    assert!(
        !specs
            .iter()
            .any(|s| s.group == "G1" && matches!(s.bucket(), ChartBucket::Core(1))),
        "core 1's tab in the group it left is swept"
    );
    assert!(
        specs
            .iter()
            .any(|s| s.group == "G2" && matches!(s.bucket(), ChartBucket::Core(1))),
        "core 1's tab in the group it joined is kept"
    );
    let custom = specs
        .iter()
        .find(|s| s.num == 100_000)
        .expect("custom tab survives");
    assert_eq!(
        custom.custom_coins.as_deref(),
        Some([(2, "ETHUSDT".to_string()), (5, "SOLUSDT".to_string())].as_slice()),
        "the moved core's coin leaves the old group's custom tab"
    );
}

/// Retargeting a core's bundle rebuilds: bucket keys change and `chart_tabs_sig` does not hash the
/// bundle, so a live window would keep composing the old tabs. Nothing is pruned — no core left a
/// group and none disappeared.
#[test]
fn rebundling_a_core_rebuilds_without_dropping_specs() {
    let before = desk();
    let after: Vec<CoreEntry> = before
        .iter()
        .map(|c| {
            if c.uid == 1 {
                core(1, "G1", "scalp")
            } else {
                c.clone()
            }
        })
        .collect();

    let delta = core_delta(&before, &after);
    assert_eq!(delta.rebundled, vec![1]);
    assert!(delta.needs_window_rebuild(false));

    let mut specs = chart_tabs();
    let before_tabs = surviving(&specs);
    assert!(!prune_orphaned_chart_specs(
        &mut specs,
        &delta.moved,
        &delta.removed
    ));
    assert_eq!(surviving(&specs), before_tabs);
}

/// Toggling split-by-core rebuilds on its own: every bucket key changes at once, with no server
/// edit to detect. This is the save that reaches the `!struct_changed` branch, since neither split
/// mode nor a bundle is part of `AppConfig::structural_sig`.
#[test]
fn toggling_split_rebuilds_on_its_own() {
    let before = desk();
    let delta = core_delta(&before, &before);
    assert_eq!(delta, Default::default());
    assert!(!delta.needs_window_rebuild(false));
    assert!(delta.needs_window_rebuild(true));

    // Nothing left a group and nothing vanished, so the rebuild must not prune persistence: the
    // tabs are recomposed under new bucket keys, not deleted.
    let mut specs = chart_tabs();
    let before_tabs = surviving(&specs);
    assert!(!prune_orphaned_chart_specs(&mut specs, &[], &[]));
    assert_eq!(surviving(&specs), before_tabs);
}

/// Renaming a group is a move for every core in it. Its CORE-bound persistence goes with it; the
/// group-bound records (Main, Shared, Bundle) keep the old name and are simply never addressed
/// again — nothing prunes them, and the renamed group's Main starts from defaults.
#[test]
fn renaming_a_group_moves_all_of_its_cores() {
    let before = desk();
    let after: Vec<CoreEntry> = before
        .iter()
        .map(|c| {
            if c.group == "G1" {
                core(c.uid, "Desk-1", &c.bundle)
            } else {
                c.clone()
            }
        })
        .collect();

    let delta = core_delta(&before, &after);
    assert_eq!(
        delta.moved,
        vec![(1, "G1".to_string()), (2, "G1".to_string())]
    );

    let mut specs = chart_tabs();
    prune_orphaned_chart_specs(&mut specs, &delta.moved, &delta.removed);
    assert!(
        !specs
            .iter()
            .any(|s| s.group == "G1" && matches!(s.bucket(), ChartBucket::Core(_))),
        "dedicated tabs of the old group name are swept"
    );
    // Main and the shared tab keep the old group name: they are not bound to any core, and the
    // group window for the new name simply starts from defaults.
    assert!(specs.iter().any(|s| s.group == "G1" && s.num == 0));
}
