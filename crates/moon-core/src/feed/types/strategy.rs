//! Strategy actions, edit lifecycle and schema models.

use super::WalletKind;

/// Engine action accepted or rejected by the core through `Event::EngineAction`.
///
/// This is decoupled from moonproto so the UI formats the toast text itself.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineActionKind {
    CancelAllOrders,
    SetLeverage {
        market: String,
        leverage: i32,
    },
    SetHedgeMode {
        on: bool,
    },
    ChangePositionType {
        market: String,
    },
    ConvertDust,
    ConfirmRiskLimit {
        market: String,
    },
    SetMaMode {
        on: bool,
    },
    TransferAsset {
        asset: String,
        qty: f64,
        from: WalletKind,
        to: WalletKind,
    },
    ReloadOrderBook,
}

/// Result of an asynchronous Engine action on the core.
///
/// A result also arrives on disconnect with `success=false` and a disconnected error, preserving
/// the toast that reports the action was not delivered.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineActionResult {
    pub kind: EngineActionKind,
    pub success: bool,
    /// Exchange or core error code; `0` means no error.
    pub error_code: i32,
    /// Error text, empty on success.
    pub error_msg: String,
}

/// One core strategy for the Strategies window, decoupled from moonproto.
#[derive(Debug, Clone)]
pub struct StrategyRow {
    pub id: u64,
    /// Strategy name from `StrategyName`, or a fallback.
    pub name: String,
    /// Human-readable strategy type or kind.
    pub kind: String,
    /// Kind ordinal used to associate the strategy with its schema sections and fields.
    pub kind_ordinal: u8,
    /// Folder placement preserved verbatim from `StrategySnapshot::path` for the UI to parse.
    pub folder_path: String,
    /// Whether the strategy checkbox is checked; this selection does not prove it is running.
    pub checked: bool,
    pub is_short: bool,
    /// Strategy field values as name-to-formatted-string pairs that populate editable controls.
    pub fields: Vec<(String, String)>,
}

/// Phase of a strategy edit that has not yet reached a terminal outcome.
///
/// Pollable from `strategy_edits()`, unlike a resolution: moonproto keeps a `Pending`/`TimedOut`
/// edit in its map until something else happens to it, so [`StrategyEditSnapshot::open`] can
/// always be rebuilt from scratch and a dropped publish cannot strand a phantom pending marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrategyEditPhase {
    Pending,
    TimedOut,
}

/// Terminal outcome of a strategy edit, reached once and never revisited.
///
/// A separate enum from [`StrategyEditPhase`] rather than one five-arm enum: folding `Confirmed`
/// in there would let `open` hold a row claiming that phase, and that state does not exist —
/// moonproto removes an edit from its map in the same step that resolves it, so a resolution is a
/// one-time fact carried by an event, never a phase a row sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrategyEditResult {
    Confirmed,
    Adjusted,
    Superseded,
}

/// One strategy edit still awaiting a terminal outcome, with desired values formatted for UI consumers.
#[derive(Debug, Clone, PartialEq)]
pub struct StrategyEditRow {
    pub id: u64,
    pub phase: StrategyEditPhase,
    pub submitted_at_ms: i64,
    /// Desired field values formatted through the same `fmt_field` [`StrategyRow::fields`] uses,
    /// so a pending value is string-comparable with a confirmed one. `moon-core` cannot localize
    /// (`rust_i18n::i18n!` is declared in `moon-ui-gpui`), and `moon-ui-gpui` never sees a raw
    /// `StrategySnapshot`, so the desired values must leave this crate already formatted.
    pub fields: Vec<(String, String)>,
}

/// How many adjusted fields a log line or a banner names before it ends with `+N`.
pub const STRATEGY_ADJUSTMENT_PREVIEW: usize = 3;

/// One place the core kept a different value from the snapshot this terminal sent.
///
/// `name` is the schema field, or `checked` / `path` / `kind` when that part of the snapshot
/// differs. `sent` and `saved` are already formatted for display; this crate does not localize,
/// so the UI wraps them with `t!`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrategyFieldChange {
    pub name: String,
    pub sent: String,
    pub saved: String,
}

/// One resolved strategy edit plus, for [`StrategyEditResult::Adjusted`], the fields that differ.
#[derive(Debug, Clone, PartialEq)]
pub struct StrategyEditResolution {
    pub id: u64,
    pub result: StrategyEditResult,
    /// Empty unless `result` is [`StrategyEditResult::Adjusted`].
    pub changes: Vec<StrategyFieldChange>,
}

/// One resolved strategy edit: the core's final verdict on a submission this terminal made.
#[derive(Debug, Clone, PartialEq)]
pub struct StrategyEditNote {
    /// Generated PER CORE, meaningful only within the `CoreData` that produced it. A consumer
    /// carrying one scalar cursor across cores would suppress another core's lower-sequence notes.
    pub seq: u64,
    pub id: u64,
    pub result: StrategyEditResult,
    pub at_ms: i64,
    /// Empty unless `result` is [`StrategyEditResult::Adjusted`]. The UI formats a bounded prefix
    /// of this list; the full list stays here so `+N` counts what was actually different.
    pub changes: Vec<StrategyFieldChange>,
}

/// Strategy-edit state published on its own cadence, faster than the heavy [`StrategyRow`]
/// rebuild, so a button press gets feedback before a user concludes it did nothing.
///
/// `open` is a FULL REPLACE and `resolved` is a batch, travelling in the SAME message: a dropped
/// `open` message costs nothing because the next one is self-healing, but `open` and `resolved`
/// must apply atomically, or a poller observing between them sees either an edit still pending
/// after its own resolution, or a resolution for a row that no longer exists.
#[derive(Debug, Clone, PartialEq)]
pub struct StrategyEditSnapshot {
    /// Absence means resolved: a strategy id with no row here has no open edit.
    pub open: Vec<StrategyEditRow>,
    pub resolved: Vec<StrategyEditResolution>,
}

/// Cap on resolved strategy-edit notes retained per core.
pub const STRATEGY_EDIT_NOTE_CAP: usize = 64;

/// Classification of one pending strategy edit against a core's resolved notes and open rows,
/// returned by [`crate::session::store::CoreData::resolve_strategy_edit`].
///
/// Shared by every caller that watches a submitted edit for its terminal outcome, so the
/// three-way rule -- a resolving note wins, else a `TimedOut` open row, else still pending --
/// lives in exactly one place instead of being reimplemented per caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrategyEditOutcome {
    /// A resolved note for this id was pushed since the caller's cursor.
    Resolved(StrategyEditResult),
    /// No resolving note, but the still-open row reports `TimedOut` -- marked in place on the
    /// row, never carried by a note of its own.
    TimedOut,
    /// Neither a resolving note nor a `TimedOut` row: still awaiting a verdict.
    Pending,
}

/// Schema-field widget kind from moonproto `StrategyFieldUiKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaFieldUi {
    Edit,
    Checkbox,
    Combo,
    Color,
}

/// Description of one strategy-schema field, decoupled from moonproto.
#[derive(Debug, Clone)]
pub struct SchemaField {
    pub name: String,
    /// Type name from the core schema, such as `Bool`, `Int32`, `Double`, or `String`. The UI uses
    /// it to avoid rendering numeric fields as multiline memos (see `is_memo_field`) and to decide
    /// whether typed text is a value this field can hold at all (see
    /// [`field_text_is_valid`](crate::feed::field_text_is_valid)).
    pub type_name: String,
    pub ui: SchemaFieldUi,
    /// Static value list used to populate the field's Combo editor.
    #[allow(dead_code)]
    pub picklist: Vec<String>,
    /// Formatted default value when the schema provides one.
    pub default: Option<String>,
}

/// Field section for one strategy kind, such as main or filters.
#[derive(Debug, Clone)]
pub struct SchemaSection {
    pub title: String,
    pub fields: Vec<SchemaField>,
}

/// Schema for one strategy kind and its sections.
#[derive(Debug, Clone)]
pub struct SchemaKind {
    pub ordinal: u8,
    /// Kind name from the core schema, authoritative over hard-coded `strat_kind_name` and consumed
    /// by strategy creation and kind/filter UI.
    #[allow(dead_code)]
    pub name: String,
    pub sections: Vec<SchemaSection>,
}

/// Complete schema for all core strategy kinds, sent when the schema revision changes.
#[derive(Debug, Clone, Default)]
pub struct StrategySchemaModel {
    pub kinds: Vec<SchemaKind>,
}
