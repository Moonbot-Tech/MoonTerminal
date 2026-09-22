//! Unit checks for the filter tuner's search-status facts.
//!
//! Explicit imports throughout: the parent re-exports `gpui::*`, whose own `test` shadows the
//! built-in attribute and makes `#[test]` expand recursively.

use super::super::filter::state::{SearchSplit, SuggestJob, SuggestState};
use super::{
    SearchStatusView, StatusTone, SuggestWork, note_axis_move, search_status_view, status_facts,
    status_tooltip,
};
use moon_core::db::metrics::Tally;
use moon_core::db::tuner::threshold_search::SearchHandle;
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

/// `shell.rs:note_axis_move` appends the axis caption only on a finished marked run.
///
/// Breakage: dropping the `SuggestState::Done` match, so a finished composition never says the
/// report time axis shifted, or appending the note while the search is still running, where it
/// fights the progress caption for the fixed-width band. With the mark clear, the facts the
/// band already built must stay byte for byte the same.
#[test]
fn a_result_fitted_across_an_axis_move_says_so_and_warns() {
    let _locale = en();
    let note = t!("analytics.tuner.sugg_axis_moved").to_string();
    let work = SuggestWork::Plain { completed: 4 };

    let mut marked = status_facts(SearchStatusView::Done(work));
    let kept_head = marked.tail.clone();
    assert!(
        !kept_head.is_empty(),
        "precondition: a finished caption already occupies the tail"
    );
    note_axis_move(&mut marked, &done_state(false, true, true));
    assert_eq!(marked.tone, StatusTone::Warn);
    assert_eq!(marked.tail.len(), kept_head.len() + 1);
    assert_eq!(&marked.tail[..kept_head.len()], kept_head.as_slice());
    assert_eq!(marked.tail.last().map(String::as_str), Some(note.as_str()));

    let mut unmarked = status_facts(SearchStatusView::Done(work));
    let essential = unmarked.essential.clone();
    let tail = unmarked.tail.clone();
    let tone = unmarked.tone;
    note_axis_move(&mut unmarked, &done_state(false, false, true));
    assert_eq!(unmarked.essential, essential);
    assert_eq!(unmarked.tail, tail);
    assert_eq!(unmarked.tone, tone);

    let mut running_facts = status_facts(SearchStatusView::ComposeStarted);
    let running_tail = running_facts.tail.clone();
    let running_tone = running_facts.tone;
    let running = SuggestState::Running(SuggestJob::Compose {
        handle: SearchHandle::new(),
        axis_moved: true,
    });
    note_axis_move(&mut running_facts, &running);
    assert_eq!(running_facts.tail, running_tail);
    assert_eq!(running_facts.tone, running_tone);
}

/// A run the user stopped already warns. The axis caption must not be what flips that colour.
///
/// Accepted trade, not a defect: `SearchStatusView::Stopped` in `status_facts` already sets
/// `StatusTone::Warn`, so `note_axis_move` is a no-op on the tone when `Done` is both
/// `stopped` and `axis_moved`. The two cases share a tone and differ only by the extra tail
/// entry. A finished run that was not stopped does move `Muted` to `Warn`, and an empty
/// finished run moves `Soft` to `Warn`, which is why the stopped case can stay colour-stable.
#[test]
fn a_stopped_run_warns_either_way_and_only_the_caption_changes() {
    let _locale = en();
    let note = t!("analytics.tuner.sugg_axis_moved").to_string();
    let (essential_still, tail_still, tone_still) = captioned(&done_state(true, false, false));
    let (essential_moved, tail_moved, tone_moved) = captioned(&done_state(true, true, false));
    assert_eq!(tone_still, StatusTone::Warn);
    assert_eq!(tone_moved, tone_still);
    assert_eq!(essential_moved, essential_still);
    let mut expected = tail_still.clone();
    expected.push(note.clone());
    assert_eq!(tail_moved, expected);

    let (_, _, fitted_tone) = captioned(&done_state(false, false, true));
    assert_eq!(fitted_tone, StatusTone::Muted);
    let (_, fitted_tail, fitted_moved_tone) = captioned(&done_state(false, true, true));
    assert_eq!(fitted_moved_tone, StatusTone::Warn);
    assert_eq!(fitted_tail.last().map(String::as_str), Some(note.as_str()));

    let (_, _, empty_tone) = captioned(&done_state(false, false, false));
    assert_eq!(empty_tone, StatusTone::Soft);
    let (_, empty_tail, empty_moved_tone) = captioned(&done_state(false, true, false));
    assert_eq!(empty_moved_tone, StatusTone::Warn);
    assert_eq!(empty_tail.last().map(String::as_str), Some(note.as_str()));
}

/// `locales/analytics.yml:analytics.tuner.sugg_axis_moved` must be a real string in ru, en, and es.
///
/// Breakage: deleting one language. `rust_i18n` echoes the missing key, so the status band would
/// show `analytics.tuner.sugg_axis_moved` instead of the axis-shift caption.
#[test]
fn sugg_axis_moved_is_translated_for_every_shipped_locale() {
    for code in ["ru", "en", "es"] {
        let _locale = crate::test_locale::force(code);
        let text = t!("analytics.tuner.sugg_axis_moved").to_string();
        assert_ne!(
            text, "analytics.tuner.sugg_axis_moved",
            "{code} must not echo the key"
        );
        assert!(!text.is_empty(), "{code} caption must not be empty");
    }
}

fn done_state(stopped: bool, axis_moved: bool, with_split: bool) -> SuggestState {
    SuggestState::Done {
        work: SuggestWork::Plain { completed: 4 },
        stopped,
        split: with_split.then(|| SearchSplit {
            train: Tally::default(),
            holdout: None,
            composed: None,
            compose_skipped: None,
        }),
        axis_moved,
    }
}

fn captioned(sugg: &SuggestState) -> (Vec<String>, Vec<String>, StatusTone) {
    let mut facts = status_facts(search_status_view(sugg));
    note_axis_move(&mut facts, sugg);
    (facts.essential, facts.tail, facts.tone)
}
