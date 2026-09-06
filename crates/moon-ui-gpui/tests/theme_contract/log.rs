//! Static Log badge contracts for the binary-only GPUI crate.

use super::support::{braced_body, code_only, read_src};

/// `common.rs:side_badge` re-inlining its builder, or `tag_badge` swapping foreground/background
/// color, variant, or size, would fork Report and Log pills or make their semantic text unreadable.
#[test]
fn side_badge_delegates_to_the_shared_soft_tiny_tag_builder() {
    let common = read_src("panels/common.rs");
    let side_badge = code_only(braced_body(&common, "pub(crate) fn side_badge("));
    let tag_badge = code_only(braced_body(&common, "pub(crate) fn tag_badge("));

    assert!(
        side_badge
            .contains("tag_badge(side_word(is_short), if is_short { p.red } else { p.green })"),
        "side_badge must delegate its Report LONG/SHORT pill to tag_badge"
    );
    assert!(
        tag_badge.contains("MoonBadge::new(text)")
            && tag_badge.contains(".variant(MoonBadgeVariant::Soft)")
            && tag_badge.contains(".size(MoonBadgeSize::Tiny)")
            && tag_badge.contains(".bg_color(color)")
            && tag_badge.contains(".text_color(color)"),
        "tag_badge must retain the shared Soft/Tiny builder with color in both fill and text"
    );
}
