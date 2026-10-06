//! Summary chart hover helpers.

use super::*;

pub(in crate::analytics::summary) const CHART_H: f32 = 170.0;
/// Weight the chart value labels are drawn at — they set none, so it is GPUI's normal.
/// `mono_caption_text_width` needs it as a number.
pub(in crate::analytics::summary) const LABEL_WEIGHT: f32 = 400.0;

/// Width of the WIDEST of a set of already-formatted value labels, in pixels.
///
/// MEASURED, never a constant. A guessed width is wrong in both directions and both are
/// expensive: too narrow and the collision passes believe two labels are clear when the digits
/// overlap — the exact defect they exist to prevent — too wide and they drop labels that would
/// have fitted. The old constants were guessed against example strings (`"-1268 USDT"`), and
/// that example was not even a string these charts produce: `pnl_suffix` is `"%"` or nothing,
/// never a ticker.
///
/// The Analytics view draws in the MONO family (`analytics/render.rs`), so a glyph advance is
/// the same for every character and the widest label is simply the longest one — picking it by
/// `chars().count()` and measuring that one string is exact, not an approximation, and costs a
/// single measurement per frame rather than one per label.
///
/// Args:
///     texts: The formatted labels this chart is about to consider drawing.
///     cx: Analytics view context.
///
/// Returns:
///     Width of the longest label in pixels, or 0.0 when there are none.
pub(in crate::analytics::summary) fn widest_label_w<'a>(
    texts: impl Iterator<Item = &'a str>,
    cx: &Context<AnalyticsView>,
) -> f32 {
    texts
        .max_by_key(|s| s.chars().count())
        .map(|s| design::mono_caption_text_width(cx, s, LABEL_WEIGHT))
        .unwrap_or(0.0)
}
/// Plot width both summary charts assume, in RAW pixels — deliberately neither font- nor
/// UI-scaled.
///
/// The card's real width is a layout result these functions never learn: it is only known
/// inside the paint closure, while the labels are absolutely-positioned divs outside it, and
/// threading a measured width in would widen both chart signatures through the summary card.
/// So the pass measures against the narrowest the card realistically gets — the Analytics
/// window's own minimum (860 wide, `analytics/mod.rs`) less the page padding (2x10), the
/// inter-card gap (8) and the card's own padding (2x12), over two equal cards:
/// `(860 - 20 - 8) / 2 - 24`. At the default 1240-wide window the real plot is ~584, so
/// assuming the minimum only ever drops a label that would in fact have fitted — the safe
/// direction; assuming wide puts overlap back.
///
/// **Raw, not scaled.** 860 is a fixed physical size and every padding it is reduced by goes
/// through `ui()`, which tracks the UI scale but NOT the Font slider — so a larger font never
/// widens this plot, and a larger UI scale only eats further INTO it. Passing this through
/// `font_w_px` would have grown the room the collision pass believes it has at exactly the
/// setting that makes the labels physically wider, which is the application's default (+3).
///
/// ONE constant for BOTH charts on purpose: they are the two `flex_1` siblings of the same
/// row, with the same page padding, the same gap and the same card chrome. Two copies could
/// drift apart from a layout that cannot.
pub(in crate::analytics::summary) const PLOT_W_NOMINAL: f32 = 392.0;

/// Chart identity and bucket together prevent a late dismissal from closing another popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analytics::summary) enum PopupKey {
    /// One daily profit bucket.
    Daily(usize),
    /// Running totals through one bucket.
    Cumulative(usize),
    /// One strategy type.
    Kind(usize),
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
    cumulative_first_move: bool,
}

impl PopupHover {
    /// Require an entry event on the next movement, even if the retained target is unchanged.
    pub(in crate::analytics::summary) fn cumulative_pointer_entered(&mut self) {
        self.cumulative_first_move = true;
    }

    /// Decide entry from view state only; repeated movement in one bucket schedules nothing.
    pub(in crate::analytics::summary) fn cumulative_move_should_enter(
        &mut self,
        bi: usize,
    ) -> bool {
        let changed = self.cumulative_first_move
            || self.target != Some(PopupKey::Cumulative(bi))
            || self.over_popup;
        self.cumulative_first_move = false;
        changed
    }

    /// Leave a source once, including an uncovered gap inside the single hover element.
    pub(in crate::analytics::summary) fn cumulative_pointer_left(&mut self) -> Option<PopupKey> {
        let entered = !self.cumulative_first_move;
        self.cumulative_first_move = true;
        self.target
            .filter(|key| entered && matches!(key, PopupKey::Cumulative(_)))
    }

    /// Cancel queued actions when reloading changes what their bucket index identifies.
    pub(in crate::analytics) fn reset_for_reload(&mut self, reset_daily: bool) {
        if reset_daily || matches!(self.target, Some(PopupKey::Kind(_))) {
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
    this.hover_daily_bucket = match key {
        Some(PopupKey::Daily(i)) => Some(i),
        _ => None,
    };
    this.hover_cum_bucket = match key {
        Some(PopupKey::Cumulative(i)) => Some(i),
        _ => None,
    };
    this.hover_kind = match key {
        Some(PopupKey::Kind(i)) => Some(i),
        _ => None,
    };
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
        let selected = this
            .hover_daily_bucket
            .map(PopupKey::Daily)
            .or(this.hover_cum_bucket.map(PopupKey::Cumulative))
            .or(this.hover_kind.map(PopupKey::Kind));
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
