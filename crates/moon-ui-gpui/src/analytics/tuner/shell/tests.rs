//! Unit checks for the filter tuner's search-status facts.
//!
//! Explicit imports throughout: the parent re-exports `gpui::*`, whose own `test` shadows the
//! built-in attribute and makes `#[test]` expand recursively.

use super::{SearchStatusView, StatusTone, SuggestWork, status_facts, status_tooltip};
use rust_i18n::t;

/// Hold English for the whole assertion so a parallel locale switch cannot split the two sides.
fn en() -> crate::test_locale::LocaleGuard {
    crate::test_locale::force("en")
}

/// Composition progress keeps the step in the never-clipped head and the option count in the tail.
///
/// Breakage: formatting the whole `compose_progress` line as one truncated string. At the
/// Parameters card's ordinary 470-unit width that clips to `step 2 · option…`, so the user cannot
/// tell which option of how many is running.
#[test]
fn compose_progress_keeps_the_step_and_clips_the_option_count() {
    let _locale = en();
    let facts = status_facts(SearchStatusView::Compose {
        step: 2,
        option: 3,
        total: 12,
    });

    assert_eq!(
        facts.essential,
        vec![t!("analytics.tuner.compose_progress_step", step = 2).to_string()],
        "the step is the one fact the band must still show when the tail clips"
    );
    assert_eq!(
        facts.tail,
        vec![
            t!(
                "analytics.tuner.compose_progress_option",
                done = 3,
                total = 12
            )
            .to_string()
        ],
        "option N of M is the first thing a narrow band may drop"
    );
    assert_eq!(facts.tone, StatusTone::Soft);
    assert_eq!(
        status_tooltip(&facts),
        t!(
            "analytics.tuner.compose_progress",
            step = 2,
            done = 3,
            total = 12
        )
        .to_string(),
        "the tooltip must recover the same full sentence the locales already spell"
    );
}

/// Until a composition publishes a stage, the band says only that it has started.
///
/// Breakage: inventing a `0 of N` denominator so the row always has an option count. How many
/// candidates the run will weigh is decided by what it finds; a number here can only be wrong.
#[test]
fn compose_started_has_no_invented_denominator() {
    let _locale = en();
    let facts = status_facts(SearchStatusView::ComposeStarted);

    assert_eq!(
        facts.essential,
        vec![t!("analytics.tuner.compose_started").to_string()]
    );
    assert!(
        facts.tail.is_empty(),
        "a started composition has no option count to clip"
    );
    assert_eq!(status_tooltip(&facts), facts.essential[0]);
}

/// All-fields progress is one essential fact so both the current restart and the total stay in
/// the never-clipped head.
///
/// Breakage: moving `sugg_progress` into the tail, which lets a narrow band drop `of %{total}`
/// while the user still thinks they can read how far the joint search has got.
#[test]
fn all_fields_progress_stays_in_the_head() {
    let _locale = en();
    let facts = status_facts(SearchStatusView::AllFields {
        done: 4,
        total: 100,
    });

    assert_eq!(
        facts.essential,
        vec![t!("analytics.tuner.sugg_progress", done = 4, total = 100).to_string()]
    );
    assert!(facts.tail.is_empty());
    assert_eq!(status_tooltip(&facts), facts.essential[0]);
}

/// Idle (and the blocking single-field overlay mapped onto it) draws no band.
#[test]
fn idle_has_no_facts() {
    let facts = status_facts(SearchStatusView::Idle);
    assert!(facts.essential.is_empty() && facts.tail.is_empty());
    assert_eq!(status_tooltip(&facts), "");
}

/// A failed read's own words sit in the tail so a long database message clips instead of
/// pushing the search buttons off the row, and the tooltip still carries them.
///
/// The oracle is the fixture detail string, which `status_facts` only prefixes. Breakage:
/// putting the failure in the essential head as one unclipped string, which is how a long
/// `CANTOPEN` line would steal the controls' width.
#[test]
fn a_failed_read_clips_and_the_tooltip_keeps_the_detail() {
    let _locale = en();
    let detail = "reports.sqlite CANTOPEN (12)";
    let facts = status_facts(SearchStatusView::Failed(detail.to_string()));

    assert!(facts.essential.is_empty(), "a failure must yield");
    assert_eq!(facts.tail.len(), 1);
    assert!(
        facts.tail[0].contains(detail),
        "the database's own words must reach the row, got {:?}",
        facts.tail[0]
    );
    assert_eq!(status_tooltip(&facts), facts.tail[0]);
    assert_eq!(facts.tone, StatusTone::Danger);
}

/// Finished captions — done, stopped, too-few-trades — sit in the tail with a tooltip that
/// repeats them, so a long composition summary cannot steal the controls either.
#[test]
fn finished_captions_yield_and_stay_in_the_tooltip() {
    let _locale = en();

    let done = status_facts(SearchStatusView::Done(SuggestWork::Composed {
        ranking_per_candidate: 25,
        completed_units: 400,
    }));
    let expected_done = t!(
        "analytics.tuner.compose_done_units",
        ranking = 25,
        units = 400
    )
    .to_string();
    assert!(done.essential.is_empty());
    assert_eq!(done.tail, vec![expected_done.clone()]);
    assert_eq!(status_tooltip(&done), expected_done);
    assert_eq!(done.tone, StatusTone::Muted);

    let stopped = status_facts(SearchStatusView::Stopped(SuggestWork::Plain {
        completed: 7,
    }));
    let expected_stopped = t!("analytics.tuner.sugg_stopped", rounds = 7).to_string();
    assert!(stopped.essential.is_empty());
    assert_eq!(stopped.tail, vec![expected_stopped.clone()]);
    assert_eq!(status_tooltip(&stopped), expected_stopped);
    assert_eq!(stopped.tone, StatusTone::Warn);

    let empty = status_facts(SearchStatusView::DoneEmpty);
    assert!(empty.essential.is_empty());
    assert_eq!(
        empty.tail,
        vec![t!("analytics.tuner.sugg_small").to_string()]
    );
}
