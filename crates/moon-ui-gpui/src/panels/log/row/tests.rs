// Explicit imports on purpose: `use super::*` would pull in the parent's `gpui::*`
// re-export, whose `test` shadows the built-in attribute and makes `#[test]` expand
// recursively ("recursion limit reached").
use super::{badge, cat_badge};
use crate::panels::line_list::{Cat, Sev};
use moon_ui::MoonPalette;

/// `row.rs:badge` or `row.rs:cat_badge` remapping a tag to a different palette field would make
/// Log severity/category pills disagree with their established semantic colors.
#[test]
fn log_badges_keep_the_shared_severity_and_category_mapping() {
    let p = MoonPalette::default();

    assert_eq!(badge(Sev::Error, p), Some(("ERR", p.red)));
    assert_eq!(badge(Sev::Warn, p), Some(("WARN", p.amber)));
    for sev in [Sev::Info, Sev::Dim, Sev::Noise] {
        assert_eq!(badge(sev, p), None);
    }

    assert_eq!(cat_badge(Cat::Reject, p), Some(("REJ", p.orange)));
    assert_eq!(cat_badge(Cat::Conn, p), Some(("NET", p.yellow)));
    assert_eq!(cat_badge(Cat::None, p), None);
}
