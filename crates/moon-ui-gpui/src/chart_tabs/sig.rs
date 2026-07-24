//! Chart-tab input signature (`chart_tabs_sig`), a cheap hash of the `Backend` state that can
//! actually change this group's tab strip or stack. `ChartTabs` compares it in its backend observer
//! and skips the expensive pass when nothing changed.

use crate::Backend;

/// Hash Backend inputs that can change one group's chart-tab composition or commands.
///
/// Args:
///     b: Shared runtime state observed by `ChartTabs`.
///     group: Main window group whose relevant inputs are hashed.
///
/// Returns:
///     A cheap deterministic signature for early-return comparisons.
pub(super) fn chart_tabs_sig(b: &Backend, group: &str) -> u64 {
    let mut sig = if b
        .open_request
        .as_ref()
        .is_some_and(|(core, _)| b.core_belongs_to_group(group, *core))
    {
        b.open_request_rev
    } else {
        0
    };
    if b.open_compare_request
        .as_ref()
        .is_some_and(|(core, _)| b.core_belongs_to_group(group, *core))
    {
        sig = sig
            .wrapping_mul(31)
            .wrapping_add(b.open_compare_request_rev);
    }
    sig = sig
        .wrapping_mul(31)
        .wrapping_add(u64::from(b.config.charts_split_by_core));
    if b.price_scale_group.as_deref() == Some(group) {
        sig = sig.wrapping_mul(31).wrapping_add(b.price_scale_rev);
    }
    if b.switch_charts_group.as_deref() == Some(group) {
        sig = sig.wrapping_mul(31).wrapping_add(b.switch_charts_rev);
    }
    // This revision is global rather than group-addressed because Shift+Esc closes every Main stack.
    sig = sig.wrapping_mul(31).wrapping_add(b.close_all_charts_rev);
    if b.close_active_chart_group.as_deref() == Some(group) {
        sig = sig.wrapping_mul(31).wrapping_add(b.close_active_chart_rev);
    }
    for (g, n, bucket) in &b.chart_repin_request {
        if g == group {
            sig = sig
                .wrapping_mul(31)
                .wrapping_add(*n as u64)
                .wrapping_mul(31)
                .wrapping_add(text_sig(&format!("{bucket:?}")));
        }
    }
    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    if b.debug_fill_main_chart_group.as_deref() == Some(group) {
        sig = sig
            .wrapping_mul(31)
            .wrapping_add(b.debug_fill_main_chart_rev);
    }
    let store = b.session.store();
    for s in b.session.sessions().iter().filter(|s| s.group == group) {
        // Include the core ID itself, not only `detects_rev`: adding or removing a group core must
        // change the signature so `ChartTabs::ingest` recomposes tabs even before a new core has
        // detects (`detects_rev=0`). The previous composition only affected it indirectly through
        // `*31`, which was fragile.
        sig = sig.wrapping_mul(31).wrapping_add(s.id);
        if let Some(d) = store.core(s.id) {
            sig = sig.wrapping_mul(31).wrapping_add(d.detects_rev);
        }
    }
    sig
}

fn text_sig(text: &str) -> u64 {
    let mut sig = 0xcbf29ce484222325u64;
    for byte in text.bytes() {
        sig ^= byte as u64;
        sig = sig.wrapping_mul(0x100000001b3);
    }
    sig
}
