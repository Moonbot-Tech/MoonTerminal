//! How an auto-figure configuration is written to a file and read back.
//!
//! Four rules, each about not letting a file cost more than itself:
//!
//! 1. **Only what differs is written.** A row states the fields it changed; a detector states the
//!    parameters that differ from its kind's defaults; the row list itself is left out while it is
//!    the shipped set. A profile therefore FOLLOWS every default a later build improves, and a
//!    value the user did set is never rewritten.
//! 2. **Absent is not empty.** No `rows` key means the shipped set; `rows = []` is a user who
//!    deleted every row, and stays empty.
//! 3. **A newer build's detector survives.** A `kind` this build has no module for is kept as the
//!    raw table and written back as it was read — the way `figures::store` keeps a figure it cannot
//!    parse — so stepping back a version and forward again loses nothing.
//! 4. **One broken row costs one row.** The whole layout is ONE deserialization, and
//!    `layout.toml` holds every window position: a field that cannot be read takes its default, a
//!    row that is not a table at all is dropped with a warning, and nothing here ever fails the
//!    document.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{
    AF_OPACITY_DEFAULT, AutoFigurePreset, AutoFigureRow, AutoFiguresCfg, CaptionPart, CaptionPlace,
    CaptionShow, DetectorKind, DetectorParams, EventMarks, Extend, LineCaption, LineStyle,
    RowDetector, StrengthBasis, Take, TakeOrder, TakeSide, TfSource, UnknownDetector, presets,
};
use crate::config::chart_labels::LabelColor;
use crate::config::layout::de_lenient;
use crate::figures::LineKind;

/// The detector table's tag key — `DetectorParams` is internally tagged by it.
pub(super) const KIND_KEY: &str = "kind";

/// The configuration as READ: every key optional, every value lenient.
#[derive(Deserialize)]
pub(super) struct CfgRead {
    #[serde(default, deserialize_with = "de_lenient")]
    enabled: Option<bool>,
    /// Raw values, parsed one by one, so a row that cannot be read is dropped alone.
    #[serde(default, deserialize_with = "de_lenient")]
    rows: Option<Vec<Value>>,
    #[serde(default, deserialize_with = "de_lenient")]
    opacity: Option<f32>,
    #[serde(default, deserialize_with = "de_lenient")]
    modifier_only: Option<bool>,
}

/// The configuration as WRITTEN.
///
/// `enabled` before `rows`: TOML wants plain values ahead of an array of tables.
#[derive(Serialize)]
pub(super) struct CfgWrite {
    /// Written only when OFF; absent reads as on.
    #[serde(skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
    /// Written only when it is not [`AF_OPACITY_DEFAULT`], so a profile follows the default.
    #[serde(skip_serializing_if = "Option::is_none")]
    opacity: Option<f32>,
    /// Written only when OFF; absent reads as on, so a profile follows the shipped behaviour.
    #[serde(skip_serializing_if = "Option::is_none")]
    modifier_only: Option<bool>,
    /// Written only when it is not the shipped set.
    #[serde(skip_serializing_if = "Option::is_none")]
    rows: Option<Vec<RowWire>>,
}

impl From<CfgRead> for AutoFiguresCfg {
    fn from(wire: CfgRead) -> Self {
        let rows = match wire.rows {
            None => presets::shipped_rows(),
            Some(list) => list
                .into_iter()
                .enumerate()
                .filter_map(|(ix, raw)| match AutoFigureRow::try_from(raw) {
                    Ok(row) => Some(row),
                    Err(err) => {
                        log::warn!("auto_figures: row {ix} dropped: {err}");
                        None
                    }
                })
                .collect(),
        };
        let mut cfg = AutoFiguresCfg {
            enabled: wire.enabled.unwrap_or(true),
            rows,
            opacity: wire.opacity.unwrap_or(AF_OPACITY_DEFAULT),
            modifier_only: wire.modifier_only.unwrap_or(true),
        };
        cfg.sanitize();
        cfg
    }
}

impl From<AutoFiguresCfg> for CfgWrite {
    fn from(cfg: AutoFiguresCfg) -> Self {
        Self {
            enabled: (!cfg.enabled).then_some(false),
            opacity: (cfg.opacity != AF_OPACITY_DEFAULT).then_some(cfg.opacity),
            modifier_only: (!cfg.modifier_only).then_some(false),
            rows: (cfg.rows != presets::shipped_rows())
                .then(|| cfg.rows.into_iter().map(RowWire::from).collect()),
        }
    }
}

/// One row on disk. Every field optional: absent means the default, a value that cannot be read
/// means the default too.
///
/// Plain values first, tables after — the order TOML needs.
#[derive(Serialize, Deserialize)]
pub(super) struct RowWire {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    name: Option<String>,
    /// Read leniently: an older build opening a newer profile loses the row's preset NAME, never
    /// the row.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    preset: Option<AutoFigurePreset>,
    /// Written only when hidden.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    visible: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    tf: Option<TfSource>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    take: Option<TakeWire>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    events: Option<EventsWire>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    style: Option<LineStyle>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    caption: Option<LineCaption>,
    /// Always written, with at least its `kind`: a row without one would be "a swing" by default,
    /// which is a guess about a file, not a reading of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    detector: Option<Value>,
}

impl From<AutoFigureRow> for RowWire {
    fn from(row: AutoFigureRow) -> Self {
        /// `Some(v)` only when `v` differs from the default, which is what the file states.
        fn changed<T: PartialEq>(v: T, default: T) -> Option<T> {
            (v != default).then_some(v)
        }
        Self {
            name: changed(row.name, String::new()),
            preset: row.preset,
            visible: changed(row.visible, true),
            tf: changed(row.tf, TfSource::default()),
            take: changed(row.take, Take::default()).map(TakeWire::from),
            events: changed(row.events, EventMarks::default()).map(EventsWire::from),
            style: changed(row.style, LineStyle::default()),
            caption: changed(row.caption, LineCaption::default()),
            detector: Some(detector_value(&row.detector)),
        }
    }
}

impl TryFrom<Value> for AutoFigureRow {
    type Error = String;

    /// Read one row, failing only when it is not a table or its detector is unreadable as one.
    fn try_from(raw: Value) -> Result<Self, Self::Error> {
        let wire: RowWire = serde_json::from_value(raw).map_err(|e| e.to_string())?;
        let detector = match wire.detector {
            None => RowDetector::default(),
            Some(raw) => read_detector(raw)?,
        };
        Ok(AutoFigureRow {
            name: wire.name.unwrap_or_default(),
            preset: wire.preset,
            visible: wire.visible.unwrap_or(true),
            detector,
            tf: wire.tf.unwrap_or_default(),
            take: wire.take.map(Take::from).unwrap_or_default(),
            style: wire.style.unwrap_or_default(),
            caption: wire.caption.unwrap_or_default(),
            events: wire.events.map(EventMarks::from).unwrap_or_default(),
        })
    }
}

/// The detector table to write: a known one with only its changed parameters, an unknown one as
/// it was read.
///
/// The strip is TOP-LEVEL only. A nested struct (`Squeeze::level`, `broken`) is written whole once
/// any part of it differs: a partial nested table would fill its gaps from the NESTED type's own
/// default on the way back, which is not the enclosing kind's default for it.
fn detector_value(detector: &RowDetector) -> Value {
    match detector {
        RowDetector::Unknown(unknown) => Value::Object(unknown.raw.clone()),
        RowDetector::Known(params) => {
            let mut map = to_map(params);
            let defaults = to_map(&params.kind().default_params());
            map.retain(|k, v| k == KIND_KEY || defaults.get(k) != Some(v));
            let mut value = Value::Object(map);
            // A NaN reaches here as `null` (serde_json's spelling of it), and TOML has no null:
            // left in, it would fail the whole layout's save. Dropped, it reads back as the
            // default — which is what `sanitize` would have made of the NaN anyway.
            drop_nulls(&mut value);
            value
        }
    }
}

/// A known detector's parameters as a JSON map, `kind` included.
fn to_map(params: &DetectorParams) -> Map<String, Value> {
    match serde_json::to_value(params) {
        Ok(Value::Object(map)) => map,
        // Unreachable for an internally tagged enum of plain structs; an empty table would still
        // read back as this kind's defaults rather than fail.
        _ => Map::new(),
    }
}

/// Read a detector table: known kinds into their parameters, unknown kinds kept raw.
fn read_detector(raw: Value) -> Result<RowDetector, String> {
    let Value::Object(mut map) = raw else {
        return Err("detector is not a table".into());
    };
    let Some(kind_name) = map.get(KIND_KEY).and_then(Value::as_str).map(str::to_owned) else {
        return Err("detector names no kind".into());
    };
    let Ok(kind) = serde_json::from_value::<DetectorKind>(Value::String(kind_name.clone())) else {
        // A newer build's detector: kept as read. Nulls cannot come from TOML; one that came from
        // a hand-made JSON copy would make the next TOML save fail, so it is the one thing dropped.
        drop_nulls_in(&mut map);
        return Ok(RowDetector::Unknown(UnknownDetector { raw: map }));
    };
    match serde_json::from_value::<DetectorParams>(Value::Object(map.clone())) {
        Ok(params) => Ok(RowDetector::Known(params)),
        Err(err) => {
            log::warn!("auto_figures: {kind_name} detector repaired field by field: {err}");
            Ok(RowDetector::Known(repair_detector(kind, &map)))
        }
    }
}

/// Rebuild a known kind's parameters from a table one of whose fields does not read, keeping
/// every field that does.
///
/// The kind's defaults are laid down first and each stated field is tried on top of them, one at
/// a time: a field is kept only when the whole still parses. A hand-edited typo then costs that
/// field, not the row's other parameters.
fn repair_detector(kind: DetectorKind, stated: &Map<String, Value>) -> DetectorParams {
    let defaults = kind.default_params();
    let mut base = to_map(&defaults);
    for (key, value) in stated {
        if key == KIND_KEY {
            continue;
        }
        let mut trial = base.clone();
        trial.insert(key.clone(), value.clone());
        if serde_json::from_value::<DetectorParams>(Value::Object(trial.clone())).is_ok() {
            base = trial;
        }
    }
    serde_json::from_value(Value::Object(base)).unwrap_or(defaults)
}

/// Remove every `null`, at any depth: TOML cannot state one.
fn drop_nulls(value: &mut Value) {
    match value {
        Value::Object(map) => drop_nulls_in(map),
        Value::Array(items) => {
            items.retain(|v| !v.is_null());
            items.iter_mut().for_each(drop_nulls);
        }
        _ => {}
    }
}

/// [`drop_nulls`] over a table's values, the table itself included.
fn drop_nulls_in(map: &mut Map<String, Value>) {
    map.retain(|_, v| !v.is_null());
    map.values_mut().for_each(drop_nulls);
}

/// [`LineStyle`] on disk: only the fields that differ from [`LineStyle::default`].
#[derive(Serialize, Deserialize)]
pub(super) struct StyleWire {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    line_kind: Option<LineKind>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    thickness: Option<f32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    extend: Option<Extend>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    zone: Option<f32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    broken_kind: Option<LineKind>,
    /// Last: a `Fixed` colour is a table, and TOML wants tables after plain values.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    color: Option<LabelColor>,
}

impl From<StyleWire> for LineStyle {
    fn from(w: StyleWire) -> Self {
        let d = LineStyle::default();
        Self {
            color: w.color.unwrap_or(d.color),
            line_kind: w.line_kind.unwrap_or(d.line_kind),
            thickness: w.thickness.unwrap_or(d.thickness),
            extend: w.extend.unwrap_or(d.extend),
            // The default is "no zone", so absent and `None` are the same answer.
            zone: w.zone,
            broken_kind: w.broken_kind.unwrap_or(d.broken_kind),
        }
    }
}

impl From<LineStyle> for StyleWire {
    fn from(s: LineStyle) -> Self {
        let d = LineStyle::default();
        Self {
            color: (s.color != d.color).then_some(s.color),
            line_kind: (s.line_kind != d.line_kind).then_some(s.line_kind),
            // Compared bit-for-bit, so a NaN (unequal to everything) is still written rather than
            // silently replaced — `sanitize` is where a NaN is repaired, not the writer.
            thickness: (s.thickness.to_bits() != d.thickness.to_bits()).then_some(s.thickness),
            extend: (s.extend != d.extend).then_some(s.extend),
            zone: s.zone,
            broken_kind: (s.broken_kind != d.broken_kind).then_some(s.broken_kind),
        }
    }
}

/// [`LineCaption`] on disk: only the fields that differ from [`LineCaption::default`].
#[derive(Serialize, Deserialize)]
pub(super) struct CaptionWire {
    /// A part this build does not know is dropped alone; the rest of the list stays.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_caption_parts"
    )]
    parts: Option<Vec<CaptionPart>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    place: Option<CaptionPlace>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    show: Option<CaptionShow>,
}

impl From<CaptionWire> for LineCaption {
    fn from(w: CaptionWire) -> Self {
        let d = LineCaption::default();
        Self {
            parts: w.parts.unwrap_or(d.parts),
            place: w.place.unwrap_or(d.place),
            show: w.show.unwrap_or(d.show),
        }
    }
}

impl From<LineCaption> for CaptionWire {
    fn from(c: LineCaption) -> Self {
        let d = LineCaption::default();
        Self {
            parts: (c.parts != d.parts).then_some(c.parts),
            place: (c.place != d.place).then_some(c.place),
            show: (c.show != d.show).then_some(c.show),
        }
    }
}

/// Read a caption's part list, dropping the parts this build does not know.
///
/// A list that is not a list at all reads as absent — the default parts.
fn de_caption_parts<'de, D>(d: D) -> Result<Option<Vec<CaptionPart>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    /// A part this build knows, or anything else — accepted and dropped.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Part {
        Known(CaptionPart),
        Other(serde::de::IgnoredAny),
    }

    Ok(de_lenient::<D, Vec<Part>>(d)?.map(|parts| {
        parts
            .into_iter()
            .filter_map(|p| match p {
                Part::Known(part) => Some(part),
                Part::Other(_) => None,
            })
            .collect()
    }))
}

/// `Some(v)` when `v` differs from `d`, compared bit for bit so a NaN (unequal to everything) is
/// still written rather than silently replaced — `sanitize` repairs a NaN, the writer does not.
fn diff_f32(v: f32, d: f32) -> Option<f32> {
    (v.to_bits() != d.to_bits()).then_some(v)
}

/// `Some(v)` when `v` differs from `d`.
fn diff<T: PartialEq>(v: T, d: T) -> Option<T> {
    (v != d).then_some(v)
}

/// [`Take`] on disk: only the fields that differ from [`Take::default`], each read on its own.
///
/// Field by field rather than the whole struct, for the same two reasons as [`StyleWire`]: a
/// default a later build improves reaches every row that never changed THAT field, and a value an
/// older build cannot read (a newer `strength` basis) costs that field, not the row's count and
/// side.
#[derive(Serialize, Deserialize)]
pub(super) struct TakeWire {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    count: Option<u8>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    side: Option<TakeSide>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    order: Option<TakeOrder>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    strength: Option<StrengthBasis>,
}

impl From<TakeWire> for Take {
    fn from(w: TakeWire) -> Self {
        let d = Take::default();
        Self {
            count: w.count.unwrap_or(d.count),
            side: w.side.unwrap_or(d.side),
            order: w.order.unwrap_or(d.order),
            strength: w.strength.unwrap_or(d.strength),
        }
    }
}

impl From<Take> for TakeWire {
    fn from(t: Take) -> Self {
        let d = Take::default();
        Self {
            count: diff(t.count, d.count),
            side: diff(t.side, d.side),
            order: diff(t.order, d.order),
            strength: diff(t.strength, d.strength),
        }
    }
}

/// [`EventMarks`] on disk: only the fields that differ from [`EventMarks::default`], each read on
/// its own — see [`TakeWire`] for why.
#[derive(Serialize, Deserialize)]
pub(super) struct EventsWire {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    approach: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    squeeze: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    breakout: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    false_break: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    bounce: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    role_flip: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    stop_hunt: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    confluence: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    approach_pct: Option<f32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    stop_hunt_pct: Option<f32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_lenient"
    )]
    confluence_pct: Option<f32>,
}

impl From<EventsWire> for EventMarks {
    fn from(w: EventsWire) -> Self {
        let d = EventMarks::default();
        Self {
            approach: w.approach.unwrap_or(d.approach),
            squeeze: w.squeeze.unwrap_or(d.squeeze),
            breakout: w.breakout.unwrap_or(d.breakout),
            false_break: w.false_break.unwrap_or(d.false_break),
            bounce: w.bounce.unwrap_or(d.bounce),
            role_flip: w.role_flip.unwrap_or(d.role_flip),
            stop_hunt: w.stop_hunt.unwrap_or(d.stop_hunt),
            confluence: w.confluence.unwrap_or(d.confluence),
            approach_pct: w.approach_pct.unwrap_or(d.approach_pct),
            stop_hunt_pct: w.stop_hunt_pct.unwrap_or(d.stop_hunt_pct),
            confluence_pct: w.confluence_pct.unwrap_or(d.confluence_pct),
        }
    }
}

impl From<EventMarks> for EventsWire {
    fn from(e: EventMarks) -> Self {
        let d = EventMarks::default();
        Self {
            approach: diff(e.approach, d.approach),
            squeeze: diff(e.squeeze, d.squeeze),
            breakout: diff(e.breakout, d.breakout),
            false_break: diff(e.false_break, d.false_break),
            bounce: diff(e.bounce, d.bounce),
            role_flip: diff(e.role_flip, d.role_flip),
            stop_hunt: diff(e.stop_hunt, d.stop_hunt),
            confluence: diff(e.confluence, d.confluence),
            approach_pct: diff_f32(e.approach_pct, d.approach_pct),
            stop_hunt_pct: diff_f32(e.stop_hunt_pct, d.stop_hunt_pct),
            confluence_pct: diff_f32(e.confluence_pct, d.confluence_pct),
        }
    }
}
