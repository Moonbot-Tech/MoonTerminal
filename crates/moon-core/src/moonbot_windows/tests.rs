use std::io::Write;

use super::*;

/// A fresh scratch directory for one test.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "moon-core-moonbot-{tag}-{}-{}",
        std::process::id(),
        crate::util::time::now_unix_ms_i64()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A zip of `files` (name, content).
fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, data) in files {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(data).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[test]
fn the_exe_is_found_one_folder_down() {
    let dir = scratch("exe");
    unpack(
        &zip_of(&[("Moonbot/MoonBot.exe", b"x"), ("Moonbot/a.dll", b"y")]),
        dir.as_path(),
    )
    .unwrap();
    assert_eq!(
        find_exe(dir.as_path()).unwrap(),
        dir.as_path().join("Moonbot").join("MoonBot.exe")
    );
}

#[test]
fn an_entry_leaving_the_folder_refuses_the_archive() {
    let dir = scratch("escape");
    let target = dir.as_path().join("bot");
    assert!(unpack(&zip_of(&[("../evil.txt", b"x")]), &target).is_err());
    assert!(!dir.as_path().join("evil.txt").exists());
}

#[test]
fn a_folder_with_files_gets_a_moonbot_folder_inside() {
    let dir = scratch("target");
    assert_eq!(target_dir(dir.as_path()).unwrap(), dir.as_path());
    std::fs::write(dir.as_path().join("notes.txt"), b"mine").unwrap();
    assert_eq!(
        target_dir(dir.as_path()).unwrap(),
        dir.as_path().join("Moonbot")
    );
    std::fs::create_dir(dir.as_path().join("Moonbot")).unwrap();
    std::fs::write(dir.as_path().join("Moonbot").join("BotConfig.bcfg"), b"k").unwrap();
    assert!(target_dir(dir.as_path()).is_err());
}
