use super::*;

fn minute(index: u64, station: f32, machine: f32) -> Minute {
    Minute {
        index,
        station_sum: station * 60.0,
        machine_sum: machine * 60.0,
        samples: 60,
    }
}

fn snapshot(station: f32, machine: f32, rss_mb: f32) -> MetricsSnapshot {
    MetricsSnapshot {
        cpu_process: station,
        cpu_system: machine,
        mem_mb: rss_mb,
        mem_available_mb: 300.0,
        mem_total_mb: 955.0,
        ..Default::default()
    }
}

/// A watch whose disk thread finds nothing to walk.
fn watch() -> HostWatch {
    HostWatch::start(Instant::now(), PathBuf::from("no-such-station-data-root"))
}

/// A window averages its whole minutes and peaks at the busiest one, in tenths of a percent;
/// the minute still running is not in it.
#[test]
fn a_window_is_the_mean_of_its_minutes_and_its_busiest_one() {
    let history: VecDeque<Minute> = [
        minute(0, 10.0, 20.0),
        minute(1, 30.0, 40.0),
        minute(2, 2.0, 90.0),
        minute(3, 99.0, 99.0),
    ]
    .into_iter()
    .collect();
    let w = window(history.iter(), 3, 60).unwrap();
    assert_eq!(w.minutes, 3);
    assert_eq!(w.station_avg_permille, 140);
    assert_eq!(w.station_peak_permille, 300);
    assert_eq!(w.machine_avg_permille, 500);
    assert_eq!(w.machine_peak_permille, 900);
    // Only the last two minutes by the clock.
    let w = window(history.iter(), 3, 2).unwrap();
    assert_eq!((w.minutes, w.station_avg_permille), (2, 160));
    assert!(window(std::iter::empty(), 5, 60).is_none());
}

/// The hour is the last sixty minutes by the clock: a gap (a stalled loop) shortens what it holds,
/// never stretches it over older minutes.
#[test]
fn a_window_is_time_not_a_count_of_minutes() {
    let history: VecDeque<Minute> = [minute(10, 80.0, 80.0), minute(100, 10.0, 10.0)]
        .into_iter()
        .collect();
    let hour = window(history.iter(), 101, 60).unwrap();
    assert_eq!(hour.minutes, 60);
    assert_eq!(hour.station_peak_permille, 100);
    let day = window(history.iter(), 101, 1440).unwrap();
    assert_eq!(day.minutes, 101);
    assert_eq!(day.station_peak_permille, 800);
}

/// Samples fold into the minute of their time; a snapshot without a reading — before the
/// sampler's first poll, or a second sysinfo did not see the process — counts for nothing.
#[test]
fn only_readings_fold_into_minutes() {
    let mut watch = watch();
    let t0 = watch.first_minute;
    watch.observe(t0 + Duration::from_secs(1), MetricsSnapshot::default());
    watch.observe(t0 + Duration::from_secs(2), snapshot(5.0, 50.0, 0.0));
    watch.observe(t0 + Duration::from_secs(3), snapshot(f32::NAN, 50.0, 400.0));
    assert_eq!(watch.current.samples, 0);
    assert!(watch.last.is_none());

    watch.observe(t0 + Duration::from_secs(4), snapshot(10.0, 50.0, 400.0));
    watch.observe(t0 + Duration::from_secs(5), snapshot(30.0, 70.0, 450.0));
    assert!(watch.minutes.is_empty());
    watch.observe(t0 + MINUTE, snapshot(20.0, 60.0, 420.0));
    assert_eq!(watch.minutes.len(), 1);
    assert_eq!(watch.minutes[0].averages(), (20.0, 60.0));
    assert_eq!(watch.minutes[0].index, 0);
    assert_eq!(watch.current.index, 1);
    assert_eq!(watch.rss_peak_mb, 450.0);

    // Ten minutes of silence, then a reading: one minute kept, no empty ones.
    watch.observe(t0 + MINUTE * 11, snapshot(1.0, 1.0, 400.0));
    assert_eq!(watch.minutes.len(), 2);
    assert_eq!(watch.current.index, 11);
}

/// A day of minutes is the most kept.
#[test]
fn a_day_of_minutes_is_kept() {
    let mut watch = watch();
    let t0 = watch.first_minute;
    for i in 0..(KEEP_MINUTES as u32 + 5) {
        watch.observe(t0 + MINUTE * i, snapshot(1.0, 1.0, 400.0));
    }
    assert_eq!(watch.minutes.len(), KEEP_MINUTES);
    assert_eq!(watch.minutes[0].index, 4);
}

#[test]
fn a_percentage_becomes_tenths_within_the_machine() {
    assert_eq!(permille(12.34), 123);
    assert_eq!(permille(150.0), 1000);
    assert_eq!(permille(-1.0), 0);
    assert_eq!(permille(f32::NAN), 0);
}

/// A database and its companions are one entry, a directory the sum of what is under it, the
/// largest first.
#[test]
fn the_data_root_is_listed_by_database_and_directory() {
    let root = std::env::temp_dir().join(format!("station-host-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("logs/old")).unwrap();
    let write =
        |name: &str, bytes: usize| std::fs::write(root.join(name), vec![0u8; bytes]).unwrap();
    write("reports.sqlite", 1000);
    write("reports.sqlite-wal", 500);
    write("reports.sqlite-shm", 32);
    write("telegram.json", 10);
    write("logs/a.log", 200);
    write("logs/old/b.log", 300);
    let files = data_files(&root);
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(
        files,
        vec![
            DataFile {
                name: "reports.sqlite".into(),
                bytes: 1532,
            },
            DataFile {
                name: "logs/".into(),
                bytes: 500,
            },
            DataFile {
                name: "telegram.json".into(),
                bytes: 10,
            },
        ]
    );
    assert_eq!(database_of("-wal"), "-wal");
    assert!(data_files(&root).is_empty());
}

/// A minute that has just ended counts before the next tick folds it.
#[test]
fn the_minute_just_ended_counts_before_the_next_tick() {
    let mut watch = watch();
    let t0 = watch.first_minute;
    watch.observe(t0 + Duration::from_secs(5), snapshot(40.0, 50.0, 400.0));
    let whole: Vec<u64> = watch.whole_minutes(1).map(|m| m.index).collect();
    assert_eq!(whole, vec![0]);
    assert_eq!(watch.whole_minutes(0).count(), 0);
}
