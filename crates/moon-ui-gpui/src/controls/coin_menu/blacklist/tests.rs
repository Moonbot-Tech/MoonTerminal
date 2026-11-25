//! Section ordering for the combined blacklist submenu.

use super::{Section, submenu_plan};

/// Keep temporary presets between permanent targets and the places dialog.
#[test]
fn blacklist_submenu_order_permanent_sep_temp_sep_where() {
    use Section::*;
    assert_eq!(
        submenu_plan(true),
        [Permanent, Sep, Temporary, Sep, WhereListed]
    );
    assert_eq!(submenu_plan(false), [Permanent, Sep, WhereListed]);
}
