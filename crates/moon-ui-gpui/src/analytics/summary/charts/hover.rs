//! Summary chart hover helpers.

use super::*;

/// Chart identity and bucket together prevent a late dismissal from closing another popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analytics::summary) enum PopupKey {
    /// One bucket of the combined profit chart.
    Profit(usize),
}

/// Pointer ownership and generation fence for delayed popup dismissal.
#[derive(Default)]
pub(in crate::analytics) struct PopupHover {
    /// Bucket currently owning the popup.
    pub(super) target: Option<PopupKey>,
    /// Whether the pointer reached the popup instead of merely leaving its source column.
    pub(super) over_popup: bool,
    /// Invalidates outstanding leave timers when the pointer returns or changes buckets.
    pub(super) revision: u64,
    /// View-owned entry fence, retained across renders until the first valid chart movement.
    profit_first_move: bool,
}

impl PopupHover {
    /// Require an entry event on the next movement, even if the retained target is unchanged.
    pub(in crate::analytics::summary) fn profit_pointer_entered(&mut self) {
        self.profit_first_move = true;
    }

    /// Decide entry from view state only; repeated movement in one bucket schedules nothing.
    pub(in crate::analytics::summary) fn profit_move_should_enter(&mut self, bi: usize) -> bool {
        let changed =
            self.profit_first_move || self.target != Some(PopupKey::Profit(bi)) || self.over_popup;
        self.profit_first_move = false;
        changed
    }

    /// Leave a source once, including an uncovered gap inside the single hover element.
    pub(in crate::analytics::summary) fn profit_pointer_left(&mut self) -> Option<PopupKey> {
        let entered = !self.profit_first_move;
        self.profit_first_move = true;
        self.target.filter(|_| entered)
    }

    /// Cancel queued actions when reloading changes what their bucket index identifies.
    pub(in crate::analytics) fn reset_for_reload(&mut self, reset: bool) {
        if reset {
            self.target = None;
            self.over_popup = false;
            self.revision = self.revision.wrapping_add(1);
        }
    }

    /// Transfer ownership on entry and cancel any previous leave timer.
    pub(super) fn enter(&mut self, key: PopupKey, over_popup: bool) {
        self.target = Some(key);
        self.over_popup = over_popup;
        self.revision = self.revision.wrapping_add(1);
    }

    /// Begin a grace period unless this is an obsolete source or the popup still owns hover.
    pub(super) fn leave(&mut self, key: PopupKey, from_popup: bool) -> Option<u64> {
        if self.target != Some(key) || (!from_popup && self.over_popup) {
            return None;
        }
        self.over_popup = false;
        self.revision = self.revision.wrapping_add(1);
        Some(self.revision)
    }

    /// Expire only the exact leave event; re-entry and bucket switches invalidate it.
    pub(super) fn expire(&mut self, key: PopupKey, revision: u64) -> bool {
        if !self.is_current(key, revision) || self.over_popup {
            return false;
        }
        self.target = None;
        true
    }

    /// Accept a delayed action only while the same pointer event still owns the popup.
    pub(super) fn is_current(&self, key: PopupKey, revision: u64) -> bool {
        self.target == Some(key) && self.revision == revision
    }
}

/// Keep the existing chart highlights synchronized with the popup's single pointer owner.
pub(super) fn select_popup(this: &mut AnalyticsView, key: Option<PopupKey>) {
    this.hover_profit_bucket = key.map(|PopupKey::Profit(i)| i);
}

/// Transfer pointer ownership; return a revision for delayed reveal or None for immediate selection.
pub(super) fn enter_decision(
    hover: &mut PopupHover,
    selected: Option<PopupKey>,
    key: PopupKey,
    from_popup: bool,
) -> Option<u64> {
    let switching = !from_popup && hover.target.is_some_and(|old| old != key) && selected.is_some();
    hover.enter(key, from_popup);
    switching.then_some(hover.revision)
}

/// Allow travel across the anchor gap without closing; a generation fence rejects stale timers.
pub(in crate::analytics::summary) fn chart_hover(
    this: &mut AnalyticsView,
    key: PopupKey,
    hovered: bool,
    from_popup: bool,
    cx: &mut Context<AnalyticsView>,
) {
    let pending = if hovered {
        // Crossing dense neighbouring columns on the way to the popup must not make the card
        // chase the pointer. A brief dwell switches buckets; entering the card cancels that dwell.
        let selected = this.hover_profit_bucket.map(PopupKey::Profit);
        if let Some(revision) =
            enter_decision(&mut this.summary_popup_hover, selected, key, from_popup)
        {
            Some((revision, true))
        } else {
            select_popup(this, Some(key));
            cx.notify();
            None
        }
    } else {
        this.summary_popup_hover
            .leave(key, from_popup)
            .map(|revision| (revision, false))
    };
    if let Some((revision, reveal)) = pending {
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            executor
                .timer(Duration::from_millis(if reveal { 150 } else { 300 }))
                .await;
            let _ = this.update(cx, |this, cx| {
                if reveal && this.summary_popup_hover.is_current(key, revision) {
                    select_popup(this, Some(key));
                    cx.notify();
                } else if !reveal && this.summary_popup_hover.expire(key, revision) {
                    select_popup(this, None);
                    cx.notify();
                }
            });
        })
        .detach();
    }
}
