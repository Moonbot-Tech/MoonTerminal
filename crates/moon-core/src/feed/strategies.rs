//! Feed-side strategies: moonproto schema decoupling, alert parameters,
//! field-value formatting/parsing, and kind names.

use moonproto::{
    FieldValue, StrategyFieldType, StrategyFieldUiKind, StrategyFields, StrategySchema,
    StrategySnapshot,
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
    /// The sound the strategy names, as a lowercase WAV stem (`babytoy`, `ding1`, or a user's own
    /// file), or `None` for silence. Taken from the `SoundKind` field, which is what Moonbot calls
    /// it in every snapshot on record; the value is a bare name or `NAME.wav`, and the player, not
    /// this layer, decides whether a file answers to it.
    pub sound_name: Option<String>,
}

/// The strategy field that names the sound. Both the strategy schema and every stored snapshot
/// spell it this way; `SoundAlert` beside it is the flag that lets the sound play at all.
const SOUND_KIND_FIELD: &str = "SoundKind";

/// Moonbot's spelling of "no sound" in `SoundKind`.
const SOUND_NONE: &str = "NONE";

/// Normalizes a `SoundKind` value to a lowercase stem: trimmed, lowercased, `.wav` stripped.
/// Moonbot stores `BABYTOY` and `BABYTOY.wav` for the same file. No list is consulted here —
/// which names exist is the player's knowledge, and a name it lacks is reported there rather than
/// silently dropped on the way.
///
/// Returns `None` for an empty value or the explicit `NONE`.
fn sound_stem(val: &str) -> Option<String> {
    let low = val.trim().to_ascii_lowercase();
    let stem = low.strip_suffix(".wav").unwrap_or(&low);
    (!stem.is_empty() && !stem.eq_ignore_ascii_case(SOUND_NONE)).then(|| stem.to_string())
}

/// The `SoundKind` value a snapshot carries, if any: `Some(Some(stem))` for a named sound,
/// `Some(None)` for an explicit `NONE`, `None` when the field is absent (equal to the schema
/// default, which the server omits).
fn sound_kind_of(s: &StrategySnapshot) -> Option<Option<String>> {
    match s.fields.get(SOUND_KIND_FIELD)? {
        FieldValue::String(val) => Some(sound_stem(val)),
        _ => None,
    }
}

/// Reads alert defaults `(SoundAlert, sound)` from the SCHEMA for a strategy kind.
/// The server does NOT send fields equal to their schema defaults (as with all other strategy
/// fields), so a strategy using the DEFAULT sound arrives without a sound field and cannot be
/// found in the snapshot. Read it from this kind's schema-field `default_value`s.
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
                (SOUND_KIND_FIELD, Some(FieldValue::String(sv))) => sound = sound_stem(sv),
                _ => {}
            }
        }
    }
    (sound_alert, sound)
}

/// An integer out of one field value, accepting ANY numeric or boolean moonproto type.
///
/// Anything else — negative, NaN, past `u32`, a non-numeric type — is `None`, so the caller falls
/// back to its default. Quietly folding a broken number to the EDGE of the range is wrong in both
/// directions here: `0` in these fields is a MEANING ("do not add" / "keep forever"), and a
/// saturated `u32::MAX` in `AddToChart` would open a real tab number 4294967295 and write it to
/// `charts.json`.
fn field_num(v: &FieldValue) -> Option<u32> {
    // `as` on a float saturates silently (NaN becomes 0), so the range is checked here instead.
    // Checking in f64 is required: `u32::MAX as f32` rounds UP, past the bound of the type.
    fn float_num(v: f64) -> Option<u32> {
        (0.0..=u32::MAX as f64).contains(&v).then_some(v as u32)
    }
    let n: i128 = match v {
        FieldValue::Int32(v) => (*v).into(),
        FieldValue::Int64(v) => (*v).into(),
        FieldValue::UInt32(v) => (*v).into(),
        FieldValue::UInt64(v) => (*v).into(),
        FieldValue::Byte(v) => (*v).into(),
        FieldValue::Word(v) => (*v).into(),
        FieldValue::Bool(b) => (*b).into(),
        FieldValue::Double(v) => return float_num(*v),
        FieldValue::Single(v) => return float_num(*v as f64),
        _ => return None,
    };
    u32::try_from(n).ok()
}

/// A numeric strategy field (AddToChart/KeepInChart/KeepAlert): a missing field, or garbage in it,
/// yields `default` — except that a `schema` resolves a MISSING one first.
///
/// The server does not send fields equal to their schema default, and for `KeepInChart` a
/// hardcoded constant then lies: a strategy left at the default `0` — "keep forever" — would
/// arrive as "sixty seconds". No schema, or no such field in it, still yields `default`.
///
/// What matters about `default_value`: moonproto fills it only under its "default non-zero" flag,
/// `FLAG_DEFAULT_NZ` in `commands/strategy_schema.rs`. A field that IS in the schema while its
/// `default_value` is `None` therefore has a default of ZERO — and moonproto's own writer omits a
/// zero value in exactly that case. Without this branch the fix would miss the case it exists for:
/// `KeepInChart = 0` is the schema default, the server omits the field, the lookup is `None`, and
/// we would answer 60 instead of "keep forever".
///
/// What this cannot do: a detect that arrives BEFORE the core has sent its schema has no schema to
/// consult, so an omitted `KeepInChart` still resolves to `default` for that detect. It corrects
/// itself on the next detect for the same market, which pushes the chart's TTL forward again.
///
/// The flat `sc.field()` rather than a walk of the editor sections: the walk filters by strategy
/// kind and would quietly lose the default for a field that kind's editor does not show.
fn field_secs_or(
    s: &StrategySnapshot,
    schema: Option<&StrategySchema>,
    name: &str,
    default: u32,
) -> u32 {
    match s.fields.get(name) {
        // PRESENT settles it. The server sends a field only when it DIFFERS from the schema
        // default, so consulting the schema for a value we merely failed to read would answer with
        // the one number this field's presence has already ruled out — and for `KeepInChart` that
        // number is 0, "keep forever". Unreadable falls back to the caller's `default` instead.
        Some(v) => field_num(v).unwrap_or(default),
        None => schema
            .and_then(|sc| sc.field(name))
            .and_then(|f| match f.default_value.as_ref() {
                // No NZ flag means the schema default IS zero — a value, not "no data".
                None => Some(0),
                // Garbage in the default value falls back to the caller's `default`, not 0.
                Some(v) => field_num(v),
            })
            .unwrap_or(default),
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
    let sound_name = match sound_kind_of(s) {
        Some(chosen) => chosen,
        None if sound_alert => def_sound,
        None => None,
    };
    AlertParams {
        sound_alert,
        // No schema for these two on purpose. The same "the server omits a field equal to its
        // schema default" rule applies to them, but what a zero MEANS there is a separate
        // question: `AddToChart = 0` is "do not add", which the fallback already says, and
        // `KeepAlert` governs the detects feed rather than a chart. Resolving them here would
        // change what a default-configured strategy does on two more surfaces at once.
        keep_alert_secs: field_secs_or(s, None, "KeepAlert", 60),
        add_to_chart: field_secs_or(s, None, "AddToChart", 0),
        // Through the schema: 0 here means keep the chart in the tab INDEFINITELY, Moonbot's
        // meaning, not "zero seconds" — and 0 is the schema default, so the field never arrives.
        keep_in_chart_secs: field_secs_or(s, schema, "KeepInChart", 60),
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

/// Whether `text` typed into a field of schema type `type_name` is a value the core can be sent.
///
/// Answers THROUGH [`fv_from_str`] on purpose, so the panel marks a field rejected by the same
/// rule the sender applies instead of a copy of it that can drift. `type_name` is the name
/// [`SchemaField::type_name`](crate::feed::SchemaField) carries, so the UI needs no protocol type
/// of its own. An unknown type name, like `String`, accepts anything.
///
/// One case still parts them: the sender prefers the type of the value the CORE last sent for that
/// field, while this knows only the schema's type. Where a core disagrees with its own schema the
/// panel can accept text the sender then refuses — the field keeps its value and the log says so,
/// which is why the sender warns rather than trusting this check.
///
/// Empty text counts as rejected: for a single strategy a numeric field always resolves to a value
/// (the schema default, or `0`), so an empty control means the user cleared it, and clearing a
/// number is not an edit the core can carry out. The empty control a MIXED selection renders is the
/// caller's business, not this rule's.
pub fn field_text_is_valid(type_name: &str, text: &str) -> bool {
    // Looked up through `name()` rather than spelled out here: `type_name` was produced by that
    // very function, and a hand-written inverse of it in this repository would answer `true` for
    // every field of a type moonproto renamed — silently turning the check off.
    let Some(stype) = FIELD_TYPES.iter().copied().find(|t| t.name() == type_name) else {
        return true;
    };
    fv_from_str(None, Some(stype), text).is_some()
}

/// Every typed schema field a strategy can carry. `Unknown` is deliberately absent: it names a wire
/// type this build cannot judge, and both this module's callers treat it as free text.
const FIELD_TYPES: [StrategyFieldType; 9] = [
    StrategyFieldType::Bool,
    StrategyFieldType::Int32,
    StrategyFieldType::Int64,
    StrategyFieldType::UInt32,
    StrategyFieldType::UInt64,
    StrategyFieldType::Byte,
    StrategyFieldType::Word,
    StrategyFieldType::Double,
    StrategyFieldType::Single,
];

/// The numeric text behind a UI field: trimmed, with a decimal COMMA rewritten as a dot.
///
/// A Russian keyboard produces "0,5" for half a percent, and `parse` accepts only the dot. This
/// follows the rule every other typed-number path in the terminal already uses
/// (`order_edit::parse_num`, `analytics::tuner::parse_num`, `settings::general`,
/// `shell::core_settings::draft`), deliberately: the same text must not mean one number in the
/// order dialog and another here. A comma reads as the DECIMAL separator, so "1,000" is one, not a
/// thousand — the forms where that is genuinely ambiguous ("1,000.5", "1,2,3") end up unparsable
/// and are refused.
fn num_text(s: &str) -> std::borrow::Cow<'_, str> {
    let trimmed = s.trim();
    if trimmed.contains(',') {
        std::borrow::Cow::Owned(trimmed.replace(',', "."))
    } else {
        std::borrow::Cow::Borrowed(trimmed)
    }
}

/// The schema type that matches a value the core already sent, so both sources of type information
/// can be answered by ONE dispatch below.
fn field_type_of(v: &FieldValue) -> StrategyFieldType {
    match v {
        FieldValue::Bool(_) => StrategyFieldType::Bool,
        FieldValue::Int32(_) => StrategyFieldType::Int32,
        FieldValue::Int64(_) => StrategyFieldType::Int64,
        FieldValue::UInt32(_) => StrategyFieldType::UInt32,
        FieldValue::UInt64(_) => StrategyFieldType::UInt64,
        FieldValue::Byte(_) => StrategyFieldType::Byte,
        FieldValue::Word(_) => StrategyFieldType::Word,
        FieldValue::Double(_) => StrategyFieldType::Double,
        FieldValue::Single(_) => StrategyFieldType::Single,
        FieldValue::String(_) => StrategyFieldType::String,
    }
}

/// Builds a `FieldValue` from a UI string according to the field TYPE, preferring the type of the
/// value the core last sent, then the schema type, then string.
///
/// `None` means the text is NOT a value of that type — the caller must then leave the field alone
/// rather than send something the user never typed. This function used to answer `0` for anything
/// unparsable, and that zero went to the core as a real edit: a comma, a stray `%`, a fraction in
/// an integer field all silently became "no distance", "no stop", "no size", and the core answered
/// with its own default. Out of range is rejected for the same reason, instead of the `as` casts
/// that wrapped 300 into a byte field as 44.
///
/// Bool and String stay total: a checkbox has only two states, and any text is a valid string.
pub(super) fn fv_from_str(
    existing: Option<&FieldValue>,
    stype: Option<StrategyFieldType>,
    s: &str,
) -> Option<FieldValue> {
    let b = || {
        matches!(
            s.trim().to_ascii_lowercase().as_str(),
            "yes" | "true" | "1" | "on"
        )
    };
    let i = || num_text(s).parse::<i64>().ok();
    let u = || num_text(s).parse::<u64>().ok();
    // Two ways a float parse produces a number nobody typed, and both end in the silent zero this
    // function exists to stop: `1e400` becomes `inf` and `1e-400` becomes `0.0`, neither of them an
    // error. So a result of zero is only accepted from text that actually spells zero.
    let f = || {
        num_text(s)
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .filter(|v| *v != 0.0 || !s.bytes().any(|c| c.is_ascii_digit() && c != b'0'))
    };
    // `as f32` underflows a small but perfectly good f64 straight to zero — the same defect one
    // type down, so the cast is checked rather than trusted.
    let single = || {
        f().and_then(|v| {
            let narrowed = v as f32;
            (narrowed.is_finite() && (narrowed != 0.0 || v == 0.0)).then_some(narrowed)
        })
    };
    match existing.map(field_type_of).or(stype) {
        Some(StrategyFieldType::Bool) => Some(FieldValue::Bool(b())),
        Some(StrategyFieldType::Int32) => i()
            .and_then(|v| i32::try_from(v).ok())
            .map(FieldValue::Int32),
        Some(StrategyFieldType::Int64) => i().map(FieldValue::Int64),
        Some(StrategyFieldType::UInt32) => u()
            .and_then(|v| u32::try_from(v).ok())
            .map(FieldValue::UInt32),
        Some(StrategyFieldType::UInt64) => u().map(FieldValue::UInt64),
        Some(StrategyFieldType::Byte) => {
            u().and_then(|v| u8::try_from(v).ok()).map(FieldValue::Byte)
        }
        Some(StrategyFieldType::Word) => u()
            .and_then(|v| u16::try_from(v).ok())
            .map(FieldValue::Word),
        Some(StrategyFieldType::Double) => f().map(FieldValue::Double),
        Some(StrategyFieldType::Single) => single().map(FieldValue::Single),
        _ => Some(FieldValue::String(s.to_string())),
    }
}

/// Convert `(name, text)` pairs into strategy fields, dropping any the core could not be sent.
///
/// Shared by the create and restore paths, which differ only in what they call the strategy in the
/// log. An unparsable field is OMITTED rather than zeroed: the core then gives it the default it
/// stands behind, which for a field whose schema carries no non-zero default is the same zero the
/// writer would have skipped anyway. Empty text is that very case rather than a defect —
/// `ops::default_fields` spells "no schema default" as an empty string — so it passes silently,
/// while text that means something and cannot be read does say so.
pub(super) fn fields_from_text(
    schema: Option<&StrategySchema>,
    pairs: &[(String, String)],
    server_id: u64,
    what: &str,
) -> StrategyFields {
    let mut fields = StrategyFields::new();
    for (name, val) in pairs {
        let stype = schema.and_then(|s| s.field(name)).map(|f| f.type_id);
        match fv_from_str(None, stype, val) {
            Some(value) => {
                fields.insert(name.as_str(), value);
            }
            None if !val.trim().is_empty() => log::warn!(
                "core {} {what}: field {name} omitted, {val:?} is not a value of its type",
                super::core_label(server_id)
            ),
            None => {}
        }
    }
    fields
}

/// Builds a decoupled model from moonproto `StrategySchema`: each kind contains its editor
/// sections and their fields (name/type/widget kind/picklist/default).
pub(super) fn build_schema_model(schema: &StrategySchema) -> StrategySchemaModel {
    // One line per schema arrival: what the core supplies as its own list for the sound field.
    // The terminal's editor shows that list plus every sound of its own the list lacks, so "a
    // sound is not in the dropdown" has two possible causes — the core's list and the terminal's
    // folder scan — and this line settles the first.
    let sound_list: Vec<&str> = schema
        .fields
        .iter()
        .filter(|f| f.name == SOUND_KIND_FIELD)
        .flat_map(|f| f.static_picklist.iter().map(String::as_str))
        .collect();
    log::info!(
        "strategy schema: {} kinds, {} fields, {SOUND_KIND_FIELD} picklist from the core: {sound_list:?}",
        schema.kinds.len(),
        schema.fields.len()
    );
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
    let name = strat_display_name(s);
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

/// Returns the user-visible name of a strategy, or `strat <id>` when the core sent none.
///
/// The serializer does NOT transmit a field equal to its schema default, so an unnamed strategy
/// arrives with no `StrategyName` at all; an explicitly emptied name arrives as `""`. Both are the
/// same thing to a reader, so both take the identifier fallback — a strategy always has an id, and
/// a blank label in a table or on a detect card names nothing.
pub(super) fn strat_display_name(s: &StrategySnapshot) -> String {
    s.strategy_name()
        .filter(|n| !n.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| strat_id_name(s.strategy_id))
}

/// Names a strategy by the one thing it always has.
fn strat_id_name(strategy_id: u64) -> String {
    format!("strat {strategy_id}")
}

/// Returns the name to carry on a detect: [`strat_display_name`] on one line and bounded, or empty
/// when NO strategy produced the detect.
///
/// Empty therefore never means "a strategy that nobody named" — such a strategy comes back as
/// `strat <id>`, exactly as the Strategies window and the report database name it. It means the
/// snapshot behind this detect is absent: an alert firing, which is a drawn chart object and has no
/// strategy at all, or a detect that beat its core's strategy set. The two are one case for a
/// reader, because the second carries no sound and no TTL either, so nothing but an alert becomes a
/// card; a chart caption, which reads every row, prints nothing for both.
///
/// The name is core-supplied text of unbounded length that ends up in a 2000-row-per-core ring and
/// on a chart caption, so it takes the same treatment as the detect's own line beside it: control
/// characters become spaces — a name is drawn on ONE line, and fusing the words around a newline
/// would rename it — invisible format characters are dropped, and the result is cut to
/// [`crate::feed::DETECT_STRAT_NAME_KEEP`].
pub(super) fn detect_strat_name(s: Option<&StrategySnapshot>) -> String {
    let Some(s) = s else {
        return String::new();
    };
    // Same sanitizer the venue captions use, for the same reason: a name of nothing but bidi marks
    // must not count as a name and then draw as one. Control characters go too — this is printed on
    // ONE line.
    let flattened: String = s
        .strategy_name()
        .unwrap_or_default()
        .chars()
        .filter(|c| !crate::venue::is_invisible_format(*c))
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    // Cut only AFTER trimming, then tidy the new tail: cutting first lets a name padded with
    // leading blanks come back empty, which is the one answer this must never give for a strategy
    // that exists. Anything left with nothing to show falls back to the identifier, exactly as
    // `strat_display_name` does for the same strategy elsewhere.
    let bounded: String = flattened
        .trim()
        .chars()
        .take(crate::feed::DETECT_STRAT_NAME_KEEP)
        .collect();
    match bounded.trim_end() {
        "" => strat_id_name(s.strategy_id),
        name => name.to_string(),
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

#[cfg(test)]
mod tests;
