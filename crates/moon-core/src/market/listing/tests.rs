use super::*;

// Platform codes as the venue directory and the arbitrage stream number them.
const BYBIT_F: u8 = 2;
const BINANCE_S: u8 = 3;
const BINANCE_F: u8 = 4;
const BINANCE_Q: u8 = 6;
const BYBIT_S: u8 = 7;
const GATE_S: u8 = 8;
const HL_S: u8 = 12;
const HL_F: u8 = 13;
/// The first Hyperliquid deployer's arbitrage code.
const HL_DEPLOYER_1: u8 = 51;

fn set(codes: &[u8]) -> VenueSet {
    let mut out = VenueSet::default();
    for code in codes {
        out.insert(*code);
    }
    out
}

fn venue_of(code: u8) -> Option<crate::venue::Venue> {
    crate::venue::venue(code)
}

fn row<'a>(code: u8, market: &'a str, identity: &'a str) -> RowFacts<'a> {
    RowFacts {
        venue: venue_of(code),
        exchange: ExchangeId { code, dex: 0 },
        market,
        identity,
        futures_flag: false,
    }
}

/// A book where the stream quotes the given codes somewhere, with nothing else in it.
fn book_seeing(codes: &[u8]) -> ListingBook {
    ListingBook {
        seen: set(codes),
        ..ListingBook::default()
    }
}

fn put_market(book: &mut ListingBook, code: u8, market: &str, quoted: &[u8], asked: &[u8]) {
    book.by_market
        .entry(ExchangeId { code, dex: 0 })
        .or_default()
        .insert(
            Box::from(market),
            Evidence {
                quoted: set(quoted),
                asked: set(asked),
            },
        );
}

/// Give an exchange a donor of its own: quotes on enough of its markets to be the stream.
fn give_donor(book: &mut ListingBook, code: u8, quoted: &[u8], asked: &[u8]) {
    for index in 0..MIN_CARRIED_MARKETS {
        put_market(book, code, &format!("FILLER{index}USDT"), quoted, asked);
    }
}

/// Where an exchange has a donor, a market that donor has no price for cannot be `Only`.
///
/// Breakage: letting the coin's reading across other donors stand in. An identity can glue two
/// coins — `1000CAT` and `CAT` are both `CAT` on Binance — and a market the server maps to
/// nothing on its own exchange (a Bybit USDC perpetual) would borrow a negative from a
/// namesake. The coin's reading still proves a listing: a positive is safe from anywhere.
#[test]
fn a_market_unknown_on_its_own_donor_does_not_borrow_a_negative() {
    let mut book = book_seeing(&[BYBIT_S, BINANCE_F]);
    // Bybit futures has a donor, which quotes other markets but not this one.
    give_donor(&mut book, BYBIT_F, &[BYBIT_S], &[BYBIT_S]);
    book.by_identity.insert(
        "CAT".to_string(),
        Evidence {
            quoted: set(&[BINANCE_F]),
            asked: set(&[BYBIT_S, BINANCE_F]),
        },
    );
    assert_eq!(
        book.sign(&row(BYBIT_F, "1000CATPERP", "CAT")),
        Listing::Unknown
    );
    // The same coin reading, now holding a Bybit spot price: that is a listing, and it counts.
    book.by_identity
        .get_mut("CAT")
        .expect("inserted above")
        .quoted
        .insert(BYBIT_S);
    assert_eq!(
        book.sign(&row(BYBIT_F, "1000CATPERP", "CAT")),
        Listing::Both
    );
}

/// One stray price does not make an exchange "an exchange with a donor".
///
/// Breakage: counting any exchange with an entry as one. A single price landing on one market of
/// a core that is not really receiving the stream would then mark every OTHER market of that
/// exchange as unknown to the stream, and switch off the coin's answer for all of them.
#[test]
fn a_stray_quote_does_not_silence_the_coins_reading() {
    let mut book = book_seeing(&[BINANCE_F, BYBIT_F]);
    put_market(&mut book, BINANCE_S, "STRAYUSDT", &[BYBIT_F], &[]);
    book.by_identity.insert(
        "RAY".to_string(),
        Evidence {
            quoted: set(&[BYBIT_F]),
            asked: set(&[BINANCE_F, BYBIT_F]),
        },
    );
    assert_eq!(
        book.sign(&row(BINANCE_S, "RAYUSDC", "RAY")),
        Listing::SpotOnly
    );
}

/// A market its own donor knows may be answered by the coin when that donor cannot answer.
///
/// Breakage: taking the market's own reading as the only witness. A donor whose mask lacks the
/// other kind's code, or one still warming up, would leave every market it quotes unknown for
/// good, although other donors hold the same coin and have asked.
#[test]
fn a_known_market_is_answered_by_the_coin_when_its_donor_did_not_ask() {
    let mut book = book_seeing(&[BINANCE_S, BYBIT_F, GATE_S]);
    give_donor(&mut book, BINANCE_F, &[BYBIT_F], &[BYBIT_F]);
    put_market(&mut book, BINANCE_F, "LITUSDT", &[BYBIT_F], &[BYBIT_F]);
    assert_eq!(
        book.sign(&row(BINANCE_F, "LITUSDT", "LIT")),
        Listing::Unknown,
        "nobody has asked about Binance spot yet"
    );
    book.by_identity.insert(
        "LIT".to_string(),
        Evidence {
            quoted: set(&[BYBIT_F, GATE_S]),
            asked: set(&[BINANCE_S, BYBIT_F, GATE_S]),
        },
    );
    assert_eq!(
        book.sign(&row(BINANCE_F, "LITUSDT", "LIT")),
        Listing::FuturesOnly
    );
}

/// The codes are bytes and the set has to hold every one of them, the far words included.
#[test]
fn a_venue_set_holds_any_byte_and_only_what_was_put_in() {
    let codes = set(&[0, 63, 64, 127, 128, 200, 255]);
    for code in [0u8, 63, 64, 127, 128, 200, 255] {
        assert!(codes.contains(code), "{code}");
    }
    for code in [1u8, 62, 65, 129, 254] {
        assert!(!codes.contains(code), "{code}");
    }
    assert!(set(&[64]).is_subset(&codes));
    assert!(!set(&[64, 65]).is_subset(&codes));
    assert!(VenueSet::default().is_subset(&codes));
}

/// Spot's other kind is every derivatives venue of the brand; a derivative's is its spot.
#[test]
fn the_other_side_is_the_same_brand_and_the_other_kind() {
    assert_eq!(
        other_side(Brand::Binance, true),
        set(&[BINANCE_F, BINANCE_Q])
    );
    assert_eq!(other_side(Brand::Binance, false), set(&[BINANCE_S]));
    assert_eq!(other_side(Brand::Bybit, false), set(&[BYBIT_S]));
    // A HIP-3 deployer's market is a Hyperliquid perpetual, so it counts for a spot row — and no
    // deployer is anybody's spot.
    let hl_spot_other = other_side(Brand::Hyperliquid, true);
    assert!(hl_spot_other.contains(HL_F) && hl_spot_other.contains(HL_DEPLOYER_1));
    assert_eq!(other_side(Brand::Hyperliquid, false), set(&[HL_S]));
}

/// A price from the brand's other kind on the row's own market is the whole proof.
#[test]
fn a_quote_from_the_other_kind_means_both() {
    let mut book = book_seeing(&[BINANCE_S, BYBIT_F]);
    put_market(
        &mut book,
        BINANCE_F,
        "BTCUSDT",
        &[BINANCE_S, BYBIT_F],
        &[BINANCE_S, BYBIT_F],
    );
    assert_eq!(book.sign(&row(BINANCE_F, "BTCUSDT", "BTC")), Listing::Both);
}

/// `Only` needs a coin the stream knows, a donor that asked, and a kind the stream carries.
#[test]
fn only_takes_a_known_coin_an_asking_donor_and_a_carried_venue() {
    let mut book = book_seeing(&[BINANCE_S, BYBIT_F]);
    put_market(
        &mut book,
        BINANCE_F,
        "LITUSDT",
        &[BYBIT_F],
        &[BINANCE_S, BYBIT_F],
    );
    assert_eq!(
        book.sign(&row(BINANCE_F, "LITUSDT", "LIT")),
        Listing::FuturesOnly
    );
    put_market(&mut book, GATE_S, "LIT_USDT", &[BYBIT_F], &[9, BYBIT_F]);
    book.seen.insert(9);
    assert_eq!(
        book.sign(&row(GATE_S, "LIT_USDT", "LIT")),
        Listing::SpotOnly
    );
}

/// A market nobody quotes is unknown, not "listed nowhere else".
///
/// Breakage: this is the Bybit USDC perpetual and the Hyperliquid `kPEPE` — the server maps them
/// to no other venue although their coin has a spot market, and reading the empty slot set as a
/// negative printed `Futures Only` on about a hundred coins that are on both.
#[test]
fn a_market_with_no_quote_at_all_is_unknown() {
    let mut book = book_seeing(&[BYBIT_S, BINANCE_F]);
    put_market(
        &mut book,
        BYBIT_F,
        "1000BONKPERP",
        &[],
        &[BYBIT_S, BINANCE_F],
    );
    assert_eq!(
        book.sign(&row(BYBIT_F, "1000BONKPERP", "BONKPERP")),
        Listing::Unknown
    );
}

/// A venue the stream never quotes cannot be missing from a market.
///
/// Breakage: Hyperliquid spot was measured at zero prices on every donor; without this every
/// Hyperliquid perpetual, `BTC` included, read `Futures Only`.
#[test]
fn silence_about_a_venue_the_stream_never_carries_is_unknown() {
    let mut book = book_seeing(&[BYBIT_F, BINANCE_F]);
    put_market(
        &mut book,
        HL_F,
        "BTC",
        &[BYBIT_F, BINANCE_F],
        &[HL_S, BYBIT_F, BINANCE_F],
    );
    assert_eq!(book.sign(&row(HL_F, "BTC", "BTC")), Listing::Unknown);
}

/// A donor that never asked about the other kind proves nothing by not hearing from it.
///
/// The same reading a donor inside its warm-up gives: its `asked` is empty until then.
#[test]
fn a_donor_that_did_not_ask_cannot_say_only() {
    let mut book = book_seeing(&[BINANCE_S, BYBIT_F]);
    put_market(&mut book, BINANCE_F, "LITUSDT", &[BYBIT_F], &[BYBIT_F]);
    assert_eq!(
        book.sign(&row(BINANCE_F, "LITUSDT", "LIT")),
        Listing::Unknown
    );
    put_market(&mut book, BINANCE_F, "LITUSDT", &[BYBIT_F], &[]);
    assert_eq!(
        book.sign(&row(BINANCE_F, "LITUSDT", "LIT")),
        Listing::Unknown
    );
}

/// An exchange with no donor of its own reads what other donors hold for the same coin.
#[test]
fn a_row_without_its_own_donor_is_answered_by_the_coin() {
    let mut book = book_seeing(&[BINANCE_F, BYBIT_F]);
    book.by_identity.insert(
        "BTC".to_string(),
        Evidence {
            quoted: set(&[BINANCE_F, BYBIT_F]),
            asked: set(&[BINANCE_F, BYBIT_F]),
        },
    );
    book.by_identity.insert(
        "RAY".to_string(),
        Evidence {
            quoted: set(&[BYBIT_F]),
            asked: set(&[BINANCE_F, BYBIT_F]),
        },
    );
    assert_eq!(book.sign(&row(BINANCE_S, "BTCUSDC", "BTC")), Listing::Both);
    assert_eq!(
        book.sign(&row(BINANCE_S, "RAYUSDC", "RAY")),
        Listing::SpotOnly
    );
    // A dated contract keeps its expiry in the identity on Binance USD-M; it is still BTC.
    assert_eq!(
        book.sign(&row(BINANCE_S, "BTCUSDC", "btc_0925")),
        Listing::Both
    );
}

/// A connected core of the other kind proves a listing, and its catalog never proves an absence.
///
/// Breakage: the same coin is `LUNA2` on Binance futures and `LUNA` on Binance spot, so reading
/// a missing identity as "not there" called both of them one-sided.
#[test]
fn a_catalog_can_say_both_and_never_only() {
    let mut book = book_seeing(&[]);
    book.catalogs
        .entry((Brand::Hyperliquid, true))
        .or_default()
        .insert("BTC".to_string());
    assert_eq!(book.sign(&row(HL_F, "BTC", "BTC")), Listing::Both);
    assert_eq!(book.sign(&row(HL_F, "kPEPE", "KPEPE")), Listing::Unknown);
    // The catalog is the OTHER kind's: a spot coin does not prove itself.
    assert_eq!(book.sign(&row(HL_S, "@142", "BTC")), Listing::Unknown);
}

/// The core's own flag counts where it was measured to mean a cross-listing, and nowhere else.
///
/// Breakage: on the perpetual-only venues every market carries its settlement currency in that
/// field, so trusting it everywhere printed `Both` on an exchange with no spot market at all.
#[test]
fn the_futures_flag_counts_only_where_it_means_a_listing() {
    let book = book_seeing(&[]);
    let mut flagged = row(BINANCE_F, "BTCUSDT", "BTC");
    flagged.futures_flag = true;
    assert_eq!(book.sign(&flagged), Listing::Both);
    let mut gate_spot = row(GATE_S, "BTC_USDT", "BTC");
    gate_spot.futures_flag = true;
    assert_eq!(book.sign(&gate_spot), Listing::Unknown);
    let mut hl = row(HL_F, "BTC", "BTC");
    hl.futures_flag = true;
    assert_eq!(book.sign(&hl), Listing::Unknown);
}

/// An exchange this build cannot name has no other kind to ask about.
#[test]
fn an_unnamed_exchange_is_unknown_whatever_the_book_holds() {
    let mut book = book_seeing(&[BINANCE_S, BINANCE_F]);
    book.by_identity.insert(
        "BTC".to_string(),
        Evidence {
            quoted: set(&[BINANCE_S, BINANCE_F]),
            asked: set(&[BINANCE_S, BINANCE_F]),
        },
    );
    let mut unnamed = row(221, "BTCUSDT", "BTC");
    unnamed.futures_flag = true;
    assert_eq!(unnamed.venue, None);
    assert_eq!(book.sign(&unnamed), Listing::Unknown);
}
