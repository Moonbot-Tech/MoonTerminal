//! Health strip: the verdict and every chip stay inside the strip, and chips do not overlap.
//!
//! Explicit imports (no `use super::*`): the parent module glob-imports `gpui`.

use std::rc::Rc;

use gpui::{Context, IntoElement, ParentElement, Render, Styled, Window};
use moon_ui::v_flex;

use super::super::health::{self, HealthFilter};
use super::super::layout_fixture::{self, LOCALES, WIDTHS};
use super::health_strip;
use crate::design;

/// One strip at one dock width. Counts come from [`health::health_counts`], not a hand copy.
struct StripProbe {
    /// Verdict the strip prints.
    verdict: health::Verdict,
    /// Chip counts, including the alarm window.
    counts: health::HealthCounts,
}

impl Render for StripProbe {
    /// Paint the production strip. Clicks are recorded nowhere: the test only reads bounds.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let on_filter: Rc<dyn Fn(HealthFilter, &mut gpui::App)> = Rc::new(|_, _| {});
        let on_alarms: Rc<dyn Fn(&mut gpui::App)> = Rc::new(|_| {});
        v_flex().size_full().child(health_strip(
            &self.verdict,
            &self.counts,
            None,
            None,
            2,
            cx,
            on_filter,
            on_alarms,
        ))
    }
}

/// The verdict and all five chips stay inside the strip at every dock width and locale.
///
/// A chip's count has no debug id of its own (the button segment is anonymous), so the count
/// oracle is the chip box: it is at least as wide as the count measured at caption weight 600,
/// which is the lower bound of the button's own face. A chip narrower than that count is clipped.
#[gpui::test]
fn health_strip_fits_verdict_and_chips_in_every_locale(cx: &mut gpui::TestAppContext) {
    layout_fixture::install_dark(cx);
    let rows = layout_fixture::cores();
    let now_ms = 1_700_000_000_000_i64;
    let counts = health::health_counts(&rows, &[now_ms - 1_000], now_ms);
    let verdict = health::verdict(&rows, &counts, 2);
    assert!(counts.offline_failed > 0, "offline chip");
    assert!(counts.ping_high > 0, "ping chip");
    assert!(counts.version_behind > 0, "version chip");
    assert!(counts.api_expiring > 0, "api chip");
    assert!(counts.alarms_24h > 0, "alarms chip");
    let chips = [
        ("core-status-health-offline", counts.offline_failed),
        ("core-status-health-ping", counts.ping_high),
        ("core-status-health-version", counts.version_behind),
        ("core-status-health-api", counts.api_expiring),
        ("core-status-health-alarms", counts.alarms_24h),
    ];

    for code in LOCALES {
        let _locale = crate::test_locale::force(code);
        for width in WIDTHS {
            let mut visual =
                layout_fixture::open(cx, width, 280.0, move |_, _| StripProbe { verdict, counts });
            layout_fixture::draw(&mut visual);
            let at = format!("{code} @ {width}");
            let strip = visual.debug_bounds("cs-health").expect("strip");
            let verdict_box = visual.debug_bounds("cs-health-verdict").expect("verdict");
            assert!(
                layout_fixture::inside(verdict_box, strip),
                "verdict left the strip ({at}): {verdict_box:?} / {strip:?}"
            );
            let mut boxes = Vec::new();
            for (selector, count) in chips {
                let chip = visual.debug_bounds(selector).expect(selector);
                assert!(
                    layout_fixture::inside(chip, strip),
                    "{selector} left the strip ({at}): {chip:?} / {strip:?}"
                );
                let inner = visual
                    .debug_bounds(layout_fixture::leak_selector(format!("{selector}-inner")))
                    .unwrap_or_else(|| panic!("{selector} inner at {at}"));
                let label = visual
                    .debug_bounds(layout_fixture::leak_selector(format!("{selector}-label")))
                    .unwrap_or_else(|| panic!("{selector} label at {at}"));
                let count_box = visual
                    .debug_bounds(layout_fixture::leak_selector(format!("{selector}-count")))
                    .unwrap_or_else(|| panic!("{selector} count at {at}"));
                assert!(
                    layout_fixture::inside(label, chip) && layout_fixture::inside(count_box, chip),
                    "{selector} label or count left the chip at {at}: {label:?} / {count_box:?} / {chip:?}"
                );
                assert!(
                    layout_fixture::width_of(label) + layout_fixture::width_of(count_box)
                        <= layout_fixture::width_of(inner) + 0.5,
                    "{selector} label plus count exceed the chip interior at {at}: label {:?} count {:?} inner {:?}",
                    label,
                    count_box,
                    inner
                );
                let count_w = visual
                    .update(|_, app| design::ui_caption_text_width(app, &count.to_string(), 600.0));
                assert!(
                    layout_fixture::width_of(count_box) + 0.5 >= count_w,
                    "{selector} count box is narrower than {count} ({count_w}px) at {at}: {count_box:?}"
                );
                boxes.push((selector, chip));
            }
            for (index, (left_name, left)) in boxes.iter().enumerate() {
                for (right_name, right) in boxes.iter().skip(index + 1) {
                    assert!(
                        !layout_fixture::overlaps(*left, *right),
                        "{left_name} overlaps {right_name} at {at}: {left:?} / {right:?}"
                    );
                }
            }
        }
    }
}
