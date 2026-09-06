//! Unit tests for the left-pane cache: what its keys must cover, and that the walks it removed
//! stayed removed.
//!
//! Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
//! shadows the built-in attribute and makes `#[test]` expand recursively.

// The key-equality tests a reader expects here are absent on purpose: `PlanKey` and `LabelKey`
// derive `PartialEq`, so "a different field compares unequal" is a property of the derive and
// cannot fail. What CAN fail is a producer growing an input nobody added to the key, which is what
// the scans below are for.

/// Reads `start_stop_plan`'s own source and demands that every piece of WINDOW state it touches
/// maps to a named component of `PlanKey`.
///
/// The map is explicit, and deliberately not "appears somewhere in `data_sig`": the plan key takes
/// the signature's STORE half and its staged digest only, so a plan that grew a view-state
/// condition — `filter.active_only`, `sel`, `selected_folder` — would be hashed by `data_sig` and
/// still absent from this key. Reading the signature as a whole would call that covered and wave
/// through the exact staleness this test exists for: a footer dispatching the pre-change target set
/// until the store or staging moves.
///
/// Store reads (`cores`, `cd.strategies`) arrive as ARGUMENTS rather than through `self`, so this
/// scan does not see them; they are the store half, kept complete by the tree's own contract test.
#[test]
fn the_plan_key_covers_every_field_the_plan_reads() {
    let actions = include_str!("../../actions.rs");
    let body = actions
        .split("pub(super) fn start_stop_plan(")
        .nth(1)
        .and_then(|tail| tail.split("\n    }").next())
        .expect("the plan builder must exist");
    // Whitespace-stripped so a field reached across a line break is found like an inline one.
    let packed: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let packed_cache: String = include_str!("../pane_cache.rs")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();

    /// Which component of `PlanKey` carries this read, if any.
    fn key_component(field: &str) -> Option<&'static str> {
        match field {
            // The staged digest `data_sig` computes and `TreeSig` carries.
            "staged" => Some("staged:sig.staged"),
            // The accessor IS the component.
            "action_workspace_generation" => Some("generation:self.action_workspace_generation("),
            _ => None,
        }
    }

    let mut missing = Vec::new();
    let mut checked = 0;
    for chunk in packed.split("self.").skip(1) {
        let field: String = chunk
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if field.is_empty() {
            continue;
        }
        checked += 1;
        // The map is only worth anything while the key still builds that component, so both halves
        // are checked: the field is claimed, and the claim is present in the key.
        match key_component(&field) {
            Some(component) if packed_cache.contains(component) => {}
            _ => missing.push(field),
        }
    }

    assert!(
        checked >= 2,
        "the scan found only {checked} state reads — the plan builder was renamed or split and \
         this test now checks nothing"
    );
    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "the Start/Stop plan reads these but the cache key does not, so a cached plan can dispatch \
         a stale target set:\n{}",
        missing.join("\n")
    );
}

/// The footer's density decision is made from a measured width, so the key that retains it has to
/// carry everything a measurement depends on. Scoped to the construction site: a presence check
/// over the whole file would pass on the struct definition or a doc comment alone.
#[test]
fn the_label_key_is_built_from_locale_typography_and_staging() {
    let built = include_str!("../pane_cache.rs")
        .split("let key = LabelKey {")
        .nth(1)
        .and_then(|tail| tail.split("};").next())
        .expect("the label key must be built somewhere");

    for component in ["locale", "metrics", "staged"] {
        assert!(
            built.contains(component),
            "a measured width that outlives a change in {component} is a stale footer"
        );
    }
}

/// The plan is the one producer that cannot simply be made private — `apply_start_stop` needs it
/// too — so a scan stands in for privacy. The kinds walk needed no such guard: it moved INTO the
/// cache module as a private fn, where the compiler enforces what this would only assert.
#[test]
fn the_footer_never_rebuilds_the_plan_per_frame() {
    for (name, source) in [
        ("tree/mod.rs", include_str!("../mod.rs")),
        ("tree/ui.rs", include_str!("../ui.rs")),
        ("strategies/mod.rs", include_str!("../../mod.rs")),
    ] {
        assert!(
            !source.contains("start_stop_plan("),
            "{name} must capture the cached plan, not rebuild it per frame"
        );
    }
    assert!(include_str!("../pane_cache.rs").contains("self.start_stop_plan("));
}

/// `can_paste` used to re-derive the visible core list on every frame to answer a question its
/// caller already held. Scoped to that one statement on purpose: the paste ACTION next to it must
/// keep resolving its own target at click time, when the workspace may have moved since the frame.
#[test]
fn the_selection_toolbar_takes_core_visibility_from_its_caller() {
    let can_paste = include_str!("../ui.rs")
        .split("let can_paste =")
        .nth(1)
        .and_then(|tail| tail.split(';').next())
        .expect("the paste enablement statement must exist");

    assert!(
        !can_paste.contains("visible_strategy_cores("),
        "enablement must use the caller's canonical core list"
    );
    assert!(can_paste.contains("has_visible_cores"));
}

/// The same demand as the plan key, for the move buttons' enablement: every piece of WINDOW state
/// the planner touches must be covered by a named component of `MoveKey`.
///
/// Two functions are scanned, not one. `movable_selection` reads most of it, but it reaches the
/// unconfirmed-order overlay through `displayed_rows`; whitelisting that name as if it were a field
/// would put everything behind it — today `pending_order`, tomorrow whatever is added — outside the
/// scan this test is named for.
///
/// And covering the view HALF is only half an argument, since that half is built elsewhere. So the
/// second block below checks the other end: that `data_sig` still folds each of these fields into
/// it. Without that pair, `MoveKey` could name a component that no longer carries what it claims.
///
/// One input the scan cannot see either way: `workspace_cores`, which the planner reads through
/// `selected_keys` and which NEITHER half of the tree signature hashes — a preset naming exactly
/// the connected cores moves no signature at all. It gets its own component, asserted at the end.
#[test]
fn the_move_key_covers_every_field_the_enablement_reads() {
    /// Window state the planner reads directly, all of it inside the signature's view half.
    const VIEW_FIELDS: [&str; 4] = [
        "filter",
        "expanded_cores",
        "rail_expanded_core",
        // Read by `displayed_rows`, which is scanned alongside the planner for exactly this reason.
        "pending_order",
    ];

    let reorder = include_str!("../reorder.rs");
    let body = |marker: &str| {
        reorder
            .split(marker)
            .nth(1)
            .and_then(|tail| tail.split("\n    }").next())
            .expect("the scanned function must exist")
            .to_string()
    };
    let scanned = format!(
        "{}{}",
        body("fn movable_selection<"),
        body("fn displayed_rows<")
    );
    let packed: String = scanned.chars().filter(|c| !c.is_whitespace()).collect();
    let cache: String = include_str!("../pane_cache.rs")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let signature = include_str!("../cache.rs");

    let mut missing = Vec::new();
    let mut checked = 0;
    for chunk in packed.split("self.").skip(1) {
        let field: String = chunk
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        // `displayed_rows` is the scanned callee itself, not an input of its own.
        if field.is_empty() || field == "displayed_rows" {
            continue;
        }
        checked += 1;
        if !VIEW_FIELDS.contains(&field.as_str()) {
            missing.push(field);
        }
    }
    assert!(
        checked >= 5,
        "the scan found only {checked} field reads — a read moved into a helper and left this \
         test guarding less than it claims"
    );
    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "these inputs decide the move buttons but are absent from MoveKey, so their enablement \
         would go stale:\n{}",
        missing.join("\n")
    );

    // The key names the view half...
    assert!(
        cache.contains("view:sig.view"),
        "the view half must be a component of MoveKey, since every field above lives in it"
    );
    // ... and the view half still carries each of those fields.
    for field in VIEW_FIELDS {
        assert!(
            signature.contains(&format!("view.{field}")),
            "data_sig no longer folds `{field}` into the view half, so MoveKey's claim to cover it \
             through `sig.view` is empty"
        );
    }

    // The one input neither scan nor signature can account for.
    assert!(
        packed.contains("selected_keys(self)"),
        "the selection must come from the canonical resolver, which is what applies the workspace \
         scope the component below stands for"
    );
    assert!(
        cache.contains("workspace:workspace_digest(self.workspace_cores"),
        "the workspace scope is hashed by neither half of the tree signature, so MoveKey must \
         carry it directly"
    );
}

/// The enablement walks every strategy of every selected core and allocates a canonical folder path
/// per row. It belongs behind the pane cache, and every render path must take the answer from its
/// caller — the same rule `the_plan_key_covers_every_field_the_plan_reads` enforces for the
/// Start/Stop plan.
///
/// The context menu is exempt and named here so the exemption is deliberate: it is built on a
/// right-click, not on a frame.
#[test]
fn the_move_buttons_take_their_enablement_from_the_pane_cache() {
    for (name, source) in [
        ("tree/ui.rs", include_str!("../ui.rs")),
        ("tree/mod.rs", include_str!("../mod.rs")),
        ("tree/moon.rs", include_str!("../moon.rs")),
        ("tree/cache.rs", include_str!("../cache.rs")),
        ("strategies/mod.rs", include_str!("../../mod.rs")),
    ] {
        assert!(
            !source.contains("move_availability("),
            "{name} must render the cached answer, not re-derive it per frame"
        );
    }
    assert!(include_str!("../pane_cache.rs").contains("self.move_availability("));
}
