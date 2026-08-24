//! The ONE "apply to other tabs" mechanism, shared by every chart popup that has the button.
//!
//! Each popup used to carry its own copy of the same walk — apply to Main, apply to every Add,
//! Custom and detached stack, write each one's `charts.json` spec — differing only in WHICH values
//! travelled. The values are already expressible as [`StackSetting`], which knows both how to reach
//! a stack and how to write itself to a spec, so a press is fully described by [`ApplyAll`]: the
//! values, the optional X scale that rides along with the candle popup's, WHICH kinds of tab it
//! addresses, and whether it sets those kinds' DEFAULTS or just writes their tabs.
//!
//! # Setting a default
//!
//! The press used to do two things at once: copy the values into every tab in the group AND
//! overwrite the one global default. That fusion is why the main chart needed a rule of its own —
//! it follows the default live, so overwriting the default changed Main as surely as editing it,
//! and a press from anywhere else had to freeze Main first to stop it.
//!
//! Split apart, both halves get simpler. A press either:
//!
//! - **sets the default** for the kinds it names ([`ApplyAll::as_default`]) — which also CLEARS the
//!   override of every tab of those kinds, open or closed, so they follow the new default rather
//!   than each keeping a frozen copy of it. A later change of that default reaches them again;
//!   copying, which is what the old press did, was a one-way ticket.
//! - **writes the values** into the tabs of those kinds as their own overrides. This is what the ⚙
//!   layout popup does, because its values have no default to set: they are per-tab only.
//!
//! # Reaching the other windows
//!
//! A group's tab strip owns only its own stacks. Defaults are global, so clearing overrides has to
//! reach every group window — done by [`ClearDefaults`], which the pressing strip leaves on Backend
//! and every strip drains, its own included. Detached windows cannot walk the group at all, so
//! their press travels the other way, as [`ApplyAllRequest`] for their group's strip to perform.

use gpui::*;
use moon_core::config::ChartTabKind;

use super::common::{GlobalSlot, LayoutPopupSnapshot, StackSetting, set_stack_setting};
use crate::persistence::chart_persist::{ChartTabSpec, StackOrientation};
use moon_core::config::ChartBucket;

/// Which of the three tab kinds a press addresses.
///
/// A set rather than one kind: the reader who wants the main chart and the windows to agree says so
/// with two ticks, and the popup that offers the ticks is the same one for every setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct KindTargets([bool; 3]);

impl KindTargets {
    /// The set holding just this kind — what a press starts as, its own kind ticked.
    pub(crate) fn only(kind: ChartTabKind) -> Self {
        let mut out = Self::default();
        out.set(kind, true);
        out
    }

    pub(crate) fn has(self, kind: ChartTabKind) -> bool {
        self.0[Self::index(kind)]
    }

    pub(crate) fn set(&mut self, kind: ChartTabKind, on: bool) {
        self.0[Self::index(kind)] = on;
    }

    /// Whether the press addresses anything at all. An empty set is a press that does nothing, and
    /// the popup disables its button rather than performing one.
    pub(crate) fn any(self) -> bool {
        self.0.iter().any(|on| *on)
    }

    /// Stable slot of a kind in the target set, and in the counts beside it.
    pub(crate) fn index(kind: ChartTabKind) -> usize {
        match kind {
            ChartTabKind::Main => 0,
            ChartTabKind::AddTo => 1,
            ChartTabKind::Compare => 2,
        }
    }
}

/// One press: what to write, where it lands, and whether it becomes a default.
///
/// A value that has a default of its own carries that fact through [`StackSetting::global_slot`],
/// so a press cannot name a default and a value that disagree.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ApplyAll {
    /// Values the press carries, applied in this order.
    pub values: Vec<StackSetting>,
    /// Window X scale copied along with the values, and stored per group for new charts. Only the
    /// candle popup sends one; `None` leaves every target's scale alone.
    pub x_ppm: Option<f32>,
    /// Which kinds of tab the press addresses.
    pub targets: KindTargets,
    /// Whether this press sets those kinds' DEFAULTS and clears their tabs' overrides, rather than
    /// writing the values into those tabs as overrides.
    pub as_default: bool,
}

/// A detached window's press, queued through Backend for its group's tab strip to perform.
///
/// Detached hosts own one panel and no group stacks, so they cannot walk the targets themselves.
pub(crate) struct ApplyAllRequest {
    /// Group whose tab strip must perform the walk.
    pub group: String,
    pub apply: ApplyAll,
}

/// How many default-setting presses the queue remembers.
///
/// Far above what any sequence of presses produces — the drain that can queue several at once
/// handles one detached window's popups — and bounded because a window that has not observed
/// Backend in that long has bigger problems than a stale override.
const CLEAR_QUEUE_KEEP: usize = 64;

/// A default-setting press, for every group window to drop the matching overrides from its stacks.
///
/// Defaults are global while stacks are not: the pressing strip can clear its own tabs and every
/// stored spec, but a live stack in another window holds its override in memory and would keep
/// drawing it — and re-persist it on its next write. This is what tells that window.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ClearDefaults {
    /// Kinds whose tabs must drop their override.
    pub targets: KindTargets,
    /// Settings to drop. Only these: a caption default must not clear anyone's candles.
    pub slots: Vec<GlobalSlot>,
}

/// Clear one setting on a spec, and report whether the spec actually held it.
fn clear_spec_slot(slot: GlobalSlot, spec: &mut ChartTabSpec) -> bool {
    match slot {
        GlobalSlot::CandleView => spec.candle_view.take().is_some(),
        GlobalSlot::Graphics => spec.chart_graphics.take().is_some(),
        GlobalSlot::Labels => spec.chart_labels.take().is_some(),
    }
}

/// Whether a spec holds its own value for this setting.
fn spec_holds_slot(slot: GlobalSlot, spec: &ChartTabSpec) -> bool {
    match slot {
        GlobalSlot::CandleView => spec.candle_view.is_some(),
        GlobalSlot::Graphics => spec.chart_graphics.is_some(),
        GlobalSlot::Labels => spec.chart_labels.is_some(),
    }
}

/// The kind a STORED tab is, from what its spec records.
///
/// The live stack is the authority for a tab that is open — it knows the lock that was just clicked
/// — and this is for the ones that are not: a closed tab whose override would otherwise outlive the
/// default it is supposed to follow and come back on the next launch.
fn spec_kind(spec: &ChartTabSpec) -> ChartTabKind {
    ChartTabKind::of(spec.detached.is_some(), spec.compare_anchor.is_some())
}

/// The settings the press can set a default for, in its own order.
fn pressed_slots(values: &[StackSetting]) -> Vec<GlobalSlot> {
    values.iter().filter_map(|v| v.global_slot()).collect()
}

impl super::ChartTabs {
    /// Perform one press: see [`ApplyAll`].
    pub(super) fn apply_all(&mut self, apply: ApplyAll, cx: &mut Context<Self>) {
        if !apply.targets.any() {
            return;
        }
        match apply.as_default {
            true => self.set_kind_defaults(apply, cx),
            false => self.write_into_kinds(apply, cx),
        }
        cx.notify();
    }

    /// Store the pressed values as the targeted kinds' defaults, and drop the overrides that hid them.
    fn set_kind_defaults(&mut self, apply: ApplyAll, cx: &mut Context<Self>) {
        let slots = pressed_slots(&apply.values);
        if slots.is_empty() {
            // Nothing storable, but the candle popup's X scale still travels: it has no default of
            // its own and rides along with the press.
            self.apply_x_ppm(&apply, cx);
            return;
        }
        let targets = apply.targets;
        let rebuild_orderbook = apply.values.iter().any(|v| v.rebuilds_orderbook_demand());
        self.backend.update(cx, |b, bcx| {
            let mut moved = false;
            for kind in ChartTabKind::ALL.into_iter().filter(|k| targets.has(*k)) {
                for value in &apply.values {
                    if let Some(slot) = value.global_slot() {
                        moved |= slot.write_default(&mut b.layout, kind, value.clone());
                    }
                }
            }
            // Every stored tab of a targeted kind, in EVERY group: an override that outlived this
            // press would come back on the next launch, ahead of the default just set.
            let mut specs_changed = false;
            for spec in b.chart_specs.iter_mut() {
                if !targets.has(spec_kind(spec)) {
                    continue;
                }
                for slot in &slots {
                    specs_changed |= clear_spec_slot(*slot, spec);
                }
            }
            if moved {
                b.layout_dirty = true;
            }
            if specs_changed {
                b.chart_specs_dirty = true;
            }
            // Broadcast even when nothing moved in the file: another window's LIVE stack can still
            // be holding an override of its own, and dropping that is the point of the press.
            b.chart_defaults_rev = b.chart_defaults_rev.wrapping_add(1);
            let rev = b.chart_defaults_rev;
            b.chart_defaults_clear
                .push_back((rev, ClearDefaults { targets, slots }));
            while b.chart_defaults_clear.len() > CLEAR_QUEUE_KEEP {
                b.chart_defaults_clear.pop_front();
            }
            if rebuild_orderbook {
                b.rebuild_orderbook_wanted();
            }
            // Charts in other group windows follow these defaults and are not walked here; every
            // chart panel observes Backend, and this is what makes them re-read.
            bcx.notify();
        });
        // Including this strip's own stacks, through the same path every other window takes.
        self.drain_default_clears(cx);
        self.apply_x_ppm(&apply, cx);
    }

    /// Write the pressed values into the tabs of the targeted kinds, as their own overrides.
    fn write_into_kinds(&mut self, apply: ApplyAll, cx: &mut Context<Self>) {
        let targets = apply.targets;
        if targets.has(self.main.read(cx).default_kind()) {
            // Filtered per TARGET, not once for the press: a value can mean something on one
            // addressed tab and nothing on the next, and writing it where it means nothing leaves a
            // setting in the spec that the tab's own popup does not show and cannot clear.
            let values = applicable(&apply.values, true, false);
            self.main.update(cx, |s, c| {
                for v in &values {
                    set_stack_setting!(s, c, v.clone());
                }
            });
            self.upsert_spec(cx, 0, &ChartBucket::Shared, |s| {
                for v in &values {
                    v.clone().write_spec(s);
                }
            });
        }
        for (num, bucket, stack) in self.group_stacks(cx) {
            if !targets.has(stack.read(cx).default_kind()) {
                continue;
            }
            let values = applicable(&apply.values, false, self.is_custom_tab(num, &bucket, cx));
            stack.update(cx, |s, c| {
                for v in &values {
                    set_stack_setting!(s, c, v.clone());
                }
            });
            self.upsert_spec(cx, num, &bucket, move |s| {
                for v in &values {
                    v.clone().write_spec(s);
                }
            });
        }
        let rebuild_orderbook = apply.values.iter().any(|v| v.rebuilds_orderbook_demand());
        if rebuild_orderbook {
            self.backend.update(cx, |b, _| b.rebuild_orderbook_wanted());
        }
        self.apply_x_ppm(&apply, cx);
    }

    /// Copy the candle popup's X scale, when it sent one, to the targeted stacks and the stored
    /// per-group value new charts inherit.
    fn apply_x_ppm(&mut self, apply: &ApplyAll, cx: &mut Context<Self>) {
        let Some(ppm) = apply.x_ppm else {
            return;
        };
        let targets = apply.targets;
        if targets.has(self.main.read(cx).default_kind()) {
            self.main.update(cx, |s, c| s.set_x_ppm(Some(ppm), true, c));
        }
        for (num, bucket, stack) in self.group_stacks(cx) {
            if targets.has(stack.read(cx).default_kind()) {
                stack.update(cx, |s, c| s.set_x_ppm(Some(ppm), true, c));
                // A detached window's scale lives in its own spec, and the walk used to write it
                // there. Without this the scale reaches the window live and is gone next launch.
                self.upsert_spec(cx, num, &bucket, move |s| s.x_ppm = Some(ppm));
            }
        }
        let group = self.group.clone();
        self.backend.update(cx, |b, _| {
            // Store the scale for THIS group and every known group window so their new charts
            // inherit it. Live windows of other groups update their own charts separately.
            b.layout.chart_x_ppm_by_group.insert(group, ppm);
            let groups: Vec<String> = b.layout.groups.keys().cloned().collect();
            for g in groups {
                b.layout.chart_x_ppm_by_group.insert(g, ppm);
            }
            b.layout_dirty = true;
        });
    }

    /// Drop the overrides a default-setting press cleared, from THIS strip's own stacks.
    ///
    /// Called from the backend observer in every group window, the pressing one included: a stack
    /// holds its override in memory, and a window that never heard about the press would go on
    /// drawing it and write it back on its next persist.
    pub(super) fn drain_default_clears(&mut self, cx: &mut Context<Self>) {
        let seen = self.last_defaults_rev;
        let pending: Vec<ClearDefaults> = {
            let b = self.backend.read(cx);
            if b.chart_defaults_rev == seen {
                // The common case by a wide margin: this runs from the backend observer, so it is
                // on every notification of every group window.
                return;
            }
            b.chart_defaults_clear
                .iter()
                .filter(|(rev, _)| *rev > seen)
                .map(|(_, clear)| clear.clone())
                .collect()
        };
        self.last_defaults_rev = self.backend.read(cx).chart_defaults_rev;
        for clear in pending {
            self.apply_one_clear(&clear, cx);
        }
        cx.notify();
    }

    /// Apply one default-setting press to this strip's own stacks and their specs.
    fn apply_one_clear(&mut self, clear: &ClearDefaults, cx: &mut Context<Self>) {
        // The LIVE stack is the authority on what a tab is: the anchor lock is not persisted for
        // every kind of tab, so the spec pass — which reads the stored kind — can both miss a
        // locked tab and clear one it should not have. Both are repaired here.
        let main_kind = self.main.read(cx).default_kind();
        let main_held: Vec<(GlobalSlot, StackSetting)> = clear
            .slots
            .iter()
            .filter_map(|slot| slot.main_value(self.main.read(cx)).map(|v| (*slot, v)))
            .collect();
        if !main_held.is_empty() {
            match clear.targets.has(main_kind) {
                true => {
                    self.main.update(cx, |s, c| {
                        for (slot, _) in &main_held {
                            slot.clear_on_main(s, c);
                        }
                    });
                    // Main's stored override too, or it returns on the next launch ahead of the
                    // default this press just set.
                    self.upsert_spec(cx, 0, &ChartBucket::Shared, move |s| {
                        for (slot, _) in &main_held {
                            clear_spec_slot(*slot, s);
                        }
                    });
                }
                // Not addressed, but the spec pass may have cleared it by a stale stored kind:
                // write back what the live stack still holds.
                false => self.upsert_spec(cx, 0, &ChartBucket::Shared, move |s| {
                    for (_, value) in main_held {
                        value.write_spec(s);
                    }
                }),
            }
        }
        for (num, bucket, stack) in self.group_stacks(cx) {
            let kind = stack.read(cx).default_kind();
            let held: Vec<(GlobalSlot, StackSetting)> = clear
                .slots
                .iter()
                .filter_map(|slot| slot.stack_value(stack.read(cx)).map(|v| (*slot, v)))
                .collect();
            if held.is_empty() {
                // Nothing to drop, and nothing to write back. Skipping keeps the press from
                // CREATING a spec for a tab that never had one.
                continue;
            }
            match clear.targets.has(kind) {
                true => {
                    stack.update(cx, |s, c| {
                        for (slot, _) in &held {
                            slot.clear_on_stack(s, c);
                        }
                    });
                    self.upsert_spec(cx, num, &bucket, move |s| {
                        for (slot, _) in &held {
                            clear_spec_slot(*slot, s);
                        }
                    });
                }
                false => self.upsert_spec(cx, num, &bucket, move |s| {
                    for (_, value) in held {
                        value.write_spec(s);
                    }
                }),
            }
        }
    }

    /// Whether `(num, bucket)` is a CUSTOM tab — a set of markets its owner picked.
    ///
    /// Asked of both the strip and the spec: a custom tab torn off into a window has left
    /// `self.custom` for `self.detached`, where nothing distinguishes it from an ordinary
    /// AddToChart window except the market list its spec still carries.
    fn is_custom_tab(&self, num: u32, bucket: &ChartBucket, cx: &App) -> bool {
        if self.custom.iter().any(|(n, b, _)| *n == num && b == bucket) {
            return true;
        }
        self.backend
            .read(cx)
            .chart_specs
            .iter()
            .any(|s| s.matches(&self.group, num, bucket) && s.custom_coins.is_some())
    }

    /// This group's stacks, wherever they live: Add tabs, custom tabs, detached windows.
    fn group_stacks(&self, _cx: &App) -> Vec<(u32, ChartBucket, Entity<super::AddChartStack>)> {
        self.add
            .iter()
            .chain(self.custom.iter())
            .chain(self.detached.iter())
            .map(|(n, b, p)| (*n, b.clone(), p.clone()))
            .collect()
    }
}

/// The values of a press that mean something on a tab described by these two facts.
///
/// One line, called at every target, so a setting that does not belong on a tab reaches neither its
/// stack nor its spec — see [`StackSetting::applies_to`] for why the popup's own visibility check
/// is not enough on its own.
fn applicable(values: &[StackSetting], is_main: bool, is_custom: bool) -> Vec<StackSetting> {
    values
        .iter()
        .filter(|v| v.applies_to(is_main, is_custom))
        .cloned()
        .collect()
}

/// How many stored tabs of each kind a press would overwrite, for the popup to state.
///
/// Counted from the SPECS rather than the open tabs: a tab with no override of its own follows the
/// default already and is not overwritten by a press that changes it, while a closed tab that holds
/// one is — and is exactly the surprise worth naming before the press. Free-standing because the
/// detached windows ask it too, and they own no group bookkeeping.
pub(super) fn override_counts(specs: &[ChartTabSpec], values: &[StackSetting]) -> [usize; 3] {
    let slots = pressed_slots(values);
    let mut out = [0usize; 3];
    for spec in specs {
        if slots.iter().any(|slot| spec_holds_slot(*slot, spec)) {
            out[KindTargets::index(spec_kind(spec))] += 1;
        }
    }
    out
}

/// Build the ⚙ layout popup's value set from the source tab's settings.
///
/// The snapshot already carries every resolved layout value the popup shows; the ones that are not
/// in it — both mode heights, the price scale, and the detect cap — come from the source's fields,
/// because the popup edits those in text fields whose uncommitted contents are the source of truth.
///
/// Args:
///     snap: The source tab's resolved layout settings.
///     height_fit: Fit-mode slot extent, or `None` for the default.
///     height_scroll: Scroll-mode slot extent, or `None` for the default.
///     scale: Price scale, or `None` for Auto.
///     orientation: Source orientation, unresolved: `None` copies "no orientation named".
///     max_charts: Detect cap from the popup field, or `None` for uncapped.
///
/// Returns:
///     The values a ⧉ press on the layout popup copies, in application order. The caller keeps only
///     the ones its own tab actually has, through `LayoutPopupHost::applicable_here`.
pub(super) fn layout_values(
    snap: &LayoutPopupSnapshot,
    height_fit: Option<u16>,
    height_scroll: Option<u16>,
    scale: Option<f32>,
    orientation: Option<StackOrientation>,
    max_charts: Option<u16>,
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
        StackSetting::ArrivalFlash(snap.arrival_flash),
        StackSetting::MaxCharts(max_charts, snap.max_charts_evict),
    ]
}

#[cfg(test)]
mod tests;
