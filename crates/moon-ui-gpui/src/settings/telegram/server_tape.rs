//! The station's window around a trade, in its section: what the station records with now, read
//! from its status, and "Write to the station" to change it (STATION.md §1 п. 14). Beside it, the
//! station's catch-up of missed tape: what it is doing, its counts, its two switches and how far
//! back each exchange still serves trades.
//!
//! The station's window is its own. The terminal never sends it behind the user's back — neither
//! at start nor when Settings -> Storage changes the terminal's window: a terminal on old values
//! would silently rewrite the station's, and several terminals may share one station. Only a new
//! station starts with the installing terminal's window.

use std::rc::Rc;
use std::time::Duration;

use chrono::TimeZone as _;
use chrono_tz::Tz;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use moon_core::config::storage as storage_cfg;
use moon_core::market::trade_replay::venue_caps::{self, VenueReach};
use moon_core::station_api::{CatchupSwitches, TapeCatchup, TapeWindow};
use moon_remote::ssh::Target;
use moon_remote::station::bot::BotState;
use moon_ui::{
    MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonDisclosure, MoonDisclosureDirection,
    MoonDropdown, MoonInputEvent, MoonInputState, MoonNumberInput, MoonPalette, MoonTag, MoonTone,
    h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::super::SettingsView;
use super::super::frame::{RemoteBar, SaveBar};
use crate::backend::station::job::Job;
use crate::design;

/// Shown in both steppers when the station has not reported a window.
const UNKNOWN_VALUE: &str = "—";

/// The station looks for trades without tape about once an hour after its last pass.
const PASS_EVERY_MS: i64 = 3_600_000;

/// How often a running catch-up is read again while its tab is open.
const RUNNING_REREAD: Duration = Duration::from_secs(60);

/// The window and the catch-up switches as the station reported them and as edited here.
#[derive(Default)]
pub(in crate::settings) struct TapeEd {
    /// The station's window as last read.
    seen: Option<TapeWindow>,
    /// The window as edited; follows `seen` while untouched.
    draft: Option<TapeWindow>,
    /// The station's catch-up switches as last read; `None` from a station predating them.
    seen_switches: Option<CatchupSwitches>,
    /// The switches as edited; follow `seen_switches` while untouched.
    draft_switches: Option<CatchupSwitches>,
    /// "How long exchanges keep trades" is open.
    venues_open: bool,
    /// The pending re-read of a running catch-up.
    reread: Option<Task<()>>,
    /// The long-position field, made on the first render that has a window.
    long_input: Option<Entity<MoonInputState>>,
    /// The minutes the field last showed from the draft; a draft that moved past it (a station
    /// read) is written into the field, a field that moved past it is written into the draft.
    long_shown: Option<u32>,
}

/// Take a new read into `seen`; an untouched draft follows it, an edited one stays. A read
/// without a value (the station restarting, stopped, or older than the report) changes nothing:
/// the last known value and the edit stay.
fn follow<T: Copy + PartialEq>(seen: &mut Option<T>, draft: &mut Option<T>, read: Option<T>) {
    if read.is_none() || read == *seen {
        return;
    }
    if *draft == *seen {
        *draft = read;
    }
    *seen = read;
}

/// The switches a station reports; `None` when it predates them.
fn reported_switches(catchup: &TapeCatchup) -> Option<CatchupSwitches> {
    Some(CatchupSwitches {
        auto: catchup.auto?,
        archives: catchup.archives.unwrap_or(true),
    })
}

/// What the long-position field shows after the user typed `typed`, clamped to `kept`: the same
/// number when it was in range; `None` when it was not, so the next sync writes `kept` back.
pub(super) fn long_shown_after(typed: u32, kept: u32) -> Option<u32> {
    (typed == kept).then_some(kept)
}

impl TapeEd {
    /// Take one read's switches. `None` is no status read at all and changes nothing; a status
    /// read without switches (a station rolled back, or another, older station) forgets them, so
    /// "Write to the station" never sends that station keys it would refuse.
    fn take_switches(&mut self, read: Option<Option<CatchupSwitches>>) {
        match read {
            None => {}
            Some(None) => self.forget_switches(),
            Some(switches) => follow(&mut self.seen_switches, &mut self.draft_switches, switches),
        }
    }

    /// Drop the switches as read and as edited: they belonged to another station or version.
    pub(super) fn forget_switches(&mut self) {
        self.seen_switches = None;
        self.draft_switches = None;
    }
}

/// The catch-up report of the last status read, if the station sent one.
fn reported_catchup(bot: Option<&BotState>) -> Option<&TapeCatchup> {
    bot.and_then(|state| state.station.as_ref())
        .and_then(|status| status.tape_catchup.as_deref())
}

/// The locale key of `base` for count `n`: Slavic one/few/many, one or many for the other
/// languages (each dictionary carries all three forms).
pub(in crate::settings) fn plural_key(base: &str, n: u64) -> String {
    let form = if matches!(rust_i18n::locale().as_ref(), "ru" | "uk") {
        if (11..=14).contains(&(n % 100)) {
            "many"
        } else {
            match n % 10 {
                1 => "one",
                2..=4 => "few",
                _ => "many",
            }
        }
    } else if n == 1 {
        "one"
    } else {
        "many"
    };
    format!("{base}_{form}")
}

/// The switches "Write to the station" sends: only to a station that reported them, because an
/// older station's `[tape]` refuses unknown keys.
fn job_switches(
    seen: Option<CatchupSwitches>,
    draft: Option<CatchupSwitches>,
) -> Option<CatchupSwitches> {
    seen.and(draft)
}

/// Which one line sits beside "Write to the station".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TapeStatus {
    /// The draft matches the station's window.
    Clean,
    /// The draft differs from the station's window.
    Draft,
    /// A job the user waits on is running.
    Busy,
    /// No window has been read, and the service did answer a status without one.
    Older,
    /// No window has been read because the station did not answer.
    Down,
}

/// The facts the tape line is chosen from. Names keep the four booleans from swapping.
pub(super) struct TapeFacts {
    /// A window was read, so the steppers have numbers.
    pub known: bool,
    /// The draft differs from the station's window.
    pub dirty: bool,
    /// A job the user waits on is running.
    pub busy: bool,
    /// A status was read and it carried no window: the service predates the report.
    pub older_service: bool,
}

/// Pick the tape block's one status line.
///
/// Unknown values win over a running job, because the dashes need either the danger line or
/// the older-service sentence. A running job wins over an unsaved draft, because the button
/// is already disabled. An untouched draft that matches the station is clean.
///
/// Args:
/// * `facts`: What the block knows about the window and the running job.
///
/// Returns:
/// The line to show. Exactly one.
pub(super) fn tape_status(facts: TapeFacts) -> TapeStatus {
    if !facts.known {
        return if facts.older_service {
            TapeStatus::Older
        } else {
            TapeStatus::Down
        };
    }
    if facts.busy {
        return TapeStatus::Busy;
    }
    if facts.dirty {
        return TapeStatus::Draft;
    }
    TapeStatus::Clean
}

/// What the station's catch-up of missed tape is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CatchupState {
    /// The station does not report catch-up: it predates it.
    NoData,
    /// Automatic catch-up is switched off.
    Off,
    /// A pass is fetching now.
    Running,
    /// The last pass left trades it could not download.
    Errors,
    /// Trades are queued for the next pass.
    Waiting,
    /// Nothing within reach is missing.
    Done,
}

/// Pick the catch-up state, in this priority: no data, off, running, errors, waiting, done. A
/// queue never reads as "all caught up".
pub(super) fn catchup_state(catchup: Option<&TapeCatchup>) -> CatchupState {
    let Some(catchup) = catchup else {
        return CatchupState::NoData;
    };
    if catchup.auto == Some(false) {
        CatchupState::Off
    } else if catchup.running {
        CatchupState::Running
    } else if catchup.failed > 0 {
        CatchupState::Errors
    } else if catchup.pending > 0 {
        CatchupState::Waiting
    } else {
        CatchupState::Done
    }
}

/// Wall-clock `HH:MM` of a UTC instant in `zone`.
pub(super) fn clock(ms: i64, zone: Tz) -> String {
    zone.timestamp_millis_opt(ms)
        .single()
        .map(|at| at.format("%H:%M").to_string())
        .unwrap_or_else(|| UNKNOWN_VALUE.to_string())
}

/// The state's tag word and tone.
fn catchup_tag(state: CatchupState) -> (&'static str, MoonTone) {
    match state {
        CatchupState::NoData => ("telegram.server.catchup_tag_nodata", MoonTone::Muted),
        CatchupState::Off => ("telegram.server.catchup_tag_off", MoonTone::Muted),
        CatchupState::Running => ("telegram.server.catchup_tag_running", MoonTone::Info),
        CatchupState::Errors => ("telegram.server.catchup_tag_errors", MoonTone::Warning),
        CatchupState::Waiting => ("telegram.server.catchup_tag_waiting", MoonTone::Info),
        CatchupState::Done => ("telegram.server.catchup_tag_done", MoonTone::Positive),
    }
}

/// The one plain sentence beside the tag. The next check is an approximation — the station looks
/// again about an hour after its last pass — and a failure promises no retry time.
pub(super) fn catchup_sentence(
    state: CatchupState,
    catchup: Option<&TapeCatchup>,
    zone: Tz,
) -> String {
    let Some(c) = catchup else {
        return t!("telegram.server.catchup_nodata").to_string();
    };
    let last = c.last_pass_ms;
    let next = last.map(|last| clock(last + PASS_EVERY_MS, zone));
    match state {
        CatchupState::NoData => t!("telegram.server.catchup_nodata").to_string(),
        CatchupState::Off => t!("telegram.server.catchup_off").to_string(),
        CatchupState::Running => t!(
            &plural_key("telegram.server.catchup_running", c.pending.into()),
            filled = c.filled,
            pending = c.pending
        )
        .to_string(),
        CatchupState::Errors => t!(
            &plural_key("telegram.server.catchup_errors", c.failed.into()),
            n = c.failed
        )
        .to_string(),
        CatchupState::Waiting => match next {
            Some(next) => t!(
                &plural_key("telegram.server.catchup_waiting", c.pending.into()),
                n = c.pending,
                next = next
            )
            .to_string(),
            None => t!(
                &plural_key("telegram.server.catchup_waiting_first", c.pending.into()),
                n = c.pending
            )
            .to_string(),
        },
        CatchupState::Done => match (last, next) {
            (Some(last), Some(next)) => t!(
                "telegram.server.catchup_done",
                last = clock(last, zone),
                next = next
            )
            .to_string(),
            _ => t!("telegram.server.catchup_done_never").to_string(),
        },
    }
}

/// One count tile: its number, its word and the short reason under it (locale keys).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Tile {
    pub(super) value: u32,
    pub(super) label: &'static str,
    pub(super) hint: &'static str,
}

/// Up to four counts; "Failed" takes the last tile from "No source" while the last pass failed.
/// None while catch-up is off or unreported.
pub(super) fn catchup_tiles(state: CatchupState, catchup: &TapeCatchup) -> Vec<Tile> {
    if matches!(state, CatchupState::NoData | CatchupState::Off) {
        return Vec::new();
    }
    let tile = |value, label, hint| Tile { value, label, hint };
    let last = if catchup.failed > 0 {
        tile(
            catchup.failed,
            "telegram.server.catchup_tile_failed",
            "telegram.server.catchup_tile_failed_hint",
        )
    } else {
        tile(
            catchup.no_route,
            "telegram.server.catchup_tile_no_route",
            "telegram.server.catchup_tile_no_route_hint",
        )
    };
    vec![
        tile(
            catchup.filled,
            "telegram.server.catchup_tile_filled",
            "telegram.server.catchup_tile_filled_hint",
        ),
        tile(
            catchup.pending,
            "telegram.server.catchup_tile_pending",
            "telegram.server.catchup_tile_pending_hint",
        ),
        tile(
            catchup.out_of_reach,
            "telegram.server.catchup_tile_out_of_reach",
            "telegram.server.catchup_tile_out_of_reach_hint",
        ),
        last,
    ]
}

/// Why the last pass's jobs failed, in plain words with a count each — `None` while nothing
/// failed or the station is too old to say.
pub(super) fn catchup_breakdown(catchup: &TapeCatchup) -> Option<String> {
    if catchup.failed == 0 || catchup.failed_by.is_empty() {
        return None;
    }
    let parts = catchup
        .failed_by
        .iter()
        .map(|(reason, n)| {
            let key = format!("telegram.server.catchup_reason_{}", reason.key());
            format!("{} — {n}", t!(&key))
        })
        .collect::<Vec<_>>()
        .join(" · ");
    Some(format!("{} {parts}", t!("telegram.server.catchup_why")))
}

/// The breakdown under the tiles: one muted wrapping text, never a row of separate pieces.
pub(super) fn tape_breakdown(text: String, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    div()
        .w_full()
        .min_w_0()
        .font_family(design::ui_font())
        .text_size(design::t_caption(cx))
        .text_color(rgba_from(p.text_muted, 1.0))
        .debug_selector(|| "server-tape-breakdown".into())
        .child(text)
        .into_any_element()
}

/// The tag, the sentence and how old the numbers are.
pub(super) fn tape_status_strip(
    state: CatchupState,
    sentence: String,
    snapshot: Option<String>,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let (tag, tone) = catchup_tag(state);
    h_flex()
        .w_full()
        .min_w_0()
        .flex_wrap()
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .debug_selector(|| "server-tape-strip".into())
        .child(
            div().flex_none().child(
                MoonTag::new().tone(tone).mono(false).child(
                    div()
                        .font_family(design::ui_font())
                        .child(t!(tag).to_string()),
                ),
            ),
        )
        .child(
            div()
                .flex_1()
                .flex_basis(design::ui_px(cx, 200.0))
                .min_w_0()
                .font_family(design::ui_font())
                .text_color(rgba_from(p.text, 1.0))
                .debug_selector(|| "server-tape-sentence".into())
                .child(sentence)
                .children(snapshot.map(|at| {
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(rgba_from(p.text_muted, 1.0))
                        .child(at)
                })),
        )
        .into_any_element()
}

/// How many equal tiles go in one row: four where `width` holds four of the widest label, two
/// where it holds two, else one.
fn tile_columns(width: f32, tile: f32, gap: f32, count: usize) -> usize {
    [4, 2]
        .into_iter()
        .find(|&cols| cols as f32 * tile + (cols - 1) as f32 * gap <= width)
        .unwrap_or(1)
        .min(count.max(1))
}

/// The count tiles in rows of equal-width, equal-height tiles; the column count comes from
/// `width`, so the row reserves exactly the height it paints.
pub(super) fn tape_tiles(tiles: &[Tile], width: f32, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    let gap = design::ui_value(cx, 8.0);
    let pad = design::ui_value(cx, 10.0);
    let widest = tiles
        .iter()
        .map(|tile| design::ui_body_text_width(cx, &t!(tile.label), 400.0))
        .fold(0.0_f32, f32::max);
    let tile_min = (widest + 2.0 * pad + 2.0).max(design::font_w(cx, 150.0));
    let cols = tile_columns(width, tile_min, gap, tiles.len());
    let tile_el = |index: usize, tile: &Tile| {
        v_flex()
            .flex_1()
            .flex_basis(px(0.0))
            .min_w_0()
            .gap(design::ui_px(cx, 2.0))
            .px(px(pad))
            .py(design::ui_px(cx, 6.0))
            .rounded(design::ui_px(cx, 5.0))
            .border_1()
            .border_color(rgba_from(p.border, 1.0))
            .debug_selector(move || format!("server-tape-tile-{index}"))
            // The mockup's tile: a small muted caption over a prominent mono value.
            .child(
                div()
                    .min_w_0()
                    .font_family(design::ui_font())
                    .text_size(design::t_caption(cx))
                    .text_color(rgba_from(p.text_muted, 1.0))
                    .debug_selector(move || format!("server-tape-tile-label-{index}"))
                    .child(t!(tile.label).to_string()),
            )
            .child(
                div()
                    .font_family(design::mono())
                    .text_size(design::t_title(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgba_from(p.text, 1.0))
                    .child(tile.value.to_string()),
            )
            .child(
                div()
                    .min_w_0()
                    .font_family(design::ui_font())
                    .text_size(design::t_caption(cx))
                    .text_color(rgba_from(p.text_muted, 1.0))
                    .child(t!(tile.hint).to_string()),
            )
            .into_any_element()
    };
    v_flex()
        .w_full()
        .min_w_0()
        .gap(px(gap))
        .debug_selector(|| "server-tape-tiles".into())
        .children(tiles.chunks(cols).enumerate().map(|(row, chunk)| {
            h_flex()
                .w_full()
                .min_w_0()
                .items_stretch()
                .gap(px(gap))
                .children(
                    chunk
                        .iter()
                        .enumerate()
                        .map(|(i, tile)| tile_el(row * cols + i, tile)),
                )
                // A short last row keeps the tile width of the rows above.
                .children((chunk.len()..cols).map(|_| {
                    div()
                        .flex_1()
                        .flex_basis(px(0.0))
                        .min_w_0()
                        .into_any_element()
                }))
        }))
        .into_any_element()
}

/// How far back a venue's REST trades reach, in words.
fn venue_rest(reach: &VenueReach) -> String {
    const HOUR_MS: i64 = 3_600_000;
    const DAY_MS: i64 = 24 * HOUR_MS;
    match (reach.rest, reach.rest_retention_ms) {
        (None, _) => t!("telegram.server.catchup_rest_none").to_string(),
        (Some(_), None) => t!("telegram.server.catchup_rest_unbounded").to_string(),
        (Some(_), Some(ms)) if ms >= DAY_MS => {
            let n = ms / DAY_MS;
            t!(
                &plural_key("telegram.server.catchup_rest_days", n.unsigned_abs()),
                n = n
            )
            .to_string()
        }
        (Some(_), Some(ms)) => {
            let n = ms / HOUR_MS;
            t!(
                &plural_key("telegram.server.catchup_rest_hours", n.unsigned_abs()),
                n = n
            )
            .to_string()
        }
    }
}

/// What reaches trades older than the API does: the daily archive, or nothing. A venue whose
/// API has no limit needs nothing older.
fn venue_older(reach: &VenueReach) -> String {
    match reach.archive {
        Some(_) if reach.archive_linear_only => {
            t!("telegram.server.catchup_archive_linear").to_string()
        }
        Some(_) => t!("telegram.server.catchup_archive_years").to_string(),
        None if reach.rest.is_some() && reach.rest_retention_ms.is_none() => {
            UNKNOWN_VALUE.to_string()
        }
        None => t!("telegram.server.catchup_archive_none").to_string(),
    }
}

/// Three columns only when the measured venue, reach and archive columns all fit `width`.
fn venue_fits(width: f32, venue: f32, rest: f32, older: f32, gaps: f32) -> bool {
    width >= venue + rest + older + gaps
}

/// "How long exchanges keep trades": a caret line and, open, one row per venue from the core's
/// own reach facts — a three-column table where it fits `width`, otherwise each venue stacked
/// over its reach.
pub(super) fn venue_reach_table(
    open: bool,
    rows: &[VenueReach],
    width: f32,
    on_toggle: Rc<dyn Fn(&mut Window, &mut App)>,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let muted = rgba_from(p.text_muted, 1.0);
    let caret_toggle = on_toggle.clone();
    let caret = h_flex()
        .id("server-tape-venues")
        .flex_none()
        .max_w_full()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .cursor_pointer()
        .text_color(muted)
        .debug_selector(|| "server-tape-venues".into())
        .on_click(move |_, window, app| on_toggle(window, app))
        .child(
            MoonDisclosure::button("server-tape-venues-caret", open)
                .direction(MoonDisclosureDirection::RightDown)
                .size(design::DISCLOSURE_GLYPH)
                .box_size(design::DISCLOSURE_BOX)
                .hover_color(p.text)
                .on_toggle(move |_, window, app| {
                    caret_toggle(window, app);
                    app.stop_propagation();
                }),
        )
        .child(
            div()
                .min_w_0()
                .font_family(design::ui_font())
                .child(t!("telegram.server.catchup_venues").to_string()),
        );
    let block = v_flex()
        .w_full()
        .min_w_0()
        .gap(design::ui_px(cx, 4.0))
        .child(caret);
    if !open {
        return block.into_any_element();
    }
    let rows: Vec<(String, String, String)> = rows
        .iter()
        .map(|reach| {
            (
                format!(
                    "{} {}",
                    reach.venue.brand.display(),
                    t!(reach.venue.kind.label_key())
                ),
                venue_rest(reach),
                venue_older(reach),
            )
        })
        .collect();
    let col_venue = t!("telegram.server.catchup_venues_col_venue").to_string();
    let col_rest = t!("telegram.server.catchup_venues_col_rest").to_string();
    let col_older = t!("telegram.server.catchup_venues_col_older").to_string();
    let widest = |head: &str, cell: &dyn Fn(&(String, String, String)) -> f32| {
        rows.iter()
            .map(cell)
            .fold(design::ui_caption_text_width(cx, head, 400.0), f32::max)
            .ceil()
    };
    let venue_need = widest(&col_venue, &|row| {
        design::ui_body_text_width(cx, &row.0, 400.0)
    });
    let rest_need = widest(&col_rest, &|row| {
        design::mono_body_text_width(cx, &row.1, 400.0)
    });
    let older_need = widest(&col_older, &|row| {
        design::ui_body_text_width(cx, &row.2, 400.0)
    });
    let indent = design::ui_value(cx, 18.0);
    let gap = design::ui_value(cx, 8.0);
    let wide = venue_fits(width - indent, venue_need, rest_need, older_need, 2.0 * gap);
    let table = v_flex()
        .w_full()
        .min_w_0()
        .gap(design::ui_px(cx, 2.0))
        .pl(px(indent))
        .debug_selector(|| "server-tape-venue-table".into());
    let table = if wide {
        let line = |venue: String, rest: String, older: String, rest_font: SharedString| {
            h_flex()
                .w_full()
                .min_w_0()
                .items_start()
                .gap(px(gap))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_family(design::ui_font())
                        .child(venue),
                )
                .child(
                    div()
                        .flex_none()
                        .w(px(rest_need))
                        .font_family(rest_font)
                        .child(rest),
                )
                .child(
                    div()
                        .flex_none()
                        .w(px(older_need))
                        .font_family(design::ui_font())
                        .child(older),
                )
        };
        let head = line(col_venue, col_rest, col_older, design::ui_font())
            .text_size(design::t_caption(cx))
            .text_color(muted);
        table
            .child(head)
            .children(rows.into_iter().map(|(venue, rest, older)| {
                line(venue, rest, older, design::mono()).text_color(rgba_from(p.text, 1.0))
            }))
    } else {
        // Too narrow for three columns: the venue on one line, its reach under it, each value
        // named by its column's word.
        let caption = |text: String| {
            div()
                .flex_none()
                .font_family(design::ui_font())
                .text_size(design::t_caption(cx))
                .text_color(muted)
                .child(text)
        };
        table
            .gap(design::ui_px(cx, 6.0))
            .children(
                rows.into_iter()
                    .enumerate()
                    .map(|(index, (venue, rest, older))| {
                        v_flex()
                            .w_full()
                            .min_w_0()
                            .debug_selector(move || format!("server-tape-venue-{index}"))
                            .child(
                                div()
                                    .min_w_0()
                                    .font_family(design::ui_font())
                                    .text_color(rgba_from(p.text, 1.0))
                                    .child(venue),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .min_w_0()
                                    .flex_wrap()
                                    .items_start()
                                    .gap_x(px(gap))
                                    .child(caption(col_rest.clone()))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .font_family(design::mono())
                                            .text_color(rgba_from(p.text, 1.0))
                                            .child(rest),
                                    )
                                    .child(caption(col_older.clone()))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .font_family(design::ui_font())
                                            .text_color(rgba_from(p.text, 1.0))
                                            .child(older),
                                    ),
                            )
                    }),
            )
    };
    block.child(table).into_any_element()
}

/// The two catch-up switches. Archives need automatic catch-up; a station that reports counts
/// but predates the switches shows them unchangeable.
pub(super) fn catchup_switches(
    switches: CatchupSwitches,
    editable: bool,
    on_auto: impl Fn(&bool, &mut Window, &mut App) + 'static,
    on_archives: impl Fn(&bool, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    v_flex()
        .flex_1()
        .flex_basis(design::font_w_px(cx, 220.0))
        .min_w_0()
        .max_w_full()
        .gap(design::ui_px(cx, 8.0))
        .debug_selector(|| "server-tape-switches".into())
        .child(
            MoonCheckbox::new("server-tape-auto")
                .checked(switches.auto)
                .disabled(!editable)
                .label(t!("telegram.server.catchup_auto").to_string())
                .description(t!("telegram.server.catchup_auto_hint").to_string())
                .on_change(on_auto),
        )
        .child(
            MoonCheckbox::new("server-tape-archives")
                .checked(switches.archives)
                .disabled(!editable || !switches.auto)
                .label(t!("telegram.server.catchup_archives").to_string())
                .description(
                    if switches.auto {
                        t!("telegram.server.catchup_archives_hint")
                    } else {
                        t!("telegram.server.catchup_archives_needs_auto")
                    }
                    .to_string(),
                )
                .on_change(on_archives),
        )
        .into_any_element()
}

/// The page that renders the tape block, whose open state keeps a running catch-up re-read.
pub(super) fn shows_tape_block(page: super::super::Tab) -> bool {
    page == super::super::Tab::StOverview
}

impl SettingsView {
    /// A new read of the station's window and catch-up switches: an untouched draft follows it,
    /// an edited one stays. A read without one changes nothing.
    ///
    /// While a catch-up pass runs and the page showing the tape block is open, the station is
    /// read again a minute later, so the numbers move without a click.
    pub(in crate::settings) fn server_tape_sync(&mut self, cx: &mut Context<Self>) {
        let (read, switches, running) = {
            let bot = self.backend.read(cx).station.bot.as_ref();
            let catchup = reported_catchup(bot);
            (
                bot.and_then(|state| state.tape),
                bot.and_then(|state| state.station.as_ref())
                    .map(|_| catchup.and_then(reported_switches)),
                catchup_state(catchup) == CatchupState::Running,
            )
        };
        let shown = shows_tape_block(self.active);
        let ed = &mut self.telegram.server.tape;
        follow(&mut ed.seen, &mut ed.draft, read);
        ed.take_switches(switches);
        if running && shown && ed.reread.is_none() {
            ed.reread = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(RUNNING_REREAD).await;
                let _ = this.update(cx, |this, cx| {
                    this.telegram.server.tape.reread = None;
                    if shows_tape_block(this.active) {
                        this.backend.update(cx, |b, bcx| b.station_refresh_bot(bcx));
                    }
                });
            }));
        }
    }

    fn server_tape_margin(&mut self, secs: u32, cx: &mut Context<Self>) {
        if let Some(draft) = self.telegram.server.tape.draft.as_mut() {
            draft.margin_s = storage_cfg::snap_trade_margin_s(secs);
            cx.notify();
        }
    }

    /// Keep the long-position field and the draft in step: made once, subscribed once, and
    /// rewritten only when a station read moved the draft.
    pub(in crate::settings) fn server_tape_input_sync(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let draft = self.telegram.server.tape.draft.map(|d| d.long_position_min);
        let input = match self.telegram.server.tape.long_input.clone() {
            Some(input) => input,
            None => {
                let input =
                    super::super::common::number_state(draft.unwrap_or_default(), window, cx);
                cx.subscribe(&input, |this, state, ev: &MoonInputEvent, cx| {
                    if !matches!(ev, MoonInputEvent::Change) {
                        return;
                    }
                    let Some(v) = MoonNumberInput::read_value(&state, cx) else {
                        return;
                    };
                    let typed = v.round().max(0.0) as u32;
                    let v = storage_cfg::clamp_long_position_min(typed);
                    let ed = &mut this.telegram.server.tape;
                    if let Some(draft) = ed.draft.as_mut() {
                        draft.long_position_min = v;
                        // An out-of-range number is shown clamped: forgetting what the field
                        // shows makes the next render write the draft's value back into it.
                        ed.long_shown = long_shown_after(typed, v);
                        cx.notify();
                    }
                })
                .detach();
                self.telegram.server.tape.long_input = Some(input.clone());
                self.telegram.server.tape.long_shown = draft;
                input
            }
        };
        if let Some(v) = draft
            && self.telegram.server.tape.long_shown != Some(v)
        {
            self.telegram.server.tape.long_shown = Some(v);
            input.update(cx, |st, cx| st.set_value(v.to_string(), window, cx));
        }
    }

    /// The window pick: one of the margin steps, as on the Storage page.
    fn server_tape_margin_picker(
        &self,
        secs: u32,
        enabled: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let current = storage_cfg::snap_trade_margin_s(secs);
        let steps = storage_cfg::TRADE_MARGIN_STEPS_S;
        let selected = steps.iter().position(|step| *step == current).unwrap_or(0);
        let options = steps
            .iter()
            .copied()
            .enumerate()
            .map(|(i, step)| {
                (
                    i,
                    SharedString::from(format!("server-tape-margin-{step}")),
                    SharedString::from(Self::trades_margin_label(step)),
                )
            })
            .collect::<Vec<_>>();
        let view = cx.entity().downgrade();
        let items = crate::panels::radio_items(
            options,
            selected,
            crate::panels::RadioMark::Check,
            move |app, row: usize| {
                let secs = storage_cfg::TRADE_MARGIN_STEPS_S
                    .get(row)
                    .copied()
                    .unwrap_or(current);
                let _ = view.update(app, |this, cx| this.server_tape_margin(secs, cx));
            },
        );
        MoonDropdown::new("server-tape-margin")
            .label(SharedString::from(Self::trades_margin_label(current)))
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::density(cx))
            .trigger_width(design::font_w(cx, 120.0))
            .menu_width_scaled(140.0)
            .disabled(!enabled)
            .items(items)
            .into_any_element()
    }

    fn server_tape_switch(&mut self, edit: impl FnOnce(&mut CatchupSwitches)) {
        if let Some(draft) = self.telegram.server.tape.draft_switches.as_mut() {
            edit(draft);
        }
    }

    /// The station's trade window and its catch-up of missed tape: the state and its counts,
    /// the two steppers, the two switches, how long exchanges keep trades, and whether the draft
    /// has been written. "Write to the station" stays live only while the draft differs from
    /// what the station reported.
    ///
    /// `width` is the settings column's width, which decides the venue table's shape.
    /// The Trade tape page's one write bar: the draft's window and catch-up switches go to the
    /// station together, only while they differ from what it last reported and no job runs.
    pub(in crate::settings) fn station_tape_bar(&self, cx: &Context<Self>) -> SaveBar {
        let Some(target) = self.telegram.server.known().cloned() else {
            return SaveBar::None;
        };
        let ed = &self.telegram.server.tape;
        let Some(draft) = ed.draft else {
            return SaveBar::None;
        };
        let busy = self.backend.read(cx).station.busy();
        let changed = ed.seen != Some(draft) || ed.draft_switches != ed.seen_switches;
        let catchup = job_switches(ed.seen_switches, ed.draft_switches);
        let view = cx.entity().downgrade();
        SaveBar::Remote(RemoteBar {
            msg: t!(if changed {
                "settings.savebar.dirty_other"
            } else {
                "settings.mode.remote"
            })
            .to_string(),
            dirty: changed && !busy,
            apply: std::rc::Rc::new(move |_, app| {
                let job = Job::Tape {
                    target: target.clone(),
                    tape: draft,
                    catchup,
                };
                let _ = view.update(app, |this, cx| this.server_bot_run(Ok(job), cx));
            }),
            revert: None,
            label: t!("telegram.server.tape_set").to_string(),
        })
    }

    pub(super) fn server_tape_block(
        &self,
        target: &Target,
        width: f32,
        cx: &Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let backend = self.backend.read(cx);
        let busy = backend.station.busy();
        let job_name = backend
            .station
            .running_label
            .map(|label| label.text())
            .unwrap_or_default();
        let ed = &self.telegram.server.tape;
        let older = ed.draft.is_none()
            && backend
                .station
                .bot
                .as_ref()
                .is_some_and(|bot| bot.station.is_some() && bot.tape.is_none());
        let known = ed.draft.is_some();
        let switches_changed = ed.draft_switches != ed.seen_switches;
        let changed = ed
            .draft
            .is_some_and(|draft| ed.seen != Some(draft) || switches_changed);
        let status = tape_status(TapeFacts {
            known,
            dirty: changed,
            busy,
            older_service: older,
        });
        let sentence = |text: String, color: u32| {
            div()
                .font_family(design::ui_font())
                .text_size(design::t_caption(cx))
                .text_color(rgba_from(color, 1.0))
                .child(text)
        };
        let (status_text, status_color) = match status {
            TapeStatus::Clean => (
                t!("telegram.server.tape_clean").to_string(),
                design::positive_color(p),
            ),
            TapeStatus::Draft => {
                let margin = ed
                    .seen
                    .map(|window| Self::trades_margin_label(window.margin_s))
                    .unwrap_or_else(|| UNKNOWN_VALUE.to_string());
                let long = ed
                    .seen
                    .map(|window| {
                        t!("storage.trades_min", min = window.long_position_min).to_string()
                    })
                    .unwrap_or_else(|| UNKNOWN_VALUE.to_string());
                (
                    t!("telegram.server.tape_draft", margin = margin, long = long).to_string(),
                    p.amber,
                )
            }
            TapeStatus::Busy => (
                t!("telegram.server.tape_busy", job = job_name).to_string(),
                p.text_muted,
            ),
            TapeStatus::Down => {
                let status_button = t!("telegram.server.status").to_string();
                (
                    t!("telegram.server.tape_down", status = status_button).to_string(),
                    design::danger_color(p),
                )
            }
            TapeStatus::Older => (t!("telegram.server.tape_unknown").to_string(), p.text_muted),
        };
        let row = |label: String, control: AnyElement, explanation: Option<String>| {
            h_flex()
                .w_full()
                .min_w(px(0.0))
                .flex_wrap()
                .gap(design::ui_px(cx, 12.0))
                .items_start()
                .child(
                    div()
                        .w(design::font_w_px(cx, 190.0))
                        .max_w_full()
                        .flex_shrink_0()
                        .pt(design::ui_px(cx, 4.0))
                        .font_family(design::ui_font())
                        .text_color(rgba_from(p.text, 1.0))
                        .child(label),
                )
                .child(control)
                .children(explanation.map(|explanation| {
                    div()
                        .flex_1()
                        .min_w(design::font_w_px(cx, 220.0))
                        .max_w_full()
                        .pt(design::ui_px(cx, 4.0))
                        .child(sentence(explanation, p.text_muted))
                }))
        };
        let _ = target;
        let catchup = reported_catchup(backend.station.bot.as_ref());
        let state = catchup_state(catchup);
        let zone = crate::chartdx::axes::display_zone();
        let snapshot = backend
            .station
            .bot_read_ms
            .filter(|_| state != CatchupState::NoData)
            .map(|at| t!("telegram.server.catchup_snapshot", at = clock(at, zone)).to_string());
        let tiles = catchup
            .map(|catchup| catchup_tiles(state, catchup))
            .unwrap_or_default();
        let breakdown = catchup
            .filter(|_| !tiles.is_empty())
            .and_then(catchup_breakdown);
        let switches = catchup.map(|_| {
            let view = cx.entity();
            let archives_view = view.clone();
            catchup_switches(
                ed.draft_switches.unwrap_or(CatchupSwitches {
                    auto: true,
                    archives: true,
                }),
                ed.draft_switches.is_some() && !busy,
                move |on, _, app| {
                    let on = *on;
                    view.update(app, |this, cx| {
                        this.server_tape_switch(|s| s.auto = on);
                        cx.notify();
                    });
                },
                move |on, _, app| {
                    let on = *on;
                    archives_view.update(app, |this, cx| {
                        this.server_tape_switch(|s| s.archives = on);
                        cx.notify();
                    });
                },
                cx,
            )
        });
        let view = cx.entity();
        let venues = venue_reach_table(
            ed.venues_open,
            &if ed.venues_open {
                venue_caps::reach_table()
            } else {
                Vec::new()
            },
            width - design::ui_value(cx, 24.0),
            Rc::new(move |_, app| {
                view.update(app, |this, cx| {
                    this.telegram.server.tape.venues_open ^= true;
                    cx.notify();
                })
            }),
            cx,
        );
        v_flex()
            .w_full()
            .min_w(px(0.0))
            .gap(design::ui_px(cx, 10.0))
            .font_family(design::ui_font())
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgba_from(p.text, 1.0))
                    .child(t!("telegram.server.tape_title").to_string()),
            )
            .child(sentence(
                t!("telegram.server.tape_hint").to_string(),
                p.text_muted,
            ))
            .child(tape_status_strip(
                state,
                catchup_sentence(state, catchup, zone),
                snapshot,
                cx,
            ))
            .when(!tiles.is_empty(), |block| {
                block.child(tape_tiles(&tiles, width - design::ui_value(cx, 24.0), cx))
            })
            .when_some(breakdown, |block, text| {
                block.child(tape_breakdown(text, cx))
            })
            .child(row(
                t!("storage.trades_margin").to_string(),
                match ed.draft {
                    Some(draft) => {
                        self.server_tape_margin_picker(draft.margin_s, known && !busy, cx)
                    }
                    None => sentence(UNKNOWN_VALUE.to_string(), p.text_muted).into_any_element(),
                },
                Some(t!("telegram.server.tape_margin_hint").to_string()),
            ))
            .child(row(
                t!("storage.trades_long_position").to_string(),
                match (&ed.long_input, ed.draft) {
                    (Some(input), Some(_)) => super::super::general::num_input_on(
                        "server-tape-long",
                        input,
                        {
                            let r = storage_cfg::LONG_POSITION_MIN_RANGE;
                            f64::from(*r.start())..=f64::from(*r.end())
                        },
                        1.0,
                        None,
                        known && !busy,
                    ),
                    _ => sentence(UNKNOWN_VALUE.to_string(), p.text_muted).into_any_element(),
                },
                Some(t!("telegram.server.tape_long_hint").to_string()),
            ))
            .when_some(switches, |block, switches| {
                block.child(row(
                    t!("telegram.server.catchup_section").to_string(),
                    switches,
                    None,
                ))
            })
            .child(venues)
            .child(
                h_flex()
                    .w_full()
                    .min_w(px(0.0))
                    .flex_wrap()
                    .items_center()
                    .gap(design::ui_px(cx, 12.0))
                    .pt(design::ui_px(cx, 8.0))
                    .border_t_1()
                    .border_color(rgba_from(p.border, 1.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(design::font_w_px(cx, 220.0))
                            .child(sentence(status_text, status_color)),
                    ),
            )
            .child(sentence(
                t!("telegram.server.tape_footnote").to_string(),
                p.text_muted,
            ))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests;
