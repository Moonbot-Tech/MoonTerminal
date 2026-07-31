// Explicit imports, NOT `use super::*`: the parent re-exports `gpui::*`, which carries its
// own `test` and shadows the built-in attribute — `#[test]` then expands recursively.
use super::assets_columns;
use crate::panels::assets::columns::AssetCol;

/// Pins `.no_grow()` on the actions column of
/// `panels/assets/table.rs::assets_columns`. The plausible edit: someone adds or reorders a
/// column and rewrites the builder without carrying the call over. The title-less
/// button column would rejoin the auto-width pool, claim a share of every viewport wider than
/// the column sum, and visibly push coin/qty/value apart again — the spread this pins shut.
#[test]
fn the_title_less_actions_column_never_stretches() {
    let columns = assets_columns(&AssetCol::ALL);
    let actions = columns
        .iter()
        .find(|c| c.key == "actions")
        .expect("the Assets table must offer an actions column");

    assert!(
        actions.no_grow,
        "the actions column holds two fixed-width buttons and no title, so it must stay out \
         of the auto-width pool"
    );
    assert!(
        actions.title.is_empty(),
        "the button column carries no header caption; its name exists only in the field selector"
    );
}

/// The table renders exactly the chosen fields, in canonical order, with nothing appended.
///
/// The plausible edit: restoring the old unconditional `push` of the action column after it became
/// a selectable field. The row builder emits one cell per VISIBLE field, so an extra column would
/// leave the table with more columns than cells.
#[test]
fn the_columns_follow_the_selected_fields_exactly() {
    let columns = assets_columns(&[AssetCol::Coin, AssetCol::Pnl]);
    let keys: Vec<&str> = columns.iter().map(|c| c.key.as_ref()).collect();
    assert_eq!(keys, vec!["coin", "pnl"]);
}
