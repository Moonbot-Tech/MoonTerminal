//! The station's status in words (STATION.md §1 п. 20, §4.5), in the user's language: what the
//! terminal's "Status" shows, and what the bot's chat "Status" answers — with an "Update" button
//! while a newer release carries the station's binary.

use crate::t;
use moon_core::station_api::{Host, Status, TapeWindow};
use moon_core::telegram::api::{InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup};
use moon_core::telegram::commands::STATION_UPDATE_CALLBACK;
use moon_core::telegram::runtime::Response;

/// What the station's look at the latest release found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReleaseCheck {
    /// A newer release carries the station's binary: its version.
    Newer(String),
    /// No release newer than this station carries its binary.
    Current,
    /// This build carries no release version: it is updated only by a binary from the terminal.
    Unversioned,
    /// The releases could not be read; why.
    Failed(ReleaseFailure),
}

/// Why release discovery could not finish; raw diagnostics are secondary only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReleaseFailure {
    Timeout,
    Unavailable(String),
    UnsupportedArchitecture(String),
}

/// Why the station did not file an update, localized by the bot rather than its host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateRefusal {
    UpdaterMissing,
    RequestStale,
    AlreadyRunning,
    WriteFailed(String),
}

impl UpdateRefusal {
    /// Render the action before any underlying file-system diagnostic.
    pub fn text(&self) -> String {
        match self {
            Self::UpdaterMissing => t!("station.update.updater_missing").to_string(),
            Self::RequestStale => t!("station.update.request_stale").to_string(),
            Self::AlreadyRunning => t!("station.update.running").to_string(),
            Self::WriteFailed(detail) => format!(
                "{}\n{}",
                t!("station.update.write_failed"),
                t!("station.detail", detail = detail)
            ),
        }
    }
}

impl ReleaseFailure {
    /// Render a localized release-check reason with diagnostics on a separate line.
    fn text(&self) -> String {
        match self {
            Self::Timeout => t!("station.update.release_timeout").to_string(),
            Self::UnsupportedArchitecture(arch) => {
                t!("station.update.arch", arch = arch).to_string()
            }
            Self::Unavailable(detail) => format!(
                "{}\n{}",
                t!("station.update.release_failed"),
                t!("station.detail", detail = detail)
            ),
        }
    }
}

/// How many of the data root's files the status names: the largest ones.
const LISTED_FILES: usize = 5;
/// A file name longer than this is cut, so one odd name cannot stretch the message.
const FILE_NAME_CHARS: usize = 48;

/// The chat's answer to "Status": the station's status as a rich message, a line on the latest
/// release, and — only while a newer release carries the station's binary — the "Update" button
/// under it. The dispatcher admits only the owner before the host produces this response.
pub fn station_status_reply(status: &Status, release: &ReleaseCheck) -> Response {
    let facts = StatusFacts::of(status);
    let release_line = match release {
        ReleaseCheck::Newer(version) => {
            Some(t!("telegram.station.update_available", version = version).to_string())
        }
        ReleaseCheck::Current => None,
        ReleaseCheck::Unversioned => Some(t!("telegram.station.release_unversioned").to_string()),
        ReleaseCheck::Failed(error) => {
            Some(t!("telegram.station.release_unreadable", error = error.text()).to_string())
        }
    };
    let update = match release {
        ReleaseCheck::Newer(version) => vec![vec![InlineKeyboardButton::callback(
            t!("telegram.station.update_button", version = version).to_string(),
            STATION_UPDATE_CALLBACK,
        )]],
        _ => Vec::new(),
    };
    let navigation = crate::labels::navigation_keyboard(crate::HostKind::Station, true);
    let html = facts.html(release_line.as_deref());
    if !crate::report::rich_message_fits(&html) {
        // Not expected with a bounded file list; plain text still answers the press.
        let mut text = facts.plain();
        if let Some(line) = release_line {
            text.push_str("\n\n");
            text.push_str(&line);
        }
        let keyboard = match update.is_empty() {
            true => navigation,
            false => ReplyMarkup::Inline(InlineKeyboardMarkup::from_rows(update)),
        };
        return Response::Text {
            text,
            keyboard: Some(keyboard),
        };
    }
    Response::Rich {
        html,
        keyboard: ReplyMarkup::Inline(InlineKeyboardMarkup::from_rows(update)),
        navigation: (
            t!("telegram.report_navigation_hint").to_string(),
            navigation,
        ),
    }
}

/// The station's status as short plain-text lines grouped under section headings: what the
/// terminal prints as a Status run's progress. No markup.
pub fn station_status_text(status: &Status) -> String {
    StatusFacts::of(status).plain()
}

/// The station's status as grouped label/value facts, shared by the chat's rich message and the
/// terminal's structured view and plain lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusFacts {
    /// Localized title including the version and any available uptime.
    pub title: String,
    /// Ordered headings and their label/value rows.
    pub sections: Vec<Section>,
    /// Sentences that are not a value: missing readings and what to do about them.
    pub notes: Vec<String>,
}

/// One heading and its rows; a row with an empty value is a caption on its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    /// Localized section heading.
    pub title: String,
    /// Localized labels paired with formatted values, without display markup.
    pub rows: Vec<(String, String)>,
}

impl StatusFacts {
    /// Build localized facts without flattening rows or escaping their content for chat.
    pub fn of(status: &Status) -> Self {
        let title = match &status.host {
            Some(host) => t!(
                "telegram.station.title_uptime",
                version = status.station_version,
                uptime = duration(host.uptime_s)
            ),
            None => t!("telegram.station.title", version = status.station_version),
        }
        .to_string();
        let mut service = vec![(
            t!("telegram.station.cores_label").to_string(),
            t!(
                "telegram.station.cores_value",
                ready = status.cores_ready,
                total = status.cores_total
            )
            .to_string(),
        )];
        if let Some(bot) = &status.bot {
            service.push((
                t!("telegram.station.bot_label").to_string(),
                crate::status_text(&bot.status),
            ));
        }
        if let Some(verdict) = &status.last_update {
            service.push((
                t!("telegram.station.last_update_label").to_string(),
                update_verdict(verdict),
            ));
        }
        if let Some(on) = status.auto_update {
            service.push((
                t!("telegram.station.auto_update_label").to_string(),
                match on {
                    true => t!("telegram.station.auto_update_on"),
                    false => t!("telegram.station.auto_update_off"),
                }
                .to_string(),
            ));
        }
        let mut sections = vec![Section {
            title: t!("telegram.station.section_service").to_string(),
            rows: service,
        }];
        if let Some(tape) = status.tape {
            sections.push(tape_section(tape));
        }
        let mut notes = Vec::new();
        match &status.host {
            Some(host) => host_sections(host, &mut sections, &mut notes),
            None => notes.push(t!("telegram.station.no_host").to_string()),
        }
        Self {
            title,
            sections,
            notes,
        }
    }

    /// Telegram rich HTML: a bold title, one compact two-column table per section, then notes
    /// and the release line as paragraphs. Every dynamic value is escaped.
    fn html(&self, release: Option<&str>) -> String {
        let mut html = format!("<p><b>\u{1f4e1} {}</b></p>", escape(&self.title));
        for section in &self.sections {
            html.push_str(&format!(
                "<p><b>{}</b></p><table compact>",
                escape(&section.title)
            ));
            for (label, value) in &section.rows {
                html.push_str(&format!(
                    "<tr><td><b>{}</b></td><td align=\"right\">{}</td></tr>",
                    escape(label),
                    escape(&one_line(value))
                ));
            }
            html.push_str("</table>");
        }
        for note in self.notes.iter().map(String::as_str).chain(release) {
            html.push_str(&format!("<p>{}</p>", escape(&one_line(note))));
        }
        html
    }

    /// Plain lines: the title, then each section's heading and its indented `label: value` rows.
    fn plain(&self) -> String {
        let mut lines = vec![self.title.clone()];
        for section in &self.sections {
            lines.push(section.title.clone());
            for (label, value) in &section.rows {
                let mut value = value.lines();
                lines.push(match value.next() {
                    Some(first) => format!("  {label}: {first}"),
                    None => format!("  {label}"),
                });
                lines.extend(value.map(|more| format!("    {more}")));
            }
        }
        lines.extend(self.notes.iter().cloned());
        lines.join("\n")
    }
}

/// Escape external text for Telegram's restricted rich HTML.
fn escape(value: &str) -> String {
    crate::report::escape(value)
}

/// A table cell or paragraph holds one line: a value's detail lines follow after a middle dot.
fn one_line(value: &str) -> String {
    value.lines().collect::<Vec<_>>().join(" \u{b7} ")
}

/// Translate known helper verdicts from current and older stations without changing the wire.
fn update_verdict(raw: &str) -> String {
    let (time, verdict) = match raw.split_once(' ') {
        Some((time, rest)) if time.contains('T') && time.ends_with('Z') => (Some(time), rest),
        _ => (None, raw),
    };
    let (rollback, verdict) = match verdict.strip_prefix("rollback: ") {
        Some(verdict) => (true, verdict),
        None => (
            false,
            verdict
                .strip_prefix("update from release: ")
                .or_else(|| verdict.strip_prefix("update: "))
                .unwrap_or(verdict),
        ),
    };
    let text = match verdict {
        "health=ok" if rollback => t!("station.update.rolled_back"),
        "health=ok" => t!("station.update.healthy"),
        "error: the new binary did not stay up; the previous one is back" => {
            t!("station.update.auto_rolled_back")
        }
        "health=not-started" => t!("station.update.next_start"),
        "health=failed" => t!("station.update.health_failed"),
        "update=running" | "rollback=running" => t!("station.update.running"),
        "update=refused" => t!("station.update.refused"),
        "update=none release=current" => t!("station.update.current"),
        "update=none release=unversioned" => t!("telegram.station.release_unversioned"),
        _ if verdict.starts_with("update=failed:") => t!("station.update.failed"),
        _ => t!("station.update.unknown"),
    };
    let text = match time {
        Some(time) => format!("{time} {text}"),
        None => text.to_string(),
    };
    if matches!(
        verdict,
        "health=ok"
            | "error: the new binary did not stay up; the previous one is back"
            | "health=not-started"
            | "health=failed"
            | "update=running"
            | "rollback=running"
            | "update=refused"
            | "update=none release=current"
            | "update=none release=unversioned"
    ) {
        text
    } else {
        format!("{text}\n{}", t!("station.detail", detail = verdict))
    }
}

/// The station's trade window in the same units as Settings.
fn tape_section(tape: TapeWindow) -> Section {
    let margin = match tape.margin_s.is_multiple_of(60) {
        true => t!("storage.trades_min", min = tape.margin_s / 60),
        false => t!("storage.trades_sec", s = tape.margin_s),
    };
    Section {
        title: t!("telegram.station.section_tape").to_string(),
        rows: vec![
            (
                t!("telegram.station.tape_margin").to_string(),
                margin.to_string(),
            ),
            (
                t!("telegram.station.tape_long").to_string(),
                t!(
                    "telegram.station.cpu_minutes",
                    minutes = tape.long_position_min
                )
                .to_string(),
            ),
        ],
    }
}

/// The server's measurements as one section, the data root's largest files as another, and
/// actionable guidance when a reading is missing.
fn host_sections(host: &Host, sections: &mut Vec<Section>, notes: &mut Vec<String>) {
    let mut rows = Vec::new();
    for window in &host.cpu {
        let over = match window.minutes {
            60 => t!("telegram.station.cpu_hour"),
            1440 => t!("telegram.station.cpu_day"),
            minutes => t!("telegram.station.cpu_minutes", minutes = minutes),
        };
        let load = |avg, peak| {
            t!(
                "telegram.station.percent_peak",
                value = percent(avg),
                peak = percent(peak)
            )
            .to_string()
        };
        rows.push((
            t!("telegram.station.cpu_station", over = over).to_string(),
            load(window.station_avg_permille, window.station_peak_permille),
        ));
        rows.push((
            t!("telegram.station.cpu_server", over = over).to_string(),
            load(window.machine_avg_permille, window.machine_peak_permille),
        ));
    }
    // No reading at all: a unit that hides the machine's /proc from the station (set up before
    // `status` read it) — the setup run again replaces it.
    if host.memory.is_none() && host.uptime_s >= 60 {
        notes.push(t!("telegram.station.no_readings").to_string());
    }
    if let Some(memory) = host.memory {
        rows.push((
            t!("telegram.station.memory_station").to_string(),
            t!(
                "telegram.station.size_peak",
                size = crate::size_text(memory.rss_bytes),
                peak = crate::size_text(memory.rss_peak_bytes)
            )
            .to_string(),
        ));
        rows.push((
            t!("telegram.station.memory_server").to_string(),
            t!(
                "telegram.station.free_of",
                free = crate::size_text(memory.available_bytes),
                total = crate::size_text(memory.total_bytes)
            )
            .to_string(),
        ));
    }
    if let Some(disk) = host.disk {
        rows.push((
            t!("telegram.station.disk_label").to_string(),
            t!(
                "telegram.station.free_of",
                free = crate::size_text(disk.free_bytes),
                total = crate::size_text(disk.total_bytes)
            )
            .to_string(),
        ));
    }
    if !rows.is_empty() {
        sections.push(Section {
            title: t!("telegram.station.section_server").to_string(),
            rows,
        });
    }
    if !host.files.is_empty() {
        let mut files: Vec<_> = host.files.iter().collect();
        files.sort_by_key(|file| std::cmp::Reverse(file.bytes));
        let mut rows: Vec<_> = files
            .iter()
            .take(LISTED_FILES)
            .map(|file| (file_name(&file.name), crate::size_text(file.bytes)))
            .collect();
        if files.len() > LISTED_FILES {
            rows.push((
                t!(
                    "telegram.station.files_more",
                    count = files.len() - LISTED_FILES
                )
                .to_string(),
                String::new(),
            ));
        }
        sections.push(Section {
            title: t!("telegram.station.section_files").to_string(),
            rows,
        });
    }
}

/// A file name without control characters, cut to a bounded length.
fn file_name(name: &str) -> String {
    let clean: Vec<char> = name.chars().filter(|c| !c.is_control()).collect();
    if clean.len() <= FILE_NAME_CHARS {
        return clean.into_iter().collect();
    }
    let mut cut: String = clean[..FILE_NAME_CHARS - 1].iter().collect();
    cut.push('\u{2026}');
    cut
}

/// Tenths of a percent as a percentage with one decimal.
fn percent(permille: u16) -> String {
    format!("{}.{}", permille / 10, permille % 10)
}

/// Seconds as the largest two units that say it: minutes, hours and minutes, days and hours.
fn duration(secs: u64) -> String {
    let (days, hours, minutes) = (secs / 86_400, secs % 86_400 / 3_600, secs % 3_600 / 60);
    match (days, hours) {
        (0, 0) => t!("telegram.station.uptime_m", m = minutes),
        (0, _) => t!("telegram.station.uptime_hm", h = hours, m = minutes),
        _ => t!("telegram.station.uptime_dh", d = days, h = hours),
    }
    .to_string()
}

#[cfg(test)]
mod tests;
