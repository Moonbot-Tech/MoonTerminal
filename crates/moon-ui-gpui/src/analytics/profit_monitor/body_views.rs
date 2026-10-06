//! Profit Monitor body views.

use super::*;

pub(super) const HEADER_HEIGHT: f32 = 32.0;
pub(super) const CONTEXT_REFRESH_MS: u128 = 5_000;
pub(super) const SECOND_MS: u128 = 1_000;
pub(super) const MIN_NAME_COLUMN_WIDTH: f32 = 128.0;
/// Floor the name column keeps however much the run column claims from it.
///
/// The run controls are paid for by the NAME column, so the minimum window width never grows —
/// that is the whole constraint (`MIN_WINDOW_WIDTH` covers padding + name + gap + a printable
/// profit column). This floor is what stops the third slot, now that there is one, from squeezing
/// the name down to nothing instead; past it the remainder comes out of the slack in
/// `MIN_WINDOW_WIDTH`.
pub(super) const NAME_COLUMN_FLOOR: f32 = 72.0;
/// Room the window guarantees the profit column, whatever the name column would rather have.
///
/// The column is sized from its CONTENT (`table::profit_column`), so this is not the width it draws
/// at, and it is not a reservation either: at every legal window width the budget leaves far more
/// than this, and everything the values do not claim goes to the name. It is the backstop for a
/// window narrower than the OS minimum, sized as ten monospace characters — `+999999.99`, the
/// widest ordinary two-decimal amount — at the theme base with no Font-slider delta; a raised
/// slider buys fewer characters here, which is the same trade every fixed column in this table
/// makes. The ticker is the first thing the column drops, so it is deliberately not counted in.
pub(super) const PROFIT_MIN_COLUMN_WIDTH: f32 = 68.0;
/// Design-reference logo edge drawn before a row's name.
pub(super) const EXCHANGE_LOGO_SIZE: f32 = 13.0;
/// Gap between that logo and the name it belongs to.
///
/// Named because THREE places have to agree on it: the row that draws the logo, the empty gutter a
/// logo-less row keeps, and the heading's own left padding.
pub(super) const NAME_LOGO_GAP: f32 = 5.0;
pub(super) const TRADES_COLUMN_WIDTH: f32 = 72.0;
pub(super) const WIN_RATE_COLUMN_WIDTH: f32 = 76.0;
pub(super) const AVERAGE_ORDER_COLUMN_WIDTH: f32 = 116.0;
pub(super) const TABLE_HORIZONTAL_PADDING: f32 = 10.0;
pub(super) const TABLE_COLUMN_GAP: f32 = 8.0;
pub(super) const MIN_WINDOW_WIDTH: f32 = 310.0;

/// Return the run slots the current preferences reserve on every line of the table.
///
/// Reserved by the TABLE, not by the line: a caption offers less than a core row does, but both
/// must claim the same width or the name column stops lining up with its own heading.
///
/// Args:
///     prefs: Display preferences chosen in the popup.
///
/// Returns:
///     The reserved slots; `RunSlots::any` is false when the column is off entirely.
pub(super) fn run_slots(prefs: MonitorPrefs) -> RunSlots {
    // One preference per CONTROL decides which slots exist at all; the group and heading
    // preferences only decide which LINES fill them, so neither can reserve a column on its own.
    RunSlots {
        status: prefs.core_status,
        trading: prefs.trading_buttons,
        auto: prefs.auto_buttons,
    }
}

/// Return the name column's minimum width once the run column has taken its share.
///
/// Args:
///     slots: Slots the table reserves.
///
/// Returns:
///     Minimum design-reference width of the Name column.
pub(super) fn name_min_width(slots: RunSlots) -> f32 {
    if !slots.any() {
        return MIN_NAME_COLUMN_WIDTH;
    }
    (MIN_NAME_COLUMN_WIDTH - slots.width() - TABLE_COLUMN_GAP).max(NAME_COLUMN_FLOOR)
}

/// Read the current UTC instant through the terminal's shared system-clock source.
///
/// Returns:
///     Current UTC time, or the Unix epoch when the platform clock is outside chrono's range.
pub(super) fn now_utc() -> DateTime<Utc> {
    DateTime::from_timestamp_millis(moon_core::util::now_unix_ms_i64())
        .unwrap_or(DateTime::UNIX_EPOCH)
}

/// Return the arrow suffix for one sortable heading.
///
/// Args:
///     sort: Current explicit ordering.
///     column: Heading being rendered.
///
/// Returns:
///     Direction glyph for the active heading, otherwise an empty suffix.
pub(super) fn sort_arrow(sort: Option<MonitorSort>, column: MonitorSortColumn) -> &'static str {
    match sort {
        Some(active) if active.column == column && active.descending => " ↓",
        Some(active) if active.column == column => " ↑",
        _ => "",
    }
}

/// Independently ticking terminal clock embedded in the Profit Monitor controls.
///
/// Keeping the second timer on a child entity prevents each clock tick from rebuilding, grouping,
/// and sorting the monitor's virtualized table.
pub(super) struct MonitorClockView {
    pub(super) backend: Entity<Backend>,
}

impl MonitorClockView {
    /// Construct the shared terminal clock and align its repaint loop to wall-clock seconds.
    ///
    /// Args:
    ///     backend: Shared terminal state containing the selected display zone and its revision.
    ///     cx: Child view context that owns the recurring timer.
    ///
    /// Returns:
    ///     Clock state whose repaint cadence is isolated from the parent monitor.
    pub(super) fn new(backend: Entity<Backend>, cx: &mut Context<Self>) -> Self {
        let display_time_revision = backend.read(cx).display_time_revision.clone();
        cx.observe(&display_time_revision, |_this, _revision, cx| cx.notify())
            .detach();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            loop {
                executor
                    .timer(duration_until_wall_clock_boundary(
                        SystemTime::now(),
                        SECOND_MS,
                    ))
                    .await;
                let alive = cx.update(|cx| this.update(cx, |_this, cx| cx.notify()).is_ok());
                if !alive {
                    break;
                }
            }
        })
        .detach();
        Self { backend }
    }
}

impl Render for MonitorClockView {
    /// Render the same selected-city clock and picker used by the terminal header.
    ///
    /// Args:
    ///     window: Owning Profit Monitor window whose width selects clock precision.
    ///     cx: Clock render context used to read the active palette and backend state.
    ///
    /// Returns:
    ///     Shared terminal clock element.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = MoonPalette::active(cx);
        if MonitorLayout::for_width(window_width(window), design::ui_value(cx, 1.0)).clock_seconds {
            crate::chrome::clock::header_clock(&self.backend, palette, cx)
        } else {
            crate::chrome::clock::compact_header_clock(&self.backend, palette, cx)
        }
    }
}

/// Cached sibling surface containing every expensive Profit Monitor body state.
///
/// The child reads its owner weakly, so the parent can retain it without creating an entity cycle.
/// When the clock invalidates its own branch, GPUI marks the parent dirty but reuses this clean
/// cached sibling instead of regrouping and sorting the table.
pub(super) struct ProfitMonitorBodyView {
    pub(super) owner: WeakEntity<ProfitMonitorView>,
}

impl Render for ProfitMonitorBodyView {
    /// Render the current monitor body through its owning state entity.
    ///
    /// Args:
    ///     window: Owning window used for responsive column selection.
    ///     cx: Body view context used to read the parent entity and active palette.
    ///
    /// Returns:
    ///     Current loading, error, split-currency, or table surface.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(owner) = self.owner.upgrade() else {
            return div().into_any_element();
        };
        let palette = MoonPalette::active(cx);
        let body = owner
            .read(cx)
            .body(window_width(window), palette, owner.clone(), cx);
        // Cached AnyView lays its rendered child out as an independent root. This full-size
        // vertical root transfers the allocated cache bounds to the body's elastic table slot.
        v_flex()
            .size_full()
            .min_h_0()
            .child(body)
            .into_any_element()
    }
}
