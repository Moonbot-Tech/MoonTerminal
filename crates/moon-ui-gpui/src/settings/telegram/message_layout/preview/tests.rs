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
                phone(content, PreviewCache::default()).into_any_element(),
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
