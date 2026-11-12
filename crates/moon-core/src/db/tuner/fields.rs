use super::comment_field;

/// Field class indicating which strategy Ignore flag disables its filter.
/// `IgnoreFilters` disables ALL classes; `IgnoreDelta` and `IgnoreVolume` disable their own.
/// `DeltaSlot` covers deltas without dedicated strategy parameters: they use Delta2/Delta3
/// slots (type plus min/max), so at most two can be saved in a strategy; they are ignored as
/// deltas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldClass {
    Filter,
    /// BV/SV filter: its own `UseBV_SV_Filter` switch (`false` means disabled)
    /// in addition to the general `IgnoreFilters` flag.
    BvSv,
    /// PriceBug: the Filters/Ping section with its own `IgnorePing` flag.
    Ping,
    /// Filters/Base section with its own `IgnoreBase` flag (leverage and MarkPrice delta).
    Base,
    Delta,
    DeltaSlot,
    Volume,
    /// A detect's own trigger threshold (e.g. `DropsPriceDelta`), read from the report comment
    /// rather than a column. It is the signal itself, not a filter, so no Ignore flag gates it.
    Detect,
}

/// Description of one tuner field. This is the ONLY place to edit for a new report column or
/// strategy parameter: one row in `FIELDS`; everything else (the UNION SQL projection, UI grid,
/// chips, automatic search, and strategy persistence) is derived from this table. Columns in
/// reports.sqlite are extended automatically from the core schema (db/rep.rs); a value parsed out
/// of the report comment instead is projected by `comment_field`.
pub struct FieldSpec {
    /// Report-replica column (lowercase).
    pub col: &'static str,
    /// Grid label (as in MB/V3).
    pub label: &'static str,
    /// Class indicating which strategy Ignore flag disables the filter.
    pub class: FieldClass,
    /// Strategy filter parameters (Min, Max); `None` means the value is not saved.
    pub p_min: Option<&'static str>,
    pub p_max: Option<&'static str>,
    /// `DeltaN_Type` value for slot fields (`class == DeltaSlot`): the threshold is saved
    /// through a Delta2/Delta3 slot instead of dedicated parameters.
    pub slot_type: Option<&'static str>,
}

const fn field(
    col: &'static str,
    label: &'static str,
    class: FieldClass,
    p_min: Option<&'static str>,
    p_max: Option<&'static str>,
    slot_type: Option<&'static str>,
) -> FieldSpec {
    FieldSpec {
        col,
        label,
        class,
        p_min,
        p_max,
        slot_type,
    }
}

impl FieldSpec {
    /// Whether zero disables the Min and Max sides rather than imposing a numeric bound.
    /// Source: https://moonbot.pro/documentation/filters-tab/filters-ping-tab
    /// https://moonbot.pro/documentation/filters-tab/filters-volume-tab
    /// https://moonbot.pro/documentation/filters-tab/filters-base-tab
    /// These parameters ignore zero; the support KB also defines MarkPrice's zero as off.
    pub fn zero_disables(&self) -> (bool, bool) {
        match self.col {
            "lev" => (false, true),
            "dmark" | "pricebug" | "hvolf" | "vd1m" | "bvsvratio" => (true, true),
            _ => (false, false),
        }
    }

    /// Whether a value disables the requested side (`max_side` selects Max instead of Min).
    pub fn bound_is_off(&self, max_side: bool, v: f64) -> bool {
        let (min, max) = self.zero_disables();
        (if max_side { max } else { min }) && v == 0.0
    }

    /// Turn saved zero-off values and an unsavable Max into absent bounds before scoring.
    pub(super) fn canonical_bounds(&self, from: f64, to: f64) -> (Option<f64>, Option<f64>) {
        (
            (!self.bound_is_off(false, from)).then_some(from),
            (!self.min_only() && !self.bound_is_off(true, to)).then_some(to),
        )
    }

    /// Whether the threshold can be saved to the strategy through parameters or a slot.
    /// Unmapped fields (da1m, d5s) are marked in the grid and excluded from automatic
    /// suggestions by default.
    pub fn mapped(&self) -> bool {
        self.p_min.is_some() || self.p_max.is_some() || self.slot_type.is_some()
    }

    /// Whether the strategy can hold only a lower bound for this field: its upper bound is never
    /// saved, so the what-if ignores one and the searches never suggest one.
    pub fn min_only(&self) -> bool {
        self.p_min.is_some() && self.p_max.is_none() && self.slot_type.is_none()
    }
}

impl FieldClass {
    /// Parent MoonBot section: BV/SV lives under Filters/Volume, Delta2/Delta3 slots live
    /// under Filters/Delta, and every other class is its own parent.
    pub fn parent(self) -> FieldClass {
        match self {
            FieldClass::BvSv => FieldClass::Volume,
            FieldClass::DeltaSlot => FieldClass::Delta,
            other => other,
        }
    }
}

/// Report fields available to filters. This is the ONLY source of column names allowed into
/// tuner SQL (the whitelist). The order matches the grid and the MoonBot Filters sections:
/// Base -> Ping -> Volume (with BV/SV nested inside) -> Delta (with Delta2/Delta3 slots nested
/// inside). The parameter mapping was checked on 2026-07-17 against the union of parameters
/// from every strategy type. Fields WITHOUT parameters or a slot type (da1m, d5s) are shown
/// with a "no parameter" marker: what-if calculations work for them, but there is nowhere to
/// save the threshold in a strategy. Slot types 2h/30m/Pump5m have no report column and cannot
/// be represented.
pub const FIELDS: &[FieldSpec] = &[
    // Filters/Base (IgnoreFilters | IgnoreBase): leverage and mark-price delta (±%).
    field(
        "lev",
        "Lev",
        FieldClass::Base,
        Some("MinLeverage"),
        Some("MaxLeverage"),
        None,
    ),
    field(
        "dmark",
        "dMark",
        FieldClass::Base,
        Some("MarkPriceMin"),
        Some("MarkPriceMax"),
        None,
    ),
    // Filters/Ping (IgnoreFilters | IgnorePing).
    field(
        "pricebug",
        "PriceBug",
        FieldClass::Ping,
        Some("BinancePriceBugMin"),
        Some("BinancePriceBug"),
        None,
    ),
    // Filters/Volume (IgnoreFilters | IgnoreVolume).
    field(
        "hvol",
        "H.Vol",
        FieldClass::Volume,
        Some("MinHourlyVolume"),
        Some("MaxHourlyVolume"),
        None,
    ),
    field(
        "hvolf",
        "H.VolF",
        FieldClass::Volume,
        Some("MinHourlyVolFast"),
        Some("MaxHourlyVolFast"),
        None,
    ),
    field(
        "dvol",
        "D.Vol",
        FieldClass::Volume,
        Some("MinVolume"),
        Some("MaxVolume"),
        None,
    ),
    field(
        "vd1m",
        "Vd1m",
        FieldClass::Volume,
        Some("MinuteVolDeltaMin"),
        Some("MinuteVolDeltaMax"),
        None,
    ),
    // BV/SV is a Volume SUBGROUP: its own UseBV_SV_Filter switch applies in addition to
    // IgnoreVolume; these are filter parameters, not the BV_SV_Ratio detector parameters.
    field(
        "bvsvratio",
        "bvsv",
        FieldClass::BvSv,
        Some("BV_SV_FilterRatio"),
        Some("BV_SV_FilterRatioMax"),
        None,
    ),
    // Filters/Delta (IgnoreFilters | IgnoreDelta).
    field(
        "d24h",
        "d24h",
        FieldClass::Delta,
        Some("Delta_24h_Min"),
        Some("Delta_24h_Max"),
        None,
    ),
    field(
        "d3h",
        "d3h",
        FieldClass::Delta,
        Some("Delta_3h_Min"),
        Some("Delta_3h_Max"),
        None,
    ),
    field("da1m", "da1m", FieldClass::Delta, None, None, None),
    field("d5s", "d5s", FieldClass::Delta, None, None, None),
    field(
        "btc1hdelta",
        "dBTC",
        FieldClass::Delta,
        Some("Delta_BTC_Min"),
        Some("Delta_BTC_Max"),
        None,
    ),
    field(
        "exchange1hdelta",
        "dMarket",
        FieldClass::Delta,
        Some("Delta_Market_Min"),
        Some("Delta_Market_Max"),
        None,
    ),
    field(
        "btc24hdelta",
        "d24BTC",
        FieldClass::Delta,
        Some("Delta_BTC_24_Min"),
        Some("Delta_BTC_24_Max"),
        None,
    ),
    field(
        "exchange24hdelta",
        "dM24",
        FieldClass::Delta,
        Some("Delta_Market_24_Min"),
        Some("Delta_Market_24_Max"),
        None,
    ),
    field(
        "btc5mdelta",
        "dBTC5m",
        FieldClass::Delta,
        Some("Delta_BTC_5m_Min"),
        Some("Delta_BTC_5m_Max"),
        None,
    ),
    field(
        "dbtc1m",
        "dBTC1m",
        FieldClass::Delta,
        Some("Delta_BTC_1m_Min"),
        Some("Delta_BTC_1m_Max"),
        None,
    ),
    // Delta2/Delta3 slots form a Delta SUBGROUP (at most two per strategy).
    field("d1h", "d1h", FieldClass::DeltaSlot, None, None, Some("1h")),
    field(
        "d15m",
        "d15m",
        FieldClass::DeltaSlot,
        None,
        None,
        Some("15m"),
    ),
    field("d5m", "d5m", FieldClass::DeltaSlot, None, None, Some("5m")),
    field("d1m", "d1m", FieldClass::DeltaSlot, None, None, Some("1m")),
    field(
        "pump1h",
        "Pump1H",
        FieldClass::DeltaSlot,
        None,
        None,
        Some("Pump1h"),
    ),
    field(
        "dump1h",
        "Dump1H",
        FieldClass::DeltaSlot,
        None,
        None,
        Some("Dump1h"),
    ),
    // DropsDetection: the drop the detect measured at fire time, parsed from the comment by
    // `comment_field`. The strategy fires once it reaches `DropsPriceDelta`, so raising that
    // parameter to X drops exactly the deals below X; there is no Max parameter.
    field(
        comment_field::DROPS_PRICE_DELTA,
        "xPriceDelta",
        FieldClass::Detect,
        Some("DropsPriceDelta"),
        None,
        None,
    ),
];

/// When the core checks a stored limit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Live {
    /// While the value is above zero; zero switches the check off.
    Positive,
    /// At any value but zero: a negative one is a rule of its own, not "off".
    NonZero,
    /// While the value is below this; at or above it the check is off, and zero is the
    /// strictest setting rather than "off".
    Below(f64),
}

impl Live {
    /// Whether a limit stored at `value` is checked.
    pub fn at(self, value: f64) -> bool {
        match self {
            Self::Positive => value > 0.0,
            Self::NonZero => value != 0.0,
            Self::Below(off) => value < off,
        }
    }
}

/// A strategy limit OUTSIDE the tuner's grid that Ignore flags keep asleep.
#[derive(Debug)]
pub struct WakeExtra {
    /// The strategy parameter.
    pub key: &'static str,
    /// The flags that switch it off; it works only while every one of them is `NO`.
    pub gates: &'static [&'static str],
    /// The values at which the core checks it at all.
    pub live: Live,
    /// Another parameter that must be above zero for this one to be checked.
    pub needs: Option<&'static str>,
}

/// The limits outside the grid that wake when Save clears a flag to make a fitted bound apply:
/// clearing `IgnoreFilters` turns on every group whose own flag is `NO`, and the report has no
/// column to model these by, so the save dialog names them (`StratFilters::extras`).
///
/// The groups are Moonbot's own — Time, Price, Ping and Delta, each under its flag and all of
/// them under `IgnoreFilters` (MoonDoc, `concepts/01-market-selection-and-filters.md`). Most
/// limits are off at zero; the exceptions are the ones that read wrong when taken for the
/// rule: a price step cap of `0` blocks every coin and only `100` or more switches it off,
/// the session profit and funding limits mean something at a negative value, and the ping cap
/// is checked only together with a latency cap.
///
/// Left out on purpose: limits whose check depends on the strategy type (the pauses after a
/// detect) and the ones that are not a number (the schedule, token tags, the EMA formula). A
/// `TopMarket` strategy skips the price step floor; the list names it all the same.
pub const WAKE_EXTRAS: &[WakeExtra] = &[
    WakeExtra {
        key: "PenaltyTime",
        gates: &["IgnoreFilters", "IgnoreTime"],
        live: Live::Positive,
        needs: None,
    },
    WakeExtra {
        key: "TradePenaltyTime",
        gates: &["IgnoreFilters", "IgnoreTime"],
        live: Live::Positive,
        needs: None,
    },
    WakeExtra {
        key: "FundingBefore",
        gates: &["IgnoreFilters", "IgnoreTime"],
        live: Live::NonZero,
        needs: None,
    },
    WakeExtra {
        key: "FundingAfter",
        gates: &["IgnoreFilters", "IgnoreTime"],
        live: Live::NonZero,
        needs: None,
    },
    WakeExtra {
        key: "MaxBalance",
        gates: &["IgnoreFilters", "IgnorePrice"],
        live: Live::Positive,
        needs: None,
    },
    WakeExtra {
        key: "MaxPosition",
        gates: &["IgnoreFilters", "IgnorePrice"],
        live: Live::Positive,
        needs: None,
    },
    WakeExtra {
        key: "SessionProfitMin",
        gates: &["IgnoreFilters", "IgnorePrice"],
        live: Live::NonZero,
        needs: None,
    },
    WakeExtra {
        key: "SessionProfitMax",
        gates: &["IgnoreFilters", "IgnorePrice"],
        live: Live::NonZero,
        needs: None,
    },
    WakeExtra {
        key: "TotalLoss",
        gates: &["IgnoreFilters", "IgnorePrice"],
        live: Live::Positive,
        needs: None,
    },
    WakeExtra {
        key: "WorkingPriceMin",
        gates: &["IgnoreFilters", "IgnorePrice"],
        live: Live::Positive,
        needs: None,
    },
    WakeExtra {
        key: "WorkingPriceMax",
        gates: &["IgnoreFilters", "IgnorePrice"],
        live: Live::Positive,
        needs: None,
    },
    WakeExtra {
        key: "PriceStepMin",
        gates: &["IgnoreFilters", "IgnorePrice"],
        live: Live::Positive,
        needs: None,
    },
    WakeExtra {
        key: "PriceStepMax",
        gates: &["IgnoreFilters", "IgnorePrice"],
        live: Live::Below(100.0),
        needs: None,
    },
    WakeExtra {
        key: "MinPing",
        gates: &["IgnoreFilters", "IgnorePing"],
        live: Live::Positive,
        needs: None,
    },
    WakeExtra {
        key: "MaxLatency",
        gates: &["IgnoreFilters", "IgnorePing"],
        live: Live::Positive,
        needs: None,
    },
    WakeExtra {
        key: "MaxPing",
        gates: &["IgnoreFilters", "IgnorePing"],
        live: Live::Positive,
        needs: Some("MaxLatency"),
    },
    WakeExtra {
        key: "GlobalFilterPenalty",
        gates: &["IgnoreFilters", "IgnoreDelta"],
        live: Live::Positive,
        needs: None,
    },
    WakeExtra {
        key: "FilterMin",
        gates: &["IgnoreFilters", "IgnoreDelta"],
        live: Live::NonZero,
        needs: None,
    },
    WakeExtra {
        key: "FilterMax",
        gates: &["IgnoreFilters", "IgnoreDelta"],
        live: Live::NonZero,
        needs: None,
    },
];

/// `DeltaN_Type` value for a slot field (`None` means the field is not a slot).
pub fn slot_type_for(field: &str) -> Option<&'static str> {
    FIELDS
        .iter()
        .find(|s| s.col == field)
        .and_then(|s| s.slot_type)
}
