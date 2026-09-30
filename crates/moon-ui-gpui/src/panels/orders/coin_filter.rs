//! The Orders toolbar's coin filter: which rows a typed coin query keeps.

/// Whether an order row whose Token column reads `coin` survives the typed `query`.
///
/// A case-insensitive SUBSTRING of the token, so `sol` keeps `SOL` and `1000SOL` alike. The query
/// is trimmed here rather than at the field, so a stray space never empties the table. Byte-wise
/// ASCII folding, the Alerts filter's rule: tokens are ASCII, and a non-ASCII query is compared as
/// typed rather than mapped into bytes a token could never hold.
///
/// Args:
///     coin: The row's token, exactly as the Token column renders it (`OrderRow::coin`).
///     query: The field's text; empty keeps every row.
///
/// Returns:
///     `true` when the row stays in the table.
pub(super) fn matches_coin(coin: &str, query: &str) -> bool {
    let pat = query.trim().as_bytes();
    if pat.is_empty() {
        return true;
    }
    coin.as_bytes()
        .windows(pat.len())
        .any(|w| w.eq_ignore_ascii_case(pat))
}

#[cfg(test)]
mod tests;
