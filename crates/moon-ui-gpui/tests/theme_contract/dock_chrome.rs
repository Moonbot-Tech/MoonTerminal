//! Static contracts for the dock chrome shared by every panel (Goal C): the pinned-scope chip,
//! the one footer rule, and the toolbar band `detects/popup.rs` shares with its siblings.
//!
//! `panels/common.rs` is consumed by five panel hosts and six footers at once, so nothing here
//! belongs to any single panel's own module — it is the shared surface itself.

use super::support::*;

/// `panels/common.rs::pinned_scope_label`'s label child must stay `.min_w_0().truncate()`.
///
/// Mutation: drop either call from the label child. Consequence: a long pinned core name
/// overflows a narrow (~400 px) dock instead of clipping, pushing the sibling toolbar controls
/// off screen — and five panels inherit the chip at once, so one dropped clamp regresses all of
/// them together.
#[test]
fn pinned_scope_label_stays_clipped() {
    let common = code_only(&read_src("panels/common.rs"));
    let label = braced_body(&common, "fn pinned_scope_label(");
    assert!(
        label.contains(".min_w_0()") && label.contains(".truncate()"),
        "pinned_scope_label's label child must keep both .min_w_0() and .truncate()"
    );
}

/// `pinned_scope_label` is a STATIC, non-interactive chip — the host owns the tooltip and the
/// selection stays workspace-owned.
///
/// Mutation: add an `.on_click(` / `.hover(` / `.cursor_pointer()` to it. Consequence: a scope
/// the workspace owns becomes clickable and can silently rewrite the retained Classic selection;
/// `core_host.rs:98 core_selection_pinned`'s guard is only the second half of that defence, so a
/// clickable chip would reopen the hole from the other side.
#[test]
fn pinned_scope_label_stays_non_interactive() {
    let common = code_only(&read_src("panels/common.rs"));
    let label = braced_body(&common, "fn pinned_scope_label(");
    assert!(
        !label.contains(".on_click(")
            && !label.contains(".hover(")
            && !label.contains(".cursor_pointer()"),
        "pinned_scope_label must stay a static chip: no .on_click(, .hover(, or .cursor_pointer()"
    );
}

/// `panels/common.rs::footer_row` must carry a height floor.
///
/// Mutation: drop `min_h(panel_band_min_h_px(cx))`. Consequence: footer height jumps between
/// panels — the one carrying a Micro button sits taller than the ones carrying only text — which
/// is the exact inconsistency this change removes.
#[test]
fn footer_row_keeps_its_height_floor() {
    let common = code_only(&read_src("panels/common.rs"));
    let footer_row = braced_body(&common, "fn footer_row(");
    assert!(
        footer_row.contains("min_h(design::panel_band_min_h_px(cx))"),
        "footer_row must keep min_h(design::panel_band_min_h_px(cx)) or footer heights drift again"
    );
}

/// Every footer that adopts the shared helper must keep adopting it — the "one footer rule" is
/// only a rule while every site actually calls the shared builder.
///
/// Mutation: a panel keeps (or a new panel grows) its own hand-rolled
/// `h_flex()....px_2().py_1()` footer container instead of `common::footer_row` /
/// `common::footer_caption`. Consequence: the one-footer rule silently stops being one rule, one
/// site at a time. The six footers are `report/render.rs`, `assets/table.rs`,
/// `orders/render.rs`, `core_status/mod.rs`, `news/mod.rs`, and `assets/balances.rs`'s caption
/// path.
#[test]
fn every_footer_adopts_the_shared_footer_helpers() {
    let sites: [(&str, &str); 6] = [
        ("panels/report/render.rs", "footer_row("),
        ("panels/assets/table.rs", "footer_row("),
        ("panels/orders/render.rs", "footer_row("),
        ("panels/core_status/mod.rs", "footer_row("),
        ("panels/news/mod.rs", "footer_row("),
        ("panels/assets/balances.rs", "footer_caption("),
    ];
    for (path, needle) in sites {
        let source = code_only(&read_src(path));
        assert!(
            source.contains(needle),
            "{path} must adopt the shared `{needle}` helper rather than a hand-rolled footer \
             container of its own"
        );
    }
}

/// `panels/common.rs::pinned_scope_host` must actually wrap the chip in the tooltip that explains
/// why the pinned selector cannot be clicked — the tooltip host the fix barrier factored OUT of
/// all seven call sites into this one place.
///
/// Mutation: drop the `.tooltip(...)` call. Consequence: every pinned selector across Report,
/// Orders, Assets, Core Status, Alerts, and both Log hosts silently stops explaining itself at
/// once — a single dropped line regresses all seven, which is the entire reason the shared host
/// exists.
#[test]
fn pinned_scope_host_keeps_its_tooltip() {
    let common = code_only(&read_src("panels/common.rs"));
    let host = braced_body(&common, "fn pinned_scope_host(");
    assert!(
        host.contains(".tooltip(text_tooltip(")
            && host.contains("workspace.scope.pinned_hint")
            && host.contains("scope = label.clone()")
            && host.contains(".child(pinned_scope_label(chip_id, label, width, p, cx))"),
        "pinned_scope_host must wrap the chip in the shared tooltip, or every pinned selector \
         stops explaining itself at once"
    );
}

/// `design::action_control_h_value` and `design::glyph_btn_w` are two INDEPENDENTLY written
/// mirrors of the same upstream MoonUI metric — `glyph_btn_w`'s own doc names it "which
/// `MoonButtonSize::Action` resolves to" — so they must keep agreeing with each other even though
/// neither can be checked against the private `MoonButtonMetrics` directly. Source vs source, not
/// a constant compared against its own literal: this is the one oracle available for either.
///
/// Mutation: move `action_control_h_value`'s triple without moving `glyph_btn_w`'s, or vice versa.
/// Consequence: the pinned chip (sized off `action_control_h_value`) stops matching the
/// Action-size controls standing beside it in the same row — a font-scale drift neither
/// function's own body would ever reveal by itself.
#[test]
fn action_control_h_value_agrees_with_glyph_btn_w() {
    let design = code_only(&read_src("design.rs"));
    let action = braced_body(&design, "fn action_control_h_value(");
    let glyph = braced_body(&design, "fn glyph_btn_w(");
    let triple = "fit_h_value(cx, 26.0, 14.0, 6.0)";
    assert!(
        action.contains(triple) && glyph.contains(triple),
        "action_control_h_value and glyph_btn_w must mirror the SAME MoonUI Action metrics: {triple}"
    );
}

/// The two new locale keys Goal C lands must each carry all three languages.
///
/// Mutation: land `workspace.scope.pinned_hint` or `assets.refresh_hint` with `ru` only.
/// Consequence: `locales/*.yml` is compiled by the `i18n!` proc macro, so a key missing in one
/// language surfaces at runtime as the raw key rather than as a build error — the raw key would
/// render for `en`/`es` users.
#[test]
fn goal_c_locale_keys_carry_all_three_languages() {
    let cases = [
        ("workspace.yml", "workspace.scope.pinned_hint"),
        ("assets.yml", "assets.refresh_hint"),
    ];
    for (file, key) in cases {
        let locales = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join("locales")
                .join(file),
        )
        .unwrap_or_else(|err| panic!("failed to read locales/{file}: {err}"))
        .replace("\r\n", "\n");
        let after = locales
            .split_once(&format!("{key}:\n"))
            .unwrap_or_else(|| panic!("locales/{file} does not define {key}"))
            .1;
        let members: Vec<&str> = after
            .lines()
            .take_while(|line| line.starts_with("  "))
            .collect();
        assert_eq!(
            members.len(),
            3,
            "{key} in locales/{file} must define exactly ru, en, and es"
        );
        for locale in ["ru", "en", "es"] {
            assert!(
                members
                    .iter()
                    .any(|line| line.starts_with(&format!("  {locale}: "))),
                "{key} in locales/{file} must carry {locale}, or that language shows the raw \
                 key instead"
            );
        }
    }
}

/// `panels/detects/popup.rs::toolbar` must stay on the shared `panel_band` chrome, and
/// `p.tabbar` must have no other use in the crate once it lands.
///
/// Mutation: reintroduce a hard-coded `.h(design::fit_h_px(cx, 28.0` or `.bg(rgb(p.tabbar))`
/// there. Consequence: the Detects band misaligns with every sibling tab again — the defect this
/// change fixes. `p.tabbar` is expected to have no other use in the crate after this change; a
/// contract on that is cheap.
#[test]
fn detects_toolbar_keeps_the_shared_band_and_drops_tabbar() {
    let popup = code_only(&read_src("panels/detects/popup.rs"));
    let toolbar = braced_body(&popup, "fn toolbar(");
    assert!(
        !toolbar.contains("design::fit_h_px(cx, 28.0") && !toolbar.contains(".bg(rgb(p.tabbar))"),
        "Detects' toolbar must not reintroduce its own hard-coded height or tabbar background"
    );
    assert!(
        toolbar.contains("panel_band("),
        "Detects' toolbar must adopt the shared panel_band chrome"
    );

    let mut sources = Vec::new();
    rust_sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut sources,
    );
    let hits: Vec<String> = sources
        .iter()
        .filter_map(|path| {
            let text = fs::read_to_string(path).ok()?.replace("\r\n", "\n");
            code_only(&text)
                .contains("p.tabbar")
                .then(|| path.display().to_string())
        })
        .collect();
    assert!(
        hits.is_empty(),
        "p.tabbar is expected to have no remaining use once Goal C lands; found in {hits:?}"
    );
}
