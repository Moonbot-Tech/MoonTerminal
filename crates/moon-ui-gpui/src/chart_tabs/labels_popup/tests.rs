//! Unit proofs for chart-label popup label fitting.

use super::fit_row_name;

/// `labels_popup::fit_row_name` must reserve the suffix before fitting the head: fitting the whole
/// name first makes the rendered label wider than its button and hides the used-parts count.
#[test]
fn fits_name_head_before_pinned_parts_count() {
    let suffix = "  ·5";
    let budget = 11.0;
    let measure = |text: &str| text.chars().count() as f32;

    let fitted = fit_row_name("orders exposure liquidity", suffix, budget, measure);

    assert!(
        fitted.ends_with(suffix),
        "the used-parts count must remain verbatim at the end of the fitted label"
    );
    assert!(
        measure(&fitted) <= budget,
        "the fitted label must stay inside the name button's text budget"
    );
}

/// Reverting the band trigger to the fixed 104 box, or `content_width` to a literal band term,
/// must fail: every band name shows whole in every language and font step, and a module line at
/// the drawn trigger width still fits the popup.
#[gpui::test]
fn band_names_show_whole_and_the_line_fits_the_popup(cx: &mut gpui::TestAppContext) {
    use gpui::{InteractiveElement as _, IntoElement as _, ParentElement as _, Styled as _};
    use moon_core::config::LabelZone;

    crate::controls::trigger_fit::probe::every_locale(
        cx,
        |_, cx| {
            let mut row = gpui::div().flex();
            for (ix, z) in LabelZone::ALL.iter().enumerate() {
                row = row.child(
                    gpui::div()
                        .flex_none()
                        .debug_selector(move || format!("zone-{ix}"))
                        .child(super::rows::zone_dropdown(
                            ix,
                            rust_i18n::t!(z.locale_key()).to_string(),
                            cx,
                        )),
                );
            }
            row.into_any_element()
        },
        |frame| {
            let mut widest = 0.0f32;
            for (ix, z) in LabelZone::ALL.iter().enumerate() {
                let sel = format!("zone-{ix}");
                frame.assert_holds(&sel, &rust_i18n::t!(z.locale_key()));
                widest = widest.max(f32::from(frame.bounds(&sel).size.width));
            }
            let (content, line) = frame.measure(|app| {
                let rest =
                    9.0 * super::MICRO_W + super::NAME_W + super::GAP_W + 11.0 * super::ROW_GAP;
                (
                    f32::from(super::content_width(app)),
                    crate::design::font_w(app, rest) + widest,
                )
            });
            assert!(
                line <= content,
                "module line {line} overflows popup {content}: {}",
                frame.at
            );
        },
    );
}
