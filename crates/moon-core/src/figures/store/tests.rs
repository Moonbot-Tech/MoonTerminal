use super::*;
use crate::figures::tools::{Channel, HLine};
use crate::figures::{DrawStyle, FigNode, FigureKind};

/// A `figures.json` written by the build that predates the tool modules: a bare array, figures as
/// externally-tagged struct variants, and no `shared` field.
const V1_FILE: &str = r#"[
  {
    "core": 1,
    "market": "BTCUSDT",
    "figures": [
      {"id":3,"kind":{"HLine":{"price":100.5}},"color":[64,196,255,255],"thickness":1.0,
       "line_kind":"Dash","created_ms":1700000000000,"alert":false,"strategy_id":0},
      {"id":4,"kind":{"Segment":{"a":{"time_ms":1.0,"price":2.0},"b":{"time_ms":3.0,"price":4.0}}},
       "color":[1,2,3,255],"thickness":2.0,"line_kind":"Solid","created_ms":1,"alert":true,
       "strategy_id":7},
      {"id":5,"kind":{"Triangle":{"a":{"time_ms":1.0,"price":2.0},"b":{"time_ms":3.0,"price":4.0},
       "c":{"time_ms":5.0,"price":6.0}}},"color":[1,2,3,255],"thickness":1.0,"line_kind":"Dot",
       "created_ms":1,"alert":false,"strategy_id":0},
      {"id":6,"kind":{"Channel":{"price1":10.0,"price2":20.0}},"color":[1,2,3,255],
       "thickness":1.0,"line_kind":"Dash","created_ms":1,"alert":false,"strategy_id":0}
    ]
  }
]"#;

/// A file holding a figure only a NEWER build understands, next to one this build reads.
const UNKNOWN_FILE: &str = r#"[{"core":1,"market":"BTCUSDT","figures":[
    {"id":1,"kind":{"Pitchfork":{"a":{"time_ms":1.0,"price":2.0}}},"color":[1,2,3,4],
     "thickness":1.0,"created_ms":1,"alert":false},
    {"id":2,"kind":{"HLine":{"price":7.0}},"color":[1,2,3,4],"thickness":1.0,
     "created_ms":1,"alert":false}
]}]"#;

fn fig(kind: FigureKind) -> Figure {
    Figure::new(kind, DrawStyle::default(), 0.0)
}

fn hline(price: f64) -> Figure {
    fig(FigureKind::HLine(HLine { price }))
}

#[test]
fn a_v1_file_still_loads_with_every_figure_type() {
    let store = FigureStore::from_json(V1_FILE);
    let figs = store.figures(1, "BTCUSDT");
    assert_eq!(figs.len(), 4, "one arm of the union stopped parsing");
    assert_eq!(figs[0].kind, FigureKind::HLine(HLine { price: 100.5 }));
    assert_eq!(figs[1].strategy_id, 7);
    assert!(
        figs[2..].iter().all(|f| !f.shared),
        "shared defaults to off"
    );
    assert!(figs.iter().all(|f| !f.from_server));
}

#[test]
fn a_ratio_scale_survives_a_save_and_load() {
    // The tool is local-only (the core has no blob payload for it), so the file is the ONLY place
    // it lives: a serialization that does not round-trip loses the drawing outright.
    use crate::figures::tools::FibRetracement;
    let mut store = FigureStore::default();
    let drawn = FibRetracement {
        a: FigNode::new(1_700_000_000_000.0, 63_713.5),
        b: FigNode::new(1_700_000_600_000.0, 61_240.25),
        // NOT the serde default: a field that only ever round-trips its default value is a field
        // whose serialization nobody tested. Levels 1 and 4 switched off.
        hidden_levels: 0b1_0010,
    };
    let id = store.add(1, "BTCUSDT", fig(FigureKind::FibRetracement(drawn)));
    let json = serde_json::to_string(&store.to_persist()).expect("store must serialize");
    let back = FigureStore::from_json(&json);
    assert_eq!(
        back.get(1, "BTCUSDT", id).map(|f| f.kind.clone()),
        Some(FigureKind::FibRetracement(drawn)),
        "the scale came back a different figure: {json}"
    );
    // And an OLD file, written before levels could be switched off, must load with the whole
    // scale rather than with some levels silently missing.
    let older = json.replace(",\"hidden_levels\":18", "");
    assert!(
        !older.contains("hidden_levels"),
        "the field was not dropped: {older}"
    );
    let back = FigureStore::from_json(&older);
    assert_eq!(
        back.get(1, "BTCUSDT", id).map(|f| f.kind.clone()),
        Some(FigureKind::FibRetracement(FibRetracement {
            hidden_levels: 0,
            ..drawn
        })),
        "an older file did not load with every level shown: {older}"
    );
}

#[test]
fn a_fill_survives_a_save_and_load() {
    let mut store = FigureStore::default();
    let mut f = hline(10.0);
    f.fill = [9, 8, 7, 123];
    let id = store.add(1, "BTCUSDT", f);
    let json = serde_json::to_string(&store.to_persist()).expect("store must serialize");
    let back = FigureStore::from_json(&json);
    assert_eq!(
        back.get(1, "BTCUSDT", id).map(|f| f.fill),
        Some([9, 8, 7, 123]),
        "the chosen fill did not come back: {json}"
    );
}

#[test]
fn a_figure_saved_before_fills_existed_keeps_its_look() {
    // Painting one on would be a one-way rewrite of a file the user cannot edit back.
    let store = FigureStore::from_json(V1_FILE);
    for f in store.figures(1, "BTCUSDT") {
        assert_eq!(
            f.fill[3], 0,
            "an old figure was given a fill nobody can remove"
        );
    }
}

#[test]
fn ids_continue_after_a_load_instead_of_colliding() {
    let mut store = FigureStore::from_json(V1_FILE);
    let id = store.add(1, "BTCUSDT", hline(1.0));
    assert!(id > 6, "a new figure reused a loaded id ({id})");
}

#[test]
fn a_broken_file_loads_as_empty_rather_than_failing() {
    assert!(
        FigureStore::from_json("{not json")
            .figures(1, "X")
            .is_empty()
    );
    assert!(FigureStore::from_json("").figures(1, "X").is_empty());
}

#[test]
fn the_saved_form_round_trips_and_stays_readable_by_the_shipped_build() {
    let mut store = FigureStore::default();
    store.add(1, "BTCUSDT", hline(10.0));
    store.add(
        2,
        "ETHUSDT",
        fig(FigureKind::Channel(Channel {
            price1: 1.0,
            price2: 2.0,
        })),
    );
    let json = serde_json::to_string(&store.to_persist()).expect("store must serialize");
    // A flat array, exactly as every shipped build parses it. An object wrapper here would make
    // this file load as "empty" on a downgrade, and the next save would erase the drawings.
    assert!(json.starts_with('['), "not an array: {json}");

    let back = FigureStore::from_json(&json);
    assert_eq!(back.figures(1, "BTCUSDT").len(), 1);
    assert_eq!(back.figures(2, "ETHUSDT").len(), 1);
}

#[test]
fn a_figure_only_a_newer_build_understands_costs_only_itself_and_survives_a_save() {
    let store = FigureStore::from_json(UNKNOWN_FILE);
    let figs = store.figures(1, "BTCUSDT");
    assert_eq!(figs.len(), 1, "the readable figure must survive the load");
    assert_eq!(figs[0].id, 2);

    let json = serde_json::to_string(&store.to_persist()).expect("store must serialize");
    assert!(
        json.contains("Pitchfork"),
        "the unreadable figure was destroyed by the save: {json}"
    );
    // And it is still there after a second round trip, not just the first.
    let again = FigureStore::from_json(&json);
    let json2 = serde_json::to_string(&again.to_persist()).expect("store must serialize");
    assert!(json2.contains("Pitchfork"));
    assert!(json2.contains("HLine"));
}

#[test]
fn a_new_figure_never_takes_the_id_of_a_retained_unreadable_one() {
    // The figure only a newer build can read has id 9 and the readable one id 2; a new figure
    // must clear BOTH, or the file would hold two figures with one id for the build that reads
    // both of them.
    let file = r#"[{"core":1,"market":"BTCUSDT","figures":[
        {"id":9,"kind":{"Pitchfork":{"a":{"time_ms":1.0,"price":2.0}}},"color":[1,2,3,4],
         "thickness":1.0,"created_ms":1,"alert":false},
        {"id":2,"kind":{"HLine":{"price":7.0}},"color":[1,2,3,4],"thickness":1.0,
         "created_ms":1,"alert":false}
    ]}]"#;
    let mut store = FigureStore::from_json(file);
    assert!(
        store.add(1, "BTCUSDT", hline(1.0)) > 9,
        "a new figure landed on the id of a figure only a newer build can read"
    );
}

#[test]
fn an_entry_whose_figures_are_all_unreadable_still_pins_the_core_uid_floor() {
    let file = r#"[{"core":9,"market":"BTCUSDT","figures":[
        {"id":1,"kind":{"Pitchfork":{"a":{"time_ms":1.0,"price":2.0}}},"color":[1,2,3,4],
         "thickness":1.0,"created_ms":1,"alert":false}
    ]}]"#;
    let mut store = FigureStore::from_json(file);
    assert_eq!(store.max_core_uid(), Some(9));
    // A server sync drops entries left empty by the skipped figures; the floor must survive it,
    // or a deleted core's uid could be reissued while its drawing is still in the file.
    store.set_server_figures(std::collections::HashMap::new());
    assert_eq!(
        store.max_core_uid(),
        Some(9),
        "the uid floor dropped below a core that still owns a drawing"
    );
}

#[test]
fn a_file_claiming_both_alert_and_shared_loses_the_sharing() {
    let file = r#"[{"core":1,"market":"BTCUSDT","figures":[
        {"id":1,"kind":{"HLine":{"price":7.0}},"color":[1,2,3,4],"thickness":1.0,
         "created_ms":1,"alert":true,"shared":true}
    ]}]"#;
    let store = FigureStore::from_json(file);
    let f = &store.figures(1, "BTCUSDT")[0];
    assert!(f.alert);
    assert!(
        !f.shared,
        "an alert belongs to one core, so it cannot be shared"
    );
}

#[test]
fn server_figures_are_not_persisted() {
    let mut store = FigureStore::default();
    let mut remote = hline(5.0);
    remote.id = 42;
    remote.from_server = true;
    store
        .by_key
        .insert((1, "BTCUSDT".to_string()), vec![remote, hline(6.0)]);
    let json = serde_json::to_string(&store.to_persist()).expect("store must serialize");
    assert!(
        !json.contains("42"),
        "a server alert reached the file: {json}"
    );
}

#[test]
fn a_shared_figure_is_visible_from_every_core_and_counted_once() {
    let mut store = FigureStore::default();
    let id = store.add(1, "BTCUSDT", hline(10.0));
    store.add(1, "BTCUSDT", hline(11.0));
    assert_eq!(store.visible(2, "BTCUSDT").count(), 0);

    assert!(store.set_shared(1, "BTCUSDT", id, true));
    assert_eq!(store.visible(2, "BTCUSDT").count(), 1, "shared to core 2");
    assert_eq!(
        store.visible(1, "BTCUSDT").count(),
        2,
        "the owner still sees its own two figures, not three"
    );
    assert_eq!(
        store.visible(2, "ETHUSDT").count(),
        0,
        "sharing spans cores, not markets"
    );
}

#[test]
fn a_shared_figure_is_edited_and_removed_in_its_owners_set() {
    let mut store = FigureStore::default();
    let id = store.add(1, "BTCUSDT", hline(10.0));
    store.set_shared(1, "BTCUSDT", id, true);

    assert!(store.get(2, "BTCUSDT", id).is_some(), "found from core 2");
    assert!(store.edit(2, "BTCUSDT", id, |f| {
        f.kind = FigureKind::HLine(HLine { price: 99.0 });
        true
    }));
    assert_eq!(
        store.get(1, "BTCUSDT", id).map(|f| f.kind.anchor_price()),
        Some(99.0),
        "the edit must land on the original"
    );

    assert!(store.remove(2, "BTCUSDT", id).is_some());
    assert!(store.get(1, "BTCUSDT", id).is_none());
}

#[test]
fn an_armed_or_server_figure_refuses_to_be_shared() {
    let mut store = FigureStore::default();
    let armed = store.add(1, "BTCUSDT", hline(10.0));
    store.edit(1, "BTCUSDT", armed, |f| {
        f.alert = true;
        true
    });
    assert!(!store.set_shared(1, "BTCUSDT", armed, true));
    assert!(!store.get(1, "BTCUSDT", armed).expect("figure").shared);

    let mut remote = hline(5.0);
    remote.id = 77;
    remote.from_server = true;
    store
        .by_key
        .insert((3, "SOLUSDT".to_string()), vec![remote]);
    assert!(!store.set_shared(3, "SOLUSDT", 77, true));
}

#[test]
fn sharing_survives_a_save_and_load() {
    let mut store = FigureStore::default();
    let id = store.add(1, "BTCUSDT", hline(10.0));
    store.set_shared(1, "BTCUSDT", id, true);
    let json = serde_json::to_string(&store.to_persist()).expect("store must serialize");
    let back = FigureStore::from_json(&json);
    assert!(back.get(1, "BTCUSDT", id).expect("figure").shared);
    assert_eq!(back.visible(2, "BTCUSDT").count(), 1);
}

#[test]
fn a_store_change_bumps_the_revision_that_gates_redraws() {
    let mut store = FigureStore::default();
    let rev0 = store.rev();
    let id = store.add(1, "BTCUSDT", hline(10.0));
    assert_ne!(store.rev(), rev0);
    let rev1 = store.rev();
    assert!(!store.edit(1, "BTCUSDT", id, |_| false));
    assert_eq!(store.rev(), rev1, "a no-op edit must not force a rebuild");
}

/// Builds the two sets `reconcile_local_alerts` is judged against.
fn sets(
    held: &[(CoreId, u64)],
    authoritative: &[CoreId],
) -> (
    std::collections::HashSet<(CoreId, u64)>,
    std::collections::HashSet<CoreId>,
) {
    (
        held.iter().copied().collect(),
        authoritative.iter().copied().collect(),
    )
}

/// A local figure must not keep claiming an alert the core no longer holds.
///
/// Moonbot removes an object when its alert fires, and it tells nobody: the terminal only sees the
/// object stop being in the core's set. Before this, the box stayed ticked over an alert that
/// existed nowhere — and that box is what both the panel and the chart badge read.
#[test]
fn a_local_alert_the_core_no_longer_holds_is_disarmed() {
    let mut store = FigureStore::default();
    let id = store.add(1, "BTCUSDT", hline(100.0));
    store.edit(1, "BTCUSDT", id, |f| {
        f.alert = true;
        true
    });
    let rev = store.rev();

    let (held, authoritative) = sets(&[], &[1]);
    assert!(store.reconcile_local_alerts(&held, &authoritative, 60_000.0));
    assert!(
        !store.figures(1, "BTCUSDT")[0].alert,
        "the core does not hold it and has said so; the flag must go"
    );
    assert_ne!(store.rev(), rev, "a disarm must invalidate the drawn set");
}

/// The core's silence during a round trip is not an answer.
///
/// A reconcile can be triggered by ANY core between our upsert and this core's echo — measured at
/// ~1.3 s on a live core — and reading that gap as "Moonbot dropped it" would untick the box the
/// user just ticked.
#[test]
fn an_upsert_still_in_flight_keeps_its_flag() {
    let mut store = FigureStore::default();
    let id = store.add(1, "BTCUSDT", hline(100.0));
    store.edit(1, "BTCUSDT", id, |f| {
        f.alert = true;
        f.alert_sent_ms = 10_000.0;
        true
    });

    let (held, authoritative) = sets(&[], &[1]);
    assert!(!store.reconcile_local_alerts(&held, &authoritative, 11_000.0));
    assert!(store.figures(1, "BTCUSDT")[0].alert);

    // Once the grace has passed, the same silence IS the answer.
    assert!(store.reconcile_local_alerts(&held, &authoritative, 30_000.0));
    assert!(!store.figures(1, "BTCUSDT")[0].alert);
}

/// Seeing the figure on the core ends the round trip: a LATER absence is then real at once, with
/// no second grace period to wait out.
#[test]
fn being_seen_on_the_core_closes_the_round_trip() {
    let mut store = FigureStore::default();
    let id = store.add(1, "BTCUSDT", hline(100.0));
    store.edit(1, "BTCUSDT", id, |f| {
        f.alert = true;
        f.alert_sent_ms = 10_000.0;
        true
    });

    let (held, authoritative) = sets(&[(1, id)], &[1]);
    assert!(!store.reconcile_local_alerts(&held, &authoritative, 10_100.0));
    assert!(store.figures(1, "BTCUSDT")[0].alert);

    let (gone, authoritative) = sets(&[], &[1]);
    assert!(store.reconcile_local_alerts(&gone, &authoritative, 10_200.0));
    assert!(!store.figures(1, "BTCUSDT")[0].alert);
}

/// A core that has not reported its set — offline, or connected but silent so far — cannot disarm
/// anything. Its empty set means "has not said", and taking it for "holds nothing" would wipe every
/// alert of every core that happened to be reconnecting.
#[test]
fn a_core_that_has_not_reported_disarms_nothing() {
    let mut store = FigureStore::default();
    let id = store.add(1, "BTCUSDT", hline(100.0));
    store.edit(1, "BTCUSDT", id, |f| {
        f.alert = true;
        true
    });

    let (held, none) = sets(&[], &[]);
    assert!(!store.reconcile_local_alerts(&held, &none, 60_000.0));
    assert!(store.figures(1, "BTCUSDT")[0].alert);
}

/// Server-owned figures are NOT this function's business. They are rebuilt wholesale by
/// `set_server_figures`, so one Moonbot dropped is already gone by the time this runs; touching
/// their flag here would fight that.
#[test]
fn a_server_figure_is_left_alone() {
    let mut store = FigureStore::default();
    let mut fig = hline(100.0);
    fig.id = 42;
    fig.alert = true;
    fig.from_server = true;
    store.set_server_figures(HashMap::from([((1, "BTCUSDT".to_string()), vec![fig])]));

    let (held, authoritative) = sets(&[], &[1]);
    assert!(!store.reconcile_local_alerts(&held, &authoritative, 60_000.0));
    assert!(store.figures(1, "BTCUSDT")[0].alert);
}
