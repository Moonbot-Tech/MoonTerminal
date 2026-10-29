use super::*;

/// Dated tape writes must retain legacy lines, rotate by UTC day, and leave other channels undated;
/// losing the old file or writing tomorrow's lines into today's file would break log retention.
#[test]
fn channel_writer_rotates_tape_and_migrates_legacy() {
    let root = std::env::temp_dir().join(format!(
        "channel-writer-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let legacy = root.join("tape_recorder.log");
    std::fs::write(&legacy, "old\n").unwrap();
    let mut writer = ChannelWriter::new("tape_recorder.log");
    assert!(writer.write_lines(&root, Some("2026-01-02"), &["new".into()]));
    assert!(!legacy.exists());
    assert_eq!(
        std::fs::read_to_string(root.join("2026-01-02_tape_recorder.log")).unwrap(),
        "old\nnew\n"
    );
    assert!(writer.write_lines(&root, Some("2026-01-03"), &["next".into()]));
    assert_eq!(
        std::fs::read_to_string(root.join("2026-01-03_tape_recorder.log")).unwrap(),
        "next\n"
    );

    let mut other = ChannelWriter::new("settings_diag.log");
    assert!(other.write_lines(&root, None, &["one".into()]));
    assert!(other.write_lines(&root, None, &["two".into()]));
    assert_eq!(
        std::fs::read_to_string(root.join("settings_diag.log")).unwrap(),
        "one\ntwo\n"
    );
    std::fs::remove_dir_all(root).unwrap();
}
