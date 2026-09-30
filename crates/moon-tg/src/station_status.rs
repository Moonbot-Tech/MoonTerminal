//! The station's status in words (STATION.md §1 п. 20, §4.5), in the user's language: what the
//! terminal's "Status" shows, and the text meant for the bot's chat "Status" too.

use moon_core::station_api::{CpuWindow, Host, Status, TapeWindow};
use rust_i18n::t;

/// The station's status as lines of plain text: version and uptime, cores, bot, tape window,
/// processor, memory, disk, and the data root's files, largest first.
pub fn station_status_text(status: &Status) -> String {
    let mut lines = Vec::new();
    lines.push(match &status.host {
        Some(host) => t!(
            "telegram.station.title_uptime",
            version = status.station_version,
            uptime = duration(host.uptime_s)
        )
        .to_string(),
        None => t!("telegram.station.title", version = status.station_version).to_string(),
    });
    lines.push(
        t!(
            "telegram.station.cores",
            ready = status.cores_ready,
            total = status.cores_total
        )
        .to_string(),
    );
    if let Some(bot) = &status.bot {
        lines.push(
            t!(
                "telegram.station.bot",
                status = crate::status_text(&bot.status)
            )
            .to_string(),
        );
    }
    if let Some(tape) = status.tape {
        lines.push(tape_line(tape));
    }
    match &status.host {
        Some(host) => host_lines(host, &mut lines),
        None => lines.push(t!("telegram.station.no_host").to_string()),
    }
    lines.join("\n")
}

fn tape_line(tape: TapeWindow) -> String {
    let margin = match tape.margin_s.is_multiple_of(60) {
        true => t!("storage.trades_min", min = tape.margin_s / 60),
        false => t!("storage.trades_sec", s = tape.margin_s),
    };
    t!(
        "telegram.station.tape",
        margin = margin,
        long = tape.long_position_min
    )
    .to_string()
}

fn host_lines(host: &Host, lines: &mut Vec<String>) {
    for window in &host.cpu {
        lines.push(cpu_line(window));
    }
    // No reading at all: a unit that hides the machine's /proc from the station (set up before
    // `status` read it) — the setup run again replaces it.
    if host.memory.is_none() && host.uptime_s >= 60 {
        lines.push(t!("telegram.station.no_readings").to_string());
    }
    if let Some(memory) = host.memory {
        lines.push(
            t!(
                "telegram.station.memory",
                rss = crate::size_text(memory.rss_bytes),
                peak = crate::size_text(memory.rss_peak_bytes),
                available = crate::size_text(memory.available_bytes),
                total = crate::size_text(memory.total_bytes)
            )
            .to_string(),
        );
    }
    if let Some(disk) = host.disk {
        lines.push(
            t!(
                "telegram.station.disk",
                free = crate::size_text(disk.free_bytes),
                total = crate::size_text(disk.total_bytes)
            )
            .to_string(),
        );
    }
    if !host.files.is_empty() {
        lines.push(t!("telegram.station.files").to_string());
        for file in &host.files {
            lines.push(format!(
                "  {} — {}",
                file.name,
                crate::size_text(file.bytes)
            ));
        }
    }
}

fn cpu_line(window: &CpuWindow) -> String {
    let over = match window.minutes {
        60 => t!("telegram.station.cpu_hour"),
        1440 => t!("telegram.station.cpu_day"),
        minutes => t!("telegram.station.cpu_minutes", minutes = minutes),
    };
    t!(
        "telegram.station.cpu",
        over = over,
        station = percent(window.station_avg_permille),
        station_peak = percent(window.station_peak_permille),
        machine = percent(window.machine_avg_permille),
        machine_peak = percent(window.machine_peak_permille)
    )
    .to_string()
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
