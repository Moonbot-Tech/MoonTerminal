use super::AccountIdentity;

/// Breakage: `account.rs::AccountIdentity::from_auth` keeping only the first present field — the
/// per-brand pick in `venue::merge_key` would lose the field its brand keys on (the address on
/// Hyperliquid, whose core also fills the Binance uid field).
#[test]
fn every_stated_field_is_kept() {
    assert_eq!(
        AccountIdentity::from_auth("acct-a", "0xabc", 42),
        Some(AccountIdentity {
            binance_uid: Some(42),
            account_id: Some("acct-a".to_string()),
            address: Some("0xabc".to_string()),
        })
    );
}

/// Breakage: `from_auth` dropping the trim, or reading a zero uid as stated — the same account
/// reported with stray whitespace would split into two keys and be summed twice.
#[test]
fn strings_are_trimmed_and_a_zero_uid_is_absent() {
    assert_eq!(
        AccountIdentity::from_auth("  acct-a \t", "   ", 0),
        Some(AccountIdentity {
            binance_uid: None,
            account_id: Some("acct-a".to_string()),
            address: None,
        })
    );
}

/// Breakage: `account.rs::normalized` dropping the hex lower-casing — two cores quoting one
/// wallet as `0xABC` and `0xabc` would be counted twice in the Assets total.
#[test]
fn two_spellings_of_one_hex_address_are_one_account() {
    assert_eq!(
        AccountIdentity::from_auth("", "0xABC", 0),
        AccountIdentity::from_auth("", "0xabc", 0)
    );
    assert_ne!(
        AccountIdentity::from_auth("Acct-A", "", 0),
        AccountIdentity::from_auth("acct-a", "", 0),
        "a non-hex id keeps its case"
    );
}

/// Breakage: `from_auth` returning an identity for an all-empty answer — every core that states
/// no account would share it and fold into one row.
#[test]
fn a_core_stating_nothing_has_no_account() {
    assert_eq!(AccountIdentity::from_auth("", "", 0), None);
    assert_eq!(AccountIdentity::from_auth(" ", "\t", 0), None);
}
