use moon_remote::core_cmds::Located;

use super::{MenuForm, menu_form};

/// Build a located core with only its installation ownership varied.
fn located(ours: bool) -> Located {
    Located {
        dir: "/root/moonbot-a".into(),
        unit: "UNIT_01.service".into(),
        ours,
        has_mb: true,
        active: None,
    }
}

/// Reordering these branches could offer reconfiguration before identity or login is known.
#[test]
fn menu_form_covers_login_checking_ours_and_hand() {
    assert_eq!(menu_form(false, None), MenuForm::NoLogin);
    assert_eq!(menu_form(false, Some(&located(true))), MenuForm::NoLogin);
    assert_eq!(menu_form(true, None), MenuForm::Checking);
    assert_eq!(menu_form(true, Some(&located(true))), MenuForm::Ours);
    assert_eq!(menu_form(true, Some(&located(false))), MenuForm::Hand);
}
