//! The ONE ⧉ "apply to all" mechanism, shared by every chart popup that has the button.
//!
//! Each popup used to carry its own copy of the same walk — apply to Main, apply to every Add,
//! Custom and detached stack, write each one's `charts.json` spec — differing only in WHICH values
//! travelled. The values are already expressible as [`StackSetting`], which knows both how to reach
//! a stack and how to write itself to a spec, so a ⧉ press is fully described by [`ApplyAll`]: the
//! values, the optional X scale that rides along with the candle popup's, and the optional global
//! default in `layout.toml` that the press also overwrites.
//!
//! Detached windows cannot reach the group's stacks, so their ⧉ sends the same [`ApplyAll`] through
//! Backend as [`ApplyAllRequest`] and the tab strip drains it (`settings::drain_apply_all`).
//!
//! The Main tab is the only target whose treatment depends on the press: see [`plan_main`], which is
//! a plain function precisely so the rule can be tested without a GPUI app.

use gpui::*;

use super::common::{LayoutPopupSnapshot, StackSetting, set_stack_setting};
use crate::persistence::chart_persist::StackOrientation;
use moon_core::config::ChartBucket;

/// One ⧉ press: what to copy, and what else it touches.
///
/// A value that has a global default in `layout.toml` carries that fact itself through
/// [`StackSetting::global_slot`], so a press cannot name a default and a value that disagree.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ApplyAll {
    /// Values copied to every target, applied in this order.
    pub values: Vec<StackSetting>,
    /// Window X scale copied along with the values, and stored per group for new charts. Only the
    /// candle popup sends one; `None` leaves every target's scale alone.
    pub x_ppm: Option<f32>,
}

/// A detached window's ⧉ press, queued through Backend for its group's tab strip to drain.
///
/// Detached hosts own one panel and no group stacks, so they cannot walk the targets themselves.
/// A queued press never includes Main: a detached source leaves it unchanged, like ⚙.
pub(crate) struct ApplyAllRequest {
    /// Group whose tab strip must perform the walk.
    pub group: String,
    pub apply: ApplyAll,
}

/// What a ⧉ press does to the MAIN tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MainAction {
    /// Copy the pressed values into Main: the press came from Main's own popup.
    Copy,
    /// Pin Main to the global default it is currently following, BEFORE that default is overwritten.
    ///
    /// Without this, a press from an Add tab would silently change Main as well — Main reads the
    /// default live, so overwriting it is indistinguishable from editing Main.
    Pin,
    /// Leave Main alone: either nothing global changes, or Main has its own override and the
    /// default cannot reach it.
    Leave,
}

/// Decide what a ⧉ press does to Main.
///
/// Args:
///     include_main: Whether Main is the SOURCE of the press (its own popup is open).
///     has_global: Whether the pressed setting has a global default that this press overwrites.
///     main_has_override: Whether Main already holds its own value for that setting.
///
/// Returns:
///     The action to perform on Main.
pub(crate) fn plan_main(
    include_main: bool,
    has_global: bool,
    main_has_override: bool,
) -> MainAction {
    if include_main {
        return MainAction::Copy;
    }
    if has_global && !main_has_override {
        return MainAction::Pin;
    }
    MainAction::Leave
}

impl super::ChartTabs {
    /// Perform one ⧉ press: copy `apply` to every stack in the group and persist each tab's spec.
    ///
    /// "Every stack" is Main (per [`plan_main`]), the strip's Add tabs, Custom tabs, and the group's
    /// detached windows. `include_main` is `true` only when the press came from Main's own popup.
    pub(super) fn apply_all(
        &mut self,
        apply: ApplyAll,
        include_main: bool,
        cx: &mut Context<Self>,
    ) {
        // The value of this press that has a global default, if any. It decides both the Main rule
        // and the default write below.
        let global = apply
            .values
            .iter()
            .copied()
            .find_map(|v| v.global_slot().map(|slot| (slot, v)));
        // Read Main only when the answer can matter: a press FROM Main resolves to Copy regardless.
        let main_has_override = !include_main
            && global.is_some_and(|(slot, _)| slot.main_has_override(self.main.read(cx)));
        match plan_main(include_main, global.is_some(), main_has_override) {
            MainAction::Copy => self.apply_all_to_main(&apply.values, apply.x_ppm, cx),
            MainAction::Pin => {
                // The value Main is following right now, read BEFORE the default is overwritten
                // below. The X scale is deliberately not pinned: it has no global default to lose.
                if let Some((slot, _)) = global {
                    let pinned = slot.read(&self.backend.read(cx).layout);
                    self.apply_all_to_main(&[pinned], None, cx);
                }
            }
            MainAction::Leave => {}
        }
        let targets: Vec<(u32, ChartBucket, Entity<super::AddChartStack>)> = self
            .add
            .iter()
            .chain(self.custom.iter())
            .chain(self.detached.iter())
            .map(|(n, b, p)| (*n, b.clone(), p.clone()))
            .collect();
        let x_ppm = apply.x_ppm;
        for (num, bucket, stack) in targets {
            stack.update(cx, |s, c| {
                for v in &apply.values {
                    set_stack_setting!(s, c, *v);
                }
                if x_ppm.is_some() {
                    s.set_x_ppm(x_ppm, true, c);
                }
            });
            self.upsert_spec(cx, num, &bucket, |s| {
                for v in &apply.values {
                    v.write_spec(s);
                }
                if x_ppm.is_some() {
                    s.x_ppm = x_ppm;
                }
            });
        }
        let rebuild_orderbook = apply.values.iter().any(|v| v.rebuilds_orderbook_demand());
        let group = x_ppm.map(|_| self.group.clone());
        self.backend.update(cx, |b, bcx| {
            if let Some((slot, value)) = global
                && slot.write(&mut b.layout, value)
            {
                b.layout_dirty = true;
                // Charts in OTHER group windows follow this default and are not walked above; every
                // chart panel observes Backend, and this notification is what makes them re-render
                // and push the new value into their engine. Gated on a REAL change, or a press that
                // stored what was already there would wake every chart in the application.
                bcx.notify();
            }
            if let Some((ppm, group)) = x_ppm.zip(group) {
                // Store the scale for THIS group and every known group window so their new charts
                // inherit it. Live windows of other groups update their own charts separately.
                b.layout.chart_x_ppm_by_group.insert(group, ppm);
                let groups: Vec<String> = b.layout.groups.keys().cloned().collect();
                for g in groups {
                    b.layout.chart_x_ppm_by_group.insert(g, ppm);
                }
                b.layout_dirty = true;
            }
            // Order-book demand can have changed on any of the targets; rebuilt ONCE for the whole
            // walk rather than per target.
            if rebuild_orderbook {
                b.rebuild_orderbook_wanted();
            }
        });
        cx.notify();
    }

    /// Apply one ⧉ walk's values, and its X scale when set, to the Main stack and its spec.
    ///
    /// The X scale reaches Main's charts but NOT Main's spec: `ChartTabSpec::x_ppm` is a detached
    /// window's own scale, and Main's lives in `layout.chart_x_ppm_by_group`, written below. A copy
    /// in Main's spec would never be read back.
    fn apply_all_to_main(
        &mut self,
        values: &[StackSetting],
        x_ppm: Option<f32>,
        cx: &mut Context<Self>,
    ) {
        self.main.update(cx, |s, c| {
            for v in values {
                set_stack_setting!(s, c, *v);
            }
            if x_ppm.is_some() {
                s.set_x_ppm(x_ppm, true, c);
            }
        });
        self.upsert_spec(cx, 0, &ChartBucket::Shared, |s| {
            for v in values {
                v.write_spec(s);
            }
        });
    }
}

/// Build the ⚙ layout popup's ⧉ value set from the source tab's settings.
///
/// The snapshot already carries every resolved layout value the popup shows; the three that are not
/// in it — both mode heights and the price scale — come from the source's fields, because the popup
/// edits the heights in text fields whose uncommitted contents are the source of truth.
///
/// Args:
///     snap: The source tab's resolved layout settings.
///     height_fit: Fit-mode slot extent, or `None` for the default.
///     height_scroll: Scroll-mode slot extent, or `None` for the default.
///     scale: Price scale, or `None` for Auto.
///     orientation: Source orientation, unresolved: `None` copies "no orientation named".
///
/// Returns:
///     The values a ⧉ press on the layout popup copies, in application order.
pub(super) fn layout_values(
    snap: &LayoutPopupSnapshot,
    height_fit: Option<u16>,
    height_scroll: Option<u16>,
    scale: Option<f32>,
    orientation: Option<StackOrientation>,
) -> Vec<StackSetting> {
    vec![
        StackSetting::Layout(Some(snap.mode), height_fit, height_scroll),
        StackSetting::Scale(scale),
        StackSetting::Orderbook(snap.orderbook),
        StackSetting::Liquidations(snap.liquidations),
        StackSetting::ShowZone(snap.show_zone),
        StackSetting::AutoPin(snap.auto_pin),
        StackSetting::Orientation(orientation),
        StackSetting::ActionPos(Some(snap.cancel_pos), Some(snap.panic_pos)),
        StackSetting::PriceAxis(snap.price_axis_pos),
        StackSetting::TimeAxis(snap.time_axis),
        StackSetting::LineLabels(snap.line_labels),
        StackSetting::CursorLabels(snap.cursor_labels),
    ]
}

#[cfg(test)]
mod tests;
