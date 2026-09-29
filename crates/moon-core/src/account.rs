//! Which exchange account a core trades on, as the core itself states it in `AuthCheck`.
//!
//! Several cores often run on one exchange account; the Assets total must count that account's
//! money once. This module only collects what the core stated — which field names the account on which brand,
//! and what merges, is `venue::merge_key`.

/// Identity of the exchange account a core is logged into: the one field `venue::merge_key` chose
/// for the core's brand.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AccountKey {
    /// The account id the core reported.
    Id(String),
    /// The wallet address the core reported.
    Address(String),
    /// The Binance account uid the core reported.
    BinanceUid(i64),
}

/// Every account identity a core stated in its `AuthCheck` answer, normalized.
///
/// Which of these names the account depends on the brand — on Hyperliquid the Binance uid field
/// is a checksum of the address, on Binance only the uid is the exchange's own account number —
/// so all three are kept and `venue::merge_key` picks per brand. A brand-blind "first present
/// field wins" pick would merge cores on a field that means something else on that brand.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct AccountIdentity {
    /// The response's `binance_account_id`, `None` when zero (absent).
    pub binance_uid: Option<i64>,
    /// The response's `account_id`, trimmed; `None` when empty.
    pub account_id: Option<String>,
    /// The response's `btc_address`, trimmed and `0x`-hex lower-cased; `None` when empty.
    pub address: Option<String>,
}

impl AccountIdentity {
    /// Normalize the identity fields of an `AuthCheck` answer.
    ///
    /// Strings are trimmed, and a `0x`-prefixed hex one is lower-cased so two spellings of one
    /// address compare equal; any other id keeps its case. A zero uid means "not stated".
    ///
    /// Args:
    ///     account_id: The response's `account_id`, empty when absent.
    ///     btc_address: The response's `btc_address`, empty when absent.
    ///     binance_account_id: The response's `binance_account_id`, zero when absent.
    ///
    /// Returns:
    ///     The identity, or `None` when the core stated none of the three.
    pub fn from_auth(account_id: &str, btc_address: &str, binance_account_id: i64) -> Option<Self> {
        let identity = Self {
            binance_uid: (binance_account_id != 0).then_some(binance_account_id),
            account_id: normalized(account_id),
            address: normalized(btc_address),
        };
        let stated = identity.binance_uid.is_some()
            || identity.account_id.is_some()
            || identity.address.is_some();
        stated.then_some(identity)
    }
}

/// Trim `raw` and lower-case it when it is `0x`-prefixed hex; `None` when nothing is left.
fn normalized(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let is_prefixed_hex = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_hexdigit()));
    Some(match is_prefixed_hex {
        true => trimmed.to_ascii_lowercase(),
        false => trimmed.to_owned(),
    })
}

#[cfg(test)]
mod tests;
