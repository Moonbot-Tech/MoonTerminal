//! The ticks and В1 decide, per kind whose entry the search varies, whether its deals owe their
//! entry order's life.

use std::collections::HashSet;

use moon_core::db::tuner::ticks::drops::KIND_DROPS;
use moon_core::db::tuner::ticks::entry::KIND_MOONSHOT;
use moon_core::db::tuner::ticks::params::{ParamGroup, params_for};

use super::{ALL, required_for, wanted};

/// The entry fields a kind's grid shows and the current entry method reads.
fn read_entry_fields(kind: &str) -> Vec<&'static str> {
    let method = super::super::model_cfg::current().entry_method;
    params_for(ParamGroup::Entry, kind)
        .filter(|p| method.reads(p.key))
        .map(|p| p.key)
        .collect()
}

/// Unticking every shown MoonShot entry field frees MoonShot deals from their order's life —
/// whatever fields the grid never shows say, the shape that kept the entry owed while every
/// visible tick was off — and ticking one back owes it again.
#[test]
fn unticking_the_shown_entry_fields_frees_moonshot() {
    assert_eq!(
        wanted(&HashSet::new(), []),
        ALL,
        "everything ticked owes the entry"
    );
    let shown = read_entry_fields(KIND_MOONSHOT);
    assert!(shown.len() >= 2, "MoonShot shows entry fields");
    let unticked: HashSet<String> = shown.iter().map(|k| k.to_string()).collect();
    assert_eq!(
        wanted(&unticked, []),
        0,
        "MoonShot no longer owes the entry"
    );
    let mut one_back = unticked.clone();
    one_back.remove(shown[0]);
    assert_eq!(wanted(&one_back, []), ALL, "one ticked entry field owes it");
}

/// An unticked entry field is held at В1's value: a typed one moves the entry as a search
/// would, so it owes the order's life too. An exit value in В1 does not.
#[test]
fn a_typed_entry_value_owes_the_entry_order_its_life() {
    let unticked: HashSet<String> = read_entry_fields(KIND_MOONSHOT)
        .into_iter()
        .map(str::to_string)
        .collect();
    let exit = params_for(ParamGroup::Exit, KIND_MOONSHOT)
        .next()
        .expect("an exit field")
        .key;
    assert_eq!(wanted(&unticked, [exit]), 0, "an exit value owes no entry");
    let entry = read_entry_fields(KIND_MOONSHOT)[0];
    assert_eq!(
        wanted(&unticked, [entry]),
        ALL,
        "an entry value owes the entry"
    );
}

/// A kind the search never varies the entry of keeps the full requirement whatever the bits:
/// DropsDetection has an entry model of its own but no entry field to untick.
#[test]
fn a_kind_without_entry_fields_keeps_the_full_requirement() {
    assert!(read_entry_fields(KIND_DROPS).is_empty());
    assert!(required_for(KIND_DROPS));
}
