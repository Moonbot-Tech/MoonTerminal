use super::*;

use hmac::{Hmac, Mac};
use sha2::Sha256;

const BOT_TOKEN: &str = "123456:authoring-fixture-token";
const NOW: u64 = 1_700_000_100;

/// Signs a plain, already percent-safe query fixture using Telegram's documented two-stage HMAC.
fn signed_init_data(fields: &[(&str, &str)]) -> String {
    let mut sorted = fields.to_vec();
    sorted.sort_unstable_by_key(|(key, _)| *key);
    let data_check_string = sorted
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n");

    let mut token_key =
        Hmac::<Sha256>::new_from_slice(b"WebAppData").expect("HMAC accepts a fixed-length key");
    token_key.update(BOT_TOKEN.as_bytes());
    let secret_key = token_key.finalize().into_bytes();

    let mut signature = Hmac::<Sha256>::new_from_slice(&secret_key)
        .expect("a SHA-256 HMAC digest is a valid HMAC key");
    signature.update(data_check_string.as_bytes());
    let hash = signature
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();

    fields
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .chain(std::iter::once(format!("hash={hash}")))
        .collect::<Vec<_>>()
        .join("&")
}

/// Returns the same signed input with a syntactically valid but different hash.
fn with_forged_hash(init_data: &str) -> String {
    let (fields, hash) = init_data
        .rsplit_once("&hash=")
        .expect("the fixture signer always appends hash last");
    let replacement = if hash.ends_with('0') { '1' } else { '0' };
    format!("{fields}&hash={}{}", &hash[..hash.len() - 1], replacement)
}

/// `telegram/init_data.rs:verify_init_data` must reject forged or stale HMAC data and duplicate
/// signed fields; otherwise an attacker can pass the Mini App session check as a Telegram user.
#[test]
fn only_a_fresh_uniquely_signed_telegram_payload_is_accepted() {
    let fresh = signed_init_data(&[
        ("auth_date", "1700000050"),
        ("query_id", "AAEAAQ"),
        ("signature", "optional-telegram-signature"),
        ("user", "{\"id\":41,\"first_name\":\"Ada\"}"),
    ]);
    assert!(
        verify_init_data(&fresh, BOT_TOKEN, NOW).is_ok(),
        "the independently signed fresh fixture is the protocol's positive control"
    );

    assert!(
        verify_init_data(&with_forged_hash(&fresh), BOT_TOKEN, NOW).is_err(),
        "changing the received hash must not authenticate the request"
    );

    let (fields, hash) = fresh
        .rsplit_once("&hash=")
        .expect("the fixture signer always appends hash last");
    let duplicate_auth_date = format!("{fields}&auth_date=1700000050&hash={hash}");
    assert!(
        verify_init_data(&duplicate_auth_date, BOT_TOKEN, NOW).is_err(),
        "a repeated signed key is ambiguous and must be rejected before it can authenticate"
    );

    let stale = signed_init_data(&[
        ("auth_date", "1699990000"),
        ("query_id", "AAEAAQ"),
        ("user", "{\"id\":41,\"first_name\":\"Ada\"}"),
    ]);
    assert!(
        verify_init_data(&stale, BOT_TOKEN, NOW).is_err(),
        "a correctly signed but old launch must not become a reusable Mini App credential"
    );
}
