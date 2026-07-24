//! News card rendering: one card per logical news item, its expandable latency chain, and the small
//! time/chip helpers.
//!
//! Collapsed, a card shows source (left), a right-aligned time (exact ms clock when fresher than a
//! minute, else relative), and an expand chevron; tickers, body (selected language, English
//! fallback), and tag chips follow. Expanding reveals the delivery latency chain (terminal receipt →
//! service send → service receive → publication). Coloured tags become filled badges and add a left
//! rail split by colour. Cards are separated by a hairline.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBadge, MoonBadgeSize, MoonBadgeVariant, MoonButton, MoonButtonSize, MoonButtonVariant,
    MoonPalette, h_flex, v_flex,
};
use rust_i18n::t;

use super::{NewsLang, NewsView, key_color};
use crate::design;
use moon_core::config::NewsTagColors;
use moon_core::feed::NewsItem;

/// Fresh-age threshold: below this the collapsed time shows the exact ms clock, above it shows a
/// relative age.
const FRESH_MS: i64 = 60_000;

/// Build a soft tinted badge (source / ticker / coloured tag) using the house `MoonBadge`, sized to
/// the card's caption tier. `color` (a `MoonPalette` token) tints both the fill and the text.
fn badge(text: impl Into<SharedString>, color: u32) -> impl IntoElement {
    MoonBadge::new(text)
        .variant(MoonBadgeVariant::Soft)
        .size(MoonBadgeSize::Tiny)
        .bg_color(color)
        .text_color(color)
        .mono(true)
        .render()
}

/// Format Unix ms as `HH:MM:SS.mmm` UTC time-of-day, by manual arithmetic like the header clock (no
/// timezone/date library needed for a within-day clock).
pub(super) fn hms_ms(ms: i64) -> String {
    let day = ms.rem_euclid(86_400_000);
    let (h, m, s, milli) = (
        day / 3_600_000,
        (day % 3_600_000) / 60_000,
        (day % 60_000) / 1000,
        day % 1000,
    );
    format!("{h:02}:{m:02}:{s:02}.{milli:03}")
}

/// Build one news card for `item` in the selected `lang`, colouring tags via `colors`. `expanded`
/// controls the latency chain; the chevron toggles it via `cx`.
pub(super) fn news_card(
    item: &NewsItem,
    lang: NewsLang,
    colors: &NewsTagColors,
    now_ms: i64,
    expanded: bool,
    p: MoonPalette,
    cx: &mut Context<NewsView>,
) -> AnyElement {
    let caption = design::t_caption(cx);

    // Left rail: one segment per COLOURED tag (neutral tags take no segment), in tag order.
    let rail_colors: Vec<u32> = item
        .tags
        .iter()
        .filter_map(|t| colors.color(t).and_then(|k| key_color(k, p)))
        .collect();
    let rail = (!rail_colors.is_empty()).then(|| {
        div()
            .absolute()
            .left(px(0.0))
            .top(px(0.0))
            .bottom(px(0.0))
            .w(px(3.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .children(rail_colors.iter().map(|&c| div().flex_1().bg(rgb(c))))
    });

    // --- meta row: source badge · pending badge · spacer · time · expand chevron ---
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
    // Prefer the terminal-receive stamp as the age anchor; fall back to publication time.
    let anchor = item.recv_terminal_ms.filter(|&t| t > 0).unwrap_or(item.time_ms);
    let when_str = if anchor > 0 && now_ms.saturating_sub(anchor) < FRESH_MS {
        hms_ms(anchor)
    } else {
        rel_time(anchor, now_ms)
    };
    let id = item.id.clone();
    let chevron = MoonButton::new(SharedString::from(format!("news-exp-{}", item.id)))
        .label(if expanded { "⌃" } else { "⌄" })
        .size(MoonButtonSize::Micro)
        .variant(MoonButtonVariant::Ghost)
        .on_click(cx.listener(move |this: &mut NewsView, _, _w, cx| this.toggle_expand(&id, cx)))
        .render();
    meta = meta
        .child(div().flex_1())
        .child(
            div()
                .flex_none()
                .text_size(caption)
                .text_color(rgb(p.text_muted))
                .child(when_str),
        )
        .child(chevron);

    let latency = expanded.then(|| latency_block(item, p, cx));

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

    // --- tag chips: coloured ones become a standout badge, neutral ones stay muted "#tag" text ---
    let tags = (!item.tags.is_empty()).then(|| {
        h_flex()
            .w_full()
            .flex_wrap()
            .gap(design::ui_px(cx, 6.0))
            .children(item.tags.iter().map(|tag| {
                match colors.color(tag).and_then(|k| key_color(k, p)) {
                    Some(c) => badge(tag.clone(), c).into_any_element(),
                    None => div()
                        .flex_none()
                        .text_size(caption)
                        .text_color(rgb(p.text_muted))
                        .child(format!("#{tag}"))
                        .into_any_element(),
                }
            }))
    });

    v_flex()
        .relative()
        .w_full()
        .flex_none()
        .gap(design::ui_px(cx, 6.0))
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 9.0))
        .border_b_1()
        .border_color(rgb(p.border))
        .when_some(rail, |this, r| this.child(r))
        .child(meta)
        .when_some(latency, |this, l| this.child(l))
        .when_some(tickers, |this, t| this.child(t))
        .when_some(body, |this, b| this.child(b))
        .when_some(tags, |this, t| this.child(t))
        .into_any_element()
}

/// Build the delivery-latency chain: terminal receipt (anchor) then service send / receive /
/// publication, each as a signed millisecond delta from the anchor plus its absolute clock. Rows for
/// absent timestamps are skipped.
fn latency_block(item: &NewsItem, p: MoonPalette, cx: &App) -> impl IntoElement {
    let anchor = item.recv_terminal_ms.filter(|&t| t > 0);
    let row = |label: String, ms: Option<i64>, is_anchor: bool| -> Option<Div> {
        let ms = ms.filter(|&t| t > 0)?;
        let val = if is_anchor {
            hms_ms(ms)
        } else if let Some(a) = anchor {
            let d = a - ms;
            let sign = if d >= 0 { "−" } else { "+" };
            format!("{sign}{} {} · {}", d.abs(), t!("news.lat.unit"), hms_ms(ms))
        } else {
            hms_ms(ms)
        };
        Some(
            h_flex()
                .w_full()
                .justify_between()
                .gap(design::ui_px(cx, 10.0))
                .child(div().text_color(rgb(p.text_muted)).child(label))
                .child(div().text_color(rgb(p.text_soft)).child(val)),
        )
    };
    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 2.0))
        .p(design::ui_px(cx, 6.0))
        .rounded(design::r_button(cx))
        .bg(rgb(p.gutter))
        .border_1()
        .border_color(rgb(p.border))
        .text_size(design::t_caption(cx))
        .children(row(t!("news.lat.terminal").to_string(), anchor, true))
        .children(row(t!("news.lat.send").to_string(), item.send_time_ms, false))
        .children(row(t!("news.lat.recv").to_string(), item.recv_time_ms, false))
        .children(row(
            t!("news.lat.pub").to_string(),
            (item.time_ms > 0).then_some(item.time_ms),
            false,
        ))
}

/// Format the age as a compact localized relative time.
///
/// Buckets: under a minute, minutes, hours, then days. An absent/zero anchor renders empty.
/// `now_ms` is the terminal clock at render time.
pub(super) fn rel_time(anchor_ms: i64, now_ms: i64) -> String {
    if anchor_ms <= 0 {
        return String::new();
    }
    // saturating_sub: the anchor is service/clock-derived and could be extreme; avoid any overflow.
    let secs = now_ms.saturating_sub(anchor_ms).max(0) / 1000;
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
