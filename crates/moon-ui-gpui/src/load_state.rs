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

    /// Keep revalidation stale data only for a caller-approved transient failure.
    ///
    /// `NotReady` never qualifies: it is a completed absence, so preserving data would promise a
    /// recovery that no retry can provide.
    pub(crate) fn apply_or_keep(&mut self, r: Result<T, ReadFail>, keep_on_failure: bool) {
        let keeps = keep_on_failure
            && matches!(r, Err(ref e) if !matches!(e, ReadFail::NotReady))
            && matches!(self, LoadState::Loading { stale: Some(_) });
        if keeps {
            if let LoadState::Loading { stale: Some(v) } = self {
                *self = LoadState::Ready(Arc::clone(v));
            }
            return;
        }
        self.apply(r);
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

/// Shared user-facing notice for a classified reports-replica failure.
///
/// Analytics, Report, and the chart overlay all branch on this so a Busy lock,
/// a damaged file, an I/O error, and a lease denial never collapse into one
/// sentence. `Recovery` is the one arm whose wording lives in
/// [`crate::report_notice::recovery_notice_text`] rather than a `common.*` key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DbReadFailedNotice {
    /// Lease/recovery preflight refused this process. Retrying cannot help.
    Recovery,
    /// Malformed database image. Retrying cannot help.
    Corrupt,
    /// Lock contention past `busy_timeout`. Retrying may succeed.
    Busy,
    /// Filesystem or SQLite error. Retrying might succeed; nothing promises it.
    Other,
}

impl DbReadFailedNotice {
    /// Map a database failure kind onto the shared notice.
    ///
    /// Args:
    ///     kind: Classified reports-replica failure.
    ///
    /// Returns:
    ///     The notice every surface uses for that kind.
    pub(crate) fn of(kind: FailKind) -> Self {
        match kind {
            FailKind::ReplicaAccessDenied => Self::Recovery,
            FailKind::Corrupt => Self::Corrupt,
            FailKind::Busy => Self::Busy,
            FailKind::Other => Self::Other,
        }
    }

    /// Whether Retry can still succeed for this notice.
    ///
    /// Args:
    ///     self: Classified notice.
    ///
    /// Returns:
    ///     `true` only for contention and unclassified I/O — the two kinds a
    ///     later read can still clear.
    pub(crate) fn retryable(self) -> bool {
        matches!(self, Self::Busy | Self::Other)
    }
}

/// Whether a later read can still clear this classified failure.
///
/// Args:
///     kind: Classified reports-replica failure.
///
/// Returns:
///     `true` for `Busy` and `Other`; `false` for `Corrupt` and lease denial.
pub(crate) fn db_read_failed_retryable(kind: FailKind) -> bool {
    DbReadFailedNotice::of(kind).retryable()
}

/// Locale key for a classified reports-replica failure, or `None` when the
/// recovery notice composer owns the wording.
///
/// Args:
///     kind: Classified reports-replica failure.
///
/// Returns:
///     The `common.db_read_failed_*` key, or `None` for lease denial.
#[cfg(test)]
pub(crate) fn db_read_failed_hint_key(kind: FailKind) -> Option<&'static str> {
    match DbReadFailedNotice::of(kind) {
        DbReadFailedNotice::Recovery => None,
        DbReadFailedNotice::Corrupt => Some("common.db_read_failed_corrupt"),
        DbReadFailedNotice::Busy => Some("common.db_read_failed_retry"),
        DbReadFailedNotice::Other => Some("common.db_read_failed_other"),
    }
}

/// Localized guidance for a classified reports-replica failure.
///
/// Lease denial uses the shared recovery composer (title plus detail). The
/// other three kinds reuse the `common.db_read_failed_*` strings already shown
/// by Analytics and Report.
///
/// Args:
///     kind: Classified reports-replica failure.
///
/// Returns:
///     Translated badge/hint copy for this kind.
pub(crate) fn db_read_failed_hint(kind: FailKind) -> String {
    match DbReadFailedNotice::of(kind) {
        DbReadFailedNotice::Recovery => {
            let (title, detail) = crate::report_notice::recovery_notice_text(
                moon_core::db::report_recovery::status(),
            );
            format!("{title} {detail}")
        }
        DbReadFailedNotice::Corrupt => t!("common.db_read_failed_corrupt").to_string(),
        DbReadFailedNotice::Busy => t!("common.db_read_failed_retry").to_string(),
        DbReadFailedNotice::Other => t!("common.db_read_failed_other").to_string(),
    }
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
///     Muted placeholder or classified database-error alert. Replica access denial uses the
///     shared recovery wording instead of displaying the internal coordination error.
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
    // period this build simply refuses to read. Access denial has its own shared recovery alert;
    // the remaining failures use the title/body/hint construction below.
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
                FailKind::ReplicaAccessDenied => {
                    let (title, detail) = crate::report_notice::recovery_notice_text(
                        moon_core::db::report_recovery::status(),
                    );
                    return div()
                        .p(design::ui_px(cx, pad))
                        .child(MoonAlert::error(id, detail).title(title).render())
                        .into_any_element();
                }
                FailKind::Corrupt | FailKind::Busy | FailKind::Other => db_read_failed_hint(kind),
            };
            (t!("common.db_read_failed").to_string(), msg, hint)
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
