//! Market naming, identity selection and funding conversion.

use super::*;

/// Read the funding pair a caption prints, from the two raw wire values.
///
/// A free function, and pure, because both halves were wrong for a year in ways only a comparison
/// with the reference terminal exposed — and neither is observable from a snapshot test:
///
/// - The RATE is already a percentage. `-0.2313` means −0.23 %, which is what Moonbot prints for
///   the same market at the same moment (measured 2026-08-24, BICO on OKX Futures). Multiplying it
///   by a hundred stated a −23 % funding, a figure no venue has ever charged. The protocol's own
///   doc comment claims `0.0001 = 0.01%`; the two live screens say otherwise, and the screens win.
/// - The TIME arrives on the CLIENT's local wall clock, not on UTC: moonproto adds this machine's
///   zone offset while reading the wire (`apply_delphi_local_funding_shift`) and its field is
///   documented as "Delphi client-local TDateTime after adding local TZShift". Read as UTC, it put
///   the next funding one zone into the future — an operator at UTC+2 saw `9h 55m` where the
///   reference terminal counted `7:53:37`.
///
/// Args:
///     rate: `funding_rate` as the wire carries it, already in percent.
///     client_local_ms: `funding_time` converted to milliseconds, still on the client's clock.
///     local_offset_ms: This machine's offset from UTC, from `util::time::local_utc_offset_ms`.
///
/// Returns:
///     `(percentage, unix milliseconds)`, both `None` where the venue charges no funding — the
///     absence test is the TIME, since a rate of exactly zero is a real answer between charges.
pub fn funding_from_wire(
    rate: f64,
    client_local_ms: i64,
    local_offset_ms: i64,
) -> (Option<f64>, Option<i64>) {
    // Zero is how "this venue has no funding" arrives, and a spot market sends nothing at all.
    if client_local_ms <= 0 {
        return (None, None);
    }
    let at_ms = client_local_ms - local_offset_ms;
    (rate.is_finite().then_some(rate), Some(at_ms))
}

/// How one market is named to the user.
///
/// Built by [`MarketDataSource::market_label`]; see it for why both fields come from one place.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MarketLabel {
    /// Coin TOKEN as the core names it: `SOL`, OKX `BEAT`, Bybit `1kBONKPERP`, COIN-M `SOL_RP`
    /// and `SOL_0925`, Hyperliquid spot `HFUN` for the market `@156`.
    ///
    /// This is the identity to WRITE and to FILTER by — the core matches its coin lists against it
    /// by exact text and its report stores it. It is not always what a coin column should show:
    /// it carries a contract tail. Use [`Self::display_coin`] for that and [`Self::match_key`] to
    /// compare two spellings of one coin.
    pub coin: String,
    /// Coin identity as the CORE resolved it — `market_currency_canonic` — or empty when the
    /// catalog does not hold this market.
    ///
    /// THE answer to "is this the same coin on another exchange", and deliberately taken whole
    /// rather than derived: the core already folds `1000BONK`, `1kBONK` and `BONK` to one `BONK`,
    /// `1000SATS` to `SATS` and `AAVE_RP` to `AAVE` — foldings no rule over a market name can
    /// reproduce, because `1000SATS` and `1000CAT` look alike and only one of them is a
    /// multiplier. Measured across 21 live cores (2026-08-24): `market_currency` splits BONK into
    /// four groups and BTC into twelve, `canonic` into one each.
    ///
    /// NOT a replacement for [`Self::coin`], which stays the token to WRITE: the core matches its
    /// own coin lists against `market_currency` by exact text and the report stores that spelling.
    /// Two fields, two questions — use [`Self::identity`] to compare, `coin` to write.
    ///
    /// Left as the core sends it, including what it does not fold: a Bybit USDC perpetual arrives
    /// as `BONKPERP` and therefore matches only its own kind. That is a core-side gap by decision,
    /// not something to paper over here — a terminal-side rule would be a second opinion about
    /// coin identity, and the whole point is to have one.
    pub canonic: String,
    /// Quote currency, uppercase, or empty when neither the catalog nor the name carries one.
    pub quote: String,
    /// Contract tail recovered from a market NAME when the catalog could not answer, so a dated
    /// contract does not share a label with its perpetual. `None` on the catalog path, where the
    /// contract is already spelled inside [`Self::coin`].
    pub contract: Option<String>,
}

impl MarketLabel {
    /// The reading a market NAME alone supports, used when the catalog does not hold the market.
    pub fn from_name(market: &str, exchange: crate::symbol::Exchange) -> Self {
        let parts = crate::symbol::parse::split_market(market, exchange);
        Self {
            coin: parts.base.to_string(),
            // No catalog, no canonic: this path exists precisely because the market is not in it.
            // [`Self::identity`] falls back to the folded token, which is what comparing did
            // before the field existed.
            canonic: String::new(),
            quote: parts.quote.to_ascii_uppercase(),
            contract: parts.contract.map(str::to_string),
        }
    }

    /// The coin WITHOUT its contract tail, for a column headed "coin": `AAVE_RP` reads `AAVE`.
    pub fn display_coin(&self) -> &str {
        crate::symbol::strip_contract_suffix(&self.coin)
    }

    /// THE key for "are these the same coin?", folding contract and case exactly as a strategy
    /// coin list is compared. Matching raw [`Self::coin`] would fail to connect `AAVE` to the
    /// COIN-M market the core calls `AAVE_RP`.
    ///
    /// This is the key for the core's own vocabulary — coin lists, the report, the news feed, the
    /// tuner — all of which compare against `market_currency`. For "the same coin on ANOTHER
    /// exchange" use [`Self::identity`]: this one keeps `1kBONK` and `1000BONK` apart, as the two
    /// cores that spell them do.
    pub fn match_key(&self) -> String {
        crate::symbol::coin_match_key(&self.coin)
    }

    /// THE key for "is this the same coin on another exchange".
    ///
    /// The core's own [`Self::canonic`] where there is one, and the folded token otherwise — a
    /// market the catalog does not hold has no canonic, and answering "never the same coin" for it
    /// would drop a chart's whole arbitrage column the moment its market was delisted.
    ///
    /// Uppercased because the two sources are not consistent about case: `1kBONK` folds to `BONK`
    /// on one core and the name-based fallback yields whatever the market name carried.
    pub fn identity(&self) -> String {
        match self.canonic.trim().is_empty() {
            true => self.match_key(),
            false => self.canonic.trim().to_ascii_uppercase(),
        }
    }

    /// `SOL-USDT` for a table cell or a chart caption, with a dated contract keeping its expiry
    /// (`SOL-USD-0925`) because two expiries are two instruments. A perpetual carries no tail:
    /// on a futures connection every market is one, so printing it everywhere says nothing.
    pub fn pair(&self) -> String {
        let base = self.display_coin();
        let mut out = if self.quote.is_empty() {
            base.to_string()
        } else {
            format!("{base}-{}", self.quote)
        };
        if let Some(expiry) = self.expiry() {
            out.push('-');
            out.push_str(expiry);
        }
        out
    }

    /// The EXPIRY this market carries, if any: `BTC_0925` → `0925`, a name-sourced `07AUG26`.
    ///
    /// `None` for a perpetual. Moonbot marks one with an `_RP` tail, which is a contract KIND and
    /// not a date — reading it as an expiry is what made the market picker prefer a quarterly
    /// over the perpetual.
    pub fn expiry(&self) -> Option<&str> {
        let base = self.display_coin();
        self.coin
            .get(base.len()..)
            .map(|tail| tail.trim_start_matches('_'))
            .filter(|tail| !tail.is_empty())
            .or(self.contract.as_deref())
            .filter(|tail| !tail.eq_ignore_ascii_case("RP"))
    }
}

/// Pick the market that carries a coin's IDENTITY on one exchange, for a chart to open beside
/// another.
///
/// A different question from [`pick_market_for_coin`] beside it, and kept apart on purpose. That
/// one answers "which market is this token" for a report row or a news click, matching the core's
/// own spelling. This one answers "show me this coin over there", where the token is spelled by a
/// different exchange and the candidates include instruments a reader did not ask for.
///
/// Three rules, in order:
///
/// 1. **Same identity.** `1kBONK` on Bybit and `1000BONK` on Binance are one coin because both
///    cores fold them to `BONK`; `BONK3L` is not, and neither is `PEPECOIN`.
/// 2. **A dated contract only when nothing else carries the coin.** One live core lists BTC under
///    ten Bybit expiries plus the perpetual; opening eleven charts answers a question nobody asked,
///    and a spread against an expiry is basis rather than arbitrage. A coin that trades ONLY as a
///    dated contract still opens — the nearest expiry, since the list arrives in no useful order.
/// 3. **The reader's own quote currency first.** A click from a USDT chart opens `BTCUSDT`, from a
///    USDC one `BTCPERP`. Then any USD stablecoin, then whatever is left, so a coin quoted only in
///    BTC still opens rather than silently doing nothing.
///
/// Ties break on the market NAME, not on catalog order: two clicks on one venue must open the same
/// chart, and the catalog is a `HashMap` walk away from being ordered differently.
///
/// Args:
///     candidates: `(market name, label)` pairs from ONE core, as `market_labels` builds them.
///     identity: [`MarketLabel::identity`] of the chart the request came from.
///     quote: Quote currency of that chart, for rule 3. Empty asks for no preference.
///
/// Returns:
///     The market to open, or `None` when this core does not carry the coin at all.
pub fn pick_market_for_identity<'a>(
    candidates: &'a [(String, MarketLabel)],
    identity: &str,
    quote: &str,
) -> Option<&'a str> {
    let wanted = identity.trim().to_ascii_uppercase();
    if wanted.is_empty() {
        return None;
    }
    let mut matching: Vec<&'a (String, MarketLabel)> = candidates
        .iter()
        .filter(|(_, label)| label.identity() == wanted)
        .collect();
    if matching.is_empty() {
        return None;
    }
    matching.sort_by(|a, b| a.0.cmp(&b.0));
    if matching.iter().any(|(_, label)| label.expiry().is_none()) {
        matching.retain(|(_, label)| label.expiry().is_none());
    }
    let quote = quote.trim();
    let exact = matching
        .iter()
        .find(|(_, label)| !quote.is_empty() && label.quote.eq_ignore_ascii_case(quote));
    let usd = || {
        matching
            .iter()
            .find(|(_, label)| crate::symbol::is_usd_stable(&label.quote))
    };
    exact
        .or_else(usd)
        .or_else(|| matching.first())
        .map(|(name, _)| name.as_str())
}

/// Pick the market a COIN belongs to, from candidates already labelled by the core's catalog.
///
/// The question "which market is `1kRATS`?" cannot be answered from market names: the market is
/// spelled `1000RATSUSDT` and only the catalog knows the core folds it to `1kRATS`. Comparing a
/// name reading against a coin the core wrote — a report row, a coin list — silently finds
/// nothing and leaves the caller inventing a market that does not exist.
///
/// Exact token first, then the folded [`MarketLabel::match_key`] so a bare `AAVE` still reaches
/// the COIN-M market the core calls `AAVE_RP`. Within each pass an undated contract wins: a coin
/// names an instrument family, not an expiry. Among undated ones the market in `quote` wins: one
/// catalog can list a token against several quotes — Hyperliquid spot has KNTQ against USDH
/// (`@254`) and USDC (`@334`), Binance `BTCUSDT` beside `BTCUSDC` — and only one of them is where
/// the core trades. Without a match in `quote` the first candidate stands, as before.
///
/// Args:
///     candidates: `(market name, label)` pairs from ONE core, as `market_labels` builds them.
///     coin: The token the core wrote — a report row's coin, a coin-list entry.
///     quote: The quote currency the core trades in, uppercase; empty asks for no preference.
pub fn pick_market_for_coin<'a>(
    candidates: &'a [(String, MarketLabel)],
    coin: &str,
    quote: &str,
) -> Option<&'a str> {
    let wanted_key = crate::symbol::coin_match_key(coin);
    let quote = quote.trim();
    let pick = |matches: &dyn Fn(&MarketLabel) -> bool| -> Option<&'a str> {
        let mut undated = None;
        let mut dated = None;
        for (name, label) in candidates {
            if !matches(label) {
                continue;
            }
            if label.expiry().is_some() {
                dated.get_or_insert(name.as_str());
                continue;
            }
            if !quote.is_empty() && label.quote.eq_ignore_ascii_case(quote) {
                return Some(name.as_str());
            }
            undated.get_or_insert(name.as_str());
        }
        undated.or(dated)
    };
    pick(&|label: &MarketLabel| label.coin.eq_ignore_ascii_case(coin))
        .or_else(|| pick(&|label: &MarketLabel| label.match_key() == wanted_key))
}

impl MarketDataSourceInner {
    /// The naming family of a market-data provider, for callers already holding the lock.
    ///
    /// Exists so a caller that needs the provider's client AND its naming family reads both under
    /// one guard: taken separately, a provider election between the two would spell markets for
    /// one exchange and price them against another's catalog.
    pub(super) fn exchange_of_provider(&self, provider: CoreId) -> crate::symbol::Exchange {
        self.provider_exchange
            .get(&provider)
            .map(|id| crate::symbol::Exchange::from_code(id.code))
            .unwrap_or_default()
    }
}
