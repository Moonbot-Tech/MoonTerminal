//! News card rendering: one card per logical news item, its expandable latency chain, and the small
//! time/chip helpers.
//!
//! Collapsed, a card shows source and inline ticker chips (left), a right-aligned relative age, and
//! an expand chevron; the body (translated or original) and tag chips follow. Expanding reveals the
//! delivery latency chain (terminal receipt → service send → service receive → publication).
//! Coloured tags become filled badges and add a left rail split by colour. Cards are hairline-split.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBadge, MoonBadgeSize, MoonBadgeVariant, MoonButton, MoonButtonSize, MoonButtonVariant,
    MoonPalette, h_flex, v_flex,
};
use rust_i18n::t;

use super::{NewsView, tag_color};
use crate::design;
use moon_core::config::{Language, NewsTagSettings};
use moon_core::feed::NewsItem;

/// Build a soft tinted badge (source / ticker / coloured tag) using the house `MoonBadge`, sized to
/// the card's caption tier. `color` (a `MoonPalette` token) tints both the fill and the text.
///
/// Also used by the chart's news-mark hover card, so both surfaces label a source identically.
pub(crate) fn badge(text: impl Into<SharedString>, color: u32) -> impl IntoElement {
    MoonBadge::new(text)
        .variant(MoonBadgeVariant::Soft)
        .size(MoonBadgeSize::Tiny)
        .bg_color(color)
        .text_color(color)
        .mono(true)
        .render()
}

/// Format Unix ms as `HH:MM:SS.mmm` UTC time-of-day, by manual arithmetic like the header clock.
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

/// Body text for the card. `translate` on shows the Russian translation (English fallback until it
/// arrives); off shows the news as delivered (English/original). The fallback rule itself lives on
/// [`NewsItem::body`], shared with the chart's news-mark card.
fn body_text(item: &NewsItem, translate: bool) -> &str {
    item.body(if translate {
        Language::Ru
    } else {
        Language::En
    })
}

/// Whether a translation is being waited on: requested but the Russian text is still absent, so the
/// English original is shown meanwhile.
fn pending(item: &NewsItem, translate: bool) -> bool {
    translate && item.ru.is_empty()
}

/// Build one news card for `item`. `translate` selects translated vs original body; `expanded`
/// controls the latency chain (toggled by the chevron via `cx`); tags colour from the user's LOCAL
/// assignment (`colors`), not the wire.
pub(super) fn news_card(
    item: &NewsItem,
    translate: bool,
    colors: &NewsTagSettings,
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
        .filter_map(|t| tag_color(t, colors, p))
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

    // --- meta row: source · inline tickers · spacer · relative time · pending · expand chevron ---
    let mut meta = h_flex()
        .w_full()
        .items_center()
        .flex_wrap()
        .gap(design::ui_px(cx, 5.0));
    if !item.source.is_empty() {
        meta = meta.child(badge(item.source.to_uppercase(), p.text_muted));
    }
    // Tickers sit on the source line, right after the source. Clicking one opens the coin on Main
    // (or a cores picker when several cores trade it) via NewsView::open_coin.
    for coin in &item.coins {
        let coin_c = coin.clone();
        meta = meta.child(
            div()
                .id(SharedString::from(format!("news-coin-{}-{coin}", item.id)))
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(
                        move |this: &mut NewsView, ev: &MouseDownEvent, window, cx| {
                            this.open_coin(&coin_c, ev.position, window, cx);
                            cx.stop_propagation();
                        },
                    ),
                )
                .child(badge(coin.clone(), p.blue)),
        );
    }
    let id = item.id.clone();
    let chevron = MoonButton::new(SharedString::from(format!("news-exp-{}", item.id)))
        .label(if expanded { "⌃" } else { "⌄" })
        .size(MoonButtonSize::Micro)
        .variant(MoonButtonVariant::Ghost)
        .on_click(cx.listener(move |this: &mut NewsView, _, _w, cx| this.toggle_expand(&id, cx)))
        .render();
    meta = meta.child(div().flex_1());
    // "Translation pending" sits just LEFT of the time while awaiting the RU text, so from the right
    // edge the order reads: time, then the pending badge.
    if pending(item, translate) {
        meta = meta.child(badge(t!("news.pending").to_string(), p.amber));
    }
    meta = meta
        .child(
            div()
                .flex_none()
                .text_size(caption)
                .text_color(rgb(p.text_muted))
                .child(rel_time(item.time_ms, now_ms)),
        )
        .child(chevron);

    let latency = expanded.then(|| latency_block(item, p, cx));

    // --- body in the selected translation mode (English fallback) ---
    let text = body_text(item, translate);
    let body = (!text.is_empty()).then(|| {
        div()
            .w_full()
            .text_size(design::t_body(cx))
            .text_color(rgb(p.text))
            .child(text.to_string())
    });

    // --- tag chips: coloured ones become a standout badge, neutral ones stay muted "#tag" text ---
    let tags = (!item.tags.is_empty()).then(|| {
        h_flex()
            .w_full()
            .flex_wrap()
            .gap(design::ui_px(cx, 6.0))
            .children(item.tags.iter().map(|tag| {
                let label = format!("#{tag}");
                match tag_color(tag, colors, p) {
                    Some(c) => badge(label, c).into_any_element(),
                    None => div()
                        .flex_none()
                        .text_size(caption)
                        .text_color(rgb(p.text_muted))
                        .child(label)
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
        .children(row(
            t!("news.lat.send").to_string(),
            item.send_time_ms,
            false,
        ))
        .children(row(
            t!("news.lat.recv").to_string(),
            item.recv_time_ms,
            false,
        ))
        .children(row(
            t!("news.lat.pub").to_string(),
            (item.time_ms > 0).then_some(item.time_ms),
            false,
        ))
}

/// Format the publication age as a compact localized relative time, starting at SECONDS.
///
/// Buckets: seconds, minutes, hours, then days. An absent/zero time renders empty.
pub(super) fn rel_time(time_ms: i64, now_ms: i64) -> String {
    if time_ms <= 0 {
        return String::new();
    }
    // saturating_sub: `time_ms` is service-controlled and could be extreme; avoid any overflow.
    let secs = now_ms.saturating_sub(time_ms).max(0) / 1000;
    if secs < 60 {
        t!("news.time.sec", n = secs).to_string()
    } else if secs < 3600 {
        t!("news.time.min", n = secs / 60).to_string()
    } else if secs < 86_400 {
        t!("news.time.hour", n = secs / 3600).to_string()
    } else {
        t!("news.time.day", n = secs / 86_400).to_string()
    }
}
