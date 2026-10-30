use super::*;

/// Measure frozen reopen-per-line writes and the retained buffered writer on isolated logs.
#[test]
#[ignore = "synthetic tape scale baseline"]
fn tape_scale_channel_lines() {
    let root = std::env::temp_dir().join(format!(
        "tape-scale-log-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("tape_recorder.log");
    let lines: Vec<String> = (0..2_000)
        .map(|index| format!("synthetic line {index}"))
        .collect();
    let mut samples = Vec::new();
    for _ in 0..3 {
        let started = std::time::Instant::now();
        for line in &lines {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .unwrap();
            std::io::Write::write_all(&mut file, line.as_bytes()).unwrap();
            std::io::Write::write_all(&mut file, b"\n").unwrap();
        }
        samples.push(started.elapsed().as_secs_f64() * 1_000.0);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "[tape_scale] channel_reopen lines=2000 median_ms={:.3} samples={samples:?}",
        samples[1]
    );
    let mut writer = ChannelWriter::new("buffered.log");
    let mut buffered = Vec::new();
    for _ in 0..3 {
        let started = std::time::Instant::now();
        for line in &lines {
            assert!(writer.write_lines(&root, None, std::slice::from_ref(line)));
        }
        buffered.push(started.elapsed().as_secs_f64() * 1_000.0);
    }
    buffered.sort_by(f64::total_cmp);
    println!(
        "[tape_scale] channel_buffered lines=2000 median_ms={:.3} samples={buffered:?}",
        buffered[1]
    );
    std::fs::remove_dir_all(root).unwrap();
}
