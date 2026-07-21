// Explicit imports, NOT `use super::*`: the parent re-exports `gpui::*`, which carries its
// own `test` and shadows the built-in attribute — `#[test]` then expands recursively.
use super::assets_columns;

/// Pins `.no_grow()` on the actions column of
/// `panels/assets/table.rs::assets_columns`. The plausible edit: someone adds or reorders a
/// column and rewrites the `vec![]` without carrying the builder call over. The title-less
/// button column would rejoin the auto-width pool, claim a share of every viewport wider than
/// the column sum, and visibly push coin/qty/value apart again — the spread this pins shut.
#[test]
fn the_title_less_actions_column_never_stretches() {
    let columns = assets_columns();
    let actions = columns
        .iter()
        .find(|c| c.key == "actions")
        .expect("the Assets table must keep an actions column");

    assert!(
        actions.no_grow,
        "the actions column holds two fixed-width buttons and no title, so it must stay out \
         of the auto-width pool"
    );
}
