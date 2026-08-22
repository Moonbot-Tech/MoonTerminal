//! How a caption configuration is written to a file and read back — including the file the
//! PREVIOUS shape wrote.
//!
//! Two decisions live here, and both are about not baking a ceiling into somebody's profile:
//!
//! 1. **Variable length.** In memory a configuration is fixed arrays, because the retained text
//!    runs are addressed by index. On disk it is a LIST: trailing blank rows and unused parts are
//!    not written, and a shorter — or longer — list is read without complaint. Serde's own array
//!    support demands an EXACT length, which is what makes raising a ceiling break every saved
//!    file: `layout.toml` would fall back to its default through `de_lenient_chart_labels`, and
//!    `charts.json` would fail to parse at all, costing the user every chart tab
//!    (`chart_persist::load_all` drops the whole file on a parse error). Neither happens here.
//! 2. **The old shape still loads.** Files written before rows existed hold a flat `slots` list
//!    with an `inline` flag chaining captions into rows. That flag IS the row boundary, so the
//!    migration replays it: a chain becomes one row, each slot becomes a part, and every style the
//!    user set travels with its caption.

use serde::de::{Deserialize, Deserializer};
use serde::ser::{Serialize, Serializer};

use super::{
    ChartLabelField, ChartLabelPart, ChartLabelRow, ChartLabelsCfg, LabelAlign, LabelFlow,
    LabelPreset, LabelStyle, LabelWindow, LabelZone, PnlBasis, CHART_LABEL_PARTS,
    CHART_LABEL_ROWS,
};

/// One row as it appears in a file.
///
/// Field ORDER matters: TOML requires plain values before tables, and `parts` is an array of
/// tables. Keeping it last is what lets the same type serialize into both `layout.toml` and
/// `charts.json`.
/// Read a row's preset, discarding one this build does not know. See [`RowWire::preset`].
fn de_lenient_preset<'de, D>(d: D) -> Result<Option<LabelPreset>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    /// A preset this build knows, or anything else at all — accepted and discarded.
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Shape {
        Known(LabelPreset),
        Other(serde::de::IgnoredAny),
    }

    Ok(match Option::<Shape>::deserialize(d)? {
        Some(Shape::Known(preset)) => Some(preset),
        Some(Shape::Other(_)) | None => None,
    })
}

/// Read a caption's field, dropping one this build does not know.
///
/// The same lenience [`de_lenient_preset`] applies to a row's name, and for a heavier reason: the
/// catalogue of fields both grows AND shrinks — a figure the wire never carried gets retired — and
/// the whole configuration is deserialized as ONE value. A field name this build cannot resolve
/// would take down every caption in the file with it: `layout.toml` falls back to its default
/// through `de_lenient_chart_labels`, and `charts.json` fails outright, which costs the user every
/// chart tab. Losing the ONE caption instead keeps the row, its siblings and all their styling.
///
/// Losing, not blanking: the emptied part reads as unused, so `ChartLabelsCfg::sanitize` — which
/// every read runs — compacts it away and its neighbours close the gap. That is the intended end
/// for a RETIRED field, and the price of it is that an older build opening a newer profile drops a
/// caption it cannot name as soon as that profile is written back. A file that stays readable and
/// one caption shorter beats a file that does not open.
pub(super) fn de_lenient_field<'de, D>(d: D) -> Result<ChartLabelField, D::Error>
where
    D: serde::Deserializer<'de>,
{
    /// A field this build knows, or anything else at all — accepted and emptied.
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Shape {
        Known(ChartLabelField),
        Other(serde::de::IgnoredAny),
    }

    Ok(match Shape::deserialize(d)? {
        Shape::Known(field) => field,
        Shape::Other(_) => ChartLabelField::None,
    })
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct RowWire {
    #[serde(skip_serializing_if = "String::is_empty")]
    name: String,
    /// Ready-made module this row came from, which is how its name survives a language switch.
    /// Absent on a row the user built themselves, and on every file written before presets were
    /// remembered — those carry the localized name they were created with, in `name`.
    ///
    /// Read leniently: the list of presets GROWS, and a profile written by a newer build gets
    /// opened by an older one every time a user steps back a version. This whole configuration is
    /// deserialized as ONE value — inside `layout.toml`, part of a document that also holds every
    /// window position — so an unknown name here would take the reader's entire caption set down
    /// with it. It costs the row its NAME instead: captions, order, band and styling all survive.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "de_lenient_preset")]
    preset: Option<LabelPreset>,
    zone: LabelZone,
    align: LabelAlign,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    show_name: bool,
    /// Absent means DRAWN: every file written before the plate moved from the captions to the
    /// module had a plate under each of them, which is what a module-wide one now reproduces.
    #[serde(skip_serializing_if = "is_true")]
    plate: bool,
    /// Absent means DRAWN: every file written before this switch existed drew its rows.
    #[serde(skip_serializing_if = "is_true")]
    visible: bool,
    /// Absent means the shape every file predating these axes was drawn in: captions across a
    /// line, each module on its own line.
    #[serde(skip_serializing_if = "is_row")]
    flow: LabelFlow,
    #[serde(skip_serializing_if = "is_column")]
    placement: LabelFlow,
    /// Absent means no gap, which is how every file predating it was drawn.
    #[serde(skip_serializing_if = "is_zero")]
    gap: u8,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    parts: Vec<ChartLabelPart>,
}

impl Default for RowWire {
    /// A row absent from a file is a DRAWN row; only the derive would say otherwise.
    fn default() -> Self {
        Self {
            name: String::new(),
            preset: None,
            zone: LabelZone::default(),
            align: LabelAlign::default(),
            show_name: false,
            plate: true,
            visible: true,
            flow: LabelFlow::Row,
            placement: LabelFlow::Column,
            gap: 0,
            parts: Vec::new(),
        }
    }
}

fn is_true(v: &bool) -> bool {
    *v
}

fn is_row(v: &LabelFlow) -> bool {
    v.is_row()
}

fn is_column(v: &LabelFlow) -> bool {
    !v.is_row()
}

fn is_zero(v: &u8) -> bool {
    *v == 0
}

/// One caption as the PREVIOUS shape wrote it: a slot that carried its own band, alignment and
/// chain flag.
#[derive(serde::Deserialize)]
#[serde(default)]
struct LegacySlot {
    #[serde(deserialize_with = "de_lenient_field")]
    field: ChartLabelField,
    zone: LabelZone,
    align: LabelAlign,
    inline: bool,
    visible: bool,
    style: LabelStyle,
    pnl_basis: PnlBasis,
}

impl Default for LegacySlot {
    fn default() -> Self {
        Self {
            field: ChartLabelField::None,
            zone: LabelZone::default(),
            align: LabelAlign::default(),
            inline: false,
            // A slot written before the flag existed was drawn, so absence means visible.
            visible: true,
            style: LabelStyle::default(),
            pnl_basis: PnlBasis::default(),
        }
    }
}

/// Both spellings at once, so one read can tell "no captions were stated" from "this file predates
/// rows".
///
/// `Option` rather than two untagged variants: an untagged enum takes the FIRST arm that parses,
/// and every field of both arms has a default — so the legacy arm would never be reached.
#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct Probe {
    rows: Option<Vec<RowWire>>,
    slots: Option<Vec<LegacySlot>>,
}

impl Serialize for ChartLabelsCfg {
    /// Write the used rows, each with its used parts, and nothing else.
    ///
    /// A file that stated every slot of every row would carry a hundred and twenty-eight tables per
    /// chart tab, of which three are usually filled.
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let rows: Vec<RowWire> = self.rows[..self.used_rows()]
            .iter()
            .map(|row| RowWire {
                name: row.name.clone(),
                preset: row.preset,
                zone: row.zone,
                align: row.align,
                show_name: row.show_name,
                plate: row.plate,
                visible: row.visible,
                flow: row.flow,
                placement: row.placement,
                gap: row.gap,
                parts: row.parts[..row.used_parts()].to_vec(),
            })
            .collect();
        #[derive(serde::Serialize)]
        struct CfgWire {
            rows: Vec<RowWire>,
        }
        CfgWire { rows }.serialize(s)
    }
}

impl<'de> Deserialize<'de> for ChartLabelsCfg {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let probe = Probe::deserialize(d)?;
        if let Some(rows) = probe.rows {
            if probe.slots.is_some() {
                log::warn!("chart_labels: файл несёт и rows, и slots — читаем rows");
            }
            return Ok(from_rows(rows));
        }
        if let Some(slots) = probe.slots {
            return Ok(migrate_slots(slots));
        }
        // Neither spelling present: the table exists but says nothing about captions, which is what
        // an absent setting means everywhere else in these files.
        Ok(Self::default())
    }
}

/// Build a configuration from the rows a file states, taking what fits.
fn from_rows(rows: Vec<RowWire>) -> ChartLabelsCfg {
    let mut cfg = ChartLabelsCfg::empty();
    if rows.len() > CHART_LABEL_ROWS {
        log::warn!(
            "chart_labels: в файле {} строк, берём первые {CHART_LABEL_ROWS}",
            rows.len()
        );
    }
    for (ix, wire) in rows.into_iter().take(CHART_LABEL_ROWS).enumerate() {
        let mut row = ChartLabelRow::new(wire.zone, wire.align);
        row.name = wire.name;
        row.preset = wire.preset;
        row.show_name = wire.show_name;
        row.plate = wire.plate;
        row.visible = wire.visible;
        row.flow = wire.flow;
        row.placement = wire.placement;
        row.gap = wire.gap;
        if wire.parts.len() > CHART_LABEL_PARTS {
            log::warn!(
                "chart_labels: строка {ix} несёт {} подписей, берём первые {CHART_LABEL_PARTS}",
                wire.parts.len()
            );
        }
        for (part_ix, part) in wire.parts.into_iter().take(CHART_LABEL_PARTS).enumerate() {
            row.parts[part_ix] = part;
        }
        cfg.rows[ix] = row;
    }
    cfg.sanitize();
    cfg
}

/// Replay a flat slot list into rows.
///
/// The `inline` flag was the row boundary: a slot carrying it joined the row before it, and one
/// without it opened a new row in its own band. A chain longer than a row can hold — the old shape
/// allowed sixteen — continues into a fresh row in the same band rather than losing captions.
///
/// Only a DRAWN slot could be joined, which is the subtlety this has to reproduce: the old
/// `sanitize` skipped hidden slots when it resolved which row an `inline` caption lands on, so a
/// caption following a hidden one joined the last VISIBLE row and took ITS band. Letting a hidden
/// slot anchor the chain here would relocate that caption to a band the user never saw it in.
fn migrate_slots(slots: Vec<LegacySlot>) -> ChartLabelsCfg {
    let mut cfg = ChartLabelsCfg::empty();
    let mut next = 0usize;
    // Row a chained caption joins: the last one opened by a VISIBLE slot.
    let mut chain: Option<usize> = None;
    for slot in slots {
        if slot.field == ChartLabelField::None {
            continue;
        }
        let visible = slot.visible;
        let part = ChartLabelPart {
            field: slot.field,
            visible,
            style: slot.style,
            pnl_basis: slot.pnl_basis,
            // The old shape had no window: none of the fields it could hold reads one.
            window: LabelWindow::default(),
        };
        // Joining is only possible while such a row exists AND has room; otherwise the caption
        // opens a row of its own, in the band its chain was drawn in.
        let chained = chain.filter(|_| slot.inline);
        if let Some(row) = chained {
            if let Some(ix) = cfg.rows[row].first_free_part() {
                cfg.rows[row].parts[ix] = part;
                continue;
            }
        }
        if next >= CHART_LABEL_ROWS {
            log::warn!("chart_labels: миграция не вместила все подписи, лишние отброшены");
            break;
        }
        let (zone, align) = match chained {
            Some(row) => (cfg.rows[row].zone, cfg.rows[row].align),
            None => (slot.zone, slot.align),
        };
        let mut row = ChartLabelRow::new(zone, align);
        row.parts[0] = part;
        cfg.rows[next] = row;
        // A hidden slot keeps its own row — unhiding it must bring it back where it was — but it
        // never becomes the row that later captions chain onto, because it drew nothing.
        if visible {
            chain = Some(next);
        }
        next += 1;
    }
    cfg.sanitize();
    cfg
}
