//! Narrow records must retain both sides and every monetary digit, including near zero.

use gpui::{Styled, WhiteSpace, px, rgb};
use moon_core::crowd::Standing;
use moon_core::util::fmt::DeltaSign;

use super::{minute_sides, money, wrapping_line};

/// Compact formatting would lose cents on large figures; truncation would lose magnitude.
#[test]
fn full_money_retains_large_amounts_and_rounds_before_sign() {
    assert_eq!(
        money(1_234_567.89),
        ("+1234567.89".into(), DeltaSign::Positive)
    );
    assert_eq!(money(-98_765.43), ("-98765.43".into(), DeltaSign::Negative));
    assert_eq!(money(-0.001), ("0.00".into(), DeltaSign::Zero));
    assert_eq!(money(f64::NAN), ("—".into(), DeltaSign::Zero));
}

/// A noisy minute cannot become a net figure, and an untraded side cannot acquire a value.
#[test]
fn minute_records_keep_both_amounts_and_their_own_counts() {
    let row = Standing {
        plus: 9000.25,
        minus: 8000.75,
        trades_plus: 23,
        trades_minus: 17,
        ..Standing::default()
    };
    assert_eq!(
        minute_sides(&row).collect::<Vec<_>>(),
        vec![
            ("crowd.col.profit", 9000.25, 23),
            ("crowd.col.loss", -8000.75, 17),
        ]
    );
    let one_side = Standing {
        trades_plus: 0,
        ..row
    };
    assert_eq!(
        minute_sides(&one_side).collect::<Vec<_>>(),
        vec![("crowd.col.loss", -8000.75, 17)]
    );
}

/// The value must get its natural multiline height rather than a table row's fixed height.
#[test]
fn narrow_values_wrap_without_a_height_or_ellipsis() {
    let mut value = wrapping_line("+123456789.01".into(), rgb(0xffffff).into(), px(12.0));
    let style = value.style();
    assert_eq!(style.size.height, None);
    assert_eq!(style.flex_shrink, Some(0.0));
    let text = &style.text;
    assert_eq!(text.white_space, Some(WhiteSpace::Normal));
    assert_eq!(text.text_overflow, None);
}
