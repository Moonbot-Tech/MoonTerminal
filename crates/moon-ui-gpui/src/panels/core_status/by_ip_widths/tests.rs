// Explicit imports, NOT `use super::*`: the parent re-exports `gpui::*`, whose own `test` shadows
// the built-in attribute and makes `#[test]` expand recursively.
use std::collections::HashMap;

use crate::panels::core_status::by_ip_widths::{ByIpWidths, MAX_COL_W, MIN_COL_W};

/// Right-edge reserve of a real row: the status dot plus the tree's overlay scrollbar.
const RESERVED: f32 = 5.0 + 16.0;
/// Rem size at the shipped defaults (`12 * scale + ui_font_delta`).
const REM: f32 = 14.0;

/// An empty user-width bag: the panel as shipped, before anyone has dragged a column.
///
/// A function rather than a `const` because `HashMap::new` is not usable in a const item here.
fn none() -> HashMap<String, f32> {
    HashMap::new()
}

/// An unmeasured view keeps its design widths.
///
/// The plausible edit: dropping the `available <= 0.0` guard. The first frame reports no width, so
/// every column would collapse to the floor and visibly snap back one frame later.
#[test]
fn an_unmeasured_row_keeps_the_base_widths() {
    assert_eq!(
        ByIpWidths::for_width(0.0, RESERVED, REM, &none()),
        ByIpWidths::BASE
    );
    assert_eq!(
        ByIpWidths::for_width(-1.0, RESERVED, REM, &none()),
        ByIpWidths::BASE
    );
    assert_eq!(
        ByIpWidths::for_width(f32::NAN, RESERVED, REM, &none()),
        ByIpWidths::BASE
    );
}

/// Shrinking starts exactly where the row stops fitting — not earlier, not later.
///
/// This is the assertion the earlier version of these tests lacked: "does it shrink at all" passes
/// with ANY chrome budget, including a wildly wrong one, and the whole point of the budget is WHERE
/// the crossover sits. A row one pixel wider than the requirement keeps its design widths; one
/// pixel narrower already shrinks.
#[test]
fn shrinking_starts_exactly_where_the_row_stops_fitting() {
    let full = ByIpWidths::full_width(RESERVED, REM, &none());
    assert_eq!(
        ByIpWidths::for_width(full + 1.0, RESERVED, REM, &none()),
        ByIpWidths::BASE
    );
    assert!(
        ByIpWidths::for_width(full - 1.0, RESERVED, REM, &none()).name < ByIpWidths::BASE.name,
        "one pixel below the requirement must already shrink"
    );
}

/// The right-edge reserve is really subtracted: a bigger reserve makes the row shrink sooner.
///
/// The plausible edit: dropping `- reserved` (or the scrollbar half of it). Nothing else in these
/// tests would notice, and the status dot would end up under the tree's scrollbar again.
#[test]
fn a_larger_right_edge_reserve_shrinks_the_row_sooner() {
    let full = ByIpWidths::full_width(RESERVED, REM, &none());
    assert_eq!(
        ByIpWidths::for_width(full, RESERVED, REM, &none()),
        ByIpWidths::BASE,
        "this width fits with the small reserve"
    );
    assert!(
        ByIpWidths::for_width(full, RESERVED + 40.0, REM, &none()).name < ByIpWidths::BASE.name,
        "the same width must not fit once more is reserved"
    );
}

/// The insets follow the rem size, so a larger UI font makes the row shrink sooner.
///
/// The plausible edit: treating the inset as a fixed pixel value again. It reads as correct at the
/// default font and silently overflows once the "Шрифт" slider moves.
#[test]
fn a_larger_rem_shrinks_the_row_sooner() {
    let full = ByIpWidths::full_width(RESERVED, REM, &none());
    assert!(
        ByIpWidths::for_width(full, RESERVED, REM + 4.0, &none()).name < ByIpWidths::BASE.name,
        "a wider inset must eat into the same available width"
    );
}

/// A wide row is not stretched: the layout's own spacer absorbs the surplus.
#[test]
fn a_wide_row_keeps_the_base_widths() {
    assert_eq!(
        ByIpWidths::for_width(4000.0, RESERVED, REM, &none()),
        ByIpWidths::BASE
    );
}

/// A narrow row shrinks every column by ONE factor, so the header stays aligned with the values.
///
/// The oracle is the ratio between two columns, which a per-column collapse (dropping IP first, for
/// example) would not preserve.
#[test]
fn a_narrow_row_shrinks_every_column_by_the_same_factor() {
    let base = ByIpWidths::BASE;
    let narrow = ByIpWidths::for_width(500.0, RESERVED, REM, &none());

    assert!(narrow.name < base.name, "the name column must shrink");
    let k = narrow.name / base.name;
    for (got, want) in [
        (narrow.ip, base.ip),
        (narrow.cpu, base.cpu),
        (narrow.mem, base.mem),
        (narrow.ping, base.ping),
        (narrow.api, base.api),
        (narrow.cores, base.cores),
        (narrow.startup, base.startup),
        (narrow.icon, base.icon),
        (narrow.indent, base.indent),
    ] {
        assert!(
            (got - want * k).abs() < 0.01,
            "every column scales by the same factor: {got} != {want} * {k}"
        );
    }
}

/// The shrink stops at the floor instead of collapsing the columns to nothing.
#[test]
fn an_extremely_narrow_row_stops_at_the_floor() {
    let floored = ByIpWidths::for_width(60.0, RESERVED, REM, &none());
    assert!(
        (floored.name - ByIpWidths::BASE.name * 0.5).abs() < 0.01,
        "below the floor the row clips instead of shrinking further"
    );
}

/// `by_ip_widths.rs:ByIpWidths::resolved` and `for_width` must preserve valid dragged widths.
///
/// The plausible edit replaces `Self::resolved(user)` with `Self::BASE` in `for_width`; a user's
/// width would then disappear as soon as the row paints, so the divider snaps back and the header
/// loses its intended proportional layout on a narrow dock.
#[test]
fn persisted_widths_are_sanitized_and_drive_the_shared_shrink_factor() {
    let mut stored = HashMap::from([
        ("name".to_owned(), ByIpWidths::BASE.name + 50.0),
        ("ip".to_owned(), MIN_COL_W - 1.0),
        ("cpu".to_owned(), MAX_COL_W + 1.0),
        ("mem".to_owned(), f32::NAN),
        ("ping".to_owned(), f32::INFINITY),
        ("obsolete-column".to_owned(), 123.0),
    ]);

    let resolved = ByIpWidths::resolved(&stored);
    assert_eq!(resolved.name, ByIpWidths::BASE.name + 50.0);
    assert_eq!(resolved.ip, MIN_COL_W);
    assert_eq!(resolved.cpu, MAX_COL_W);
    assert_eq!(resolved.mem, ByIpWidths::BASE.mem);
    assert_eq!(resolved.ping, ByIpWidths::BASE.ping);
    assert_eq!(resolved.exch, ByIpWidths::BASE.exch);

    let dragged = HashMap::from([("name".to_owned(), ByIpWidths::BASE.name + 50.0)]);
    let base_full = ByIpWidths::full_width(RESERVED, REM, &none());
    let overridden_full = ByIpWidths::full_width(RESERVED, REM, &dragged);
    assert_eq!(overridden_full - base_full, 50.0);
    assert_eq!(
        ByIpWidths::for_width(overridden_full, RESERVED, REM, &dragged),
        ByIpWidths::resolved(&dragged),
        "a row that fits must leave the resolved widths unscaled"
    );

    let narrow = ByIpWidths::for_width(overridden_full - 100.0, RESERVED, REM, &dragged);
    let dragged_resolved = ByIpWidths::resolved(&dragged);
    assert!(
        narrow.name < dragged_resolved.name,
        "the chosen row must require shrinking"
    );
    assert!(
        ((narrow.name / narrow.ip) - (dragged_resolved.name / dragged_resolved.ip)).abs() < 0.01,
        "the user-selected ratio must survive the shared shrink factor"
    );

    stored.insert("name".to_owned(), 9999.0);
    assert_eq!(ByIpWidths::resolved(&stored).name, MAX_COL_W);
}

/// `by_ip_widths.rs:ByIpWidths::columns_w` must count `ping` and `exch` independently.
///
/// The plausible edit reuses `ping` for the exchange term; dragging one latency header would then
/// silently change both widths and the row would reserve the wrong amount of horizontal space.
#[test]
fn ping_and_exchange_widths_remain_independent_in_the_row_total() {
    let user = HashMap::from([("ping".to_owned(), 73.0), ("exch".to_owned(), 149.0)]);
    let resolved = ByIpWidths::resolved(&user);
    assert_eq!(resolved.ping, 73.0);
    assert_eq!(resolved.exch, 149.0);

    let base_full = ByIpWidths::full_width(RESERVED, REM, &none());
    let overridden_full = ByIpWidths::full_width(RESERVED, REM, &user);
    let expected_delta = (73.0 - ByIpWidths::BASE.ping) + (149.0 - ByIpWidths::BASE.exch);
    assert_eq!(overridden_full - base_full, expected_delta);
}
