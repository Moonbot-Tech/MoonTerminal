//! Click target of one chart strategy-filter line.
//!
//! The core sends the line as plain text. A line this cannot parse stays inert: no hit rect, no
//! navigation. Blacklist suffixes keep their coin targets; other strategy-shaped lines open the
//! strategy, revealing a field only when the reason identifies it reliably.

/// Where a parsed filter line should open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FilterLineTarget {
    /// `match global black list` — the pane core's own blacklist.
    GlobalBlacklist { coin: String },
    /// `match strategy black list` — that strategy's `CoinsBlackList` field.
    StrategyBlacklist { strategy: String, coin: String },
    /// Any other strategy refusal, optionally naming an addressable schema field.
    Strategy {
        strategy: String,
        field: Option<&'static str>,
    },
}

/// Parse one filter caption into a navigation target.
///
/// The suffix `match global black list` or `match strategy black list` is matched
/// ASCII-case-insensitively at the end of the trimmed line, as its own word. The coin is the
/// last token of the text after ` ) : ` when that prefix is present, otherwise the last token
/// of the whole head. One leading `Market` token is skipped. The token is kept as the core
/// spelled it. A strategy line also needs a `<name> ( <kind> ) : ` prefix; the name is the text
/// before the first ` ( `, and the kind must close at ` ) : `. Other complete strategy prefixes
/// navigate without a coin. Text without a complete prefix or a blacklist suffix stays inert.
///
/// Args:
///     line: One filter caption, as the core spelled it.
///
/// Returns:
///     The target, or `None` when the line must stay inert.
pub(crate) fn parse_filter_line(line: &str) -> Option<FilterLineTarget> {
    if let Some(head) = strip_suffix_ci(line, "match strategy black list") {
        let strategy = strategy_name(head)?.to_string();
        let coin = coin_token(head)?;
        return Some(FilterLineTarget::StrategyBlacklist { strategy, coin });
    }
    if let Some(head) = strip_suffix_ci(line, "match global black list") {
        let coin = coin_token(head)?;
        return Some(FilterLineTarget::GlobalBlacklist { coin });
    }
    let strategy = strategy_name(line)?.to_string();
    Some(FilterLineTarget::Strategy {
        strategy,
        field: refusal_field(coin_region(line).trim()),
    })
}

/// Map only known refusal reasons to fields present in the Strategies parameter registry.
/// Unknown delta names and ambiguous bounds open the strategy without guessing a field.
///
/// Args:
///     reason: Text after the strategy prefix, without surrounding whitespace.
///
/// Returns:
///     A known schema key, or `None` when the reason does not identify one reliably.
fn refusal_field(reason: &str) -> Option<&'static str> {
    if reason.ends_with("doesnt match strategy white list") {
        return Some("CoinsWhiteList");
    }
    if reason.starts_with("EMA filter not passed.") {
        return Some("CustomEMA");
    }
    if reason.starts_with("Market ") && reason.ends_with("must have \"TradFi\" tag") {
        return Some("BinanceTokenTags");
    }
    let (metric, range) = reason.split_once(" doesnt match strategy range (")?;
    let (name, value) = metric.split_once(" delta (")?;
    let value = value
        .strip_suffix(')')?
        .trim_end_matches('%')
        .parse::<f64>()
        .ok()?;
    let (min, max) = range.strip_suffix(')')?.split_once(" .. ")?;
    let min = min.trim_end_matches('%').parse::<f64>().ok()?;
    let max = max.trim_end_matches('%').parse::<f64>().ok()?;
    if !value.is_finite() || !min.is_finite() || !max.is_finite() {
        return None;
    }
    let (min_key, max_key) = match name {
        "3h" => ("Delta_3h_Min", "Delta_3h_Max"),
        "24h" => ("Delta_24h_Min", "Delta_24h_Max"),
        "Vol Minute" => ("MinuteVolDeltaMin", "MinuteVolDeltaMax"),
        _ => return None,
    };
    // Do not infer a failed maximum from a zero upper bound, as in the volume refusal caption.
    match (value < min, max != 0.0 && value > max) {
        (true, false) => Some(min_key),
        (false, true) => Some(max_key),
        _ => None,
    }
}

/// `head` without a case-insensitive trailing `suffix`, when `suffix` is a separate word.
fn strip_suffix_ci<'a>(line: &'a str, suffix: &str) -> Option<&'a str> {
    let line = line.trim();
    let start = line.len().checked_sub(suffix.len())?;
    let tail = line.get(start..)?;
    if !tail.eq_ignore_ascii_case(suffix) {
        return None;
    }
    let head = &line[..start];
    if !head.is_empty() && !head.ends_with(|ch: char| ch.is_whitespace()) {
        return None;
    }
    let head = head.trim();
    if head.is_empty() { None } else { Some(head) }
}

/// Strategy name from `<name> ( <kind> ) : `, or `None` when that prefix is not there.
/// Both the strategy name and the strategy kind must contain non-whitespace text.
fn strategy_name(head: &str) -> Option<&str> {
    let open = head.find(" ( ")?;
    let after_open = open + " ( ".len();
    let close = head[after_open..].find(" ) : ")?;
    if head[after_open..after_open + close].trim().is_empty() {
        return None;
    }
    let name = head[..open].trim();
    if name.is_empty() { None } else { Some(name) }
}

/// Text the coin is read from.
///
/// A strategy-shaped prefix, including the optional one on a global line, ends at ` ) : `.
/// The coin is never taken from the strategy name in front of that marker. A head without the
/// marker is its own coin text, which is how `Market 2Z match global black list` parses.
fn coin_region(head: &str) -> &str {
    let Some(open) = head.find(" ( ") else {
        return head;
    };
    let after_open = open + " ( ".len();
    match head[after_open..].find(" ) : ") {
        Some(close) => &head[after_open + close + " ) : ".len()..],
        None => head,
    }
}

/// Last coin token in `head`, in the core's own spelling.
///
/// One leading `Market` word is skipped, and only that one. The token that remains must contain
/// an alphanumeric character, so `Market match global black list` and a punctuation-only tail
/// stay inert instead of navigating to an empty coin.
///
/// Args:
///     head: Caption with the blacklist suffix already removed.
///
/// Returns:
///     The coin as written, or `None` when the tail names none.
fn coin_token(head: &str) -> Option<String> {
    let mut tokens: Vec<&str> = coin_region(head).split_whitespace().collect();
    if tokens
        .first()
        .is_some_and(|token| token.eq_ignore_ascii_case("Market"))
    {
        tokens.remove(0);
    }
    let token = tokens.pop()?;
    if token.chars().any(|ch| ch.is_alphanumeric()) {
        Some(token.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests;
