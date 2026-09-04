// Explicit imports, never `use super::*`: the parent glob-imports gpui, whose own `test` shadows
// the built-in attribute and makes `#[test]` expand recursively (CONTRIBUTING.md).
use super::{ExpertTab, TabSource};

use moon_core::feed::FieldMask;

/// The strip must carry Moonbot's pages in Moonbot's order: a trader reaches for a tab by
/// POSITION, and a reordered or shortened strip silently sends them to the wrong page.
#[test]
fn strip_reproduces_moonbots_order() {
    let ids: Vec<&str> = ExpertTab::ALL.iter().map(|t| t.id()).collect();
    assert_eq!(
        ids,
        vec![
            "login",
            "general",
            "telegram",
            "autobuy",
            "special",
            "interface",
            "hotkeys",
            "autostart",
        ]
    );
}

/// Moonbot's setup wizard and its PRO purchase are actions inside that process, not settings, so
/// this window does not carry them at all. Re-adding either means adding a page that can only draw
/// dead buttons.
#[test]
fn moonbots_action_only_tabs_are_absent_from_the_strip() {
    let ids: Vec<&str> = ExpertTab::ALL.iter().map(|t| t.id()).collect();
    assert!(
        !ids.contains(&"help"),
        "the setup wizard is not a settings page"
    );
    assert!(
        !ids.contains(&"pro"),
        "the PRO purchase is not a settings page"
    );
}

/// Index-based selection must agree with the strip it was built from, since `MoonTabStrip` reports
/// a position and nothing else.
#[test]
fn at_indexes_the_same_strip() {
    for (ix, tab) in ExpertTab::ALL.iter().enumerate() {
        assert_eq!(ExpertTab::at(ix), Some(*tab));
    }
    assert_eq!(ExpertTab::at(ExpertTab::ALL.len()), None);
}

/// Exactly the one page safe-share carries nothing for is marked absent. Marking another absent
/// would claim a page is unreachable when its values do cross the wire.
#[test]
fn only_the_wireless_page_is_absent() {
    let absent: Vec<&str> = ExpertTab::ALL
        .iter()
        .filter(|t| t.source() == TabSource::Absent)
        .map(|t| t.id())
        .collect();
    assert_eq!(absent, vec!["login"]);
}

/// `Projected` claims a page can be seeded AND sent today, which is true only of what
/// `moon_core::feed::CoreConfig` carries and the window's own mask writes. Widening this set
/// without widening the projection is how a page would come to draw controls that silently cannot
/// be sent — so this list moves only together with `CoreConfig` and `ExpertTab::add_sections`.
///
/// The order is the strip's, not alphabetical: `ExpertTab::ALL` is what Moonbot's tabs read left to
/// right, and reading the assertion against the dialog is the point of it.
#[test]
fn projected_pages_are_only_the_ones_the_terminal_reads_and_writes() {
    let projected: Vec<&str> = ExpertTab::ALL
        .iter()
        .filter(|t| t.source() == TabSource::Projected)
        .map(|t| t.id())
        .collect();
    assert_eq!(projected, vec!["general", "interface", "autostart"]);
}

/// Every page the strip calls `Projected` must actually name a section, and every page it does not
/// must name none.
///
/// `add_sections` is the single point that decides whether an OK reaches the wire at all: a tab
/// that silently added nothing would drop every edit made on it while `source()` still promised the
/// page was ready, and no other test would notice.
#[test]
fn a_page_names_sections_exactly_when_it_claims_to_be_projected() {
    for tab in ExpertTab::ALL {
        let named = tab.add_sections(FieldMask::EMPTY) != FieldMask::EMPTY;
        assert_eq!(
            named,
            tab.source() == TabSource::Projected,
            "{} claims {:?} but {} a section",
            tab.id(),
            tab.source(),
            if named { "names" } else { "names no" }
        );
    }
}

/// The window opens on a page it can actually fill, not on Moonbot's own first tab — which is the
/// one page that has no wire values at all.
#[test]
fn default_page_is_editable_today() {
    assert_eq!(ExpertTab::default().source(), TabSource::Projected);
}
