//! Text truncation and fitting to a width budget.

/// Truncate `text` with an ellipsis to the available prefix budget, returning the result and width.
///
/// Text is arbitrary Unicode and equal glyph width is not guaranteed outside Geist Mono, so the
/// prefix is accumulated character by character against the real budget rather than by counting
/// characters. Returns `text` unchanged when it already fits. A budget narrower than the ellipsis
/// returns the ellipsis alone even though no non-empty marker can fit that budget.
///
/// The width comes back because every caller needs it and measuring costs an uncached glyph layout
/// PER CHARACTER (see [`ui_text_width`]) — a caller that re-measured the result would pay for the
/// same string twice, and these run per frame.
///
/// `measure` is passed in rather than taken from the theme: the caller draws its text at its own
/// size and weight, and truncating against a narrower font underestimates the width and overflows
/// anyway. Pure, so it is unit-testable without an `App`.
pub fn fit_text(text: &str, max_w: f32, measure: impl Fn(&str) -> f32) -> (String, f32) {
    const ELLIPSIS: &str = "\u{2026}";
    let full = measure(text);
    if full <= max_w {
        return (text.to_string(), full);
    }
    let budget = max_w - measure(ELLIPSIS);
    let mut head = String::new();
    let mut used = 0.0f32;
    let mut buf = [0u8; 4];
    for ch in text.chars() {
        let w = measure(ch.encode_utf8(&mut buf));
        if used + w > budget {
            break;
        }
        used += w;
        head.push(ch);
    }
    let out = format!("{}{ELLIPSIS}", head.trim_end());
    let width = measure(&out);
    (out, width)
}

/// Whether `c` belongs to a CJK script, which writes no spaces between words.
///
/// Han ideographs, kana, and the CJK and fullwidth punctuation blocks. A line may break beside
/// any of them: GPUI's wrapper breaks before each (it treats them as non-word characters), which
/// [`unbreakable_units`] mirrors, and [`wrap_text`] breaks on either side of one. Without this a
/// Chinese sentence is one unbreakable "word".
pub fn is_cjk(c: char) -> bool {
    matches!(
        c,
        '\u{2E80}'..='\u{303F}'
            | '\u{3040}'..='\u{30FF}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{FF00}'..='\u{FFEF}'
    )
}

/// The pieces of `text` GPUI's wrapper never breaks inside.
///
/// Text between spaces is one piece, except that a piece also ends before every CJK character:
/// that is where the wrapper puts a break when there is no space to take. A label's width floor is
/// the widest of these, so a Chinese sentence can still wrap down to one character per line while
/// a Latin word stays whole.
pub fn unbreakable_units(text: &str) -> impl Iterator<Item = &str> {
    text.split_whitespace().flat_map(|word| {
        let mut rest = word;
        std::iter::from_fn(move || {
            if rest.is_empty() {
                return None;
            }
            let cut = rest
                .char_indices()
                .skip(1)
                .find(|(_, c)| is_cjk(*c))
                .map_or(rest.len(), |(at, _)| at);
            let (unit, tail) = rest.split_at(cut);
            rest = tail;
            Some(unit)
        })
    })
}

/// Closing punctuation a line must not begin with.
const NO_LINE_START: &str = "，。、；：？！）》」』】”’…%";
/// Opening punctuation a line must not end with.
const NO_LINE_END: &str = "（《「『【“‘";

/// Whether a line may end after `prev` and the next begin with `next` with no space between them.
///
/// True beside a CJK character, unless that would strand punctuation: a comma at the start of a
/// line or an opening bracket at the end of one reads as a typo.
fn cjk_break(prev: char, next: char) -> bool {
    (is_cjk(prev) || is_cjk(next)) && !NO_LINE_START.contains(next) && !NO_LINE_END.contains(prev)
}

/// Byte index of the last place `text` may break at or before `limit`, if any.
///
/// A space strictly before `limit` (the break replaces it), or the boundary beside a CJK
/// character at or before `limit`. Index 0 is never a break: it would leave an empty line.
fn last_break(text: &str, limit: usize) -> Option<usize> {
    let mut found = None;
    let mut prev: Option<char> = None;
    for (at, c) in text.char_indices() {
        if at > limit {
            break;
        }
        if let Some(prev) = prev
            && ((c == ' ' && at < limit) || cjk_break(prev, c))
        {
            found = Some(at);
        }
        prev = Some(c);
    }
    found
}

/// Break text into at most `max_lines` lines of `max_w`, cutting the last one if it still spills.
///
/// For the one thing the chart prints that is a SENTENCE rather than a figure: the core's own
/// detect line, which states half a dozen numbers and does not fit the plot's width. Cutting it —
/// what every caption does — throws away most of what it said; wrapping it keeps the rest and puts
/// the ellipsis at the end of the block instead of the end of the first line.
///
/// Lines break on SPACES, and beside a CJK character, which is written without them — except
/// where that would put closing punctuation first on a line or opening punctuation last. A word
/// longer than the whole line has nowhere to break, so it is cut by [`fit_text`] and ends the
/// block: continuing would either loop or split a number in half.
///
/// The split point is arithmetic rather than a search: `measure` is called once for the whole
/// remainder, the advance per character comes from that, and the candidate is walked back to its
/// last break. Measuring candidate after candidate would shape the same sentence a dozen times per
/// frame. The advance is an AVERAGE, so on a face that is not monospaced — or on text whose glyphs
/// are not one code point each — the candidate can come out over budget; it is therefore measured
/// and walked back break by break until it fits, which costs nothing on the monospaced face the
/// chart actually draws and stays correct on any other.
///
/// `measure` is passed in for the same reason [`fit_text`] takes it, and this is pure for the same
/// reason: it is unit-testable without an `App`.
pub fn wrap_text(
    text: &str,
    max_w: f32,
    max_lines: usize,
    measure: impl Fn(&str) -> f32,
) -> Vec<(String, f32)> {
    let mut out: Vec<(String, f32)> = Vec::new();
    let mut rest = text.trim();
    if max_lines == 0 {
        return out;
    }
    while !rest.is_empty() && out.len() < max_lines {
        let full = measure(rest);
        if full <= max_w {
            out.push((rest.to_string(), full));
            return out;
        }
        // The last line takes what fits and says, with an ellipsis, that there was more.
        if out.len() + 1 == max_lines {
            let (line, w) = fit_text(rest, max_w, &measure);
            if !line.is_empty() {
                out.push((line, w));
            }
            return out;
        }
        let count = rest.chars().count().max(1) as f32;
        let advance = full / count;
        let fits = match advance > 0.0 {
            true => (max_w / advance).floor().max(0.0) as usize,
            false => 0,
        };
        // Byte index just past the last character that fits, then back off to the last break
        // before it: a space, or the boundary beside a CJK character, which needs none.
        let end = rest
            .char_indices()
            .nth(fits)
            .map_or(rest.len(), |(at, _)| at);
        // `rest` is trimmed on every path, so a space at index 0 cannot happen; a line with no
        // break at all is the unbreakable-word case below.
        let Some(cut) = last_break(rest, end) else {
            let (line, w) = fit_text(rest, max_w, &measure);
            if !line.is_empty() {
                out.push((line, w));
            }
            return out;
        };
        // The candidate, walked back a word at a time until it MEASURES within the budget.
        let mut cut = cut;
        let (line, width) = loop {
            let line = rest[..cut].trim_end();
            let w = measure(line);
            if w <= max_w {
                break (line, w);
            }
            // Strictly before the line's last character, so every pass shortens the line.
            let last = line.char_indices().next_back().map_or(0, |(at, _)| at);
            match last_break(line, last) {
                Some(at) => cut = at,
                // One word, and it does not fit: cut it and end the block rather than split a
                // figure across two lines, where each half reads as a number of its own.
                _ => {
                    let (line, w) = fit_text(rest, max_w, &measure);
                    if !line.is_empty() {
                        out.push((line, w));
                    }
                    return out;
                }
            }
        };
        out.push((line.to_string(), width));
        rest = rest[cut..].trim_start();
    }
    out
}
