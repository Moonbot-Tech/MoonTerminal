//! MoonBot message exports use explicit strategy and folder boundaries and literal field values.
//!
//! The values are the TEXT OF MOONBOT'S PARAMETER GRID, not the wire values: booleans come as
//! `YES`/`NO`, decimals with their display precision (`0.5000`), and large numbers with the grid's
//! magnitude suffix — `30k`, `1000M`, `1E11k`. The core's parser refuses the suffixed forms, so
//! they are expanded here, where the text is still known to be MoonBot's.

use super::{ClipItem, STRATEGY_NAME_FIELD};
use moon_core::feed::SchemaKind;

#[cfg(test)]
mod tests;

/// Parse a complete MoonBot export atomically; malformed or unsupported strategies reject the batch.
pub(super) fn parse(text: &str, kinds: &[SchemaKind]) -> Option<Vec<ClipItem>> {
    let text = text.trim().trim_start_matches('\u{feff}');
    let text = if text.starts_with("```") {
        let (_, body) = text.split_once('\n')?;
        body.trim_end().strip_suffix("```")?
    } else {
        text
    };
    let mut folders = Vec::new();
    let mut fields: Option<Vec<(String, String)>> = None;
    let mut items = Vec::new();
    for line in text.lines() {
        let marker = line.trim();
        if marker.is_empty() {
            continue;
        }
        if marker == "##Begin_Strategy" {
            if fields.is_some() {
                return None;
            }
            fields = Some(Vec::new());
        } else if marker == "##End_Strategy" {
            let fields = fields.take()?;
            let value = |name: &str| {
                fields
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value.as_str())
            };
            let name = value(STRATEGY_NAME_FIELD)?;
            if name.trim().is_empty() {
                return None;
            }
            let signal = value("SignalType")?.trim();
            let kind = kinds
                .iter()
                .find(|kind| kind.name.eq_ignore_ascii_case(signal))
                .or_else(|| {
                    // MoonBot's text export names its Drops kind DropsDetection.
                    kinds.iter().find(|kind| {
                        signal.eq_ignore_ascii_case("DropsDetection")
                            && kind.name.eq_ignore_ascii_case("Drops")
                    })
                })?;
            items.push(ClipItem {
                kind_ordinal: kind.ordinal,
                kind: kind.name.clone(),
                name: name.to_string(),
                rel_path: folders.clone(),
                fields: expand_grid_magnitudes(fields, kind),
                src: None,
            });
        } else if let Some(name) = marker.strip_prefix("#Begin_Folder ") {
            if fields.is_some() || name.trim().is_empty() {
                return None;
            }
            folders.push(name.trim().to_string());
        } else if marker == "#End_Folder" {
            if fields.is_some() {
                return None;
            }
            folders.pop()?;
        } else {
            let fields = fields.as_mut()?;
            let (key, value) = line.split_once('=')?;
            let key = key.trim();
            if key.is_empty()
                || fields
                    .iter()
                    .any(|(seen, _)| seen.eq_ignore_ascii_case(key))
            {
                return None;
            }
            // Unlike MoonTerminal's format, backslashes and equals signs are literal here.
            fields.push((key.to_string(), value.to_string()));
        }
    }
    (fields.is_none() && folders.is_empty() && !items.is_empty()).then_some(items)
}

/// Expands the grid magnitude suffix in every NUMERIC field of `kind`; a string field keeps a
/// value like `10k` verbatim, and so does a field the destination schema does not know, since
/// only the schema says what the text means.
fn expand_grid_magnitudes(
    fields: Vec<(String, String)>,
    kind: &SchemaKind,
) -> Vec<(String, String)> {
    fields
        .into_iter()
        .map(|(key, value)| {
            let numeric = kind
                .sections
                .iter()
                .flat_map(|section| &section.fields)
                .find(|field| field.name.eq_ignore_ascii_case(&key))
                .is_some_and(|field| field.type_name != "String");
            let value = if numeric {
                expand_magnitude(&value).unwrap_or(value)
            } else {
                value
            };
            (key, value)
        })
        .collect()
}

/// Expands MoonBot's grid magnitude suffix — `k` for thousands, `M` for millions, either case as
/// MoonBot's own reader takes them (issue #606) — into the plain
/// number the core's field parser accepts: `30k` → `30000`, `20.00k` → `20000`, `1E11k` →
/// `100000000000000`. `None` when the text is not a number with such a suffix, so the caller keeps
/// it verbatim and the field parser stays the one place that decides what is refused.
///
/// Done as decimal text, not through a float: a shifted decimal point is exact for every mantissa
/// the grid prints, where `0.1 * 1000` already is not.
fn expand_magnitude(raw: &str) -> Option<String> {
    let text = raw.trim();
    let (mantissa, shift) = match text.as_bytes().last()? {
        b'k' | b'K' => (&text[..text.len() - 1], 3i32),
        b'M' | b'm' => (&text[..text.len() - 1], 6i32),
        _ => return None,
    };
    let (negative, unsigned) = match mantissa.as_bytes().first()? {
        b'-' => (true, &mantissa[1..]),
        b'+' => (false, &mantissa[1..]),
        _ => (false, mantissa),
    };
    // Delphi prints the exponent as `1E11`; the decimal separator of a grid value can be either.
    let (digits_part, exponent) = match unsigned.split_once(['E', 'e']) {
        // A range test, not `abs()`: `i32::MIN.abs()` wraps silently in this workspace
        // (`debug-assertions = false`) and would pass an exponent that then sizes a 2 GB string.
        Some((d, e)) => (d, e.parse::<i32>().ok().filter(|e| (-30..=30).contains(e))?),
        None => (unsigned, 0),
    };
    let (int_part, frac_part) = digits_part
        .split_once(['.', ','])
        .unwrap_or((digits_part, ""));
    if int_part.is_empty() && frac_part.is_empty()
        || !int_part.bytes().all(|c| c.is_ascii_digit())
        || !frac_part.bytes().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let digits = format!("{int_part}{frac_part}");
    let point = i32::try_from(int_part.len()).ok()? + exponent.checked_add(shift)?;
    let len = i32::try_from(digits.len()).ok()?;
    let mut out = if point >= len {
        format!("{digits}{}", "0".repeat((point - len) as usize))
    } else if point <= 0 {
        format!("0.{}{digits}", "0".repeat((-point) as usize))
    } else {
        let (head, tail) = digits.split_at(point as usize);
        format!("{head}.{tail}")
    };
    if out.contains('.') {
        out = out.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    let trimmed = out.trim_start_matches('0');
    let body = if trimmed.is_empty() || trimmed.starts_with('.') {
        format!("0{trimmed}")
    } else {
        trimmed.to_string()
    };
    Some(if negative && body != "0" {
        format!("-{body}")
    } else {
        body
    })
}
