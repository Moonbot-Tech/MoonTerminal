//! Feed-side strategies: moonproto schema decoupling, alert parameters,
//! field-value formatting/parsing, and kind names.

use moonproto::{
    FieldValue, StrategyFieldType, StrategyFieldUiKind, StrategySchema, StrategySnapshot,
};

use super::{SchemaField, SchemaFieldUi, SchemaKind, SchemaSection, StrategySchemaModel};

/// Source-strategy parameters that affect the detect UI.
/// When resolved by [`alert_params`], missing fields default to (false, 60): show the detect
/// button only when SoundAlert=Yes and retain it for KeepAlert seconds.
#[derive(Default)]
pub(super) struct AlertParams {
    pub sound_alert: bool,
    pub keep_alert_secs: u32,
    /// Chart-tab number (0 means do not add).
    pub add_to_chart: u32,
    pub keep_in_chart_secs: u32,
    /// Sound name (WAV stem such as BABYTOY/ding1/…) when set by the strategy. `None` means
    /// no sound. This is extracted by scanning string fields: the sound field's schema name
    /// is unstable, but its VALUE matches the file stem.
    pub sound_name: Option<String>,
}

/// Lowercase stems of embedded sounds, used to recognize a strategy's sound field.
/// Kept here in moon-core because extraction happens in the feed layer; this list mirrors
/// `moon-ui-gpui::sound::SOUNDS`.
const SOUND_STEMS: &[&str] = &[
    "alarm",
    "babytoy",
    "bark",
    "comegetsome",
    "cork",
    "ding1",
    "ding2",
    "error",
    "fatality",
    "gold",
    "hallo",
    "letsrock",
    "milord",
    "pfiff",
    "ringin",
    "ringout",
    "turnon",
    "yes_mast",
];

/// Normalizes a field value to a sound stem by trimming, lowercasing, and stripping the extension.
/// Moonbot stores names as both `BABYTOY` and `BABYTOY.wav`; a non-sound returns None.
fn sound_stem(val: &str) -> Option<String> {
    let low = val.trim().to_ascii_lowercase();
    let stem = low.strip_suffix(".wav").unwrap_or(&low);
    SOUND_STEMS.contains(&stem).then(|| stem.to_string())
}

/// Finds a string value in the strategy fields that matches a sound stem.
fn sound_name_of(s: &StrategySnapshot) -> Option<String> {
    for (_, v) in s.fields.iter() {
        if let FieldValue::String(val) = v {
            if let Some(stem) = sound_stem(val) {
                return Some(stem);
            }
        }
    }
    None
}

/// Detects an explicit `no sound` selection: a sound-related field (SoundKind/…) set to `NONE`.
/// Such a detect stays SILENT and does NOT fall back to the default sound. Requiring `sound` in
/// the field name prevents an unrelated `none` value from muting the sound.
fn sound_is_none(s: &StrategySnapshot) -> bool {
    for (name, v) in s.fields.iter() {
        if let FieldValue::String(val) = v {
            if val.trim().eq_ignore_ascii_case("none")
                && name.to_string().to_ascii_lowercase().contains("sound")
            {
                return true;
            }
        }
    }
    false
}

/// Reads alert defaults `(SoundAlert, sound)` from the SCHEMA for a strategy kind.
/// The server does NOT send fields equal to their schema defaults (as with all other strategy
/// fields), so a strategy using the DEFAULT sound arrives without a sound field and cannot be
/// found by scanning the snapshot. Read it from this kind's schema-field `default_value`s.
fn schema_alert_defaults(
    schema: &StrategySchema,
    s: &StrategySnapshot,
) -> (Option<bool>, Option<String>) {
    let mut sound_alert = None;
    let mut sound = None;
    for sec in schema.editor_sections_for_strategy_kind(s.kind()) {
        for f in &sec.fields {
            match (f.name.as_str(), f.default_value.as_ref()) {
                ("SoundAlert", Some(FieldValue::Bool(b))) => sound_alert = Some(*b),
                (_, Some(FieldValue::String(sv))) => {
                    if sound.is_none() {
                        sound = sound_stem(sv);
                    }
                }
                _ => {}
            }
        }
    }
    (sound_alert, sound)
}

/// Reads an integer strategy field (AddToChart/KeepInChart/KeepAlert), accepting ANY numeric
/// or Boolean moonproto type and returning `default` otherwise.
fn field_secs_or(s: &StrategySnapshot, name: &str, default: u32) -> u32 {
    match s.fields.get(name) {
        Some(FieldValue::Int32(v)) => (*v).max(0) as u32,
        Some(FieldValue::Int64(v)) => (*v).max(0) as u32,
        Some(FieldValue::UInt32(v)) => *v,
        Some(FieldValue::UInt64(v)) => *v as u32,
        Some(FieldValue::Byte(v)) => *v as u32,
        Some(FieldValue::Word(v)) => *v as u32,
        Some(FieldValue::Bool(b)) => *b as u32,
        Some(FieldValue::Double(v)) => v.max(0.0) as u32,
        Some(FieldValue::Single(v)) => v.max(0.0) as u32,
        _ => default,
    }
}

pub(super) fn alert_params(s: &StrategySnapshot, schema: Option<&StrategySchema>) -> AlertParams {
    let (def_sound_alert, def_sound) = schema
        .map(|sc| schema_alert_defaults(sc, s))
        .unwrap_or((None, None));
    // SoundAlert: use the snapshot value when present. Absence means `equal to the schema
    // default` (the server omits such values), so use the schema default.
    let sound_alert = if s.fields.get("SoundAlert").is_some() {
        s.field_bool_or_false("SoundAlert")
    } else {
        def_sound_alert.unwrap_or(false)
    };
    // Play EXACTLY the sound selected by the strategy:
    //  - an explicit stem in the snapshot wins;
    //  - SoundKind=NONE means silence (NOT the default);
    //  - no sound field (= schema default) uses the schema default when SoundAlert is enabled.
    let sound_name = if let Some(n) = sound_name_of(s) {
        Some(n)
    } else if sound_is_none(s) {
        None
    } else if sound_alert {
        def_sound
    } else {
        None
    };
    AlertParams {
        sound_alert,
        keep_alert_secs: field_secs_or(s, "KeepAlert", 60),
        add_to_chart: field_secs_or(s, "AddToChart", 0),
        keep_in_chart_secs: field_secs_or(s, "KeepInChart", 60),
        sound_name,
    }
}

/// Formats a strategy field value for read-only display in badges.
pub(super) fn fmt_field(v: &FieldValue) -> String {
    match v {
        FieldValue::Bool(b) => if *b { "Yes" } else { "No" }.to_string(),
        FieldValue::Int32(n) => n.to_string(),
        FieldValue::Int64(n) => n.to_string(),
        FieldValue::UInt32(n) => n.to_string(),
        FieldValue::UInt64(n) => n.to_string(),
        FieldValue::Byte(n) => n.to_string(),
        FieldValue::Word(n) => n.to_string(),
        FieldValue::Double(d) => crate::util::fmt::compact(*d, 6),
        FieldValue::Single(f) => crate::util::fmt::compact(*f as f64, 6),
        FieldValue::String(s) => s.clone(),
    }
}

/// Builds a `FieldValue` from a UI string according to the field TYPE, prioritizing the existing
/// snapshot value's type, then the schema type, then string. An invalid number becomes 0.
pub(super) fn fv_from_str(
    existing: Option<&FieldValue>,
    stype: Option<StrategyFieldType>,
    s: &str,
) -> FieldValue {
    let b = || {
        matches!(
            s.trim().to_ascii_lowercase().as_str(),
            "yes" | "true" | "1" | "on"
        )
    };
    let i = |def: i64| s.trim().parse::<i64>().unwrap_or(def);
    let u = || s.trim().parse::<u64>().unwrap_or(0);
    let f = || s.trim().parse::<f64>().unwrap_or(0.0);
    // Follow the existing value's type.
    if let Some(ev) = existing {
        return match ev {
            FieldValue::Bool(_) => FieldValue::Bool(b()),
            FieldValue::Int32(_) => FieldValue::Int32(i(0) as i32),
            FieldValue::Int64(_) => FieldValue::Int64(i(0)),
            FieldValue::UInt32(_) => FieldValue::UInt32(u() as u32),
            FieldValue::UInt64(_) => FieldValue::UInt64(u()),
            FieldValue::Byte(_) => FieldValue::Byte(u() as u8),
            FieldValue::Word(_) => FieldValue::Word(u() as u16),
            FieldValue::Double(_) => FieldValue::Double(f()),
            FieldValue::Single(_) => FieldValue::Single(f() as f32),
            FieldValue::String(_) => FieldValue::String(s.to_string()),
        };
    }
    // Follow the schema type.
    match stype {
        Some(StrategyFieldType::Bool) => FieldValue::Bool(b()),
        Some(StrategyFieldType::Int32) => FieldValue::Int32(i(0) as i32),
        Some(StrategyFieldType::Int64) => FieldValue::Int64(i(0)),
        Some(StrategyFieldType::UInt32) => FieldValue::UInt32(u() as u32),
        Some(StrategyFieldType::UInt64) => FieldValue::UInt64(u()),
        Some(StrategyFieldType::Byte) => FieldValue::Byte(u() as u8),
        Some(StrategyFieldType::Word) => FieldValue::Word(u() as u16),
        Some(StrategyFieldType::Double) => FieldValue::Double(f()),
        Some(StrategyFieldType::Single) => FieldValue::Single(f() as f32),
        _ => FieldValue::String(s.to_string()),
    }
}

/// Builds a decoupled model from moonproto `StrategySchema`: each kind contains its editor
/// sections and their fields (name/type/widget kind/picklist/default).
pub(super) fn build_schema_model(schema: &StrategySchema) -> StrategySchemaModel {
    let kinds = schema
        .kinds
        .iter()
        .map(|k| {
            let kind = k.kind();
            let sections = schema
                .editor_sections_for_strategy_kind(kind)
                .into_iter()
                .map(|sec| SchemaSection {
                    title: sec.title,
                    fields: sec
                        .fields
                        .iter()
                        .map(|f| SchemaField {
                            name: f.name.clone(),
                            type_name: f.type_id.name().to_string(),
                            ui: map_ui(f.ui_kind),
                            picklist: f.static_picklist.clone(),
                            default: f.default_value.as_ref().map(fmt_field),
                        })
                        .collect(),
                })
                .collect();
            SchemaKind {
                ordinal: k.ordinal(),
                name: k.name.clone(),
                sections,
            }
        })
        .collect();
    StrategySchemaModel { kinds }
}

fn map_ui(u: StrategyFieldUiKind) -> SchemaFieldUi {
    match u {
        StrategyFieldUiKind::Checkbox => SchemaFieldUi::Checkbox,
        StrategyFieldUiKind::Combo => SchemaFieldUi::Combo,
        StrategyFieldUiKind::Color => SchemaFieldUi::Color,
        _ => SchemaFieldUi::Edit, // Edit + Unknown
    }
}

/// Schema field defaults by kind: kind ordinal → [(name, default)]. This feed-loop cache is
/// rebuilt when the schema revision changes and normalizes strat_db dumps. The server does NOT
/// send fields whose value equals the schema default; without materializing defaults, a field
/// that `disappeared` (= became default) would create phantom versions.
pub(super) fn schema_default_fields(
    schema: &StrategySchema,
) -> std::collections::HashMap<u8, Vec<(String, FieldValue)>> {
    let mut out = std::collections::HashMap::new();
    for k in &schema.kinds {
        let mut defs: Vec<(String, FieldValue)> = Vec::new();
        for sec in schema.editor_sections_for_strategy_kind(k.kind()) {
            for f in &sec.fields {
                if let Some(dv) = f.default_value.as_ref() {
                    defs.push((f.name.clone(), dv.clone()));
                }
            }
        }
        out.insert(k.ordinal(), defs);
    }
    out
}

/// Converts a field value to its JSON representation for strat_db dumps.
fn fv_json(v: &FieldValue) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        FieldValue::Bool(b) => J::from(*b),
        FieldValue::Int32(x) => J::from(*x),
        FieldValue::Int64(x) => J::from(*x),
        FieldValue::UInt32(x) => J::from(*x),
        FieldValue::UInt64(x) => J::from(*x),
        FieldValue::Byte(x) => J::from(*x),
        FieldValue::Word(x) => J::from(*x),
        FieldValue::Double(x) => J::from(*x),
        FieldValue::Single(x) => J::from(*x as f64),
        FieldValue::String(s) => J::from(s.clone()),
    }
}

/// Builds a normalized strategy dump for strat_db: the kind's schema defaults overridden by
/// explicit snapshot fields. `serde_json::Map` keys are sorted (BTreeMap), making serialization
/// canonical and content comparison stable.
pub(super) fn strat_db_dump(
    s: &StrategySnapshot,
    defaults: &std::collections::HashMap<u8, Vec<(String, FieldValue)>>,
    local_edit: bool,
) -> crate::strat_db::StratDump {
    let mut fields = serde_json::Map::new();
    if let Some(defs) = defaults.get(&s.kind().ordinal()) {
        for (n, v) in defs {
            fields.insert(n.clone(), fv_json(v));
        }
    }
    for (n, v) in s.fields.iter() {
        fields.insert(n.to_string(), fv_json(v));
    }
    let name = s
        .strategy_name()
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("strat {}", s.strategy_id));
    crate::strat_db::StratDump {
        // Signed representation: the core writes an order's strategyid as a Delphi signed value.
        strategy_id: s.strategy_id as i64,
        name,
        kind: strat_kind_name(s.kind().ordinal()).to_string(),
        kind_ordinal: s.kind().ordinal(),
        folder_path: s.path.to_string(),
        is_short: s.is_short(),
        checked: s.checked,
        server_ver: s.strategy_ver,
        server_ms: s.last_date as i64,
        fields,
        local_edit,
    }
}

/// Returns the Moonbot strategy type (kind) for a `StrategyKind` ordinal.
pub(super) fn strat_kind_name(ordinal: u8) -> &'static str {
    match ordinal {
        0 => "Unknown",
        1 => "Telegram",
        2 => "Drops",
        3 => "Walls",
        4 => "Volumes",
        5 => "PumpDetection",
        6 => "MoonShot",
        7 => "V Lite",
        8 => "Delta",
        9 => "Waves",
        10 => "Combo",
        11 => "UDP",
        12 => "Manual",
        13 => "MoonStrike",
        14 => "New Listing",
        15 => "Liquidations",
        16 => "TopMarket",
        17 => "EMA",
        18 => "Spread",
        19 => "Chart Wall",
        20 => "MoonHook",
        21 => "Activity",
        22 => "Alerts",
        23 => "Watcher",
        _ => "?",
    }
}

/// Reads a Boolean order-strategy field, falling back to the schema default. The strategy
/// serializer (mirrored by Delphi and moonproto) does NOT transmit fields equal to the schema
/// default, so a missing field means `= default`, not `false`. No strategy snapshot means false.
pub(super) fn strat_field_bool(
    snap: &moonproto::MoonStateSnapshot,
    strat_id: u64,
    name: &str,
) -> bool {
    let Some(s) = snap.strats().snapshot(strat_id) else {
        return false;
    };
    if let Some(v) = s.fields.get_bool(name) {
        return v;
    }
    snap.strats()
        .strategy_schema()
        .and_then(|sc| sc.field(name))
        .and_then(|f| f.default_value.as_ref())
        .is_some_and(|v| matches!(v, FieldValue::Bool(true)))
}

/// Reads a numeric order-strategy field with schema-default fallback; see [`strat_field_bool`].
pub(super) fn strat_field_double(
    snap: &moonproto::MoonStateSnapshot,
    strat_id: u64,
    name: &str,
) -> Option<f64> {
    let s = snap.strats().snapshot(strat_id)?;
    if let Some(v) = s.fields.get_double(name) {
        return Some(v);
    }
    snap.strats()
        .strategy_schema()
        .and_then(|sc| sc.field(name))
        .and_then(|f| f.default_value.as_ref())
        .and_then(|v| match v {
            FieldValue::Double(d) => Some(*d),
            FieldValue::Int32(i) => Some(f64::from(*i)),
            FieldValue::Int64(i) => Some(*i as f64),
            _ => None,
        })
}

/// Resolves an order's effective strategy: its own (`strat_id != 0`) or the core settings'
/// `manual strategy` (`use_manual_strategy` → `manual_strategy_id`), which governs manual MB
/// orders. 0 means no strategy at all (manual-order stops use ClientSettings defaults).
pub(super) fn effective_strat_id(snap: &moonproto::MoonStateSnapshot, strat_id: u64) -> u64 {
    if strat_id != 0 {
        return strat_id;
    }
    snap.settings()
        .client_settings
        .as_ref()
        .filter(|c| c.use_manual_strategy)
        .map(|c| c.manual_strategy_id)
        .unwrap_or(0)
}
