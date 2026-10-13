//! What the Summary's "Reset filters" changes.
//!
//! Explicit imports, never `use super::*`: the parent glob-imports `gpui::*`, whose own `test`
//! would shadow the built-in `#[test]` attribute.

use super::{SummaryResetPlan, summary_reset_plan};
use moon_core::db::SideFilter;

/// Every filter off its default is reset once; defaults stay untouched and an all-default
/// state changes nothing, so no read starts; a pinned core filter is kept.
#[test]
fn reset_plan_clears_set_filters_and_keeps_the_pin() {
    let all = summary_reset_plan(SideFilter::Long, Some(true), "XALPHA", false, false);
    assert_eq!(
        all,
        SummaryResetPlan {
            side: true,
            emu: true,
            mask: true,
            cores: true
        }
    );
    assert!(all.any());
    let none = summary_reset_plan(SideFilter::All, None, "", true, false);
    assert_eq!(none, SummaryResetPlan::default());
    assert!(!none.any(), "nothing to reset must not start a read");
    let pinned = summary_reset_plan(SideFilter::All, None, "", false, true);
    assert!(
        !pinned.cores && !pinned.any(),
        "a pinned core filter is kept"
    );
    let mask_only = summary_reset_plan(SideFilter::All, None, "XALPHA", true, false);
    assert!(mask_only.mask && !mask_only.side && !mask_only.emu && !mask_only.cores);
}
