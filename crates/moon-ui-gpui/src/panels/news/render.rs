//! News card rendering: one card per logical news item, plus the small time/chip helpers.
//!
//! Visual language matches the terminal: a neutral source badge on the left, a right-aligned
//! relative time, blue ticker chips, the body in the selected language (English fallback), and muted
//! tag chips. Cards are separated by a hairline so items read as distinct blocks.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonBadge, MoonBadgeSize, MoonBadgeVariant, MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::NewsLang;
use crate::design;
use moon_core::feed::NewsItem;

/// Build a soft tinted badge (source / ticker / pending) using the house `MoonBadge`, sized to the
/// card's caption tier. `color` (a `MoonPalette` token) tints both the fill and the text.
fn badge(text: impl Into<SharedString>, color: u32) -> impl IntoElement {
    MoonBadge::new(text)
        .variant(MoonBadgeVariant::Soft)
        .size(MoonBadgeSize::Tiny)
        .bg_color(color)
        .text_color(color)
        .mono(true)
        .render()
}

/// Build one news card for `item` in the selected `lang`.
pub(super) fn news_card(
    item: &NewsItem,
    lang: NewsLang,
    now_ms: i64,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let caption = design::t_caption(cx);

    // --- meta row: source badge (left) · pending badge · spacer · relative time (right) ---
    let mut meta = h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 6.0));
    if !item.source.is_empty() {
        meta = meta.child(badge(item.source.to_uppercase(), p.text_muted));
    }
    if lang.missing(item) {
        meta = meta.child(badge(t!("news.pending").to_string(), p.amber));
    }
    meta = meta.child(div().flex_1()).child(
        div()
            .flex_none()
            .text_size(caption)
            .text_color(rgb(p.text_muted))
            .child(rel_time(item.time_ms, now_ms)),
    );

    // --- ticker chips ---
    let tickers = (!item.coins.is_empty()).then(|| {
        h_flex()
            .w_full()
            .flex_wrap()
            .gap(design::ui_px(cx, 5.0))
            .children(item.coins.iter().map(|coin| badge(coin.clone(), p.blue)))
    });

    // --- body in the selected language (English fallback done by NewsLang::text) ---
    let body_text = lang.text(item);
    let body = (!body_text.is_empty()).then(|| {
        div()
            .w_full()
            .text_size(design::t_body(cx))
            .text_color(rgb(p.text))
            .child(body_text.to_string())
    });

    // --- tag chips ---
    let tags = (!item.tags.is_empty()).then(|| {
        h_flex()
            .w_full()
            .flex_wrap()
            .gap(design::ui_px(cx, 6.0))
            .children(item.tags.iter().map(|tag| {
                div()
                    .flex_none()
                    .text_size(caption)
                    .text_color(rgb(p.text_muted))
                    .child(format!("#{tag}"))
            }))
    });

    v_flex()
        .w_full()
        .flex_none()
        .gap(design::ui_px(cx, 6.0))
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 9.0))
        .border_b_1()
        .border_color(rgb(p.border))
        .child(meta)
        .when_some(tickers, |this, t| this.child(t))
        .when_some(body, |this, b| this.child(b))
        .when_some(tags, |this, t| this.child(t))
        .into_any_element()
}

/// Format the publication age as a compact localized relative time.
///
/// Buckets: under a minute, minutes, hours, then days. An absent/zero time renders empty because it
/// carries no age. `now_ms` is the terminal clock at render time.
pub(super) fn rel_time(time_ms: i64, now_ms: i64) -> String {
    if time_ms <= 0 {
        return String::new();
    }
    // saturating_sub: `time_ms` is service-controlled and could be extreme; avoid any overflow.
    let secs = now_ms.saturating_sub(time_ms).max(0) / 1000;
    if secs < 60 {
        t!("news.time.now").to_string()
    } else if secs < 3600 {
        t!("news.time.min", n = secs / 60).to_string()
    } else if secs < 86_400 {
        t!("news.time.hour", n = secs / 3600).to_string()
    } else {
        t!("news.time.day", n = secs / 86_400).to_string()
    }
}
