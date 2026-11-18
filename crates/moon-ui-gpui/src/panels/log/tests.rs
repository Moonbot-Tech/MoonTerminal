//! Regression tests for effective Log selection and incremental cursor behavior.

// Explicit imports on purpose: `use super::*` would pull in the parent's `gpui::*` re-export,
// whose `test` shadows the built-in attribute and makes `#[test]` expand recursively
// ("recursion limit reached").
use super::paging::{FilePage, NamedPage};
use super::{LogFile, LogPanel, LogSource, LogSourceItem, resolve_workspace_log_selection};

/// Keep each fixture isolated under concurrent tests.
struct NamedPageFixture(std::path::PathBuf);

impl NamedPageFixture {
    /// Creates a unique directory for a test's log files.
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("moon-named-log-{}-{seq}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for NamedPageFixture {
    /// Deletes only the files the fixture created, then removes its directory.
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0.join("2023-11-14_CORE_A.log"));
        let _ = std::fs::remove_file(self.0.join("2023-11-15_CORE_A.log"));
        let _ = std::fs::remove_dir(&self.0);
    }
}

/// Create independent small files for the completion and retention checks.
fn named_page_fixture() -> (NamedPageFixture, String, String) {
    let temp = NamedPageFixture::new();
    let a = temp.0.join("2023-11-14_CORE_A.log");
    let b = temp.0.join("2023-11-15_CORE_A.log");
    std::fs::write(&a, "12:00:00\tI\tCORE_A\tfirst\n").unwrap();
    std::fs::write(
        &b,
        "12:00:00\tI\tCORE_A\tsecond\n12:00:01\tW\tCORE_A\tthird\n",
    )
    .unwrap();
    (temp, a.to_str().unwrap().into(), b.to_str().unwrap().into())
}

/// A worker's index and tail must contain the same rows, in the same order, as direct opening.
#[gpui::test]
async fn background_named_page_matches_synchronous_open(cx: &mut gpui::TestAppContext) {
    let (_temp, _, name) = named_page_fixture();
    let mut selected = NamedPage::default();
    let (generation, opened) = selected.request(&name, &cx.executor()).unwrap();
    assert!(selected.publish(generation, opened.await));
    let mut expected = FilePage::default();
    expected.open(&name);
    assert_eq!(selected.page.total(), expected.total());
    assert_eq!(selected.page.lines().len(), expected.lines().len());
    for (actual, expected) in selected.page.lines().iter().zip(expected.lines()) {
        assert_eq!(actual.ts, expected.ts);
        assert_eq!(actual.level, expected.level);
        assert_eq!(actual.target, expected.target);
        assert_eq!(actual.msg, expected.msg);
    }
}

/// A later file pick wins and the prior page stays available until a new index is published.
#[gpui::test]
async fn named_page_rejects_out_of_order_completion(cx: &mut gpui::TestAppContext) {
    let (_temp, a, b) = named_page_fixture();
    let mut selected = NamedPage::default();
    let (first, opened) = selected.request(&a, &cx.executor()).unwrap();
    assert!(selected.publish(first, opened.await));
    selected.invalidate();
    let (older, a_opened) = selected.request(&a, &cx.executor()).unwrap();
    let (latest, b_opened) = selected.request(&b, &cx.executor()).unwrap();
    assert_eq!(selected.page.total(), 1);
    assert!(selected.publish(latest, b_opened.await));
    assert!(!selected.publish(older, a_opened.await));
    assert!(selected.shows(&b));
    assert_eq!(selected.page.total(), 2);
}

/// Indexing a different file must hide the previous file's rows and preserve the clipboard.
#[gpui::test]
async fn pending_named_file_clears_display_and_copy_rows(cx: &mut gpui::TestAppContext) {
    let (_temp, a, b) = named_page_fixture();
    let mut page = NamedPage::default();
    let mut buf = super::buffer::RowBuffer::new(chrono_tz::UTC);
    let filters = super::buffer::Filters {
        errors_only: false,
        query: "",
        coin: None,
    };
    let executor = cx.executor();

    let (generation, opened) =
        LogPanel::request_named_page(&mut page, &mut buf, &a, &executor).unwrap();
    assert!(page.publish(generation, opened.await));
    buf.ingest(page.page.lines().to_vec(), super::VIEW_LIMIT, filters);
    assert_eq!(buf.visible(), 1);
    cx.update(|app| LogPanel::copy_view_rows(&buf, 0..1, app));
    let before = cx.read_from_clipboard().unwrap().text().unwrap();
    assert!(before.contains("first"));

    let (generation, opened) =
        LogPanel::request_named_page(&mut page, &mut buf, &b, &executor).unwrap();
    assert_eq!(buf.total(), 0);
    assert_eq!(buf.visible(), 0);
    cx.update(|app| LogPanel::copy_view_rows(&buf, 0..1, app));
    assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), before);

    assert!(page.publish(generation, opened.await));
    buf.ingest(page.page.lines().to_vec(), super::VIEW_LIMIT, filters);
    assert_eq!(buf.visible(), 2);
    cx.update(|app| LogPanel::copy_view_rows(&buf, 0..2, app));
    let copied = cx.read_from_clipboard().unwrap().text().unwrap();
    assert!(copied.contains("second") && copied.contains("third") && !copied.contains("first"));
}

/// Open an isolated 50 MB named file repeatedly, including indexing and parsing the tail.
#[test]
#[ignore = "bench: S-smooth"]
fn bench_s_log_open_named() {
    use crate::test_support::bench;
    use std::io::Write;

    let temp = NamedPageFixture::new();
    let path = temp.0.join("2023-11-14_CORE_A.log");
    let mut out = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
    const ROWS: usize = 500_000;
    let line = format!("12:00:00\tI\tCORE_A\t{:0<81}\n", "UNIT_01 ");
    assert_eq!(line.len() * ROWS, 50_000_000);
    for _ in 0..ROWS {
        out.write_all(line.as_bytes()).unwrap();
    }
    out.flush().unwrap();
    drop(out);
    let name = path.to_str().unwrap();
    let cx = gpui::TestAppContext::single();
    let executor = cx.executor();
    bench::measure(
        "bench_s_log_open_named_ui_thread stimulus=queue_named_50mb_index",
        12,
        || NamedPage::default().request(name, &executor).unwrap(),
    );
    bench::measure(
        "bench_s_log_open_named stimulus=50mb_cold_index_warm_file_cache",
        12,
        || {
            let mut page = super::paging::FilePage::default();
            // An absolute path stays inside the fixture without changing the process-wide data root.
            page.open(name);
            assert_eq!(page.total(), ROWS);
            assert_eq!(page.lines().len(), super::VIEW_LIMIT);
            page
        },
    );
}

/// `mod.rs:resolve_workspace_log_selection` must overlay rather than assign retained selectors.
///
/// Mutation: return `retained_file` while `workspace_owned` is true, or replace either retained
/// value with the effective result. The named-file assertion then catches Auto reading history,
/// while the final Classic assertion catches loss of the exact source/file on restore.
#[test]
fn workspace_source_and_live_file_override_restore_retained_log_selection() {
    let retained_source = LogSource::Core(91);
    let retained_file = LogFile::Named("2026-08-08_core-91.log".to_string());

    let auto = resolve_workspace_log_selection(true, Some(7), &retained_source, &retained_file);
    assert_eq!(auto, (LogSource::Core(7), LogFile::Live, true));
    assert_eq!(retained_source, LogSource::Core(91));
    assert_eq!(
        retained_file,
        LogFile::Named("2026-08-08_core-91.log".to_string())
    );

    let overview = resolve_workspace_log_selection(true, None, &retained_source, &retained_file);
    assert_eq!(overview, (LogSource::Aggregate, LogFile::Live, true));

    let classic = resolve_workspace_log_selection(false, None, &retained_source, &retained_file);
    assert_eq!(classic, (retained_source, retained_file, false));
}

/// `sources_sig` must move for every change that leaves buffered rows mislabelled.
///
/// Rows carry a core's display name, copied when they were pulled, and appending cannot relabel
/// rows it is not touching — so this signature is the only thing that forces the full reload which
/// does. A signature blind to renames leaves rows under a name that selects nothing, and
/// `select_source_by_name` then resolves them to no core at all.
#[test]
fn sources_sig_moves_for_add_remove_and_rename() {
    let entry = |id: u64, display: &str| LogSourceItem {
        source: LogSource::Core(id),
        display: display.to_string(),
        file_label: display.to_string(),
    };
    let base = [entry(1, "alpha"), entry(2, "beta")];
    let sig = LogPanel::sources_sig(&base);

    assert_eq!(
        sig,
        LogPanel::sources_sig(&base),
        "an unchanged list must not reload, or every revision would rebuild the buffer"
    );
    assert_ne!(
        sig,
        LogPanel::sources_sig(&[entry(1, "alpha")]),
        "a removed core must force a reload"
    );
    assert_ne!(
        sig,
        LogPanel::sources_sig(&[entry(1, "alpha"), entry(2, "beta"), entry(3, "gamma")]),
        "an added core must force a reload"
    );
    assert_ne!(
        sig,
        LogPanel::sources_sig(&[entry(1, "alpha"), entry(2, "renamed")]),
        "a renamed core must force a reload — its buffered rows carry the old name"
    );
    assert_ne!(
        sig,
        LogPanel::sources_sig(&[entry(2, "beta"), entry(1, "alpha")]),
        "reordering changes which rows sit under which label"
    );
}

/// Log source shortcuts must not select Auto cores, while token navigation remains group-scoped.
///
/// Mutation: restore `select_auto_workspace_core` in `set_source` or call `open_on_main` directly.
/// A retained row or source dropdown could then bypass a later rail selection.
#[test]
fn log_shortcuts_keep_the_shell_rail_as_auto_authority() {
    let source = include_str!("mod.rs");
    let compact: String = source.chars().filter(|ch| !ch.is_whitespace()).collect();

    assert!(!source.contains("select_auto_workspace_core"));
    assert!(compact.contains("b.open_on_main_if_authorized(workspace_group.as_deref()"));
    assert!(compact.contains("(!self.group.is_empty()).then(||self.group.clone())"));
}
