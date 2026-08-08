// Explicit imports on purpose: `use super::*` would pull in the parent's `gpui::*` re-export,
// whose `test` shadows the built-in attribute and makes `#[test]` expand recursively
// ("recursion limit reached").
use super::{LogPanel, LogSource, LogSourceItem};

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
