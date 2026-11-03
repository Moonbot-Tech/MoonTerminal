// Explicit imports, not `use super::*`: the parent re-exports `gpui::*`, whose `test` shadows the
// built-in attribute (CONTRIBUTING.md).
use std::collections::BTreeSet;

use super::{Label, Row, ViewSection};

/// The English dictionary the captions are looked up in.
const EN: &str = include_str!("../../../../../../locales/en/core_expert.en.yml");

/// Every caption, option and header key the tables name.
fn keys() -> Vec<&'static str> {
    let mut keys = Vec::new();
    for section in ViewSection::ALL {
        for row in section.rows() {
            match *row {
                Row::Flag { key, .. }
                | Row::Step { key, .. }
                | Row::Slide { key, .. }
                | Row::Sound { key, .. }
                | Row::Text { key, .. } => keys.push(key),
                Row::Choice { key, labels, .. } => {
                    keys.push(key);
                    keys.extend(labels.iter().filter_map(|label| match *label {
                        Label::Key(k) => Some(k),
                        Label::Lit(_) => None,
                    }));
                }
            }
        }
    }
    keys
}

/// A key the tables name but the dictionary lacks renders as the raw key in the window, and with a
/// hundred and fifty of them only a sweep finds the one typo.
#[test]
fn every_key_the_tables_name_is_in_the_dictionary() {
    let defined: BTreeSet<&str> = EN
        .lines()
        .filter_map(|line| line.split_once(':').map(|(key, _)| key.trim()))
        .collect();
    let missing: Vec<&str> = keys()
        .into_iter()
        .filter(|key| !defined.contains(key))
        .collect();
    assert!(missing.is_empty(), "missing from locales/en: {missing:?}");
}

/// Two rows with one id would share one control state and one mixed-value probe, so editing either
/// would stage the other's field.
#[test]
fn row_ids_are_unique() {
    let mut seen = BTreeSet::new();
    for section in ViewSection::ALL {
        for row in section.rows() {
            let id = match *row {
                Row::Flag { id, .. }
                | Row::Choice { id, .. }
                | Row::Step { id, .. }
                | Row::Slide { id, .. }
                | Row::Sound { id, .. }
                | Row::Text { id, .. } => id,
            };
            assert!(seen.insert(id), "{id} is declared twice");
        }
    }
}

/// A choice whose labels and values disagree in length would name one value with another's label,
/// or leave a value unreachable.
#[test]
fn every_choice_labels_each_value_once() {
    for section in ViewSection::ALL {
        for row in section.rows() {
            if let Row::Choice {
                id, values, labels, ..
            } = *row
            {
                assert_eq!(values.len(), labels.len(), "{id}");
                let distinct: BTreeSet<i32> = values.iter().copied().collect();
                assert_eq!(distinct.len(), values.len(), "{id} repeats a value");
            }
        }
    }
}
