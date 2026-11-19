//! The auto-line hover card's modifier gate.
//!
//! Explicit imports, never `use super::*`: the parent pulls in `gpui::*`, whose own `test` would
//! shadow the built-in `#[test]` attribute (CONTRIBUTING.md).

use moon_chart::auto_figures::{Found, FoundShape};

use super::{AutoHover, AutoHoverState};
use crate::chartdx::auto_figures_sync::LineKey;

/// The hover of a level at `price` on pane 0.
fn hover(price: f64) -> AutoHover {
    let found = Found {
        shape: FoundShape::Horizontal { price },
        touches: Vec::new(),
        strength: 1.0,
        first_ms: 0.0,
        last_ms: 0.0,
        broken: None,
        touch_volume: 0.0,
        tolerance_frac: 0.0,
        tag: None,
    };
    AutoHover {
        pane: 0,
        key: LineKey::of(0, &found),
    }
}

/// Without the modifier the pointer crossing line after line repaints nothing and shows no card;
/// with it held, every change of the hovered line repaints and the card names the hovered line.
///
/// By hand: a storm across two lines and the gaps between them is five changes — line 100, none,
/// line 105, none, line 100 again — plus a sixth event on the same line, which is no change. Not
/// held: five changes, zero repaints, no card at any point. Held: five repaints, and the card is
/// the line under the pointer.
///
/// Breakage: the card shown on a bare hover again (LinKvo, 19.11.2026: only while the news
/// card's modifier is held), or every crossing notifying GPUI — the +46 `chart_render`/s of a
/// FireTest mouse storm across the auto lines.
#[test]
fn the_card_needs_the_modifier_and_a_bare_storm_repaints_nothing() {
    let storm = [
        Some(hover(100.0)),
        None,
        Some(hover(105.0)),
        None,
        Some(hover(100.0)),
        Some(hover(100.0)),
    ];
    let mut state = AutoHoverState::default();
    let mut changes = 0;
    for hit in storm {
        if let Some(repaint) = state.apply(hit, false) {
            changes += 1;
            assert!(!repaint);
        }
        assert_eq!(state.card(false), None);
    }
    assert_eq!(changes, 5);
    // Still tracked without the modifier: the `OnHover` caption and a later Ctrl press use it.
    assert!(state.is_hovered());

    let mut state = AutoHoverState::default();
    let mut repaints = 0;
    for hit in storm {
        if state.apply(hit, true) == Some(true) {
            repaints += 1;
        }
        assert_eq!(state.card(true), hit);
    }
    assert_eq!(repaints, 5);
    // The modifier released over the line: no card, though the line is still hovered.
    assert_eq!(state.card(false), None);
    assert!(state.is_hovered());
}
