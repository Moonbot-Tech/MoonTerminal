//! Problems-mode table: what the CORES have concluded about themselves, one row per finding.
//!
//! A live view, not a log: it shows the newest list each core sent. That is why it sits with By-IP
//! and Flat rather than beside Warnings and Updates, which are records of what already happened.
//!
//! "Newest sent" is weaker than "true right now", and the protocol says so: rows are applied in
//! arrival order with no version on them, repeat confirmations of a known finding are deliberately
//! not broadcast, and nothing re-requests the list on a timer. So a row's text, time and count can
//! lag the core's own view until its next full list, and a finding leaves only when a later list
//! omits it. The store clears everything on a replacement connection rather than carrying one
//! MoonBot's findings into another.
//!
//! Deliberately NOT merged into the Warnings list. Both surfaces can describe one incident, but
//! they answer different questions — Warnings is "the terminal measured this against its own
//! threshold and here is the interval", this is "the core concluded this". Folding them together
//! would put an interval column beside rows that have no interval, and would quietly make our
//! thresholds look like the core's findings.

use std::collections::HashMap;
use std::rc::Rc;

use moon_core::feed::{CoreProblem, CoreProblemCategory};
use moon_core::session::CoreId;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonDataCell, MoonDataRow, MoonDataTable,
    MoonDataTableColumn,
};

use super::*;

/// What the panel's scope could and could not tell us, gathered in one value.
///
/// One struct rather than three loose values: they are only ever read together, and every one of
/// them exists to keep the surface from claiming more than it knows.
pub(super) struct ProblemsScope {
    /// How many cores the panel's scope covers at all.
    pub(super) cores: usize,
    /// Display names of the cores that have never delivered a list.
    ///
    /// The NAMES rather than a count, because "two cores never answered" leaves the reader asking
    /// which two — and the answer is the actionable half: those are the ones still to update. The
    /// count is `silent.len()`, so the two can never disagree.
    pub(super) silent: Vec<String>,
    /// Whether the row list was cut by [`PROBLEM_LIST_LIMIT`].
    pub(super) truncated: bool,
    /// Why the per-core actions are or are not available.
    pub(super) actions: ActionGate,
    /// Whether the chosen core has ever delivered a list — the clear has nothing to do otherwise.
    pub(super) answered: bool,
}

/// Whether the two per-core diagnostic actions can be offered, and if not, why.
///
/// A named reason rather than a bare `Option<CoreId>`, because an irreversible action must not be
/// enabled by an ACCIDENT of scope. "The scope happens to hold one core" is not "the operator chose
/// this core": a one-core group under Classic All, or a pinned Auto workspace, both resolve to a
/// single id nobody picked. Each refusal also has its own remedy, and a single greyed button with
/// one generic tooltip cannot say which one applies.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ActionGate {
    /// The operator explicitly selected exactly this core, and it is connected.
    Ready(CoreId),
    /// The selection does not name exactly one core.
    NoSingleChoice,
    /// One core is chosen, but it is not connected: the command channel would accept the command
    /// and hold it until the core returns — destroying findings gathered during the outage.
    NotConnected,
}

impl ActionGate {
    /// The core to act on, or `None` when the gate is closed.
    fn core(self) -> Option<CoreId> {
        match self {
            Self::Ready(core) => Some(core),
            _ => None,
        }
    }

    /// Why the buttons are disabled, or `None` when they are not.
    fn refusal(self) -> Option<String> {
        match self {
            Self::Ready(_) => None,
            Self::NoSingleChoice => Some(t!("core_status.problems_pick_core").to_string()),
            Self::NotConnected => Some(t!("core_status.problems_core_offline").to_string()),
        }
    }
}

/// One row: a finding, plus the core that reported it.
pub(super) struct ProblemRow {
    /// Core that confirmed the finding.
    pub(super) core: CoreId,
    /// The finding itself.
    pub(super) problem: CoreProblem,
}

/// Build the unsortable Problems columns, with the finding's own heading taking the spare width.
///
/// There is deliberately NO body column. The core's `title` and `message` overlap almost entirely —
/// measured on a live fleet, the heading read "На VDS работает Defender или…" beside a body of "На
/// VDS работает Microsoft Defender (MsMpEng.exe(pid=1908)…" — so two columns spent the whole table
/// width on one sentence and truncated both halves of it. The heading gets that width instead, and
/// the body plus the detector's evidence ride in the row's tooltip, where length costs nothing.
///
/// Unsortable for the same reason the sibling logs are: the order is the merge's own — cores in
/// scope order, findings in the order each core listed them — and a column sort would silently
/// replace an ordering the core chose with one the table invented.
///
/// Returns:
///     Ordered column descriptors whose non-resizable heading column absorbs spare table width.
fn columns() -> Vec<MoonDataTableColumn> {
    let mut title = MoonDataTableColumn::new(
        "title",
        t!("core_status.col.problem_title").to_string(),
        260.0,
    )
    .fill();
    title.resizable = false;

    vec![
        MoonDataTableColumn::new("time", t!("core_status.col.time").to_string(), 150.0).no_grow(),
        MoonDataTableColumn::new("core", t!("core_status.col.core").to_string(), 130.0).no_grow(),
        MoonDataTableColumn::new(
            "category",
            t!("core_status.col.problem_category").to_string(),
            110.0,
        )
        .no_grow(),
        MoonDataTableColumn::new(
            "kind",
            t!("core_status.col.problem_kind").to_string(),
            140.0,
        )
        .no_grow(),
        title,
    ]
}

/// The kinds among `rows` belonging to `core`, which is what "looked at" records.
///
/// Identity rather than a timestamp: a kind is either seen or it is not, so nothing here can be
/// poisoned by a clock. See `TabBadgeSettings::seen_kinds` for what the timestamp version got
/// wrong.
///
/// Taken from the ROWS the surface is about to draw, never from the store: the merged list is
/// capped, and marking a finding read that the cap dropped is exactly the lie this whole surface
/// exists to avoid.
///
/// Args:
///     rows: The findings actually being rendered, in display order.
///     core: The core to collect for.
///
/// Returns:
///     That core's kinds, with duplicates left in — the store sorts and dedups.
pub(super) fn drawn_kinds(rows: &[ProblemRow], core: CoreId) -> Vec<u8> {
    rows.iter()
        .filter(|row| row.core == core)
        .map(|row| row.problem.kind)
        .collect()
}

/// A cheap signature of the rows on screen, for the read-mark's early-out.
///
/// Order-sensitive and allocation-free, over the pairs that decide what gets marked. It must move
/// whenever the drawn set does — a finding arriving, one leaving, a core dropping out of scope —
/// because marking also prunes, and a missed prune is a returning finding that never speaks again.
///
/// Args:
///     rows: The findings being rendered, in display order.
///
/// Returns:
///     A hash of every `(core, kind)` pair, in order.
pub(super) fn mark_signature(rows: &[ProblemRow]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    rows.len().hash(&mut hasher);
    for row in rows {
        row.core.hash(&mut hasher);
        row.problem.kind.hash(&mut hasher);
    }
    hasher.finish()
}

/// How many findings the list shows in total, across every core in scope.
///
/// The wire count is a `u16`, so one core can state 65535 rows and every one of them is four heap
/// strings this arm clones. The sibling live lists cap themselves for the same reason
/// (`WARN_LIST_LIMIT`, `UPDATE_LIST_LIMIT`); a diagnostics list past this length is a broken core,
/// not a fleet worth scrolling.
pub(super) const PROBLEM_LIST_LIMIT: usize = 500;

/// Render the Problems surface: the unknown-cores notice, then the findings table.
///
/// The notice is a SIBLING of the table rather than its empty state, and that is the whole point.
/// An empty-state string shows only while the table has no rows, so a fleet with one reporting core
/// and two hundred silent ones would render one row and no hint that anything was unknown — exactly
/// the conflation this feature exists to prevent.
///
/// Args:
///     id: Stable table element identity.
///     rows: Findings in display order, already scoped and capped by the caller.
///     core_names: Core display name per core id.
///     scope: What the scope could and could not report.
///     state: Persisted table interaction state.
///     zone: User-selected display time zone.
///     cx: Panel context.
///
/// Returns:
///     The notice line when one is warranted, above a full-size table host.
pub(super) fn problems_view(
    id: &'static str,
    rows: Rc<Vec<ProblemRow>>,
    core_names: Rc<HashMap<CoreId, String>>,
    scope: &ProblemsScope,
    state: &Entity<MoonDataTableState>,
    zone: chrono_tz::Tz,
    cx: &Context<CoreStatusView>,
) -> impl IntoElement {
    let empty = rows.is_empty();
    let row_count = rows.len();
    let p = MoonPalette::active(cx);

    v_flex()
        .size_full()
        .child(actions(scope, p, cx))
        .children(notice(scope, p, cx))
        .child(crate::panels::common::data_table_host(
            SharedString::from(format!("{id}-host")),
            empty,
            empty_text(scope),
            p,
            cx,
            MoonDataTable::new(id, row_count, move |ix, _window, _app| {
                problem_row(ix, &rows[ix], &core_names, zone)
            })
            .columns(columns())
            .state(state)
            .header_height(design::TABLE_HEAD_H)
            .row_height(design::TABLE_ROW_H)
            .style(design::table_style(p)),
        ))
}

/// The two per-core diagnostic actions, above the notice.
///
/// They ship as a PAIR because the protocol makes them one: a test publishes a `test` fact that
/// stays on the core until something clears it, and clearing is the only thing that does.
///
/// That is also why BOTH are confirmed, not just the destructive one. The test looks harmless and
/// is not: the row it leaves can be removed only by the irreversible clear, so an unconfirmed test
/// press can force an operator to destroy a core's real findings to tidy up after it. The dialog
/// says that in as many words.
///
/// Feedback is a MoonUI notification raised by the action itself, not a line invented here: an
/// action whose failure only reaches the log is an action whose failure nobody sees, and the stack
/// already has the control for saying so.
///
/// Args:
///     scope: What the scope covers, including the gate for these actions.
///     p: Active palette.
///     cx: Panel context.
///
/// Returns:
///     The action row.
fn actions(
    scope: &ProblemsScope,
    p: MoonPalette,
    cx: &Context<CoreStatusView>,
) -> impl IntoElement {
    let gate = scope.actions;
    let refusal = gate.refusal();
    // The clear is additionally pointless on a core that has never reported: there is nothing of
    // ours to drop, and the press would still be irreversible on the core's own pending state.
    let clear_off = gate.core().is_none() || !scope.answered;
    let clear_refusal = match (&refusal, scope.answered) {
        (Some(reason), _) => Some(reason.clone()),
        (None, false) => Some(t!("core_status.problems_clear_nothing").to_string()),
        (None, true) => None,
    };
    let test_view = cx.entity().downgrade();
    let clear_view = cx.entity().downgrade();
    let test_core = gate.core();
    let clear_core = gate.core();

    let test = MoonButton::new("core-status-problems-test")
        .label(t!("core_status.problems_test").to_string())
        .size(MoonButtonSize::Micro)
        .variant(MoonButtonVariant::Panel)
        .disabled(test_core.is_none())
        .on_click(move |_, window, cx| {
            let (Some(view), Some(core)) = (test_view.upgrade(), test_core) else {
                return;
            };
            view.update(cx, |this, cx| this.confirm_problem_test(core, window, cx));
        });
    let clear = MoonButton::new("core-status-problems-clear")
        .label(t!("core_status.problems_clear").to_string())
        .size(MoonButtonSize::Micro)
        .variant(MoonButtonVariant::Panel)
        .disabled(clear_off)
        .on_click(move |_, window, cx| {
            let (Some(view), Some(core)) = (clear_view.upgrade(), clear_core) else {
                return;
            };
            view.update(cx, |this, cx| this.confirm_clear_problems(core, window, cx));
        });

    h_flex()
        .w_full()
        .flex_none()
        .px(design::ui_px(cx, 10.0))
        .py(design::ui_px(cx, 4.0))
        .gap(design::ui_px(cx, 6.0))
        .items_center()
        .justify_end()
        .border_b_1()
        .border_color(rgb(p.border))
        .child(match &refusal {
            Some(reason) => test.tooltip(reason.clone()).render(),
            None => test.render(),
        })
        .child(match &clear_refusal {
            Some(reason) => clear.tooltip(reason.clone()).render(),
            None => clear.render(),
        })
}

/// What the notice must say, decided apart from how it is drawn.
///
/// Split from [`notice`] so the rule can be asserted without a render context: this is the one
/// decision on the surface that must never silently become "everything is fine".
///
/// Args:
///     scope: What the scope could and could not report.
///
/// Returns:
///     The localized caveat, or `None` when the view is telling the whole story.
fn notice_text(scope: &ProblemsScope) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    match (scope.cores, scope.silent.len()) {
        (0, _) => parts.push(t!("core_status.problems_no_cores").to_string()),
        (_, 0) => {}
        (_, n) => parts.push(t!("core_status.problems_unknown", n = n).to_string()),
    }
    // A cut list reads as a complete one unless it says otherwise, which is the same conflation as
    // a silent core reading as a healthy one.
    if scope.truncated {
        parts.push(t!("core_status.problems_truncated", n = PROBLEM_LIST_LIMIT).to_string());
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// The line that states what this view cannot tell you, or nothing when it can tell you everything.
///
/// Args:
///     scope: What the scope could and could not report.
///     p: Active palette.
///     cx: Panel context, for the shared type scale.
///
/// Returns:
///     One notice element, or `None` when every core in scope has answered.
fn notice(
    scope: &ProblemsScope,
    p: MoonPalette,
    cx: &Context<CoreStatusView>,
) -> Option<impl IntoElement> {
    let text = notice_text(scope)?;
    // The names of the silent cores hang off the line rather than sitting in it: on a large fleet
    // the list is longer than the strip, and the count is what has to be readable at a glance. The
    // hover is bounded too — a two-hundred-core fleet would otherwise open a popup taller than the
    // window, and the count in the line already carries the total.
    let silent =
        (!scope.silent.is_empty()).then(|| ellipsize(&scope.silent.join(", "), DETAILS_MAX_CHARS));
    Some(
        h_flex()
            .id("cs-problems-notice")
            // Never the row that gives up height: it is the one line stating what the table cannot
            // show, so a narrow dock must shrink the table instead.
            .flex_none()
            .when_some(silent, |this, names| {
                this.tooltip(crate::panels::common::text_tooltip(names))
            })
            .w_full()
            .px(design::ui_px(cx, 10.0))
            .py(design::ui_px(cx, 4.0))
            .items_center()
            .bg(rgb(p.table_head))
            .font_family(design::mono())
            .text_size(design::t_caption(cx))
            .text_color(rgb(p.text_muted))
            .child(text),
    )
}

/// The empty state, which must never read as "everything is fine" unless it is.
///
/// An empty table has three meanings and only one is a clean bill: every core answered and none had
/// anything to report; some core never answered; or the scope holds no cores at all. The last two
/// are carried by [`notice`] above the table, so this string only has to avoid contradicting it —
/// which a bare "no problems" would.
///
/// Args:
///     scope: What the scope could and could not report.
///
/// Returns:
///     The clean-bill text, or a neutral one deferring to the notice.
fn empty_text(scope: &ProblemsScope) -> String {
    match scope.cores > 0 && scope.silent.is_empty() {
        true => t!("core_status.problems_empty").to_string(),
        false => t!("core_status.problems_nothing_listed").to_string(),
    }
}

/// Render one finding in column order.
///
/// The timestamp is the CONFIRMATION time, not first sight: the row states a fact the core has
/// established, and `first_seen` is when it began suspecting. Falling back to `first_seen` when the
/// core sent no confirmation time keeps a row timed rather than blank.
///
/// Args:
///     ix: Row index, for a stable element id on the tooltip host.
///     row: Finding and its reporting core.
///     core_names: Display names keyed by core id.
///     zone: Selected IANA display zone.
///
/// Returns:
///     Complete problems table row.
fn problem_row(
    ix: usize,
    row: &ProblemRow,
    core_names: &HashMap<CoreId, String>,
    zone: chrono_tz::Tz,
) -> MoonDataRow {
    let core = core_names
        .get(&row.core)
        .cloned()
        .unwrap_or_else(|| "—".to_string());
    // `format_minute` answers an EMPTY string for a time outside chrono's range, so the fallback
    // is driven by the FORMATTED result rather than by `Option`: a garbage `confirmed` would
    // otherwise blank the cell while a perfectly good `first_seen` sat unused.
    let time = minute_text(row.problem.confirmed_ms, zone)
        .or_else(|| minute_text(row.problem.first_seen_ms, zone))
        .unwrap_or_else(|| "—".to_string());
    MoonDataRow::new([
        MoonDataCell::text(time),
        MoonDataCell::text(core),
        MoonDataCell::text(category_label(row.problem.category)),
        // The core's own stable key, shown verbatim. It is the one field in the row that does NOT
        // move with the core's language, which is what makes it the thing to search for and to
        // quote in a bug report.
        MoonDataCell::text(row.problem.kind_name.clone()),
        // The heading is what the column shows; the body and the detector's evidence hang off it.
        // A heading long enough to clip is exactly the case where the hover has to work, so the
        // tooltip repeats nothing and adds what the row could not fit.
        MoonDataCell::element(
            div()
                .id(SharedString::from(format!("cs-problem-{ix}")))
                // Fills the cell rather than the text: the column was widened so the hover could
                // carry the body, and a content-sized host leaves most of that width dead.
                .w_full()
                .child(row.problem.title.clone())
                .when_some(details_text(&row.problem), |this, text| {
                    this.tooltip(crate::panels::common::text_tooltip(text))
                }),
        ),
    ])
}

/// A formatted civil minute, or `None` when the value is absent or outside the printable range.
fn minute_text(ms: Option<i64>, zone: chrono_tz::Tz) -> Option<String> {
    let text = moon_core::util::display_time::format_minute(ms? / 1000, zone);
    (!text.is_empty()).then_some(text)
}

/// Longest hover text kept, in characters.
///
/// The two wire strings behind it are clamped at 2000 characters EACH by the projection, and a
/// tooltip has a fixed width and no scrollbar — so the projection's bound is the wrong one here.
/// This is the bound that keeps the popup inside the window.
const DETAILS_MAX_CHARS: usize = 600;

/// Hover text: the finding's body, plus the detector evidence when the core supplied any.
///
/// The heading is NOT repeated here: the tooltip exists to carry what the cell could not, and a
/// heading that fits would read twice, while one that clips is completed by the body's own opening.
fn details_text(problem: &CoreProblem) -> Option<String> {
    let parts: Vec<&str> = [problem.message.as_str(), problem.technical_details.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect();
    // `None` rather than an empty string: a core that sent neither must not open a blank tooltip
    // popup on a cell that is hoverable regardless.
    (!parts.is_empty()).then(|| ellipsize(&parts.join("\n\n"), DETAILS_MAX_CHARS))
}

/// Cut `text` to `max_chars`, marking that something was cut.
///
/// The marker is the point: a silently shortened detector message reads as the whole message, which
/// is the same class of lie as a silent core reading as a healthy one.
fn ellipsize(text: &str, max_chars: usize) -> String {
    match text.chars().count() > max_chars {
        true => text.chars().take(max_chars).collect::<String>() + "…",
        false => text.to_string(),
    }
}

/// Localized category label.
///
/// `Unknown` prints its raw byte rather than a generic word: a category this build has never seen
/// means the TERMINAL is behind, and a reader who sees `#7` can say so, while "Other" would hide it
/// among the findings the core really did classify as other.
fn category_label(category: CoreProblemCategory) -> String {
    match category {
        CoreProblemCategory::Machine => t!("core_status.problem_cat.machine").to_string(),
        CoreProblemCategory::Exchange => t!("core_status.problem_cat.exchange").to_string(),
        CoreProblemCategory::Network => t!("core_status.problem_cat.network").to_string(),
        CoreProblemCategory::Other => t!("core_status.problem_cat.other").to_string(),
        CoreProblemCategory::Unknown(raw) => format!("#{raw}"),
    }
}

#[cfg(test)]
mod tests;
