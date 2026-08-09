//! Regression tests for effective Log selection and incremental cursor behavior.

// Explicit imports on purpose: `use super::*` would pull in the parent's `gpui::*` re-export,
// whose `test` shadows the built-in attribute and makes `#[test]` expand recursively
// ("recursion limit reached").
use super::{LogFile, LogPanel, LogSource, LogSourceItem, resolve_workspace_log_selection};

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
