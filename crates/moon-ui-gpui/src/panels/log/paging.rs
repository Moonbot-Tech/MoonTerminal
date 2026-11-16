//! Walking a day-sized log file in the Log tab, one window at a time.
//!
//! A named file used to be its last [`VIEW_LIMIT`] lines and nothing else, which was enough for the
//! hours a live copy holds and useless for a whole day pulled from a core's server: the morning was
//! on disk and out of reach. The window now moves — a screen earlier, a screen later, or straight
//! to a time of day — over an index built once when the file is opened
//! ([`moon_core::applog::pager`]), so a move costs a seek and parses only the lines it shows.

use super::*;
use moon_core::applog::pager::{self, FilePager};
use moon_ui::MoonButton;

/// Width of the time field: room for `HH:MM:SS` and the clear button.
const TIME_FIELD_W: f32 = 96.0;

/// Where the window of a named file should move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PageTarget {
    Earlier,
    Later,
    /// The first line at or after this `HH:MM:SS`.
    Clock([u8; 8]),
}

/// The window of a named log file the panel currently shows.
#[derive(Default)]
pub(super) struct FilePage {
    pager: Option<FilePager>,
    /// Zero-based line the window starts at.
    first: usize,
    lines: Vec<LogLine>,
}

impl FilePage {
    /// Opens `name` at its tail, which is where a log is read from.
    pub(super) fn open(&mut self, name: &str) {
        let pager = FilePager::open(name);
        self.first = tail_start(pager.total());
        self.lines = pager.read(self.first, VIEW_LIMIT);
        self.pager = Some(pager);
    }

    /// Forgets the file, so the next selection of the same name reads it again.
    pub(super) fn close(&mut self) {
        *self = Self::default();
    }

    /// The lines of the current window, oldest first.
    pub(super) fn lines(&self) -> &[LogLine] {
        &self.lines
    }

    /// Lines the file had when it was opened.
    pub(super) fn total(&self) -> usize {
        self.pager.as_ref().map_or(0, FilePager::total)
    }

    /// Day the open file's lines belong to, or empty when none is open.
    pub(super) fn date(&self) -> &str {
        self.pager.as_ref().map_or("", FilePager::date)
    }

    /// Moves the window and reads it; returns whether it moved.
    pub(super) fn seek(&mut self, target: PageTarget) -> bool {
        let Some(pager) = self.pager.as_ref() else {
            return false;
        };
        let total = pager.total();
        let first = match target {
            PageTarget::Earlier | PageTarget::Later => {
                step(self.first, self.lines.len(), total, target)
            }
            // A time after the day's last line shows the tail rather than an empty window.
            PageTarget::Clock(clock) => match pager.line_at(&clock) {
                line if line >= total => tail_start(total),
                line => line,
            },
        };
        if first == self.first {
            return false;
        }
        self.first = first;
        self.lines = pager.read(first, VIEW_LIMIT);
        true
    }

    /// One-based numbers of the first and last line shown, and the file's line count.
    pub(super) fn range(&self) -> (usize, usize, usize) {
        (
            self.first + usize::from(!self.lines.is_empty()),
            self.first + self.lines.len(),
            self.total(),
        )
    }

    /// Whether lines exist before the window.
    pub(super) fn has_earlier(&self) -> bool {
        self.first > 0
    }

    /// Whether lines exist after the window.
    pub(super) fn has_later(&self) -> bool {
        self.first + self.lines.len() < self.total()
    }
}

/// The file's own clock for a time of day read in the display zone.
///
/// A file's lines are stamped in one reckoning and shown in another: every row's clock is moved
/// into the selected display zone on the way to the screen. A time the user reads there and types
/// back has to make the opposite trip, or the jump lands a zone's width away. A day file holds
/// twenty-four hours, so each shown time of day names exactly one stored one; the zone's offset is
/// taken at the file's noon, which is right for every line except the hour around a clock change.
///
/// Args:
///     shown: `HH:MM:SS` as the rows display it.
///     date: The file's day, `YYYY-MM-DD`; without one the time is taken as stored.
///     zone: The selected display zone.
///
/// Returns:
///     `HH:MM:SS` in the file's clock.
fn stored_clock(shown: [u8; 8], date: &str, zone: chrono_tz::Tz) -> [u8; 8] {
    use chrono::{NaiveDate, Offset, TimeZone};

    let Some(noon) = NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .and_then(|day| day.and_hms_opt(12, 0, 0))
    else {
        return shown;
    };
    let ahead = i64::from(zone.offset_from_utc_datetime(&noon).fix().local_minus_utc());
    let field = |at: usize| i64::from(shown[at] - b'0') * 10 + i64::from(shown[at + 1] - b'0');
    let seconds = (field(0) * 3600 + field(3) * 60 + field(6) - ahead).rem_euclid(86_400);
    let mut stored = [0u8; 8];
    stored.copy_from_slice(
        format!(
            "{:02}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
        .as_bytes(),
    );
    stored
}

/// First line of the window that ends at the file's last line.
fn tail_start(total: usize) -> usize {
    total.saturating_sub(VIEW_LIMIT)
}

/// Where a one-screen step lands, given the window's start, its length and the file's line count.
///
/// A step never leaves the file: earlier stops at the first line, later stops where nothing is
/// left to show, and both return the current start when there is nowhere to go.
fn step(first: usize, shown: usize, total: usize, target: PageTarget) -> usize {
    match target {
        PageTarget::Earlier => first.saturating_sub(VIEW_LIMIT),
        PageTarget::Later if first + shown < total => first + shown,
        PageTarget::Later | PageTarget::Clock(_) => first,
    }
}

impl LogPanel {
    /// Moves the shown window of the selected file and puts the list where reading continues.
    ///
    /// A move stops tail following: the list would otherwise be pulled back to the bottom of every
    /// window, and after a jump to a time the line asked for is the window's FIRST. Stepping earlier
    /// lands on the bottom of the earlier screen, which is where the eye left the one after it.
    pub(super) fn move_file_page(&mut self, target: PageTarget, cx: &mut Context<Self>) {
        if !self.file_page.seek(target) {
            return;
        }
        self.live = false;
        self.scroll_pause = false;
        self.scroll_gen = self.scroll_gen.wrapping_add(1);
        let backend = self.backend.clone();
        self.reload_rows(backend.read(cx), cx);
        let rows = self.buf.visible();
        if rows > 0 {
            let (row, strategy) = match target {
                PageTarget::Earlier => (rows - 1, ScrollStrategy::Bottom),
                PageTarget::Later | PageTarget::Clock(_) => (0, ScrollStrategy::Top),
            };
            self.scroll.scroll_to_item(row, strategy);
        }
        cx.notify();
    }

    /// Jumps to the time typed into the time field; anything that is not a time of day moves nothing.
    ///
    /// The time is read the way the rows show theirs — in the selected display zone — and turned
    /// into the file's own clock before the search.
    pub(super) fn jump_to_typed_time(&mut self, cx: &mut Context<Self>) {
        let typed = self.page_time.read(cx).value().to_string();
        let Some(shown) = pager::parse_clock(&typed) else {
            return;
        };
        let zone =
            moon_core::util::display_time::zone_or_utc(self.backend.read(cx).header_clock_zone());
        let clock = stored_clock(shown, self.file_page.date(), zone);
        self.move_file_page(PageTarget::Clock(clock), cx);
    }

    /// The paging controls of a named file that holds more than one screen, or nothing.
    ///
    /// A file that fits the view whole needs none: every line of it is already in the list.
    pub(super) fn page_controls(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let (_, file, _) = self.effective_selection(self.backend.read(cx));
        if !matches!(file, LogFile::Named(_)) || self.file_page.total() <= VIEW_LIMIT {
            return None;
        }
        let p = MoonPalette::active(cx);
        let (from, to, total) = self.file_page.range();
        let step_button = |id: &'static str, label: String, enabled: bool, target: PageTarget| {
            MoonButton::new(id)
                .variant(MoonButtonVariant::Soft)
                .size(MoonButtonSize::density(cx))
                .label(label)
                .disabled(!enabled)
                .on_click(cx.listener(move |this, _, _, cx| this.move_file_page(target, cx)))
                .render()
        };
        Some(
            h_flex()
                .flex_none()
                .gap_2()
                .items_center()
                .child(step_button(
                    "log-page-earlier",
                    t!("log.page.earlier").to_string(),
                    self.file_page.has_earlier(),
                    PageTarget::Earlier,
                ))
                .child(step_button(
                    "log-page-later",
                    t!("log.page.later").to_string(),
                    self.file_page.has_later(),
                    PageTarget::Later,
                ))
                .child(
                    div()
                        .flex_none()
                        .text_size(crate::design::t_body(cx))
                        .text_color(rgb(p.text_muted))
                        .child(
                            t!("log.page.range", from = from, to = to, total = total).to_string(),
                        ),
                )
                .child(
                    div()
                        .id("log-page-time-tip")
                        .flex_none()
                        .w(crate::design::ui_px(cx, TIME_FIELD_W))
                        .tooltip(crate::panels::common::text_tooltip(
                            t!("log.page.time_tip").to_string(),
                        ))
                        .child(
                            MoonInput::new("log-page-time")
                                .state(&self.page_time)
                                .placeholder(t!("log.page.time").to_string())
                                .size(crate::design::INPUT_SIZE)
                                .cleanable(true),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests;
