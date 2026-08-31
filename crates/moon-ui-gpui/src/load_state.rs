//! Load state of one surface fed by a background replica read, and the
//! placeholder it renders when it has nothing to show.
//!
//! Shared by every panel that reads `reports.sqlite` in the background
//! (Analytics, Report), because they all face the same hazard: a SQLite failure
//! that renders as an empty result is indistinguishable from a period in which
//! nothing happened.
//!
//! A single exhaustive enum prevents contradictory combinations such as stale
//! data, a read failure, and an active load. Renderers therefore share one
//! precedence rule, and a new state must be handled at every match site.

use gpui::{AnyElement, App, IntoElement, ParentElement, SharedString, Styled, div};
use moon_core::db::{FailKind, ReadFail};
use moon_ui::{MoonAlert, MoonPalette, v_flex};
use rust_i18n::t;
use std::sync::Arc;

use crate::design;
use crate::design::moon;

/// State of a surface fed by a background replica read.
pub(crate) enum LoadState<T> {
    /// A request is in flight. `stale` carries the previous result so a
    /// recompute does not flash an empty panel.
    Loading { stale: Option<Arc<T>> },
    /// Replica absent, or the core schema has not arrived yet. A COMPLETED
    /// result, not a pending one — so it must not keep stale numbers alive.
    NotReady,
    /// A completed read containing the current data.
    Ready(Arc<T>),
    /// The read failed. Carries no data on purpose: figures from before a
    /// failure must not sit under a freshly changed period label.
    Failed(ReadFail),
}

impl<T> Default for LoadState<T> {
    /// Begin in a loading state without stale data.
    fn default() -> Self {
        LoadState::Loading { stale: None }
    }
}

impl<T> LoadState<T> {
    /// Data safe to render right now — fresh, or stale while recomputing.
    pub(crate) fn data(&self) -> Option<&Arc<T>> {
        match self {
            LoadState::Ready(v) => Some(v),
            LoadState::Loading { stale } => stale.as_ref(),
            LoadState::NotReady | LoadState::Failed(_) => None,
        }
    }

    /// Mark a new request as started, carrying forward only data still worth
    /// showing. Both completed non-data states drop it.
    pub(crate) fn begin(&mut self) {
        let stale = match std::mem::take(self) {
            LoadState::Ready(v) => Some(v),
            LoadState::Loading { stale } => stale,
            LoadState::NotReady | LoadState::Failed(_) => None,
        };
        *self = LoadState::Loading { stale };
    }

    /// Fold a completed read into the state.
    ///
    /// Args:
    ///     r: Successful payload or classified replica outcome.
    pub(crate) fn apply(&mut self, r: Result<T, ReadFail>) {
        *self = match r {
            Ok(v) => LoadState::Ready(Arc::new(v)),
            Err(ReadFail::NotReady) => LoadState::NotReady,
            Err(e) => LoadState::Failed(e),
        };
    }

    /// What this surface should render: either the data, or the placeholder
    /// standing in for it. `empty` decides whether loaded data counts as empty.
    ///
    /// This fold is total so a caller cannot observe neither data nor a
    /// placeholder for any representable state.
    ///
    /// Args:
    ///     empty: Predicate that classifies a successful payload as empty.
    ///
    /// Returns:
    ///     Renderable data or the exact placeholder state.
    pub(crate) fn view(&self, empty: impl FnOnce(&T) -> bool) -> Result<&Arc<T>, Note> {
        match self {
            LoadState::Loading { stale: None } => Err(Note::Loading),
            LoadState::Loading { stale: Some(v) } | LoadState::Ready(v) => {
                if empty(v) {
                    Err(Note::Empty)
                } else {
                    Ok(v)
                }
            }
            LoadState::NotReady => Err(Note::NotReady),
            LoadState::Failed(ReadFail::IncomparableQuote) => Err(Note::IncomparableQuote),
            LoadState::Failed(ReadFail::PeriodOutOfRange) => Err(Note::PeriodOutOfRange),
            // `apply` routes `NotReady` to its own state, so a `Failed` here
            // always carries a database kind; `Other` is the conservative
            // stand-in because it is the one that promises the user nothing.
            LoadState::Failed(e) => Err(Note::Failed {
                msg: e.to_string().into(),
                kind: e.kind().unwrap_or(FailKind::Other),
            }),
        }
    }
}

/// What a surface says when it has no data to render.
pub(crate) enum Note {
    /// The initial request has not completed and no stale data exists.
    Loading,
    /// The replica or required schema is unavailable.
    NotReady,
    /// Raw-money analytics cannot form one scalar because quote identity is mixed or unknown.
    IncomparableQuote,
    /// The resolved period lies outside the readable range. Distinct from `Empty`: an empty
    /// period says "nothing happened", while this says the request itself was rejected — a
    /// corrupt or absurd persisted bound, not a period with no closed trades in it.
    PeriodOutOfRange,
    /// A read failure with its originating error message. `kind` picks the guidance:
    /// only contention is worth retrying and only corruption is known to be
    /// permanent, so an I/O error must promise neither.
    Failed { msg: SharedString, kind: FailKind },
    /// Genuinely no rows in the selected period — the only one of these states
    /// that the user should read as "nothing happened".
    Empty,
}

/// Render a placeholder. Failures go through `MoonAlert` so they carry the
/// component system's error affordance instead of reading like body text —
/// the whole point is that a read failure must not look like an empty period.
///
/// `Note::Empty` renders the shared wording, and only reaches here from a
/// caller that asked `view` to classify emptiness. A panel that renders its own
/// empty state (the Report table keeps its header chrome and its own text)
/// simply never asks — see `panels::report`.
///
/// Args:
///     id: Stable MoonAlert element identifier.
///     note: Classified placeholder outcome.
///     pad: Unscaled outer padding.
///     p: Active MoonUI palette.
///     cx: Application context used for typography scaling.
///
/// Returns:
///     Muted placeholder or classified database-error alert.
pub(crate) fn note_el(
    id: &'static str,
    note: Note,
    pad: f32,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    // `(title, body, hint)` are chosen TOGETHER, one case at a time, rather than the hint coming
    // from a second match on `FailKind` alone and the title being fixed: `PeriodOutOfRange` needs
    // a hint but carries no `FailKind` at all, and it is NOT a database read failure — titling it
    // "failed to read the reports database" would send the user to repair a database over a
    // period this build simply refuses to read. Picking all three here keeps the single
    // `MoonAlert::error` construction below.
    let (title, body, hint): (String, SharedString, String) = match note {
        Note::Loading => return muted(t!("common.loading").to_string(), pad, p, cx),
        Note::Empty => return muted(t!("common.empty_period").to_string(), pad, p, cx),
        Note::NotReady => return muted(t!("common.db_not_ready").to_string(), pad, p, cx),
        Note::IncomparableQuote => {
            return muted(t!("common.incomparable_quote").to_string(), pad, p, cx);
        }
        Note::PeriodOutOfRange => (
            t!("common.period_out_of_range_title").to_string(),
            SharedString::from(t!("common.period_out_of_range").to_string()),
            t!("common.period_out_of_range_hint").to_string(),
        ),
        // Say only what is true of this failure: corruption requires repair,
        // contention may clear on retry, and I/O errors or misuse promise neither.
        Note::Failed { msg, kind } => {
            let hint = match kind {
                FailKind::Corrupt => t!("common.db_read_failed_corrupt"),
                FailKind::Busy => t!("common.db_read_failed_retry"),
                FailKind::Other => t!("common.db_read_failed_other"),
            };
            (
                t!("common.db_read_failed").to_string(),
                msg,
                hint.to_string(),
            )
        }
    };
    div()
        .p(design::ui_px(cx, pad))
        .child(
            v_flex()
                .gap(design::ui_px(cx, 4.0))
                .child(MoonAlert::error(id, body).title(title).render())
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_muted))
                        .child(hint),
                ),
        )
        .into_any_element()
}

/// Render a non-error placeholder using the shared muted body style.
fn muted(text: String, pad: f32, p: MoonPalette, cx: &App) -> AnyElement {
    div()
        .p(design::ui_px(cx, pad))
        .text_color(moon(p.text_muted))
        .child(text)
        .into_any_element()
}

#[cfg(test)]
mod tests;
