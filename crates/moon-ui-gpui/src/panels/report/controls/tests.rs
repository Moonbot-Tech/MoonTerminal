//! Structural regression coverage for the retained Report scope control.

/// Return the source between two stable function anchors.
///
/// Args:
///     source: Complete source file.
///     start: Opening function signature fragment.
///     end: Following function signature fragment.
///
/// Returns:
///     The bounded source slice containing the requested function.
fn between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let from = source.find(start).expect("start function must exist");
    let tail = &source[from..];
    let to = tail.find(end).expect("end function must exist");
    &tail[..to]
}

/// Adding an owner update to `ReportScopeControl::set_menu_open` must fail this assertion; popup
/// open/close would then invalidate the full Report data owner and restore the multi-second stall.
#[test]
fn menu_visibility_updates_only_the_retained_child() {
    let source = include_str!("../controls.rs");
    let set_open = between(source, "fn set_menu_open", "fn select_side");
    let set_open_code = set_open
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let render = between(
        source,
        "impl Render for ReportScopeControl",
        "impl ReportPanel",
    );

    assert!(set_open_code.contains("self.menu_open = open"));
    assert!(set_open_code.contains("cx.notify()"));
    assert!(!set_open_code.contains("owner"));
    assert!(render.contains(".open(self.menu_open)"));
    assert!(render.contains("this.set_menu_open(open, cx)"));
}

/// Removing the guarded owner setters from `ReportScopeControl` must fail this assertion; actual
/// side, kind, and deleted selections would otherwise repaint only the child without a new query.
#[test]
fn real_scope_selections_reach_guarded_owner_invalidation() {
    let controls = include_str!("../controls.rs");
    let actions = include_str!("../actions.rs");
    let side = between(controls, "fn select_side", "fn select_kind");
    let kind = between(controls, "fn select_kind", "fn toggle_deleted");
    let deleted = between(controls, "fn toggle_deleted", "fn toggle_comment");

    assert!(side.contains("owner.read(cx).side != side"));
    assert!(side.contains("panel.set_side(side, cx)"));
    assert!(kind.contains("owner.read(cx).kind != kind"));
    assert!(kind.contains("panel.set_kind(kind, cx)"));
    assert!(deleted.contains("panel.set_deleted_only(deleted_only, cx)"));
    assert!(actions.matches("self.request_requery(cx)").count() >= 3);
}

/// Scope state with every choice at its default.
fn default_choices() -> super::ScopeChoices {
    super::ScopeChoices {
        side: moon_core::db::SideFilter::All,
        kind: super::ReportKind::All,
        basis_key: None,
        deleted_only: false,
        open_rows_hidden: false,
    }
}

/// All-default filters list no difference, so the caption is the bare title.
#[test]
fn default_scope_has_no_difference() {
    assert!(super::scope_difference_keys(&default_choices()).is_empty());
    assert_eq!(super::scope_caption("Filters", &[], false), "Filters");
}

/// Each non-default choice contributes exactly its own short label.
#[test]
fn each_difference_names_its_own_label() {
    let base = default_choices();
    let cases: [(super::ScopeChoices, &str); 7] = [
        (
            super::ScopeChoices {
                side: moon_core::db::SideFilter::Long,
                ..base
            },
            "report.side.long",
        ),
        (
            super::ScopeChoices {
                side: moon_core::db::SideFilter::Short,
                ..base
            },
            "report.side.short",
        ),
        (
            super::ScopeChoices {
                kind: super::ReportKind::Real,
                ..base
            },
            "report.kind.real_short",
        ),
        (
            super::ScopeChoices {
                kind: super::ReportKind::Emu,
                ..base
            },
            "report.kind.emu_short",
        ),
        (
            super::ScopeChoices {
                basis_key: Some("report.period_basis.open_short"),
                ..base
            },
            "report.period_basis.open_short",
        ),
        (
            super::ScopeChoices {
                deleted_only: true,
                ..base
            },
            "report.filter.deleted_short",
        ),
        (
            super::ScopeChoices {
                open_rows_hidden: true,
                ..base
            },
            "report.filter.closed_only_short",
        ),
    ];
    for (choices, key) in cases {
        assert_eq!(super::scope_difference_keys(&choices), vec![key]);
    }
}

/// Combined differences keep menu order and join after the title with middle dots.
#[test]
fn combined_differences_follow_menu_order() {
    let choices = super::ScopeChoices {
        side: moon_core::db::SideFilter::Short,
        kind: super::ReportKind::Emu,
        basis_key: Some("report.period_basis.open_short"),
        deleted_only: true,
        open_rows_hidden: true,
    };
    assert_eq!(
        super::scope_difference_keys(&choices),
        vec![
            "report.side.short",
            "report.kind.emu_short",
            "report.period_basis.open_short",
            "report.filter.deleted_short",
            "report.filter.closed_only_short",
        ]
    );
    let labels = ["Short".to_string(), "emu".to_string(), "open".to_string()];
    assert_eq!(
        super::scope_caption("Filters", &labels, false),
        "Filters · Short · emu · open"
    );
}

/// A compact row drops the differences entirely instead of clipping them.
#[test]
fn compact_caption_is_the_bare_title() {
    let labels = ["Short".to_string(), "emu".to_string()];
    assert_eq!(super::scope_caption("Filters", &labels, true), "Filters");
}

/// The row signature is read from the choices alone — `of` takes no fit state, which is what keeps
/// the fit from feeding itself — and follows them, so a changed pick re-resolves the fit.
#[test]
fn row_labels_follow_the_scope_choices() {
    let period = super::super::Period::Today;
    let changed = super::ScopeChoices {
        deleted_only: true,
        ..default_choices()
    };
    assert_eq!(
        super::FilterRowLabels::of(&period, &default_choices()),
        super::FilterRowLabels::of(&period, &default_choices())
    );
    assert_ne!(
        super::FilterRowLabels::of(&period, &default_choices()),
        super::FilterRowLabels::of(&period, &changed)
    );
}
