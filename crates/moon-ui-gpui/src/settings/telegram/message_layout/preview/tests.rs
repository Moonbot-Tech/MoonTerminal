//! Telegram preview: word cuts and the bubble's width.

use gpui::IntoElement;
use moon_core::config::CardLayout;
use moon_tg::{Span as PreviewSpan, Tone};

use super::{PreviewCache, PreviewContent, phone, unbreakable_words};
use crate::settings::general::probe_host::{host, window_text};
use crate::settings::layout_probe::{ProbeSpec, WIDE, sweep_sizes};

/// A plain span of `text`.
fn plain(text: &str) -> PreviewSpan {
    PreviewSpan {
        text: text.to_string(),
        bold: false,
        italic: false,
        tone: Tone::Plain,
    }
}

/// The words' texts, spans joined.
fn texts(words: &[Vec<PreviewSpan>]) -> Vec<String> {
    words
        .iter()
        .map(|w| w.iter().map(|s| s.text.as_str()).collect())
        .collect()
}

/// Catches a cut inside brackets: `(+4.96%) · 250$` wrapped as `(+4.96%` over `) · 250$` because
/// the line breaker may cut before `)`. A bracketed group, even split over styled spans and
/// holding spaces, must come out as one word.
#[test]
fn a_bracketed_figure_is_one_word() {
    let spans = [
        plain("BTC "),
        PreviewSpan {
            tone: Tone::Gain,
            ..plain("+12.40$")
        },
        plain(" ("),
        PreviewSpan {
            tone: Tone::Gain,
            ..plain("+4.96 %")
        },
        plain(") · 250$ · 7m"),
    ];
    assert_eq!(
        texts(&unbreakable_words(&spans)),
        ["BTC", "+12.40$", "(+4.96 %)", "·", "250$", "·", "7m"]
    );
}

/// Catches a styled span losing its tone when a word is cut out of it.
#[test]
fn a_word_keeps_its_span_styles() {
    let spans = [
        plain("a ("),
        PreviewSpan {
            tone: Tone::Loss,
            ..plain("-1%")
        },
        plain(")"),
    ];
    let words = unbreakable_words(&spans);
    assert_eq!(words.len(), 2);
    assert_eq!(words[1][1].tone, Tone::Loss);
}

/// Catches the bubble going back to a narrow fixed width: the synthetic sample card's
/// bracketed trade line must sit on one line in the preview, in every language.
#[gpui::test]
fn the_sample_trade_line_fits_on_one_line(cx: &mut gpui::TestAppContext) {
    let lines = moon_tg::preview_card(&CardLayout::default());
    let trade = lines
        .iter()
        .position(|line| line.spans.iter().any(|s| s.text.contains('(')))
        .expect("the sample card has a bracketed trade line");
    let selector = format!("tg-preview-line-{trade}");
    sweep_sizes(
        cx,
        &[WIDE],
        host(move |_, cx| {
            let content = PreviewContent::Card(moon_tg::preview_card(&CardLayout::default()));
            window_text(
                phone(content, PreviewCache::default(), 1000.0).into_any_element(),
                cx,
            )
        }),
        move |_| ProbeSpec {
            container: "probe-body",
            inside: vec!["tg-preview-bubble".into(), selector.clone()],
            one_line: vec![vec![selector.clone()]],
            ..Default::default()
        },
    );
}

/// Card padding the Settings group box takes off each side: its 1 px border and 14 px padding.
const CARD_INSET: f32 = 15.0;

/// Catches the bubble shrink-wrapping its content (it collapsed to ~60 px in the real window):
/// at a 1700 px and a 900 px window, in every language, the bubble is exactly
/// `min(card - padding, 420)` wide and the sample trade line sits on one line.
#[gpui::test]
fn the_bubble_takes_its_explicit_width(cx: &mut gpui::TestAppContext) {
    use crate::design;
    use crate::settings::layout_probe::body_box;
    use crate::settings::telegram::card_grid_col_w;
    use gpui::{ParentElement as _, Styled as _, px};
    use moon_core::config::Language;

    cx.update(moon_ui::init);
    let lines = moon_tg::preview_card(&CardLayout::default());
    let trade = lines
        .iter()
        .position(|line| line.spans.iter().any(|s| s.text.contains('(')))
        .expect("the sample card has a bracketed trade line");
    let line_sel = crate::settings::layout_probe::leak(format!("tg-preview-line-{trade}"));
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        for size in [(1700.0_f32, 1190.0_f32), (900.0, 700.0)] {
            let window = cx.open_window(gpui::size(px(size.0), px(size.1)), |window, cx| {
                let (column_w, _) = body_box(size, cx);
                let card_w = card_grid_col_w(column_w, 2, cx);
                let inner = card_w - 2.0 * design::ui_value(cx, CARD_INSET);
                host(move |_, cx| {
                    let content =
                        PreviewContent::Card(moon_tg::preview_card(&CardLayout::default()));
                    window_text(
                        gpui::div()
                            .w(px(inner))
                            .child(phone(content, PreviewCache::default(), card_w))
                            .into_any_element(),
                        cx,
                    )
                })(window, cx, size)
            });
            let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
            for _ in 0..3 {
                visual.update(|window, cx| {
                    let _ = window.draw(cx);
                });
            }
            let (expected, line_h) = visual.update(|_, cx| {
                let (column_w, _) = body_box(size, cx);
                let card_w = card_grid_col_w(column_w, 2, cx);
                let expected = (card_w - 2.0 * design::ui_value(cx, CARD_INSET))
                    .min(design::ui_value(cx, 420.0));
                (expected, design::line_value(cx, 14.0))
            });
            let bubble = visual
                .debug_bounds("tg-preview-bubble")
                .expect("bubble laid out");
            let width = f32::from(bubble.size.width);
            assert!(
                (width - expected).abs() < 0.5,
                "bubble {width} != {expected} ({} at {}x{})",
                language.code(),
                size.0,
                size.1
            );
            if expected >= 420.0 - 0.5 {
                let line = visual.debug_bounds(line_sel).expect("trade line laid out");
                assert!(
                    f32::from(line.size.height) < 1.5 * line_h,
                    "trade line wraps at 420 ({})",
                    language.code()
                );
            }
        }
    }
}
