use super::{blacklist_add, blacklist_contains};

#[test]
fn add_to_empty() {
    assert_eq!(blacklist_add("", "ADA"), "ADA");
    assert_eq!(blacklist_add("   ", "ADA"), "ADA");
}

#[test]
fn add_appends() {
    assert_eq!(blacklist_add("BTC,ETH", "ADA"), "BTC,ETH,ADA");
    assert_eq!(blacklist_add("BTC,ETH,", "ADA"), "BTC,ETH,ADA");
}

#[test]
fn dedup_case_insensitive() {
    assert_eq!(blacklist_add("BTC,ada", "ADA"), "BTC,ada");
    assert!(blacklist_contains("BTC, ada , ETH", "ADA"));
    assert!(!blacklist_contains("BTC,ETH", "ADA"));
}
