//! Description of a Moonbot core (server) and its grouping.

use serde::{Deserialize, Serialize};

use super::secrets::Secret;

/// Number of manual-strategy quick-select slots, matching Moonbot's `ManualStratsConfig` and the
/// terminal's own `HotkeysConfig::manual_strategy` key array.
pub const MANUAL_STRAT_SLOTS: usize = 10;

/// One manual-strategy quick-select button, as this terminal owns it.
///
/// The button is its STRATEGY: Moonbot's wire carries one name per slot
/// (`trading.manual_strats_names`) and shows exactly that on the button, and so does this. A
/// separate trader-chosen caption existed here briefly and was removed — a button whose label can
/// differ from the strategy it fires is a button that can place a real order on something other
/// than what it says.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StratSlot {
    /// Name of the Manual-kind strategy this button fires; empty means the slot is unassigned.
    ///
    /// Stored by NAME rather than by id, like the core's own slots: an id is a per-core number
    /// that a re-created strategy does not keep, and the name is what both screens show.
    #[serde(default)]
    pub strategy: String,
    /// Whether this slot is drawn at all, mirroring Moonbot's `Button N` checkbox.
    ///
    /// Local like the rest of the slot: the core's own `show_button` seeds it through the settings
    /// popup's "pull from the core" action, after which this is the answer — otherwise clearing a
    /// slot the core hides would make the button vanish with no way to reach it again.
    #[serde(default)]
    pub show: bool,
}

/// Manual-strategy mode for one core, as this terminal owns it.
///
/// Terminal state, not a mirror of the core's `use_manual_strategy`/`manual_strategy_id`: the
/// order carries its strategy explicitly (`NewOrderParams::with_strategy_id`), so which strategy
/// THIS terminal fires is nothing the core has to be switched into. Two terminals on one core can
/// therefore sit on different strategies.
///
/// The terminal no longer WRITES the core's own switch, which is a narrower promise than leaving it
/// untouched: every ClientSettings edit still travels as a full snapshot, so an unrelated setting
/// changed here re-sends whatever `use_manual_strategy` the terminal last read. What is gone is
/// this mode driving that field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ManualStratState {
    /// Whether manual-strategy mode is on for this core.
    #[serde(default)]
    pub on: bool,
    /// Selected Manual-kind strategy BY NAME; empty means none is selected.
    ///
    /// The name is the ANCHOR, not the working identifier: an id is a per-core number that a
    /// re-created strategy does not keep, so the name is what survives a strategy being rebuilt.
    /// It is used to find the strategy again only when [`Self::id`] no longer names one.
    #[serde(default)]
    pub strategy: String,
    /// Id this selection was last confirmed to have, or `0` before it has ever been resolved.
    ///
    /// The WORKING identifier, and the reason it exists: Moonbot substitutes manual hook strategies
    /// while they run ("Manual strategy X turned into Hook Y"), so resolving the name against the
    /// live snapshot before every order can hand back a DIFFERENT strategy — with its own stop —
    /// for a selection the trader never touched. Pinning the id makes the choice stable; the name
    /// takes over only once that id is gone from the core.
    #[serde(default)]
    pub id: u64,
    /// Whether this core follows Moonbot's OWN stop rule for a manual-strategy order.
    ///
    /// On - the default - the stop of such an order belongs to the strategy the core applies (or to
    /// the MoonHook that strategy defers to), exactly as Moonbot behaves: the terminal shows that
    /// stop, locks its SL control while a strategy is selected, and sends no per-order override.
    /// Off re-opens the control and the visible stop is written to the order right after it is
    /// placed, whatever the core ended up applying.
    ///
    /// Defaults to TRUE, including for every file written before this field existed: repeating the
    /// core's behaviour is the baseline, and the terminal overriding it was the exception.
    #[serde(default = "default_true")]
    pub mb_logic: bool,
}

/// Hand-written rather than derived, because the derive would default [`Self::mb_logic`] to FALSE
/// and silently opt every default-constructed core out of Moonbot's own stop rule.
impl Default for ManualStratState {
    fn default() -> Self {
        Self {
            on: false,
            strategy: String::new(),
            id: 0,
            mb_logic: default_true(),
        }
    }
}

/// Core-data reception flags, implemented entirely as a client-side filter.
///
/// IMPORTANT: the core always sends these domain events. A cleared flag means do not read,
/// store, or draw them, saving CPU, database work, and windows, but it does NOT save network
/// traffic because these categories have no server-side opt-out. The order book and tape are
/// not included: they are chart-only and exist only while a window is open.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct FeedFlags {
    /// Open core orders in the bottom dock.
    #[serde(default = "default_true")]
    pub orders: bool,
    /// Detects/watcher rows/chart-only/alert-fire (`DetectEvent`).
    #[serde(default = "default_true")]
    pub detects: bool,
    /// Typed core report replication (`Event::Report` → `orders_rep`) into SQLite.
    #[serde(default = "default_true")]
    pub reports: bool,
    /// Balances and account metadata.
    #[serde(default = "default_true")]
    pub balance: bool,
    /// Strategy state (`Strat`).
    #[serde(default = "default_true")]
    pub strategies: bool,
    /// Server log (`ServerLog`).
    #[serde(default = "default_true")]
    pub log: bool,
    /// Chart alerts and chart text.
    #[serde(default = "default_true")]
    pub alerts: bool,
    /// Arbitrage (`Arb`).
    #[serde(default = "default_true")]
    pub arb: bool,
}

impl Default for FeedFlags {
    /// Defaults to receiving everything, matching behavior before these flags existed.
    fn default() -> Self {
        Self {
            orders: true,
            detects: true,
            reports: true,
            balance: true,
            strategies: true,
            log: true,
            alerts: true,
            arb: true,
        }
    }
}

/// Persisted connection and core-owned behavior for one MoonBot server.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Runtime core id (CoreId). Since schema v11 it EQUALS `uid`, making it stable across
    /// loads and server additions/removals/reordering; it was previously positional. Panels,
    /// data, databases, subscriptions, and layout bind to it within a session.
    pub id: u64,
    /// Stable core identifier that survives renaming and reordering. Metadata from settings.toml
    /// binds through it to a server in servers.enc. 0 means unassigned (older file/new entry)
    /// and is assigned during save.
    #[serde(default)]
    pub uid: u64,
    #[serde(default)]
    pub name: String,
    /// Whether the core is active (Settings checkbox). Inactive cores do not connect.
    #[serde(default = "default_true")]
    pub active: bool,
    /// Whether to draw the core window/chart. Off + active is headless: reports/detects flow
    /// into the database/store without a window. A window appears only for active && show_window.
    #[serde(default = "default_true")]
    pub show_window: bool,
    /// What to accept from the core (client-side filter).
    #[serde(default)]
    pub feed: FeedFlags,
    /// Base64 Moonbot key containing host and port; there are no separate fields for those.
    /// It also names a transport mode, but only as the seed for [`ServerConfig::transport`].
    #[serde(default)]
    pub key: Secret,
    /// Group is the name of the window containing the core. Color/icon belong to GroupConfig.
    #[serde(default = "default_group")]
    pub group: String,
    /// Default market, temporarily used until multiple markets per core are supported.
    #[serde(default = "default_market")]
    pub market: String,
    /// Server color (RGB), later used as the detect color.
    #[serde(default = "default_color")]
    pub color: [u8; 3],
    /// Synthetic benchmark core: the feed runs `synth::run` instead of `live::run` — no network,
    /// so no key either. Turned on locally without editing `servers.enc` through
    /// `MOON_CONFIG_PLAINTEXT=1` + `MOON_CONFIG_PLAINTEXT_SYNTHETIC=1`.
    #[serde(default)]
    pub synthetic: bool,
    /// AddToChart chart-bundle name. Empty follows the global setting
    /// (`charts_split_by_core`: one tab per core or all cores together). With a non-empty name,
    /// cores in the SAME group and bundle combine their AddToChart=N charts into ONE tab whose
    /// title uses the bundle name. Names are local to a group.
    #[serde(default)]
    pub chart_bundle: String,
    /// Default alert strategy (Def Strategy): id of this core's strategy of type "Alerts",
    /// automatically assigned to a new alert when its Alert checkbox is enabled. 0 means none.
    /// This is local terminal config because the protocol does not provide the core default.
    #[serde(default)]
    pub default_alert_strategy: u64,
    /// Whether this core keeps its OWN manual-trading generation ([`Self::trade`]) instead of
    /// sharing its group's. Defaults to `false`, deliberately: turning this on by default would
    /// change the numbers a trader sizes orders from on the first launch after an upgrade, and the
    /// group-local route must stay byte-for-byte unchanged until the user opts in.
    ///
    /// The `alias` reads files written while this flag meant "read the values back OUT of the
    /// core's shared config". That route is gone — the terminal owns these values and delivers
    /// them with the order — but a file carrying the old name still describes a core the user
    /// deliberately separated from its group, which is exactly what this flag means now.
    #[serde(default, alias = "use_core_manual_config")]
    pub own_trade_config: bool,
    /// This core's manual-strategy quick-select slots, or `None` while it still follows the
    /// core's own `manual_strats_names`.
    ///
    /// Deliberately INDEPENDENT of [`Self::own_trade_config`]: which strategy a button fires is a
    /// different question from which sizes and exits the toolbar edits, and a trader who wants
    /// their own buttons must not have to move their TP/SL off the group to get them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strat_slots: Option<[StratSlot; MANUAL_STRAT_SLOTS]>,
    /// This core's manual-strategy mode, or `None` while it has never been set here.
    ///
    /// `None` is what every file written before the terminal owned this mode says, and it is
    /// deliberately distinguishable from `Some(default)`: the first snapshot a core reports seeds
    /// this once from its own `use_manual_strategy`, so an upgrade does not silently drop the
    /// strategy the trader was working with. After that the core is never read for it again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_strategy: Option<ManualStratState>,
    /// This core's own manual-trading generation, used only while [`Self::own_trade_config`] is
    /// on. `None` means the core has never had one; the toggle seeds it from the group so
    /// switching cannot move the numbers under the trader's hands.
    ///
    /// Kept across a toggle-off deliberately: turning the switch back on must restore what the
    /// core had, not the group's current values (that is the whole point of a per-core set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trade: Option<super::groups::GroupTradeSettings>,
    /// MoonProto transport mode to connect with; `None` falls back to what the key encodes.
    ///
    /// See [`TransportVersion`] for why this is stored at all instead of always reading the key.
    #[serde(default)]
    pub transport: Option<TransportVersion>,
}

/// MoonProto transport mode, mirroring MoonBot's `V0 / V1 / V2` radio in Moon Proto settings.
///
/// The mode is ALSO encoded in the exported key, and that is where a core's value comes from the
/// first time its key is read ([`transport_from_key`]). It is stored separately because the two
/// switches move independently after that: MoonBot lets the user change the mode on the core
/// without issuing a new key, and a terminal that could only read the key would force a re-export
/// of every core's key to follow. Both sides simply have to agree on the same number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportVersion {
    V0,
    V1,
    V2,
}

impl TransportVersion {
    /// Every mode, in the order MoonBot lists its radio buttons.
    pub const ALL: [Self; 3] = [Self::V0, Self::V1, Self::V2];

    /// MoonBot's own label for this mode, used verbatim in the UI: it is a protocol number
    /// rather than a word, so it is neither localized nor renamed.
    pub fn label(self) -> &'static str {
        match self {
            Self::V0 => "V0",
            Self::V1 => "V1",
            Self::V2 => "V2",
        }
    }
}

impl From<TransportVersion> for moonproto::TransportMode {
    fn from(v: TransportVersion) -> Self {
        match v {
            TransportVersion::V0 => Self::V0,
            TransportVersion::V1 => Self::V1,
            TransportVersion::V2 => Self::V2,
        }
    }
}

impl From<moonproto::TransportMode> for TransportVersion {
    /// MoonProto's mode is an open `u8` wrapper; anything outside the three named modes cannot
    /// be represented here and reads as `V0`, exactly as `TransportMode::from_byte` does.
    fn from(mode: moonproto::TransportMode) -> Self {
        if mode == moonproto::TransportMode::V1 {
            Self::V1
        } else if mode == moonproto::TransportMode::V2 {
            Self::V2
        } else {
            Self::V0
        }
    }
}

/// Read the transport mode a MoonBot key was exported with.
///
/// Lives beside the key rather than in `feed`, because it answers a question about a stored
/// config value: what this core's key says before anything connects. Legacy exports carry no
/// network block at all, so they return `None` and leave the choice to the user.
///
/// Args:
///     key: Base64 MoonBot key export, as stored in `servers.enc`.
///
/// Returns:
///     The exported mode, or `None` when the key is empty, unparsable, or a legacy export.
pub fn transport_from_key(key: &str) -> Option<TransportVersion> {
    if key.trim().is_empty() {
        return None;
    }
    let info = moonproto::parse_key_info(key)?;
    info.network
        .map(|n| TransportVersion::from(n.transport_mode))
}

/// Seed a core's transport mode from its key, ONCE: a mode already set is never overwritten.
///
/// Deliberately not "whatever the newest key says". The key field emits a change per KEYSTROKE, so
/// a rule that re-reads the key on every edit would let one Backspace-and-retype swap a pinned V1
/// for the key's V0 and reconnect the core on a protocol it does not speak — and a rule comparing
/// the new key against "the previous key" compares against the previous keystroke, which is the
/// same defect wearing a disguise. Nor is the newest key a better authority in the first place:
/// the case this whole field exists for is a core whose switch was moved WITHOUT a new key, so a
/// key that keeps claiming V0 is exactly the stale opinion the user overrode.
///
/// The mode therefore comes from the key while the terminal has no answer of its own — a core
/// being added, or an older config on first load — and belongs to the user from then on. Pointing
/// a row at a different core is the one case that needs a hand: the dropdown beside the field is
/// how it gets one.
///
/// Args:
///     current: Mode stored for this core, or `None` while it has never been set.
///     key: Base64 MoonBot key to read a mode from when there is nothing stored.
///
/// Returns:
///     The stored mode when there is one, otherwise whatever the key names.
pub fn seeded_transport(current: Option<TransportVersion>, key: &str) -> Option<TransportVersion> {
    current.or_else(|| transport_from_key(key))
}

/// User-selected order for every core list in the application.
///
/// The choice is stored globally; the UI crate's `core_order` module performs ranking.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CoreSortMode {
    /// Lexicographic order by lowercase Unicode name, with uid as a tie-breaker.
    #[default]
    Name,
    /// In insertion order, oldest first, using `ServerConfig::uid`.
    AddedOldest,
    /// In insertion order, newest first, using `ServerConfig::uid`.
    AddedNewest,
}

impl CoreSortMode {
    /// Stable code persisted in `settings.toml`.
    ///
    /// `AddedOldest` deliberately serializes as `"added"`: this fixed format value means insertion
    /// order from oldest to newest.
    pub fn code(self) -> &'static str {
        match self {
            CoreSortMode::Name => "name",
            CoreSortMode::AddedOldest => "added",
            CoreSortMode::AddedNewest => "added_newest",
        }
    }

    /// Parse a `settings.toml` code, returning `None` for an unknown code.
    ///
    /// Unknown values are deliberately not approximated to a mode based on their contents;
    /// `Deserialize` conservatively maps them to `Default` (`Name`).
    pub fn from_code(s: &str) -> Option<Self> {
        match s {
            "name" => Some(CoreSortMode::Name),
            "added" => Some(CoreSortMode::AddedOldest),
            "added_newest" => Some(CoreSortMode::AddedNewest),
            _ => None,
        }
    }
}

impl Serialize for CoreSortMode {
    /// Serialize the stable lowercase code used by `settings.toml`.
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.code())
    }
}

impl<'de> Deserialize<'de> for CoreSortMode {
    /// Map an unknown string, i64/u64/f64/bool/unit, sequence, or map to the default.
    ///
    /// These forms are covered explicitly so a cosmetic field cannot reject the remaining
    /// settings. Other data forms use the standard `serde::de::Visitor` rejection.
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        /// Visitor for TOML forms that have a safe default sort mode.
        struct AnyScalar;

        impl<'de> serde::de::Visitor<'de> for AnyScalar {
            type Value = CoreSortMode;

            /// Describe accepted string codes for serde diagnostics.
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a core sort mode (name / added / added_newest)")
            }

            /// Parse a string code, mapping an unknown code to the default.
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(CoreSortMode::from_code(v).unwrap_or_default())
            }

            // Invalid scalar values affect only this cosmetic field.
            /// Map a signed 64-bit integer to the default.
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(CoreSortMode::default())
            }

            /// Map an unsigned 64-bit integer to the default.
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(CoreSortMode::default())
            }

            /// Map a floating-point value to the default.
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(CoreSortMode::default())
            }

            /// Map a Boolean value to the default.
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(CoreSortMode::default())
            }

            /// Map unit/null to the default.
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(CoreSortMode::default())
            }

            // Consume the entire invalid container so deserialization stays synchronized.
            /// Consume an entire sequence and return the default.
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
                Ok(CoreSortMode::default())
            }

            /// Consume an entire map and return the default.
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                while map
                    .next_entry::<serde::de::IgnoredAny, serde::de::IgnoredAny>()?
                    .is_some()
                {}
                Ok(CoreSortMode::default())
            }
        }

        d.deserialize_any(AnyScalar)
    }
}

/// AddToChart tab key within a group, selecting where to combine a core's charts.
/// Resolved by `ServerConfig::chart_bucket` and serialized in charts.json.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ChartBucket {
    /// All cores in the group share one `N-group` tab (global split=off, empty bundle).
    Shared,
    /// Dedicated `N-group-core` tab (global split=on, empty bundle).
    Core(crate::session::CoreId),
    /// Named `N-group-name` bundle for a subset of group cores, overriding the global flag.
    /// The bundle name appears in the tab title.
    Bundle(String),
}

impl ServerConfig {
    /// Selects where to combine this core's AddToChart charts under the current global
    /// `charts_split_by_core` flag. A non-empty bundle overrides the flag.
    pub fn chart_bucket(&self, split: bool) -> ChartBucket {
        if !self.chart_bundle.is_empty() {
            ChartBucket::Bundle(self.chart_bundle.clone())
        } else if split {
            ChartBucket::Core(self.id)
        } else {
            ChartBucket::Shared
        }
    }
}

pub fn default_color() -> [u8; 3] {
    crate::palette::ACCENT
}

pub fn default_group() -> String {
    "default".to_string()
}

pub fn default_market() -> String {
    "BTCUSDT".to_string()
}

pub fn default_true() -> bool {
    true
}

/// Default log-file retention period in days. See SettingsFile::log_retention_days.
pub fn default_log_retention_days() -> u32 {
    14
}

#[cfg(test)]
mod core_sort_parse_tests;

#[cfg(test)]
mod transport_tests;
