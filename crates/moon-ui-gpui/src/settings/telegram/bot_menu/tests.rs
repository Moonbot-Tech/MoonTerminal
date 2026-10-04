use super::{rows_by_id, shape_sig, tree_items};
use moon_core::config::telegram_menu::{BotMenu, MenuItem};

/// The tree's shape follows the button order only: showing or starting a row reads from the draft
/// at render and pushes nothing.
#[test]
fn the_shape_moves_with_the_order_only() {
    let mut menu = BotMenu::default();
    let shape = shape_sig(&menu);
    assert!(menu.set_shown(MenuItem::Report, true));
    assert!(menu.set_row_start(MenuItem::Yesterday, true));
    assert_eq!(shape_sig(&menu), shape);
    assert!(menu.move_item(MenuItem::Month, true));
    assert_ne!(shape_sig(&menu), shape);
}

/// Every keyboard button is a row under the one root, numbered by the row it sits in.
#[test]
fn rows_number_buttons_by_their_row() {
    let menu = BotMenu::default();
    let rows = rows_by_id(&menu);
    assert_eq!(rows.len(), MenuItem::ALL.len());
    let today = rows["kb:today"];
    assert!(today.first && today.starts_row && today.row == 1);
    let month = rows["kb:month"];
    assert!(month.starts_row && month.row == 2 && !month.first && !month.last);
    assert!(rows["kb:custom"].last);
    let items = tree_items(&menu);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].children.len(), MenuItem::ALL.len());
    assert!(
        items[0]
            .children
            .iter()
            .all(|c| rows.contains_key(c.id.as_ref()))
    );
}
