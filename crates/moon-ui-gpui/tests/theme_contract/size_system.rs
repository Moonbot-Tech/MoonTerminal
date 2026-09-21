//! Static contracts for the terminal's one size system.
//!
//! The terminal renders one design: ordinary controls on `design::CONTROL_TIER`, text on
//! `design::BODY_TEXT` plus a local step, scaled as a whole by the window content zoom. These
//! contracts keep a surface from forking that system back into a second one.

use super::support::{Path, braced_body, code_only, read_src, rust_sources};

/// Terminal production code must not return to MoonUI's legacy font channel outside its one mirror.
///
/// Breakage: adding `tokens.font(` or `design::font_value(` to a terminal surface sizes that text
/// on `font_delta` instead of the body size, so it stops agreeing with the controls beside it.
#[test]
fn terminal_text_uses_no_legacy_font_channel_outside_documented_mirrors() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    rust_sources(&root, &mut sources);
    let violations: Vec<String> = sources
        .iter()
        .filter(|path| path.file_name().is_some_and(|name| name != "tests.rs"))
        .filter_map(|path| {
            let rel = path
                .strip_prefix(&root)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            let code = code_only(&std::fs::read_to_string(path).ok()?);
            (!matches!(rel.as_str(), "design.rs" | "settings/connections/table.rs")
                && (code.contains("tokens.font(") || code.contains("design::font_value(")))
            .then_some(rel)
        })
        .collect();
    assert!(
        violations.is_empty(),
        "legacy font-channel calls outside documented mirrors: {violations:?}"
    );
}

/// Settings and Analytics tabs must use the control tier instead of custom pixel metrics.
///
/// Breakage: restoring `MoonButtonSize::Custom` pins a tab's text and height while its neighbours
/// follow the tier, so the rows stop aligning.
#[test]
fn settings_and_analytics_tabs_do_not_pin_custom_button_metrics() {
    for path in ["settings/render.rs", "analytics/toolbar.rs"] {
        assert!(
            !code_only(&read_src(path)).contains("MoonButtonSize::Custom {"),
            "{path} must use the control tier instead of a custom button size"
        );
    }
}

/// Ordinary controls must read the tier through `design::CONTROL_TIER`, never as a literal.
///
/// Breakage: a literal `MoonSize::Sm` in a chrome or toolbar control is a second spelling of the
/// one size system; the day the constant moves, that control stays behind.
#[test]
fn ordinary_controls_do_not_spell_the_control_tier_as_a_literal() {
    for path in [
        "controls/metric.rs",
        "controls/toolbar.rs",
        "chrome/terminal_chrome.rs",
    ] {
        assert!(
            !code_only(&read_src(path)).contains("MoonSize::Sm"),
            "{path} must read design::CONTROL_TIER instead of spelling MoonSize::Sm"
        );
    }
}

/// Toolbar strip text must carry the body text's rendered metrics rather than a font-channel label.
///
/// Breakage: removing `rendered_metrics(design::text_metrics(` makes the Size and Sell captions
/// use a legacy size, so their row no longer aligns with the preset controls.
#[test]
fn toolbar_strip_text_uses_rendered_text_metrics() {
    let toolbar = read_src("controls/toolbar.rs");
    let strip_text = code_only(braced_body(&toolbar, "fn strip_text("));
    assert!(
        strip_text.contains(".rendered_metrics(design::text_metrics("),
        "controls/toolbar.rs:strip_text must use rendered text metrics"
    );
}
