use std::path::PathBuf;

use super::*;

/// A fresh directory under the OS temp root, unique per call.
fn scratch(case: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("test clock must follow the Unix epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "moonterminal-minidump-{case}-{}-{stamp}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn touch(dir: &std::path::Path, name: &str) {
    std::fs::write(dir.join(name), b"x").expect("write");
}

fn names(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .expect("read")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn prune_leaves_room_for_the_next_dump_and_ignores_other_files() {
    let dir = scratch("prune");
    for name in [
        "crash-20260901-100000-4242.dmp",
        "crash-20260902-100000-99.dmp",
        "crash-20260903-100000-123.dmp",
        "crash-20260904-100000-7.dmp",
        "moonterminal.log",
        "other.dmp",
    ] {
        touch(&dir, name);
    }
    prune(&dir, KEEP);
    assert_eq!(
        names(&dir),
        [
            "crash-20260903-100000-123.dmp",
            "crash-20260904-100000-7.dmp",
            "moonterminal.log",
            "other.dmp",
        ],
        "the two newest dumps stay (KEEP - 1), the log and a foreign .dmp are untouched"
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn prune_on_a_missing_or_sparse_folder_is_a_no_op() {
    prune(
        std::path::Path::new(r"Z:\does\not\exist\moonterminal-minidump"),
        KEEP,
    );
    let dir = scratch("sparse");
    touch(&dir, "crash-20260901-100000-4242.dmp");
    prune(&dir, KEEP);
    assert_eq!(names(&dir), ["crash-20260901-100000-4242.dmp"]);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}
