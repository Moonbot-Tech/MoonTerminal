use moon_core::station_api::{DataFile, Disk, Memory};

use super::*;

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

fn status(host: Option<Host>) -> Status {
    Status {
        station_version: "0.1.0".into(),
        cores_ready: 26,
        cores_total: 27,
        bot: None,
        tape: Some(TapeWindow {
            margin_s: 180,
            long_position_min: 10,
        }),
        host: host.map(Box::new),
    }
}

/// Every part of the status reads as a line, sizes in their own unit (`size_text`).
#[test]
fn the_status_reads_line_by_line() {
    let _locale = crate::test_locale::force("en");
    let host = Host {
        uptime_s: 3 * 3_600 + 12 * 60 + 5,
        cpu: vec![
            CpuWindow {
                minutes: 60,
                station_avg_permille: 123,
                station_peak_permille: 870,
                machine_avg_permille: 150,
                machine_peak_permille: 1000,
            },
            CpuWindow {
                minutes: 192,
                station_avg_permille: 5,
                station_peak_permille: 870,
                machine_avg_permille: 90,
                machine_peak_permille: 1000,
            },
        ],
        memory: Some(Memory {
            rss_bytes: 420 * MIB,
            rss_peak_bytes: 520 * MIB,
            available_bytes: 300 * MIB,
            total_bytes: 955 * MIB,
        }),
        disk: Some(Disk {
            free_bytes: 15 * GIB + GIB / 2,
            total_bytes: 23 * GIB,
        }),
        files: vec![
            DataFile {
                name: "reports.sqlite".into(),
                bytes: 609 * MIB,
            },
            DataFile {
                name: "telegram.json".into(),
                bytes: 10,
            },
        ],
    };
    let text = station_status_text(&status(Some(host)));
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "Station 0.1.0 — up 3 h 12 min");
    assert_eq!(lines[1], "Cores: 26 of 27 ready");
    assert_eq!(
        lines[2],
        "Tape: 3 min around a trade, long trade from 10 min"
    );
    assert_eq!(
        lines[3],
        "Processor over the hour: station 12.3 % (peak 87.0 %), server 15.0 % (peak 100.0 %)"
    );
    assert!(lines[4].starts_with("Processor over 192 min: station 0.5 %"));
    assert_eq!(
        lines[5],
        "Memory: station 420.0 MB (peak 520.0 MB), server has 300.0 MB free of 955.0 MB"
    );
    assert_eq!(lines[6], "Disk: 15.50 GB free of 23.00 GB");
    assert_eq!(lines[7], "Files:");
    assert_eq!(lines[8], "  reports.sqlite — 609.0 MB");
    assert_eq!(lines[9], "  telegram.json — 0 KB");
}

/// A station older than the figures says so instead of showing nothing.
#[test]
fn an_older_station_is_told_to_update() {
    let _locale = crate::test_locale::force("en");
    let text = station_status_text(&status(None));
    assert!(text.starts_with("Station 0.1.0\n"));
    assert!(text.ends_with("update it."));
}

#[test]
fn uptime_takes_the_largest_two_units() {
    let _locale = crate::test_locale::force("en");
    assert_eq!(duration(59), "0 min");
    assert_eq!(duration(3_600), "1 h 0 min");
    assert_eq!(duration(2 * 86_400 + 5 * 3_600 + 59 * 60), "2 d 5 h");
}
