use moon_core::config::{GestureSlot, KeySlot};

use super::*;

/// Pins the surfaces claimed for the rows whose routing is easiest to get wrong.
///
/// Panic Sell is the one worth naming out loud, because the obvious reading of it is wrong: it does
/// NOT simply act on the focused window's chart. `shell/actions.rs::select_hotkey_target` prefers
/// the chart under the POINTER whenever that chart belongs to the same window group, and only falls
/// back to the window's main chart — so the press can hit a market the window is not showing.
///
/// Plausible breakage: `select_hotkey_target` losing its hover branch, or a new trading key added
/// with the window-only mark, either of which makes the page state the wrong market.
#[test]
fn the_aimed_trading_keys_claim_the_pointer_as_well_as_the_window() {
    let aimed = [
        KeySlot::PanicSell,
        KeySlot::PanicSellOne,
        KeySlot::CancelBuy,
        KeySlot::JoinSells,
        KeySlot::ShiftBuyUp,
        // Both split slots resolve to the same action, so they must not disagree.
        KeySlot::SplitOrder,
        KeySlot::SplitOrderX,
    ];
    for slot in aimed {
        let scope = key_slot_meta(slot).scope;
        assert!(
            scope.intersects(Scope::CURSOR) && scope.intersects(Scope::WINDOW),
            "{} no longer claims both the pointer and the window",
            slot.stem()
        );
    }
    // The group-owned keys are the contrast: no pointer is consulted for them at all.
    for slot in [
        KeySlot::OrderSize(0),
        KeySlot::SwitchCharts,
        KeySlot::ScalePlus,
        KeySlot::CancelAllBuys,
    ] {
        assert_eq!(key_slot_meta(slot).scope, Scope::WINDOW, "{}", slot.stem());
    }
}

/// A key press with nothing selected does not stop at the figure layer — it cancels the order under
/// the pointer — so the row has to admit that second, destructive surface.
#[test]
fn the_figure_delete_key_admits_its_order_cancelling_fallback() {
    let scope = key_slot_meta(KeySlot::FigDelete).scope;
    assert!(scope.intersects(Scope::SELECTION));
    assert!(scope.intersects(Scope::CURSOR));
    // The alert toggle has no such fallback: with nothing selected the key is simply unhandled.
    assert_eq!(key_slot_meta(KeySlot::FigAlert).scope, Scope::SELECTION);
}

/// A slot whose surface depends on the "separate control zones" setting holds BOTH surfaces, so an
/// overlap test cannot miss the half that is currently switched off.
#[test]
fn a_trading_gesture_holds_both_of_its_possible_surfaces() {
    for scope in [
        gesture_slot_meta(GestureSlot::BuySet).scope,
        gesture_slot_meta(GestureSlot::BuyMove).scope,
        // The manual order keys are placed through the same gate, so they answer the same way.
        key_slot_meta(KeySlot::NewLong).scope,
    ] {
        assert!(scope.intersects(Scope::BOOK));
        assert!(scope.intersects(Scope::PLOT));
    }
}

/// Both surfaces reach the label, and the label is real text rather than a missing-key echo.
///
/// The echo matters: rust-i18n answers an unknown key with `"<locale>.<key>"`, which still contains
/// a slash and still contains each part, so a laxer assertion here would pass with every new locale
/// entry deleted.
#[test]
fn a_two_surface_label_names_both_surfaces_in_words() {
    let _locale = crate::test_locale::force("en");
    assert_eq!(Scope::BOOK.label(), "book");
    assert_eq!(Scope::PLOT.label(), "plot");
    assert_eq!(
        gesture_slot_meta(GestureSlot::BuySet).scope.label(),
        "book / plot"
    );
    assert_eq!(Origin::Shared.label(), "MB");
    assert_eq!(Origin::Local.label(), "MT");
}

/// Joining two slots' surfaces reads the containment the constants document, so a two-editor row
/// never names the same place twice.
#[test]
fn a_join_absorbs_the_narrower_pointer_places() {
    let key = Scope::SELECTION.or(Scope::CURSOR);
    let gesture = Scope::FIGURE;
    assert_eq!(key.join(gesture), Scope::SELECTION.or(Scope::CURSOR));
    // Without the cursor among them the narrower places stand on their own.
    assert_eq!(
        Scope::BOOK.join(Scope::PLOT),
        Scope::BOOK.or(Scope::PLOT),
        "one slot's two alternatives are not the same place"
    );
    assert_eq!(Scope::WINDOW.join(Scope::APP), Scope::APP);
}
