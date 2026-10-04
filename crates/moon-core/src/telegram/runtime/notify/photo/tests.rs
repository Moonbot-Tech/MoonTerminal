use super::*;

/// A fresh process-scoped directory for one test.
fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("moon-tg-charts-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn store_in(dir: &Path) -> NotifyStore {
    NotifyStore {
        path: dir.join("telegram_notifications.json"),
        file: NotifyFile::default(),
        allowed: None,
    }
}

/// A picture file in the store's charts folder, written `age` ago.
fn picture(store: &NotifyStore, name: &str, age: Duration) -> PathBuf {
    let dir = chart_spool_dir(&store.path);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, b"png").unwrap();
    let file = std::fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(SystemTime::now() - age).unwrap();
    path
}

#[test]
fn a_caption_past_the_limit_is_dropped_and_the_picture_still_goes() {
    let mut file = NotifyFile::default();
    let long = "x".repeat(CAPTION_UTF16_LIMIT + 1);
    assert!(push_photo(
        &mut file,
        5,
        long,
        Some(vec![1]),
        "a.png".into(),
        9
    ));
    let row = &file.outbox[0];
    assert_eq!(row.html, "");
    assert_eq!(row.photo.as_deref(), Some("a.png"));
    assert_eq!(row.cores, Some(vec![1]));
    assert_eq!(file.next_id, 1);
}

#[test]
fn the_sweep_deletes_old_pictures_no_row_names_and_keeps_the_rest() {
    let dir = temp_dir("sweep");
    let mut store = store_in(&dir);
    push_photo(&mut store.file, 1, "c".into(), None, "named.png".into(), 0);
    let named = picture(&store, "named.png", ORPHAN_AGE * 2);
    let orphan = picture(&store, "orphan.png", ORPHAN_AGE * 2);
    let fresh = picture(&store, "fresh.png", Duration::ZERO);
    store.sweep_charts();
    assert!(named.exists());
    assert!(!orphan.exists());
    assert!(fresh.exists(), "a picture whose row is on its way in stays");
}

#[test]
fn a_picture_still_named_by_another_row_is_not_dropped() {
    let dir = temp_dir("shared");
    let mut store = store_in(&dir);
    let path = picture(&store, "shared.png", Duration::ZERO);
    push_photo(&mut store.file, 1, "c".into(), None, "shared.png".into(), 0);
    store.drop_chart("shared.png");
    assert!(path.exists());
    store.file.outbox.clear();
    store.drop_chart("shared.png");
    assert!(!path.exists());
}
