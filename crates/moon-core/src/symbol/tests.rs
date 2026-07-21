use super::coin_of_market;

#[test]
fn coin_plain() {
    assert_eq!(coin_of_market("ADAUSDT"), "ADA");
    assert_eq!(coin_of_market("BTCUSDT"), "BTC");
    assert_eq!(coin_of_market("VANRY_USDT"), "VANRY");
}

#[test]
fn coin_hip3_strips_dex() {
    assert_eq!(coin_of_market("xyz:BIRD"), "BIRD");
    assert_eq!(coin_of_market("xyz:BIRDUSDC"), "BIRD");
}
