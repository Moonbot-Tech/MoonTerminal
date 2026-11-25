//! Localized blacklist state captions and their count semantics.

use super::{BlState, HeldBan, bl_right_label, fmt_ban_coarse, permanent_count};
use crate::controls::coin_menu::places::CoinPlace;

const MIN: u64 = 60_000;

/// No permanent entry and no ban leaves the parent row's trailing label empty.
#[test]
fn bl_label_listed_nowhere_is_none() {
    assert_eq!(
        bl_right_label(&BlState {
            listed: 0,
            ban: None
        }),
        None
    );
}

/// A count alone does not introduce a temporary-ban caption.
#[test]
fn bl_label_in_n() {
    let _locale = crate::test_locale::force("ru");
    assert_eq!(
        bl_right_label(&BlState {
            listed: 3,
            ban: None
        })
        .as_deref(),
        Some("в 3")
    );
}

/// The approved compact label floors 2 h 14 min to 2 h.
#[test]
fn bl_label_in_1_with_ban_2h() {
    let _locale = crate::test_locale::force("ru");
    assert_eq!(
        bl_right_label(&BlState {
            listed: 1,
            ban: Some((134 * MIN, false))
        })
        .as_deref(),
        Some("в 1 · бан 2 ч")
    );
}

/// The compact label marks a countdown extrapolated from an offline core.
#[test]
fn bl_label_stale_ban() {
    let _locale = crate::test_locale::force("ru");
    assert_eq!(
        bl_right_label(&BlState {
            listed: 1,
            ban: Some((134 * MIN, true))
        })
        .as_deref(),
        Some("в 1 · бан ~2 ч")
    );
}

/// No separator or zero count is printed before a ban-only caption.
#[test]
fn bl_label_ban_only() {
    let _locale = crate::test_locale::force("ru");
    assert_eq!(
        bl_right_label(&BlState {
            listed: 0,
            ban: Some((134 * MIN, false))
        })
        .as_deref(),
        Some("бан 2 ч")
    );
}

/// Both types of permanent place count, while a temporary row does not.
#[test]
fn bl_label_ignores_temp_ban_in_count() {
    let places = [
        CoinPlace::Core {
            core: 1,
            list_enabled: false,
        },
        CoinPlace::Temp {
            core: 1,
            market: "XALPHAUSDT".to_string(),
            until_ms: 33,
        },
        CoinPlace::Strategy {
            core: 1,
            sid: 4,
            folder: String::new(),
            strategy: "UNIT_01".to_string(),
        },
    ];
    assert_eq!(permanent_count(&places), 2);
}

/// The coarse formatter keeps only the largest unit and floors within that unit.
#[test]
fn fmt_ban_coarse_units() {
    let _locale = crate::test_locale::force("ru");
    assert_eq!(fmt_ban_coarse(3 * 1440 * MIN + 13 * MIN), "3 дн");
    assert_eq!(fmt_ban_coarse(134 * MIN), "2 ч");
    assert_eq!(fmt_ban_coarse(14 * MIN + 59_999), "14 мин");
}

/// A lift on another selected core has no borrowed countdown.
#[test]
fn ban_on_other_core_only_lift_label() {
    let _locale = crate::test_locale::force("ru");
    let held = HeldBan {
        cores: vec![2],
        here: None,
    };
    assert_eq!(held.lift_label().as_deref(), Some("Снять бан"));
    assert_eq!(HeldBan::default().lift_label(), None);
}
