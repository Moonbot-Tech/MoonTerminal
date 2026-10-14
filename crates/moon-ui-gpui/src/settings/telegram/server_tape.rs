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
use moon_core::station_api::{CatchupSwitches, TapeCatchup, TapeKeep, TapePurge, TapeWindow};
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

mod block;
mod keep;
use keep::{KeepState, job_keep, keep_state, purge_sentence, removes_tape, reported_keep};

/// Shown in both steppers when the station has not reported a window.
const UNKNOWN_VALUE: &str = "—";

/// The station looks for trades without tape about once an hour after its last pass.
const PASS_EVERY_MS: i64 = 3_600_000;

/// How often a running catch-up is read again while its tab is open.
const RUNNING_REREAD: Duration = Duration::from_secs(60);

/// The window and the catch-up switches as the station reported them and as edited here.
#[derive(Default)]
pub(in crate::settings) struct TapeEd {
    /// Retention capability and the last acknowledged policy.
    seen_keep: Option<TapeKeep>,
    /// Retention policy edited for this target alone.
    draft_keep: Option<TapeKeep>,
    /// A destructive write needs the inline confirmation.
    confirm: bool,
    /// Narrowing waits for a fresh policy or changed purge report before polling stops.
    awaiting_keep: Option<(TapeKeep, Option<TapePurge>)>,
    /// The user-job completion counter at dispatch distinguishes a failed write from an old error.
    awaiting_finished: Option<u64>,
    /// The custom days control and the last draft value reflected into it.
    days_input: Option<Entity<MoonInputState>>,
    days_shown: Option<u16>,
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
    /// Rewrite only externally changed days; typed preset prefixes remain intact mid-entry.
    fn days_writeback(&mut self) -> Option<String> {
        let days = self.draft_keep.map(|keep| keep.max_age_days);
        if days == self.days_shown {
            return None;
        }
        self.days_shown = days;
        Some(
            days.filter(|days| *days != 0)
                .map(|days| days.to_string())
                .unwrap_or_default(),
        )
    }
    /// Prepare the same station job for the write bar and inline confirmation; cancellation
    /// leaves the draft intact, and only a confirmed narrowing creates a purge polling intent.
    fn prepare_write(
        &mut self,
        target: Target,
        confirmed: bool,
        purge: Option<TapePurge>,
    ) -> Option<Job> {
        let tape = self.draft?;
        let destructive = self
            .seen_keep
            .zip(self.draft_keep)
            .is_some_and(|(seen, draft)| removes_tape(seen, draft));
        if destructive && !confirmed {
            self.confirm = true;
            return None;
        }
        if confirmed && !self.confirm {
            return None;
        }
        let keep = job_keep(self.seen_keep, self.draft_keep);
        if destructive && let Some(keep) = keep {
            self.awaiting_keep = Some((keep, purge));
        }
        self.confirm = false;
        Some(Job::Tape {
            target,
            tape,
            catchup: job_switches(self.seen_switches, self.draft_switches),
            keep,
        })
    }
    /// A missing status preserves edits; a legacy status removes unsupported keys.
    fn take_keep(&mut self, read: Option<Option<TapeKeep>>) {
        match read {
            None => {}
            Some(None) => self.forget_keep(),
            Some(value) => follow(&mut self.seen_keep, &mut self.draft_keep, value),
        }
        if self
            .seen_keep
            .zip(self.draft_keep)
            .is_some_and(|(seen, draft)| !removes_tape(seen, draft))
        {
            self.confirm = false;
        }
    }

    /// Forget target-bound retention edits, confirmation and polling intent.
    pub(super) fn forget_keep(&mut self) {
        if self.seen_keep.is_some() || self.draft_keep.is_some() {
            self.days_input = None;
        }
        self.seen_keep = None;
        self.draft_keep = None;
        self.confirm = false;
        self.awaiting_keep = None;
        self.awaiting_finished = None;
        self.days_shown = None;
        self.reread = None;
    }
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

pub(in crate::settings) use crate::display_text::plural_key;

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
    /// Retention is deleting or compacting before catch-up can continue.
    Purging,
    /// Retention deletion or compaction failed.
    PurgeFailed,
    /// A completed purge has rebuilt a queue awaiting the next pass.
    Purged,
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

/// Pick the catch-up state: no report, maintenance or a freshly rebuilt purge queue, then the
/// automatic switch and pass state. A queue never reads as "all caught up".
pub(super) fn catchup_state(catchup: Option<&TapeCatchup>) -> CatchupState {
    let Some(catchup) = catchup else {
        return CatchupState::NoData;
    };
    match keep_state(Some(catchup)) {
        KeepState::Purging { .. } => return CatchupState::Purging,
        KeepState::Failed { reason } if reason != moon_core::station_api::PurgeFail::Deferred => {
            return CatchupState::PurgeFailed;
        }
        KeepState::Done { .. } if catchup.pending > 0 && catchup.auto != Some(false) => {
            return CatchupState::Purged;
        }
        _ => {}
    }
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
        CatchupState::Purging => ("telegram.server.keep_tag_purging", MoonTone::Info),
        CatchupState::PurgeFailed => ("telegram.server.catchup_tag_errors", MoonTone::Danger),
        CatchupState::Purged => ("telegram.server.catchup_tag_waiting", MoonTone::Warning),
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
    if let Some(text) = purge_sentence(c, zone) {
        return text;
    }
    let next = last.map(|last| clock(last + PASS_EVERY_MS, zone));
    match state {
        CatchupState::Purging | CatchupState::PurgeFailed | CatchupState::Purged => {
            unreachable!("purge sentence handled above")
        }
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

/// One station tile: a count or disk size, localized label, hint and maintenance accent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Tile {
    pub(super) value: TileValue,
    pub(super) label: &'static str,
    pub(super) hint: TileHint,
    pub(super) accent: bool,
}

/// Counts remain numeric; disk sizes are already localized text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TileValue {
    Count(u32),
    Text(String),
}

/// Keep count selection locale-free; only the rendered tile resolves static hint keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TileHint {
    Key(&'static str),
    Text(String),
}

impl TileHint {
    /// Resolve static labels at rendering time and preserve already formatted station facts.
    fn text(&self) -> String {
        match self {
            Self::Key(key) => t!(*key).to_string(),
            Self::Text(text) => text.clone(),
        }
    }
}

impl std::fmt::Display for TileValue {
    /// Render the prominent tile value without parsing a localized size as a count.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Count(n) => f.write_str(&moon_core::util::fmt::group_thousands(&n.to_string())),
            Self::Text(text) => f.write_str(text),
        }
    }
}

/// Up to four counts; "Failed" takes the last tile from "No source" while the last pass failed.
/// None while catch-up is off or unreported.
pub(super) fn catchup_tiles(state: CatchupState, catchup: &TapeCatchup) -> Vec<Tile> {
    if matches!(state, CatchupState::NoData | CatchupState::Off) {
        return Vec::new();
    }
    let tile = |value, label, hint| Tile {
        value: TileValue::Count(value),
        label,
        hint: TileHint::Key(hint),
        accent: false,
    };
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
    let mut tiles = vec![
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
    ];
    if state == CatchupState::Purging {
        tiles[1].value = TileValue::Text(UNKNOWN_VALUE.into());
    }
    tiles
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
    crate::settings::common::cap_prose(div().w_full(), cx)
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

/// Fit the four counts and the disk tile in one row when their measured content allows it.
fn tile_columns(width: f32, tile: f32, gap: f32, count: usize) -> usize {
    [5, 4, 2]
        .into_iter()
        .find(|&cols| cols as f32 * tile + (cols - 1) as f32 * gap <= width)
        .unwrap_or(1)
        .min(count.max(1))
}

/// Station tiles reserve the height they paint; a last disk tile spans the narrow two-column
/// row, while count tiles preserve the equal-width layout of earlier stations.
pub(super) fn tape_tiles(tiles: &[Tile], width: f32, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    let gap = design::ui_value(cx, 8.0);
    let pad = design::ui_value(cx, 10.0);
    let has_db = tiles
        .iter()
        .any(|tile| tile.label == "telegram.server.keep_db");
    let widest = tiles
        .iter()
        .map(|tile| {
            let label = if has_db {
                design::ui_caption_text_width(cx, &t!(tile.label), 400.0)
            } else {
                design::ui_body_text_width(cx, &t!(tile.label), 400.0)
            };
            label.max(
                design::mono_body_text_width(cx, &tile.value.to_string(), 600.0)
                    * f32::from(design::t_title(cx))
                    / f32::from(design::t_body(cx)),
            )
        })
        .fold(0.0_f32, f32::max);
    let floor = if has_db {
        design::ui_value(cx, 150.0)
    } else {
        design::font_w(cx, 150.0)
    };
    let tile_min = (widest + 2.0 * pad + 2.0).max(floor);
    let cols = tile_columns(width, tile_min, gap, tiles.len());
    let tile_el = |index: usize, tile: &Tile| {
        let db = tile.label == "telegram.server.keep_db";
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
            .when(tile.accent, |el| el.border_color(rgba_from(p.blue, 1.0)))
            .debug_selector(move || {
                if db {
                    "server-tape-tile-db".to_string()
                } else {
                    format!("server-tape-tile-{index}")
                }
            })
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
                    .child(tile.hint.text()),
            )
            .into_any_element()
    };
    v_flex()
        .w_full()
        .min_w_0()
        .gap(px(gap))
        .debug_selector(|| "server-tape-tiles".into())
        .children(tiles.chunks(cols).enumerate().map(|(row, chunk)| {
            let fill =
                if cols == 2 && chunk.len() == 1 && chunk[0].label == "telegram.server.keep_db" {
                    1
                } else {
                    cols
                };
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
                // Keep count rows aligned; the disk-only narrow row fills the available width.
                .children((chunk.len()..fill).map(|_| {
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
                .flex_none()
                .whitespace_nowrap()
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
    let auto_label = t!("telegram.server.catchup_auto").to_string();
    let archives_label = t!("telegram.server.catchup_archives").to_string();
    v_flex()
        .flex_1()
        .flex_basis(design::font_w_px(cx, 220.0))
        .min_w_0()
        .max_w_full()
        .gap(design::ui_px(cx, 8.0))
        .debug_selector(|| "server-tape-switches".into())
        .child(crate::settings::common::checkbox_with_caption(
            "server-tape-auto",
            &auto_label,
            MoonCheckbox::new("server-tape-auto")
                .checked(switches.auto)
                .disabled(!editable)
                .label(auto_label.clone())
                .on_change(on_auto),
            t!("telegram.server.keep_cpu_note").to_string(),
            cx,
        ))
        .child(crate::settings::common::checkbox_with_caption(
            "server-tape-archives",
            &archives_label,
            MoonCheckbox::new("server-tape-archives")
                .checked(switches.archives)
                .disabled(!editable || !switches.auto)
                .label(archives_label.clone())
                .on_change(on_archives),
            if switches.auto {
                t!("telegram.server.catchup_archives_hint")
            } else {
                t!("telegram.server.catchup_archives_needs_auto")
            }
            .to_string(),
            cx,
        ))
        .into_any_element()
}

/// The page that renders the tape block, whose open state keeps a running catch-up re-read.
pub(super) fn shows_tape_block(page: super::super::Tab) -> bool {
    matches!(
        page,
        super::super::Tab::StOverview | super::super::Tab::StTape
    )
}

impl SettingsView {
    /// Subscribe once to custom retention days, and write clamped station reads back to the field.
    fn server_tape_days_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let days = self.telegram.server.tape.draft_keep.map(|k| k.max_age_days);
        let input = match self.telegram.server.tape.days_input.clone() {
            Some(input) => input,
            None => {
                let input = super::super::common::number_state(
                    u32::from(days.unwrap_or(1825).max(1)),
                    window,
                    cx,
                );
                if days.is_none_or(|days| days == 0) {
                    input.update(cx, |st, cx| st.set_value("", window, cx));
                }
                cx.subscribe(&input, |this, state, ev: &MoonInputEvent, cx| {
                    if !matches!(ev, MoonInputEvent::Change) {
                        return;
                    }
                    let Some(value) = MoonNumberInput::read_value(&state, cx) else {
                        return;
                    };
                    let typed = value.round().clamp(1.0, 3650.0) as u16;
                    let ed = &mut this.telegram.server.tape;
                    if let Some(draft) = ed.draft_keep.as_mut() {
                        draft.max_age_days = typed;
                        ed.days_shown = (value == f64::from(typed)).then_some(typed);
                        cx.notify();
                    }
                })
                .detach();
                self.telegram.server.tape.days_input = Some(input.clone());
                self.telegram.server.tape.days_shown = days;
                input
            }
        };
        if let Some(value) = self.telegram.server.tape.days_writeback() {
            input.update(cx, |st, cx| st.set_value(value, window, cx));
        }
    }

    /// Open a destructive confirmation or dispatch the acknowledged target's complete policy.
    fn server_tape_write(&mut self, target: Target, confirmed: bool, cx: &mut Context<Self>) {
        if self.telegram.server.known().map(Target::addr) != Some(target.addr()) {
            return;
        }
        if self.backend.read(cx).station.busy()
            || matches!(
                keep_state(reported_catchup(self.backend.read(cx).station.bot.as_ref())),
                KeepState::Purging { .. }
            )
        {
            return;
        }
        let ed = &mut self.telegram.server.tape;
        let Some(job) = ed.prepare_write(
            target,
            confirmed,
            reported_catchup(self.backend.read(cx).station.bot.as_ref()).and_then(|c| c.purge),
        ) else {
            cx.notify();
            return;
        };
        if ed.awaiting_keep.is_some() {
            ed.awaiting_finished = Some(self.backend.read(cx).station.finished);
        }
        self.server_bot_run(Ok(job), cx);
        self.server_tape_sync(cx);
    }
    /// Follow acknowledged policies while untouched; poll during catch-up, purge and a pending
    /// narrowing acknowledgement so maintenance never needs a manual status click.
    pub(in crate::settings) fn server_tape_sync(&mut self, cx: &mut Context<Self>) {
        let (read, switches, keep, purge, running) = {
            let bot = self.backend.read(cx).station.bot.as_ref();
            let catchup = reported_catchup(bot);
            (
                bot.and_then(|state| state.tape),
                bot.and_then(|state| state.station.as_ref())
                    .map(|_| catchup.and_then(reported_switches)),
                bot.and_then(|state| state.station.as_ref())
                    .map(|_| catchup.and_then(reported_keep)),
                catchup.and_then(|c| c.purge),
                catchup.is_some_and(|c| {
                    c.running || matches!(c.purge, Some(TapePurge::Running { .. }))
                }),
            )
        };
        let shown = shows_tape_block(self.active);
        let station = &self.backend.read(cx).station;
        let failed_write = station.outcome.as_ref().is_some_and(Result::is_err)
            && self
                .telegram
                .server
                .tape
                .awaiting_finished
                .is_some_and(|sent| sent != station.finished);
        let ed = &mut self.telegram.server.tape;
        follow(&mut ed.seen, &mut ed.draft, read);
        ed.take_switches(switches);
        ed.take_keep(keep);
        if failed_write
            || ed.awaiting_keep.as_ref().is_some_and(|(expected, before)| {
                keep.flatten() == Some(*expected) || &purge != before
            })
        {
            ed.awaiting_keep = None;
            ed.awaiting_finished = None;
        }
        if (running || ed.awaiting_keep.is_some()) && shown && ed.reread.is_none() {
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

    /// Snap edits to the station window's shared margin steps.
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
        self.server_tape_days_sync(window, cx);
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
            .fit_menu_width(140.0, 560.0)
            .disabled(!enabled)
            .items(items)
            .into_any_element()
    }

    /// Edit only switches the target has reported supporting.
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
        let changed = ed.seen != Some(draft)
            || ed.draft_switches != ed.seen_switches
            || ed.seen_keep != ed.draft_keep;
        let catchup = reported_catchup(self.backend.read(cx).station.bot.as_ref());
        let purging =
            matches!(keep_state(catchup), KeepState::Purging { .. }) || ed.awaiting_keep.is_some();
        let (msg, warning_tone) = keep::bar_message(ed, catchup, busy);
        let view = cx.entity().downgrade();
        let revert_view = view.clone();
        SaveBar::Remote(RemoteBar {
            tone: warning_tone.then_some(MoonTone::Warning),
            msg,
            dirty: changed && !busy && !ed.confirm && !purging,
            apply: std::rc::Rc::new(move |_, app| {
                let _ = view.update(app, |this, cx| {
                    this.server_tape_write(target.clone(), false, cx)
                });
            }),
            revert: Some(Rc::new(move |_, app| {
                let _ = revert_view.update(app, |this, cx| {
                    let ed = &mut this.telegram.server.tape;
                    ed.draft = ed.seen;
                    ed.draft_switches = ed.seen_switches;
                    ed.draft_keep = ed.seen_keep;
                    ed.confirm = false;
                    cx.notify();
                });
            })),
            label: t!("telegram.server.tape_set").to_string(),
        })
    }

    /// Build the production tape page from the current target's report and edits.
    pub(super) fn server_tape_block(
        &self,
        target: &Target,
        width: f32,
        cx: &Context<Self>,
    ) -> AnyElement {
        let backend = self.backend.read(cx);
        let catchup = reported_catchup(backend.station.bot.as_ref());
        let ed = &self.telegram.server.tape;
        let busy = backend.station.busy();
        let locked = busy
            || ed.confirm
            || ed.awaiting_keep.is_some()
            || matches!(keep_state(catchup), KeepState::Purging { .. });
        let snapshot = backend.station.bot_read_ms.map(|at| {
            t!(
                "telegram.server.catchup_snapshot",
                at = clock(at, crate::chartdx::axes::display_zone())
            )
            .to_string()
        });
        let switches = catchup.map(|_| {
            let view = cx.entity();
            let archives_view = view.clone();
            catchup_switches(
                ed.draft_switches.unwrap_or(CatchupSwitches {
                    auto: true,
                    archives: true,
                }),
                ed.draft_switches.is_some() && !locked,
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

        let margin = ed
            .draft
            .map(|draft| self.server_tape_margin_picker(draft.margin_s, !locked, cx))
            .unwrap_or_else(|| div().child(UNKNOWN_VALUE).into_any_element());
        let long = match (&ed.long_input, ed.draft) {
            (Some(input), Some(_)) => super::super::general::num_input_on(
                "server-tape-long",
                input,
                f64::from(*storage_cfg::LONG_POSITION_MIN_RANGE.start())
                    ..=f64::from(*storage_cfg::LONG_POSITION_MIN_RANGE.end()),
                1.0,
                None,
                !locked,
            ),
            _ => div().child(UNKNOWN_VALUE).into_any_element(),
        };
        let days_view = cx.entity();
        let emu_view = days_view.clone();
        let confirm_view = days_view.clone();
        let cancel_view = days_view.clone();
        let confirm_target = target.clone();
        let actions = block::Actions {
            days: Rc::new(move |days, _, app| {
                days_view.update(app, |this, cx| {
                    if let Some(keep) = this.telegram.server.tape.draft_keep.as_mut() {
                        keep.max_age_days = days;
                        cx.notify();
                    }
                })
            }),
            emulator: Rc::new(move |on, _, app| {
                emu_view.update(app, |this, cx| {
                    if let Some(keep) = this.telegram.server.tape.draft_keep.as_mut() {
                        keep.record_emulator = on;
                        cx.notify();
                    }
                })
            }),
            confirm: Rc::new(move |_, app| {
                confirm_view.update(app, |this, cx| {
                    this.server_tape_write(confirm_target.clone(), true, cx)
                })
            }),
            cancel: Rc::new(move |_, app| {
                cancel_view.update(app, |this, cx| {
                    this.telegram.server.tape.confirm = false;
                    cx.notify();
                })
            }),
        };
        block::server_tape_block(
            ed,
            block::Snapshot {
                catchup,
                busy,
                width: width - design::ui_value(cx, 24.0),
                read_at: snapshot,
            },
            block::Controls {
                margin,
                long,
                switches,
                venues,
            },
            actions,
            cx,
        )
    }
}

#[cfg(test)]
mod tests;
