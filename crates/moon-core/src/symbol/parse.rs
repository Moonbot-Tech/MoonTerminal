//! The per-exchange market-name rules. One parser, one place, dispatched by [`Exchange`].
//!
//! Every rule here was derived from the real market catalogs of the connected cores, not from
//! documentation: 6328 distinct names across ten exchange connections. The shapes that exist are
//!
//! | exchange | shape |
//! |---|---|
//! | Binance spot/USD-M | `BTCUSDT`, dated `BTCUSDT_260925` |
//! | Binance COIN-M | `BTCUSD_PERP`, dated `BNBUSD_260925` |
//! | Bybit | `BTCUSDT`, USDC perp `BTCPERP`, dated `BTCUSDT-07AUG26` |
//! | Gate | `BTC_USDT`, `ETH_BTC` |
//! | BitGet | `BTCUSDT` |
//! | Hyperliquid | perp `BTC`, HIP-3 `xyz:BIRD`, spot index `@206`, spot pair `PURR/USDC` |
//! | OKX | `BTC-USDT`, perp `BTC-USDT-SWAP`, dated `BTC-USDT-250808` |
//!
//! IMPORTANT — what this parser deliberately does NOT do. The core's own coin token is not
//! derivable from the market name, as the live catalogs show: Bybit's `1000BONKPERP` is
//! `market_currency=1kBONKPERP`, COIN-M's `AAVEUSD_PERP` is `AAVE_RP` and `BNBUSD_260925` is
//! `BNB_0925`. Those foldings live in the catalog, which is why the CATALOG is the authority and
//! this parser is the fallback for names with no catalog entry — a log line, a delisted market, a
//! report row read offline. A `PERP` tail is therefore KEPT as part of the coin: the core keeps
//! it too, and stripping it would make a blacklist written from the terminal stop matching.
//!
//! The cores also spell their MoonBot-classic name quote-FIRST (`USDT-BTC` for OKX's
//! `BTC-USDT-SWAP`, `USDC-AAVEPERP`, `USD-AAVE_RP`), which is why [`market_in_text`] has to read
//! that order at all — it appears in core logs.

use super::exchange::Exchange;

/// Known quote currencies for concatenated names, longest first so `FDUSD`/`USDC` match before
/// `USD`. This ordering is load-bearing and must not be sorted alphabetically.
const QUOTES: [&str; 9] = [
    "FDUSD", "TUSD", "USDC", "BUSD", "USDT", "USD", "BTC", "ETH", "BNB",
];

/// Quote currencies a dated or perpetual contract can be denominated in.
///
/// Deliberately narrower than [`QUOTES`]: `BNBUSD_260925` is BNB quoted in USD, and the general
/// table would match its `BUSD` first and report the coin as `BN`. No exchange here lists a dated
/// contract against BUSD, BTC, ETH or BNB, so excluding them removes the ambiguity outright
/// instead of ordering around it.
const CONTRACT_QUOTES: [&str; 3] = ["USDT", "USDC", "USD"];

/// A market name taken apart into the pieces the UI and the core's coin lists care about.
///
/// Every field borrows from the input, so splitting a name allocates nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MarketParts<'a> {
    /// Hyperliquid HIP-3 DEX prefix (`xyz` in `xyz:BIRD`), `None` on a regular exchange.
    pub dex: Option<&'a str>,
    /// The coin, as the name spells it: `BTC` in every shape in the table above.
    ///
    /// For a Hyperliquid spot index (`@206`) the name carries no coin, so this is the index
    /// itself; resolve those through the catalog's `market_name_mb_classic` instead.
    pub base: &'a str,
    /// Quote currency exactly as the name spells it, or empty when the name does not carry one
    /// (a Hyperliquid perp, an unrecognized shape). Case follows the input.
    pub quote: &'a str,
    /// Contract tail for a dated or otherwise non-default contract: `07AUG26`, `260925`,
    /// `250228-50000-C`. A perpetual is the default contract on a futures connection and yields
    /// `None`, so `BTC-USDT-SWAP` and `BTCUSDT` part identically.
    pub contract: Option<&'a str>,
}

impl MarketParts<'_> {
    /// Whether the name is an opaque index rather than a coin (Hyperliquid spot `@206`).
    ///
    /// Named here so callers stop spelling the rule as `starts_with('@')`: which names carry no
    /// coin is naming knowledge, and it belongs with the rest of it.
    pub fn is_index(&self) -> bool {
        self.base.starts_with('@')
    }

    /// Whether this is a dated contract rather than the venue's default one.
    ///
    /// A report row or a coin list names a coin, not an expiry, so anything choosing a market for
    /// a coin wants the undated one.
    pub fn is_dated(&self) -> bool {
        self.contract.is_some()
    }
}

/// Splits a market name using `ex`'s rules.
///
/// `Exchange::Unknown` recognizes the shape instead, which is what a log line or an offline report
/// row gets. Shape recognition only ever fires on tails that cannot occur inside a coin name — a
/// literal `SWAP` after two dashes, a `DDMONYY` or `YYMMDD` date — so a coin genuinely called
/// `EZSWAP` or `SWAP` still parses as itself.
pub fn split_market(market: &str, ex: Exchange) -> MarketParts<'_> {
    // A colon occurs only in Hyperliquid HIP-3 names, where it separates the DEX from the coin.
    let (dex, rest) = match market.rsplit_once(':') {
        Some((dex, rest)) => (Some(dex), rest),
        None => (None, market),
    };
    // Hyperliquid spot spells a pair with a slash on every exchange family, and its index names
    // carry no coin at all. Both are decided before the family rules.
    if let Some((base, quote)) = rest.split_once('/') {
        return MarketParts {
            dex,
            base,
            quote,
            ..MarketParts::default()
        };
    }
    if rest.starts_with('@') {
        return MarketParts {
            dex,
            base: rest,
            ..MarketParts::default()
        };
    }
    let (base, quote, contract) = match ex {
        Exchange::Okx => dashed(rest),
        Exchange::Gate => {
            let (base, quote) = underscore_quote(rest);
            (base, quote, None)
        }
        Exchange::BinanceCoinM => dated_concat(strip_underscore_tail(rest, true), rest),
        Exchange::Binance | Exchange::BitGet | Exchange::Huobi => {
            dated_concat(strip_underscore_tail(rest, false), rest)
        }
        Exchange::Bybit => dated_concat(strip_dash_date(rest), rest),
        // A Hyperliquid perp name IS the coin; nothing to strip.
        Exchange::Hyperliquid => (rest, "", None),
        Exchange::Unknown => by_shape(rest),
    };
    MarketParts {
        dex,
        base,
        quote,
        contract,
    }
}

/// Reads `stripped` as base + quote, falling back to the whole name when no tail was removed.
///
/// A name that HAD a contract tail is read against [`CONTRACT_QUOTES`]: `BNBUSD_260925` is BNB
/// quoted in USD, but the general table would match `BUSD` first and hand back `BN`.
fn dated_concat<'a>(
    stripped: Option<(&'a str, Option<&'a str>)>,
    rest: &'a str,
) -> (&'a str, &'a str, Option<&'a str>) {
    let Some((head, contract)) = stripped else {
        let (base, quote) = concat(rest);
        return (base, quote, None);
    };
    let (base, quote) = concat_in(head, &CONTRACT_QUOTES);
    (base, quote, contract)
}

/// Recognizes the shape when the exchange is unknown.
///
/// The order matters: the dashed OKX form and the dated tails are checked first because they are
/// unambiguous, and everything that matches none of them falls through to the concatenated reading
/// that every remaining exchange shares.
fn by_shape(rest: &str) -> (&str, &str, Option<&str>) {
    // `BASE-QUOTE-SWAP` / `BASE-QUOTE-250808` / `BASE-QUOTE`: OKX and nothing else. Each helper
    // rejects a name without its separator on its own, so no `contains` guard is needed.
    let dashed_parts = dashed(rest);
    let dashed_ok = !dashed_parts.0.is_empty()
        && !dashed_parts.1.is_empty()
        && (is_known_quote(dashed_parts.1) || dashed_parts.2.is_some());
    if dashed_ok {
        return dashed_parts;
    }
    // `BTCUSDT-07AUG26`: Bybit's dated contracts.
    // `BTCUSD_PERP` / `BTCUSDT_260925`: Binance COIN-M and USD-M dated contracts.
    if let Some((head, contract)) =
        strip_dash_date(rest).or_else(|| strip_underscore_tail(rest, true))
    {
        let (base, quote) = concat_in(head, &CONTRACT_QUOTES);
        return (base, quote, contract);
    }
    // `BTC_USDT`, `BTCUSDT`, `BTCPERP`, `BTC`: one concatenated reading covers them all, because
    // `concat` also trims the separator a Gate-style name leaves behind.
    let (base, quote) = concat(rest);
    (base, quote, None)
}

/// `BASE-QUOTE`, `BASE-QUOTE-SWAP`, `BASE-QUOTE-250808`, `BASE-QUOTE-250228-50000-C`.
///
/// A perpetual reports no contract: it is the default on a futures connection, so `BTC-USDT-SWAP`
/// must part exactly like `BTCUSDT`. Everything after the second dash is kept verbatim as one
/// slice, which keeps an option chain lossless without allocating.
fn dashed(rest: &str) -> (&str, &str, Option<&str>) {
    let Some(first) = rest.find('-') else {
        return (rest, "", None);
    };
    let (base, after) = (&rest[..first], &rest[first + 1..]);
    let (quote, contract) = match after.find('-') {
        None => (after, None),
        Some(second) => {
            let tail = &after[second + 1..];
            let contract = (!tail.eq_ignore_ascii_case("SWAP")).then_some(tail);
            (&after[..second], contract)
        }
    };
    if base.is_empty() || quote.is_empty() {
        return (rest, "", None);
    }
    (base, quote, contract)
}

/// `BASE_QUOTE` when the tail is a currency we know, otherwise the concatenated reading.
fn underscore_quote(rest: &str) -> (&str, &str) {
    match rest.rsplit_once('_') {
        Some((base, quote)) if !base.is_empty() && is_known_quote(quote) => (base, quote),
        _ => concat(rest),
    }
}

/// Splits off a `_PERP` or `_YYMMDD` contract tail, or `None` when there is no such tail.
///
/// `perp` selects whether a literal `_PERP` counts: it does on Binance COIN-M, whose perpetuals are
/// named `BTCUSD_PERP`. A recognized perpetual yields `(head, None)` — a perpetual is the default
/// contract, so it reports no contract while still having had its tail removed. That distinction is
/// why this returns an `Option` around the pair rather than just the contract.
fn strip_underscore_tail(rest: &str, perp: bool) -> Option<(&str, Option<&str>)> {
    let (head, tail) = rest.rsplit_once('_')?;
    if head.is_empty() {
        return None;
    }
    if perp && tail.eq_ignore_ascii_case("PERP") {
        return Some((head, None));
    }
    is_yymmdd(tail).then_some((head, Some(tail)))
}

/// Splits off Bybit's `-07AUG26` dated tail: two digits, a three-letter month, two digits.
///
/// Shares [`strip_underscore_tail`]'s return shape so both feed [`dated_concat`] unchanged.
fn strip_dash_date(rest: &str) -> Option<(&str, Option<&str>)> {
    let (head, tail) = rest.rsplit_once('-')?;
    (!head.is_empty() && is_ddmonyy(tail)).then_some((head, Some(tail)))
}

/// `BTCUSDT` and every name that ends in a known quote, including the `BTC_USDT` and `BTC-USDT`
/// spellings whose separator is trimmed off the base.
///
/// This is the reading the terminal has always used; the rules above exist to hand it a name it
/// can read. A name with no recognized quote is all base, which is correct for a Hyperliquid perp
/// (`BTC`) and for Bybit's USDC perpetuals (`BTCPERP`), whose `PERP` the core keeps too.
fn concat(sym: &str) -> (&str, &str) {
    concat_in(sym, &QUOTES)
}

/// [`concat`] against a restricted quote table; see [`CONTRACT_QUOTES`].
fn concat_in<'a>(sym: &'a str, quotes: &[&str]) -> (&'a str, &'a str) {
    let Some(quote) = quotes.iter().copied().find(|q| ends_ci(sym, q)) else {
        return (sym, "");
    };
    let cut = sym.len() - quote.len();
    let base = sym[..cut].trim_end_matches(['_', '-', '/']);
    (base, &sym[cut..])
}

/// How `base` quoted in `quote` is SPELLED on `ex` — the inverse of [`split_market`].
///
/// Pricing a wallet needs the market for a currency: "what is BTC worth in USDT?" is a lookup of
/// the BTC/USDT market, and every caller used to answer it with `format!("{base}USDT")`. That
/// spelling exists on Binance, Bybit and BitGet and on no other exchange here, so the rate came
/// back unknown on Gate, OKX and Hyperliquid and the holding was valued at zero.
///
/// `Exchange::Unknown` yields every spelling, most specific first. Trying them all is safe because
/// each candidate is then looked up in the core's catalog: a name no exchange lists simply misses.
///
/// The iterator is LAZY on purpose. This runs on the chart's order-label path, which rebuilds per
/// frame, and the first candidate is the hit on most exchanges; building all six names eagerly
/// would allocate five throwaway strings 60 times a second.
pub fn market_names_for<'a>(
    base: &'a str,
    quote: &'a str,
    ex: Exchange,
) -> impl Iterator<Item = String> + 'a {
    // A COIN-M contract is quoted in USD whatever quote was asked for, so it answers a USD-ish
    // request and must not answer, say, a BTC-quoted one with a USD price.
    let usd_ish = crate::symbol::is_usd_stable(quote);
    let shapes: &'static [Shape] = match ex {
        Exchange::Binance | Exchange::Bybit | Exchange::BitGet | Exchange::Huobi => {
            &[Shape::Concat]
        }
        Exchange::Gate => &[Shape::Underscore],
        Exchange::Okx => &[Shape::DashedSwap, Shape::Dashed],
        Exchange::BinanceCoinM => &[Shape::CoinMPerp],
        // A Hyperliquid perp IS the coin; its USDC quote never appears in the name.
        Exchange::Hyperliquid => &[Shape::Bare],
        Exchange::Unknown => &[
            Shape::Concat,
            Shape::Underscore,
            Shape::DashedSwap,
            Shape::Dashed,
            Shape::CoinMPerp,
            Shape::Bare,
        ],
    };
    shapes
        .iter()
        .filter(move |shape| usd_ish || !matches!(shape, Shape::CoinMPerp))
        .map(move |shape| shape.render(base, quote))
}

/// One exchange's way of writing a `base`/`quote` pair.
#[derive(Clone, Copy)]
enum Shape {
    /// `BTCUSDT` — Binance, Bybit, BitGet, Huobi.
    Concat,
    /// `BTC_USDT` — Gate.
    Underscore,
    /// `BTC-USDT-SWAP` — an OKX perpetual.
    DashedSwap,
    /// `BTC-USDT` — OKX spot.
    Dashed,
    /// `BTCUSD_PERP` — a Binance COIN-M perpetual.
    CoinMPerp,
    /// `BTC` — a Hyperliquid perpetual, whose name carries no quote.
    Bare,
}

impl Shape {
    fn render(self, base: &str, quote: &str) -> String {
        match self {
            Self::Concat => format!("{base}{quote}"),
            Self::Underscore => format!("{base}_{quote}"),
            Self::DashedSwap => format!("{base}-{quote}-SWAP"),
            Self::Dashed => format!("{base}-{quote}"),
            Self::CoinMPerp => format!("{base}USD_PERP"),
            Self::Bare => base.to_string(),
        }
    }
}

/// Splits a market-shaped token found in FREE TEXT, or `None` when the token is just a word.
///
/// The log panel scans core messages for coins, where `SPK` is structurally indistinguishable from
/// `BUY`, so only a token the rules could actually take apart counts: one carrying a quote
/// (`SPKUSDT`, `SPK-USDT`, or the quote-FIRST `USDT-SPK` that core logs also write) or a contract
/// tail (`ETH-240628`, `BTC_0626`). Free text is looser than a catalog name — a log line may carry
/// a four-digit `MMDD` where a market never would — which is why this is its own entrypoint rather
/// than a laxer [`split_market`].
///
/// The base must be at least three characters and hold an uppercase letter, so a date such as
/// `2026-06-30` is not mistaken for a ticker.
///
/// Returns the coin only: the caller highlights and filters by that, and inventing a `quote` or
/// `contract` for the free-text shapes would be data nobody reads.
pub fn market_in_text(word: &str) -> Option<&str> {
    if word.len() < 4 {
        return None;
    }
    // `USDT-SPK` and `BTC-USDT` are the same shape read from opposite ends, so the leading token
    // decides: a stablecoin is ONLY ever a quote, while BTC, ETH and BNB are also coins. Reading
    // quote-first unconditionally turns `BTC-USDT` into the coin USDT; reading it last turns
    // `USDT-BTC` into the coin USDT. Neither order alone is right.
    if let Some((quote, base)) = word.split_once(['-', '_']) {
        if crate::symbol::is_usd_stable(quote) && plausible_base(base) {
            return Some(base);
        }
    }
    let parts = split_market(word, Exchange::Unknown);
    if !parts.quote.is_empty() || parts.is_dated() {
        return plausible_base(parts.base).then_some(parts.base);
    }
    // A leading quote that is also a coin (`BTC-SPK`), reached only once the ordinary reading
    // found no quote at the end.
    if let Some((quote, base)) = word.split_once(['-', '_']) {
        if is_known_quote(quote) && plausible_base(base) {
            return Some(base);
        }
    }
    // A USDC perpetual: `AAVEPERP`, `1000BONKPERP`. The tail stays part of the coin because the
    // core keeps it too (it reports `AAVEPERP`), so this only decides that the token IS a market.
    if word.strip_suffix("PERP").is_some_and(plausible_base) {
        return Some(word);
    }
    // A numeric tail after a separator: the report's `BTC_0626` spelling and any other delivery
    // contract a log line happens to print. Deliberately looser than the catalog rule in
    // [`strip_underscore_tail`], which accepts only a six-digit date: free text carries `MMDD`
    // spellings a market name never does.
    let (base, tail) = word.rsplit_once(['_', '-'])?;
    let dated = (2..=8).contains(&tail.len()) && tail.bytes().all(|b| b.is_ascii_digit());
    (dated && plausible_base(base)).then_some(base)
}

/// Whether a slice is long enough and cased enough to be a ticker rather than a number.
fn plausible_base(base: &str) -> bool {
    base.len() >= 3 && base.bytes().any(|b| b.is_ascii_uppercase())
}

/// Whether `sym` ends with `suffix`, ASCII-case-insensitively, and has something before it.
///
/// Byte comparison is safe for a non-ASCII base such as `币安人生USDT`: an ASCII tail can never
/// start inside a multi-byte character, so the cut is always on a character boundary.
fn ends_ci(sym: &str, suffix: &str) -> bool {
    sym.len() > suffix.len()
        && sym.as_bytes()[sym.len() - suffix.len()..].eq_ignore_ascii_case(suffix.as_bytes())
}

/// Whether `s` is one of the quote currencies, ignoring ASCII case.
fn is_known_quote(s: &str) -> bool {
    QUOTES.iter().any(|q| s.eq_ignore_ascii_case(q))
}

/// Whether `s` is a six-digit contract date such as `260925`.
fn is_yymmdd(s: &str) -> bool {
    s.len() == 6 && s.bytes().all(|b| b.is_ascii_digit())
}

/// Whether `s` is Bybit's `07AUG26` contract date: two digits, three letters, two digits.
fn is_ddmonyy(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 7
        && b[..2].iter().all(u8::is_ascii_digit)
        && b[2..5].iter().all(u8::is_ascii_alphabetic)
        && b[5..].iter().all(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests;
