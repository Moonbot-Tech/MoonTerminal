//! Presentation-only classification; the backend's original log remains unchanged for copying.

/// Log emphasis without changing or dropping the original message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Error,
    Warn,
    Plain,
}

/// Split an optional ISO date/time or clock prefix and classify common log severity markers.
pub(super) fn classify(line: &str) -> (Option<&str>, Kind, &str) {
    let end = line.find(char::is_whitespace).unwrap_or(line.len());
    let first = &line[..end];
    let is_clock = |s: &str| {
        s.len() >= 8
            && s.as_bytes()[2] == b':'
            && s.as_bytes()[5] == b':'
            && s.as_bytes()[..8]
                .iter()
                .enumerate()
                .all(|(i, b)| i == 2 || i == 5 || b.is_ascii_digit())
    };
    let timestamp_end = if is_clock(first) {
        Some(end)
    } else if first.len() >= 19
        && first.as_bytes()[4] == b'-'
        && first.as_bytes()[7] == b'-'
        && first.as_bytes()[10] == b'T'
        && is_clock(&first[11..])
    {
        Some(end)
    } else if first.len() == 10 && first.as_bytes()[4] == b'-' && first.as_bytes()[7] == b'-' {
        let rest = line[end..].trim_start();
        let clock = rest.split_whitespace().next().unwrap_or_default();
        is_clock(clock).then_some(line.len() - rest.len() + clock.len())
    } else {
        None
    };
    let (stamp, message) = timestamp_end.map_or((None, line), |end| {
        (Some(&line[..end]), line[end..].trim_start())
    });
    let lower = message.to_ascii_lowercase();
    let kind = if lower.contains("login stopped") {
        Kind::Warn
    } else if lower.contains("error")
        || lower.contains("failed")
        || lower.contains("out of memory")
        || lower.contains("fatal")
    {
        Kind::Error
    } else if lower.contains("warn") || lower.contains("login stopped") {
        Kind::Warn
    } else {
        Kind::Plain
    };
    (stamp, kind, message)
}

#[cfg(test)]
mod tests;
