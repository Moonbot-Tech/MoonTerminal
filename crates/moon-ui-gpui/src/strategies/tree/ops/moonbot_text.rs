//! MoonBot message exports use explicit strategy and folder boundaries and literal field values.

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
                fields,
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
