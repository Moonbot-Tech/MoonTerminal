use super::*;

/// Direction used to turn one spot close into quote units per USDT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RateOrientation {
    /// A `QUOTEUSDT` close is already USDT per quote unit.
    Direct,
    /// A `USDTQUOTE` close must be inverted.
    Inverse,
    /// USDT itself is exactly one and needs no market request.
    Identity,
}

impl RateOrientation {
    /// Stable integer persisted with rate provenance.
    ///
    /// Returns:
    ///     Database representation of the orientation.
    pub(in crate::db::valuation) const fn code(self) -> i64 {
        match self {
            Self::Direct => 0,
            Self::Inverse => 1,
            Self::Identity => 2,
        }
    }
}

/// Price within a closed candle used for historical conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RatePriceBasis {
    /// The requested minute existed, so its close preserves the original exact-minute contract.
    ExactClose,
    /// The requested minute was absent, so the first later candle contributes its open.
    SuccessorOpen,
}

impl RatePriceBasis {
    /// Stable integer persisted with rate provenance.
    ///
    /// Returns:
    ///     Database representation of the price basis.
    pub(in crate::db::valuation) const fn code(self) -> i64 {
        match self {
            Self::ExactClose => 0,
            Self::SuccessorOpen => 1,
        }
    }
}

/// One validated closed-minute conversion result ready for persistence.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResolvedRate {
    /// Persisted MoonBot quote ordinal.
    pub quote_ordinal: i64,
    /// UTC minute start in Unix seconds.
    pub minute_utc: i64,
    /// Actual closed market minute used by every conversion leg.
    pub resolved_minute_utc: i64,
    /// USDT received for one quote unit.
    pub rate_usdt: f64,
    /// Canonical provider identifier.
    pub provider: String,
    /// Spot market used by the provider.
    pub symbol: String,
    /// Direct, inverse, or identity conversion.
    pub orientation: RateOrientation,
    /// Exact close or later-candle open used to compute the conversion.
    pub price_basis: RatePriceBasis,
    /// Provider candle open time in Unix milliseconds.
    pub candle_open_ms: i64,
    /// Provider candle close time in Unix milliseconds.
    pub candle_close_ms: i64,
    /// Optional second provider for a common-minute two-leg conversion.
    pub leg2_provider: Option<String>,
    /// Optional second market for a common-minute two-leg conversion.
    pub leg2_symbol: Option<String>,
    /// Optional second market orientation.
    pub leg2_orientation: Option<RateOrientation>,
    /// Validated oriented rate contributed by the first leg.
    pub leg1_rate: f64,
    /// Validated oriented rate contributed by the optional second leg.
    pub leg2_rate: Option<f64>,
}

/// Build a zero-request identity conversion for one USDT minute.
///
/// Args:
///     quote_ordinal: Persisted USDT quote ordinal.
///     minute_utc: Requested and resolved UTC minute.
///
/// Returns:
///     Unit-rate result with explicit identity provenance.
pub(in crate::db) fn identity_rate(quote_ordinal: i64, minute_utc: i64) -> ResolvedRate {
    ResolvedRate {
        quote_ordinal,
        minute_utc,
        resolved_minute_utc: minute_utc,
        rate_usdt: 1.0,
        provider: "identity".to_string(),
        symbol: "USDT".to_string(),
        orientation: RateOrientation::Identity,
        price_basis: RatePriceBasis::ExactClose,
        candle_open_ms: minute_utc.saturating_mul(1_000),
        candle_close_ms: minute_utc.saturating_mul(1_000).saturating_add(59_999),
        leg2_provider: None,
        leg2_symbol: None,
        leg2_orientation: None,
        leg1_rate: 1.0,
        leg2_rate: None,
    }
}

/// Current report inputs used to guard a prepared valuation against same-key upserts.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TradeInput {
    /// Typed or legacy physical source.
    pub source: TradeSource,
    /// Stable runtime core identity.
    pub core_uid: i64,
    /// `newrecid` for typed rows or `db_id` for legacy rows.
    pub row_id: i64,
    /// Close timestamp in Unix seconds — the RAW replica value, exactly as `db::rep` stored it:
    /// CORE-LOCAL wall clock per `report_axis`'s three-axis model, never corrected. Every identity
    /// or ordering read of this field (`reconciliation_batch`'s keyset query and cursor, the
    /// `trade_values` coverage join and upsert key, `trade_key`) depends on it staying untouched
    /// from the moment it is decoded off the row. Only `worker::valuation_minute` may convert a
    /// COPY of it to a true-UTC rate minute, and it must floor after converting, never before.
    pub closedate: i64,
    /// Persisted quote ordinal.
    pub quote_ordinal: i64,
    /// Native quote-currency profit.
    pub profit_quote: f64,
    /// Native quote-currency positive spend, when supplied by the core.
    pub spent_quote: Option<f64>,
}
