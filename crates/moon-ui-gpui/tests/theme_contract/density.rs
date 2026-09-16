//! Static contracts for the terminal-owned compact density migration.
//!
//! These assertions land with batch 6 because the bans deliberately stay red until the earlier
//! batches finish moving text and controls onto the density-tier UI channel.

use super::support::{Path, braced_body, code_only, read_src, rust_sources};

/// Terminal production code must not return to the legacy font channel outside its one mirror.
///
/// Breakage: adding `tokens.font(` or `design::font_value(` to a terminal surface makes Compact
/// text ignore the neighboring control tier, recreating the uneven rows this migration removes.
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

/// Settings and Analytics tabs must use their density default instead of custom pixel metrics.
///
/// Breakage: restoring `MoonButtonSize::Custom` pins a tab's text and height while its neighbors
/// resize, so Compact rows again look mismatched and stop aligning.
#[test]
fn settings_and_analytics_tabs_do_not_pin_custom_button_metrics() {
    for path in ["settings/render.rs", "analytics/toolbar.rs"] {
        assert!(
            !code_only(&read_src(path)).contains("MoonButtonSize::Custom {"),
            "{path} must use the density tier instead of a custom button size"
        );
    }
}

/// Ordinary controls must not pin themselves to the Standard tier.
///
/// Breakage: restoring `MoonSize::Sm` keeps a 24px/14px control beside Compact's 20px/12px
/// controls, producing the same visibly uneven toolbar and header rows.
#[test]
fn ordinary_controls_do_not_pin_the_standard_tier() {
    for path in [
        "controls/metric.rs",
        "controls/toolbar.rs",
        "chrome/terminal_chrome.rs",
    ] {
        assert!(
            !code_only(&read_src(path)).contains("MoonSize::Sm"),
            "{path} must follow the density tier instead of pinning MoonSize::Sm"
        );
    }
}

/// Toolbar strip text must carry the tier's rendered metrics rather than a font-channel label.
///
/// Breakage: removing `rendered_metrics(design::tier_text_metrics(` makes Size and Sell captions
/// use a legacy size, so their row no longer aligns with the preset controls at Compact.
#[test]
fn toolbar_strip_text_uses_rendered_tier_metrics() {
    let toolbar = read_src("controls/toolbar.rs");
    let strip_text = code_only(braced_body(&toolbar, "fn strip_text("));
    assert!(
        strip_text.contains(".rendered_metrics(design::tier_text_metrics("),
        "controls/toolbar.rs:strip_text must use rendered tier metrics"
    );
}
