//! Compact chat presentation; the terminal's shared grouped facts remain unchanged.

use super::{
    LISTED_FILES, ReleaseCheck, duration, escape, file_name, one_line, percent, update_verdict,
};
use crate::t;
use moon_core::station_api::{Host, Status};
use moon_core::telegram::TelegramStatus;

/// Binary size units, matching the station's existing size formatting.
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
const MIB: f64 = 1024.0 * 1024.0;

/// Render the approved compact layout with escaped external values and one CPU table.
pub(super) fn html(status: &Status, release: &ReleaseCheck) -> String {
    let mut html = format!(
        "<p><b>\u{1f4e1} {}</b>",
        escape(&t!(
            "telegram.station.title",
            version = status.station_version
        ))
    );
    if let Some(host) = &status.host {
        html.push_str(&format!(
            " · {}",
            escape(&t!(
                "telegram.station.chat_uptime",
                uptime = duration(host.uptime_s)
            ))
        ));
    }
    html.push_str("</p>");
    service(status, &mut html);
    if let Some(raw) = &status.last_update {
        let healthy = raw.split_once(' ').filter(|(_, verdict)| {
            matches!(
                *verdict,
                "health=ok" | "update: health=ok" | "update from release: health=ok"
            )
        });
        let time = healthy.and_then(|(time, _)| {
            chrono::NaiveDateTime::parse_from_str(time, "%Y-%m-%dT%H:%MZ")
                .ok()
                .or_else(|| chrono::NaiveDateTime::parse_from_str(time, "%Y-%m-%dT%H:%M:%SZ").ok())
        });
        let text = match time {
            Some(time) => t!(
                "telegram.station.chat_updated",
                date = time.format("%d.%m"),
                time = time.format("%H:%M")
            )
            .to_string(),
            None => update_verdict(raw),
        };
        paragraph(&mut html, &escape(&one_line(&text)));
    }
    match release {
        ReleaseCheck::Newer(version) => paragraph(
            &mut html,
            &format!(
                "\u{26a0}\u{fe0f} {}",
                t!(
                    "telegram.station.update_available",
                    version = format!("<b>{}</b>", escape(version))
                )
            ),
        ),
        ReleaseCheck::Current => {}
        ReleaseCheck::Unversioned => paragraph(
            &mut html,
            &escape(&t!("telegram.station.release_unversioned")),
        ),
        ReleaseCheck::Failed(error) => paragraph(
            &mut html,
            &escape(&one_line(&t!(
                "telegram.station.release_unreadable",
                error = error.text()
            ))),
        ),
    }
    match &status.host {
        Some(host) => readings(host, &mut html),
        None => paragraph(&mut html, &escape(&t!("telegram.station.no_host"))),
    }
    if let Some(tape) = status.tape {
        let margin = if tape.margin_s.is_multiple_of(60) {
            t!("storage.trades_min", min = tape.margin_s / 60).to_string()
        } else {
            t!("storage.trades_sec", s = tape.margin_s).to_string()
        };
        paragraph(
            &mut html,
            &escape(&t!(
                "telegram.station.chat_tape",
                margin = margin,
                long = tape.long_position_min
            )),
        );
    }
    if let Some(host) = &status.host {
        files(host, &mut html);
    }
    html
}

/// Append one rich paragraph; callers own escaping or safe inline formatting.
fn paragraph(html: &mut String, content: &str) {
    html.push_str(&format!("<p>{content}</p>"));
}

/// Merge core readiness, the actual bot state and the optional auto-update switch.
fn service(status: &Status, html: &mut String) {
    let missing = status.cores_total.saturating_sub(status.cores_ready);
    let mut line = format!(
        "{} {} <b>{}/{}</b>",
        if missing == 0 {
            "\u{1f7e2}"
        } else {
            "\u{1f7e1}"
        },
        escape(&t!("telegram.station.cores_label")),
        status.cores_ready,
        status.cores_total
    );
    if missing > 0 {
        let text = if status.host.as_ref().is_some_and(|host| host.uptime_s < 60) {
            t!("telegram.station.chat_connecting").to_string()
        } else {
            t!("telegram.station.chat_offline", count = missing).to_string()
        };
        line.push_str(&format!(" · <b>{}</b>", escape(&text)));
    }
    if let Some(bot) = &status.bot {
        let state = match bot.status {
            TelegramStatus::Unpaired | TelegramStatus::Paired { .. } => "\u{2713}".into(),
            _ => escape(&crate::status_text(&bot.status)),
        };
        line.push_str(&format!(
            " · {} {state}",
            escape(&t!("telegram.station.chat_bot"))
        ));
    }
    if let Some(on) = status.auto_update {
        line.push_str(&format!(
            " · {} {}",
            escape(&t!("telegram.station.chat_auto_update")),
            if on { "\u{2713}" } else { "\u{2717}" }
        ));
    }
    paragraph(html, &line);
}

/// Show completed CPU windows and available resources, or the existing missing-readings note.
fn readings(host: &Host, html: &mut String) {
    let fresh = host.cpu.is_empty() && host.uptime_s < 60;
    if !fresh && host.memory.is_none() && host.uptime_s >= 60 {
        paragraph(html, &escape(&t!("telegram.station.no_readings")));
        return;
    }
    if fresh {
        paragraph(html, &escape(&t!("telegram.station.just_started")));
    } else if let (Some(hour), Some(day)) = (
        host.cpu.iter().min_by_key(|w| w.minutes),
        host.cpu.iter().max_by_key(|w| w.minutes),
    ) {
        html.push_str(&format!(
            "<table compact><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr>",
            escape(&t!("telegram.station.chat_cpu")),
            escape(&t!("telegram.station.cpu_hour")),
            escape(&t!("telegram.station.cpu_day")),
            escape(&t!("telegram.station.chat_peak"))
        ));
        for (label, avg_hour, avg_day, peak) in [
            (
                t!("telegram.station.chat_station"),
                hour.station_avg_permille,
                day.station_avg_permille,
                day.station_peak_permille,
            ),
            (
                t!("telegram.station.chat_server"),
                hour.machine_avg_permille,
                day.machine_avg_permille,
                day.machine_peak_permille,
            ),
        ] {
            html.push_str(&format!("<tr><td>{}</td><td align=\"right\">{} %</td><td align=\"right\">{} %</td><td align=\"right\">{} %</td></tr>", escape(&label), percent(avg_hour), percent(avg_day), percent(peak)));
        }
        html.push_str("</table>");
    }
    let mut free = Vec::new();
    let mut low = false;
    if let Some(memory) = host.memory {
        low |= low_resource(memory.available_bytes, memory.total_bytes);
        free.push(
            t!(
                "telegram.station.chat_memory_free",
                free = format!("<b>{:.1}</b>", memory.available_bytes as f64 / GIB),
                total = format!("{:.1}", memory.total_bytes as f64 / GIB)
            )
            .to_string(),
        );
    }
    if let Some(disk) = host.disk {
        low |= low_resource(disk.free_bytes, disk.total_bytes);
        free.push(
            t!(
                "telegram.station.chat_disk_free",
                free = format!("<b>{:.1}</b>", disk.free_bytes as f64 / GIB),
                total = format!("{:.1}", disk.total_bytes as f64 / GIB)
            )
            .to_string(),
        );
    }
    if !free.is_empty() {
        paragraph(
            html,
            &format!(
                "{}{}",
                if low { "\u{26a0}\u{fe0f} " } else { "" },
                t!("telegram.station.chat_free", resources = free.join(" · "))
            ),
        );
    }
    if let Some(memory) = host.memory {
        paragraph(
            html,
            &escape(&t!(
                "telegram.station.chat_memory_used",
                size = format!("{:.0}", memory.rss_bytes as f64 / MIB),
                peak = format!("{:.0}", memory.rss_peak_bytes as f64 / MIB)
            )),
        );
    }
}

/// Warn strictly below ten percent free; zero total is unknown, not low capacity.
pub(super) fn low_resource(free: u64, total: u64) -> bool {
    total > 0 && u128::from(free) * 10 < u128::from(total)
}

/// Keep the bounded largest-file list collapsed, with aggregate and omitted-entry sizes.
fn files(host: &Host, html: &mut String) {
    if host.files.is_empty() {
        return;
    }
    let mut files: Vec<_> = host.files.iter().collect();
    files.sort_by_key(|file| std::cmp::Reverse(file.bytes));
    let total = files
        .iter()
        .fold(0u64, |sum, file| sum.saturating_add(file.bytes));
    html.push_str(&format!(
        "<details><summary>{} · {}</summary>",
        escape(&t!("telegram.station.section_files")),
        escape(&crate::size_text(total))
    ));
    for file in files.iter().take(LISTED_FILES) {
        paragraph(
            html,
            &format!(
                "{} · {}",
                escape(&file_name(&file.name)),
                escape(&crate::size_text(file.bytes))
            ),
        );
    }
    if files.len() > LISTED_FILES {
        let omitted = files
            .iter()
            .skip(LISTED_FILES)
            .fold(0u64, |sum, file| sum.saturating_add(file.bytes));
        paragraph(
            html,
            &format!(
                "{} · {}",
                escape(&t!(
                    "telegram.station.files_more",
                    count = files.len() - LISTED_FILES
                )),
                escape(&crate::size_text(omitted))
            ),
        );
    }
    html.push_str("</details>");
}
