//! Saved Telegram message appearance and pure editing operations shared by bot hosts.

use super::telegram_menu::id_serde;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The bot's trade cards and report tables; absent preferences preserve their defaults.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageLayout {
    #[serde(default)]
    pub card: CardLayout,
    #[serde(default)]
    pub report: ReportLayout,
}

/// Ordered card lines and independently controlled name hashtags.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CardLayout {
    #[serde(default = "default_lines")]
    pub lines: Vec<Vec<CardField>>,
    #[serde(default = "yes")]
    pub coin_hashtag: bool,
    #[serde(default)]
    pub core_hashtag: bool,
}

/// Keep the coin hashtag when its switch was not saved.
fn yes() -> bool {
    true
}

/// The existing card's three lines, including its optional volume segment.
fn default_lines() -> Vec<Vec<CardField>> {
    use CardField::*;
    vec![
        vec![Mark, Coin, Profit, Volume, Duration],
        vec![Core],
        vec![Strategy],
    ]
}

impl Default for CardLayout {
    /// Use coin hashtags and plain core names when no layout was saved.
    fn default() -> Self {
        Self {
            lines: default_lines(),
            coin_hashtag: true,
            core_hashtag: false,
        }
    }
}

/// A stable card segment; unfamiliar segments survive saving by older builds.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CardField {
    Mark,
    Coin,
    Profit,
    Volume,
    Duration,
    Core,
    Strategy,
    Prices,
    Other(String),
}

/// A stable report column; unfamiliar columns survive saving by older builds.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ReportColumn {
    Profit,
    Trades,
    Average,
    Volume,
    /// Profit in each core's own quote currency; the total row leaves it empty.
    Native,
    Other(String),
}

/// Implement the open string-id sets without discarding newer saved ids.
macro_rules! open_ids {
    ($ty:ident, $count:literal, $($variant:ident => $id:literal),+ $(,)?) => {
        impl $ty {
            /// Known values in picker order.
            pub const KNOWN: [Self; $count] = [$(Self::$variant),+];
            /// Stable saved id, including ids introduced by newer builds.
            pub fn id(&self) -> &str { match self { $(Self::$variant => $id,)+ Self::Other(id) => id } }
            /// Read a stable id, retaining unfamiliar values unchanged.
            pub fn from_id(id: &str) -> Self { match id { $($id => Self::$variant,)+ id => Self::Other(id.to_owned()) } }
        }
        id_serde!($ty, Self::from_id);
    };
}
open_ids!(CardField, 8, Mark => "mark", Coin => "coin", Profit => "profit", Volume => "volume", Duration => "duration", Core => "core", Strategy => "strategy", Prices => "prices");
open_ids!(ReportColumn, 5, Profit => "profit", Trades => "trades", Average => "average", Volume => "volume", Native => "native");

/// Generate closed appearance choices using the menu's shared tolerant serde.
macro_rules! choices {
    ($ty:ident, $doc:literal, $first:ident => $first_id:literal, $second:ident => $second_id:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
        pub enum $ty {
            #[default]
            $first,
            $second,
        }
        impl $ty {
            /// Every choice in picker order.
            pub const ALL: [Self; 2] = [Self::$first, Self::$second];
            /// Stable saved id.
            pub fn id(self) -> &'static str {
                match self {
                    Self::$first => $first_id,
                    Self::$second => $second_id,
                }
            }
            /// The known choice with this id, if any.
            pub fn from_id(id: &str) -> Option<Self> {
                Self::ALL.into_iter().find(|value| value.id() == id)
            }
        }
        id_serde!($ty);
    };
}
choices!(TotalPlace, "Where the report's total row appears.", Bottom => "bottom", Top => "top");
choices!(TotalSeparation, "Whether a spacer separates the total band from the data rows.", Band => "band", GapBand => "gap_band");
choices!(GroupRowStyle, "How report group headers are drawn.", Band => "band", BoldLeft => "bold_left");

/// Ordered report columns and the appearance of totals and group headers.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReportLayout {
    #[serde(default = "default_columns")]
    pub columns: Vec<ReportColumn>,
    #[serde(default)]
    pub total: TotalPlace,
    #[serde(default)]
    pub separation: TotalSeparation,
    #[serde(default)]
    pub group_row: GroupRowStyle,
}

/// The two report columns shown before layouts became configurable.
fn default_columns() -> Vec<ReportColumn> {
    vec![ReportColumn::Profit, ReportColumn::Trades]
}

impl Default for ReportLayout {
    /// Keep the existing columns and use the total band chosen for the builder.
    fn default() -> Self {
        Self {
            columns: default_columns(),
            total: TotalPlace::Bottom,
            separation: TotalSeparation::Band,
            group_row: GroupRowStyle::Band,
        }
    }
}

impl MessageLayout {
    /// Normalize card lines and report columns with their respective fallback rules.
    /// Cards deduplicate known fields; without any known field, default lines replace all lines.
    /// Reports deduplicate every column id and prepend defaults when no known column remains.
    pub fn sanitized(&self) -> Self {
        Self {
            card: self.card.sanitized(),
            report: self.report.sanitized(),
        }
    }
}

impl CardLayout {
    /// Deduplicate known fields across lines and remove empty lines, keeping unfamiliar fields.
    /// If no known field remains, replace all lines, including unfamiliar fields, with defaults.
    pub fn sanitized(&self) -> Self {
        let mut out = self.clone();
        let mut seen = Vec::new();
        for line in &mut out.lines {
            line.retain(|field| {
                if !CardField::KNOWN.contains(field) {
                    return true;
                }
                if seen.contains(field) {
                    return false;
                }
                seen.push(field.clone());
                true
            });
        }
        out.lines.retain(|line| !line.is_empty());
        if !out
            .lines
            .iter()
            .flatten()
            .any(|field| CardField::KNOWN.contains(field))
        {
            out.lines = default_lines();
        }
        out
    }
}

impl CardLayout {
    /// Core-first card with prices, matching the MoonBot preset.
    pub fn moonbot_preset() -> Self {
        use CardField::*;
        Self {
            lines: vec![
                vec![Core],
                vec![Mark, Coin, Profit],
                vec![Prices],
                vec![Strategy],
            ],
            ..Self::default()
        }
    }
    /// Named constructor for the default coin-first preset.
    pub fn coin_first() -> Self {
        Self::default()
    }
    /// Known fields absent from every line, in picker order.
    pub fn tray(&self) -> Vec<CardField> {
        CardField::KNOWN
            .into_iter()
            .filter(|field| !self.lines.iter().flatten().any(|shown| shown == field))
            .collect()
    }
    /// Move or restore a field into a pre-move line; the end creates a line.
    /// Both indices refer to the original layout; emptied lines disappear after insertion.
    pub fn move_field(&mut self, field: &CardField, to_line: usize, to_index: usize) {
        let to_line = to_line.min(self.lines.len());
        let removed_before = self.lines.get(to_line).map_or(0, |line| {
            line.iter()
                .take(to_index)
                .filter(|shown| *shown == field)
                .count()
        });
        for line in &mut self.lines {
            line.retain(|shown| shown != field);
        }
        if to_line == self.lines.len() {
            self.lines.push(Vec::new());
        }
        let line = &mut self.lines[to_line];
        line.insert(
            to_index.saturating_sub(removed_before).min(line.len()),
            field.clone(),
        );
        self.lines.retain(|line| !line.is_empty());
    }
    /// Move a whole line to its final clamped position; invalid sources do nothing.
    pub fn move_line(&mut self, from: usize, to: usize) {
        if from >= self.lines.len() {
            return;
        }
        let line = self.lines.remove(from);
        self.lines.insert(to.min(self.lines.len()), line);
    }
    /// Whether hiding this field preserves at least one drawable field.
    pub fn can_hide(&self, field: &CardField) -> bool {
        if !self.lines.iter().flatten().any(|shown| shown == field) {
            return false;
        }
        if CardField::KNOWN.contains(field)
            && !self
                .lines
                .iter()
                .flatten()
                .any(|shown| shown != field && CardField::KNOWN.contains(shown))
        {
            return false;
        }
        true
    }
    /// Hide occurrences and empty lines, returning false for an absent or last known field.
    pub fn hide(&mut self, field: &CardField) -> bool {
        if !self.can_hide(field) {
            return false;
        }
        for line in &mut self.lines {
            line.retain(|shown| shown != field);
        }
        self.lines.retain(|line| !line.is_empty());
        true
    }
    /// Preset selection compares lines independently of hashtag switches.
    pub fn is_preset(&self, preset: &CardLayout) -> bool {
        self.lines == preset.lines
    }
}

impl ReportLayout {
    /// Normalize saved columns independently, retaining future ids and a drawable fallback.
    pub fn sanitized(&self) -> Self {
        let mut out = self.clone();
        let mut seen = Vec::new();
        out.columns.retain(|column| {
            if seen.contains(column) {
                return false;
            }
            seen.push(column.clone());
            true
        });
        if !out
            .columns
            .iter()
            .any(|col| ReportColumn::KNOWN.contains(col))
        {
            out.columns.splice(0..0, default_columns());
        }
        out
    }

    /// Iterate known columns in saved order, skipping ids this terminal cannot draw.
    pub fn drawable_columns(&self) -> impl Iterator<Item = &ReportColumn> {
        self.columns
            .iter()
            .filter(|column| ReportColumn::KNOWN.contains(column))
    }

    /// Known columns absent from the report, in picker order.
    pub fn hidden_columns(&self) -> Vec<ReportColumn> {
        ReportColumn::KNOWN
            .into_iter()
            .filter(|col| !self.columns.contains(col))
            .collect()
    }
    /// Move or restore a column to a clamped position, removing repeats first.
    pub fn move_column(&mut self, col: &ReportColumn, to_index: usize) {
        self.columns.retain(|shown| shown != col);
        self.columns
            .insert(to_index.min(self.columns.len()), col.clone());
    }
    /// Whether hiding this column preserves a drawable known column.
    pub fn can_hide_column(&self, col: &ReportColumn) -> bool {
        if !self.columns.contains(col) {
            return false;
        }
        if ReportColumn::KNOWN.contains(col)
            && !self
                .columns
                .iter()
                .any(|shown| shown != col && ReportColumn::KNOWN.contains(shown))
        {
            return false;
        }
        true
    }
    /// Hide a column unless that would remove the last drawable known column.
    /// Returns whether a shown column was removed.
    pub fn hide_column(&mut self, col: &ReportColumn) -> bool {
        if !self.can_hide_column(col) {
            return false;
        }
        self.columns.retain(|shown| shown != col);
        true
    }
    /// Append a hidden column without duplicating one already shown.
    pub fn restore_column(&mut self, col: &ReportColumn) {
        if !self.columns.contains(col) {
            self.columns.push(col.clone());
        }
    }
}

#[cfg(test)]
mod tests;
