//! Coin-list tokens shared by the strategy field reveal and the shell blacklist strips.
//!
//! Splitting follows [`moon_core::symbol::coin_list`]: comma-separated text, trimmed tokens,
//! ASCII-case-insensitive match. A pair such as `XALPHAUSDT` is a different token from `XALPHA`.
//! The strip windows around the matching token so a clipped row still shows it.

use std::ops::Range;
use std::time::Instant;

use gpui::{
    AnyElement, App, FontWeight, InteractiveElement, IntoElement, ParentElement, Pixels, Styled,
    div,
};
use moon_ui::{MoonPalette, h_flex};

use crate::design;

/// Split `text` into trimmed token spans, marking the one that names `coin`.
///
/// Empty pieces (a trailing comma, a doubled comma) are not tokens. The ranges are byte offsets
/// of the trimmed token inside `text`, so the surrounding spaces stay outside the span.
///
/// Args:
///     text: Comma-separated coin list, in the core's own spelling.
///     coin: Token to mark. Compared with [`str::eq_ignore_ascii_case`] after trimming.
///
/// Returns:
///     One entry per non-empty token, in list order. The bool is true only for that token.
pub(crate) fn split_coin_token(text: &str, coin: &str) -> Vec<(Range<usize>, bool)> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    for piece in text.split(',') {
        let lead = piece.len() - piece.trim_start().len();
        let trimmed = piece.trim();
        if !trimmed.is_empty() {
            let start = offset + lead;
            out.push((
                start..start + trimmed.len(),
                trimmed.eq_ignore_ascii_case(coin),
            ));
        }
        offset += piece.len() + 1;
    }
    out
}

/// How long a blacklist or field outline takes to fade out.
pub(crate) const FLASH_MS: f32 = 1_200.0;

/// Outline strength for a flash that started at `started`.
///
/// Args:
///     started: Instant the reveal was applied.
///
/// Returns:
///     Zero once [`FLASH_MS`] has elapsed, otherwise the remaining fraction.
pub(crate) fn flash_alpha(started: Instant) -> f32 {
    let elapsed = started.elapsed().as_secs_f32() * 1000.0;
    if elapsed >= FLASH_MS {
        0.0
    } else {
        1.0 - elapsed / FLASH_MS
    }
}

/// One piece of a token strip that keeps the matching coin on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CoinTokenPiece<'a> {
    /// Leading tokens that would have pushed the hit past the budget.
    Ellipsis,
    /// A trimmed token. `hit` is true only for the coin the strip was asked to show.
    Token { text: &'a str, hit: bool },
}

/// Window `tokens` so the hit stays inside `budget_chars`.
///
/// `None` draws every token. A budget that already reaches the hit keeps the prefix and lets
/// the row clip the tail. A budget that does not reach the hit drops the leading tokens and
/// puts an ellipsis in their place, keeping the hit and as many tokens just before it as fit.
///
/// Args:
///     tokens: Trimmed tokens in list order. At most one is the hit.
///     budget_chars: Character budget, counting token characters, commas, and a leading ellipsis.
///
/// Returns:
///     Pieces in draw order. The hit is present whenever `tokens` contained one.
pub(crate) fn window_coin_tokens<'a>(
    tokens: &[(&'a str, bool)],
    budget_chars: Option<usize>,
) -> Vec<CoinTokenPiece<'a>> {
    let Some(budget) = budget_chars else {
        return tokens
            .iter()
            .map(|(text, hit)| CoinTokenPiece::Token { text, hit: *hit })
            .collect();
    };
    let Some(hit_at) = tokens.iter().position(|(_, hit)| *hit) else {
        return tokens
            .iter()
            .map(|(text, hit)| CoinTokenPiece::Token { text, hit: *hit })
            .collect();
    };
    let mut start = 0usize;
    if piece_cost(tokens, 0, hit_at, false) > budget {
        start = hit_at;
        while start > 0 && piece_cost(tokens, start - 1, hit_at, true) <= budget {
            start -= 1;
        }
    }
    let mut pieces = Vec::new();
    if start > 0 {
        pieces.push(CoinTokenPiece::Ellipsis);
    }
    for (text, hit) in &tokens[start..] {
        pieces.push(CoinTokenPiece::Token { text, hit: *hit });
    }
    pieces
}

/// Characters used to draw `tokens[start..=end_inclusive]`, plus a leading ellipsis when asked.
fn piece_cost(
    tokens: &[(&str, bool)],
    start: usize,
    end_inclusive: usize,
    ellipsis: bool,
) -> usize {
    let mut count = 0usize;
    if ellipsis {
        count += 1;
        if start <= end_inclusive {
            count += 1;
        }
    }
    for (index, (text, _)) in tokens[start..=end_inclusive].iter().enumerate() {
        if index > 0 {
            count += 1;
        }
        count += text.chars().count();
    }
    count
}

/// How many mono glyphs fit in `width` at `size`.
///
/// A mono glyph is taken as six tenths of the font size. The budget is floored so the hit is
/// not planned into a width the row will clip.
fn char_budget(width: Pixels, size: Pixels) -> usize {
    let glyph = f32::from(size) * 0.6;
    if glyph <= 0.0 {
        return 1;
    }
    (f32::from(width) / glyph).floor().max(1.0) as usize
}

/// Mono token strip for a comma-separated coin list, with the matching token in amber.
///
/// Commas are drawn between pieces. When `max_width` is set, tokens before the hit that would
/// not fit are replaced by an ellipsis so the hit stays in the row.
///
/// Args:
///     text: Comma-separated list, in the core's own spelling.
///     coin: Token painted amber.
///     size: Font size of every piece.
///     max_width: Row cap. `None` draws every token.
///     cx: Application context for the palette.
///
/// Returns:
///     The strip element. The hit token carries the `coin-bl-token` selector.
pub(crate) fn coin_token_strip(
    text: &str,
    coin: &str,
    size: Pixels,
    max_width: Option<Pixels>,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let spans = split_coin_token(text, coin);
    let tokens: Vec<(&str, bool)> = spans
        .iter()
        .map(|(range, hit)| (&text[range.clone()], *hit))
        .collect();
    let budget = max_width.map(|width| char_budget(width, size));
    let pieces = window_coin_tokens(&tokens, budget);
    let mut row = h_flex()
        .flex_none()
        .items_center()
        .min_w_0()
        .overflow_hidden()
        .font_family(design::mono());
    if let Some(width) = max_width {
        row = row.max_w(width);
    }
    let mut first = true;
    for piece in pieces {
        if !first {
            row = row.child(
                div()
                    .flex_none()
                    .text_size(size)
                    .text_color(design::moon(p.text_muted))
                    .child(","),
            );
        }
        first = false;
        match piece {
            CoinTokenPiece::Ellipsis => {
                row = row.child(
                    div()
                        .flex_none()
                        .text_size(size)
                        .text_color(design::moon(p.text_muted))
                        .child("\u{2026}"),
                );
            }
            CoinTokenPiece::Token { text, hit } => {
                let token_el = div()
                    .flex_none()
                    .text_size(size)
                    .font_weight(if hit {
                        FontWeight::BOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(design::moon(if hit { p.amber } else { p.text }));
                row = row.child(if hit {
                    token_el
                        .id("coin-bl-token")
                        .debug_selector(|| "coin-bl-token".into())
                        .child(text.to_string())
                        .into_any_element()
                } else {
                    token_el.child(text.to_string()).into_any_element()
                });
            }
        }
    }
    row.into_any_element()
}

#[cfg(test)]
mod tests;
