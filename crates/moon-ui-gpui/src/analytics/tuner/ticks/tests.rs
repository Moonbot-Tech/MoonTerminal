use std::time::Duration;

use super::fetch_wait_text;

/// Localizing the variant marker or reverting the current heading makes the grid disagree with KPI.
#[test]
fn livefix_tick_headings_match_the_shared_vocabulary() {
    for language in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        assert_eq!(
            rust_i18n::t!("analytics.ticks.pick.col_now"),
            rust_i18n::t!("analytics.time.cur")
        );
        assert_eq!(rust_i18n::t!("analytics.ticks.pick.col_v1"), "v1");
    }
}

/// Removing the read gate would turn pending coverage into settled zero or missing counts.
#[test]
fn coverage_chips_wait_for_the_shown_scope_including_all_strategies() {
    let _locale = crate::test_locale::force("en");
    use super::why::DealChip;
    for total in [462, 8469] {
        let counts = super::rows::ChipCounts {
            rows_rev: 1,
            min_tail_s: 60,
            counts: [0, total, 0, 0, 0, 0, 0, 0],
        };
        for chip in DealChip::ALL {
            assert!(super::chip_label(counts, chip, true).ends_with(": …"));
        }
        assert_eq!(
            super::chip_label(counts, DealChip::WithTape, false),
            "with tape: 0"
        );
        assert_eq!(
            super::chip_label(counts, DealChip::NoTape, false),
            format!("without tape: {total}")
        );
        assert_eq!(super::chip_label(counts, DealChip::Fit, false), "usable: 0");
        let answered = super::rows::ChipCounts {
            counts: [400, total - 400, 357, 0, 0, 0, 0, 0],
            ..counts
        };
        assert_eq!(
            super::chip_label(answered, DealChip::Fit, false),
            "usable: 357"
        );
        assert_eq!(
            super::chip_label(answered, DealChip::Fit, true),
            "usable: …"
        );
    }
}

/// A pending read must not claim no tape, even when a reload carries old reproduction shares.
#[gpui::test]
fn reproduction_note_waits_then_restores_the_settled_explanation(cx: &mut gpui::TestAppContext) {
    let _locale = crate::test_locale::force("en");
    use super::state::{TicksData, TicksState};
    use moon_core::db::tuner::ticks::params::ParamGroup;
    cx.update(|app| {
        let p = moon_ui::MoonPalette::active(app);
        let mut state = TicksState::default();
        let mut data = TicksData {
            kinds: vec!["MoonShot".into()],
            ..Default::default()
        };
        for group in [ParamGroup::Entry, ParamGroup::Exit] {
            let name = if group == ParamGroup::Entry {
                "Entry"
            } else {
                "Exit"
            };
            state.tape_reading = true;
            assert_eq!(
                super::grid::group_note(&state, group, &data, data.fit(), p),
                Some((format!("{name}: reading the tape…"), p.text_muted))
            );
            state.tape_reading = false;
            assert_eq!(
                super::grid::group_note(&state, group, &data, data.fit(), p),
                Some((
                    format!(
                        "{name}: The reproduction share is not known yet — no trades with tape"
                    ),
                    p.text_muted
                ))
            );
        }
        data.exit_share = (0, 400);
        state.tape_reading = true;
        assert_eq!(
            super::grid::group_note(&state, ParamGroup::Exit, &data, data.fit(), p),
            Some(("Exit: reading the tape…".into(), p.text_muted))
        );
        state.tape_reading = false;
        assert!(
            super::grid::group_note(&state, ParamGroup::Exit, &data, data.fit(), p)
                .unwrap()
                .0
                .starts_with("Exit: No trade is fit for the search")
        );
    });
}

/// A station handover with no queued venue rows must not advertise a stuck 0/0 fetch.
#[test]
fn empty_fetch_jobs_leave_only_the_local_read_caption() {
    let _locale = crate::test_locale::force("en");
    let progress = super::fetch::job::Progress::default();
    for active in [false, true] {
        for station in [false, true] {
            assert!(super::fetch_caption(&progress, active, station, false).is_empty());
            assert_eq!(
                super::fetch_caption(&progress, active, station, true),
                "reading the tape…"
            );
        }
    }
}

/// A configured station must never be described as a venue, including stale venue backoff.
#[test]
fn fetch_waiting_names_the_active_source_and_preserves_venue_progress() {
    let _locale = crate::test_locale::force("en");
    let mut progress = super::fetch::job::Progress {
        active: true,
        done: 2,
        total: 7,
        ..Default::default()
    };
    assert_eq!(
        super::fetch_caption(&progress, true, true, false),
        "trades: 2/7 · waiting for the station"
    );
    assert_eq!(
        super::fetch_caption(&progress, true, false, false),
        "trades: 2/7 · waiting for the venue"
    );
    progress.venue_wait = Some(Duration::from_secs(30));
    assert!(super::fetch_caption(&progress, true, true, false).contains("station"));
    assert!(super::fetch_caption(&progress, true, false, false).contains("30"));
    progress.in_flight = vec![(vec![1], "SYNTH".into())];
    assert!(super::fetch_caption(&progress, true, false, false).contains("SYNTH"));
    assert!(super::fetch_caption(&progress, false, true, false).is_empty());
}

/// A raw debug status or untranslated key would hide why a retryable row has no tape.
#[test]
fn unpublished_tape_caption_names_the_expected_utc_time_in_every_language() {
    for language in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        let caption = super::why::DealWhy::Refused(
            moon_core::market::trade_replay::TickStatus::ArchivePending {
                retry_at_ms: 216_000_000,
            },
        )
        .tape_tip()
        .text();
        assert!(caption.contains("1970-01-03 12:00 UTC"));
        assert!(
            caption.contains(&rust_i18n::t!("analytics.ticks.why.archive_pending").to_string())
        );
        assert!(
            caption.contains(
                &rust_i18n::t!(
                    "analytics.ticks.why.archive_pending.why",
                    expected = "1970-01-03 12:00 UTC"
                )
                .to_string()
            )
        );
        assert!(
            caption.contains(
                &rust_i18n::t!(
                    "analytics.ticks.why.todo",
                    action = rust_i18n::t!(
                        "analytics.ticks.why.archive_pending.todo",
                        expected = "1970-01-03 12:00 UTC"
                    )
                )
                .to_string()
            )
        );
        assert!(caption.contains(&rust_i18n::t!("analytics.ticks.why.not_your_fault").to_string()));
        assert!(!caption.contains("analytics.") && !caption.contains("ArchivePending"));
    }
}

/// The comparison caption always names its full population in every shipped language.
#[test]
fn usable_trade_caption_names_all_closed_deals() {
    for language in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        let caption = super::sample::caption(155, 405, false);
        assert!(caption.contains("155") && caption.contains("405"));
        assert!(!caption.contains("analytics."));
        let reading = super::sample::caption(155, 405, true);
        assert!(!reading.contains("155") && !reading.contains("405"));
        assert!(!reading.contains("analytics."));
    }
}

/// The wait before the fill is shown only where the core filed a placement that precedes the
/// entry fill: an absent, zero or later placement gives no line.
#[test]
fn order_wait_needs_a_placement_before_the_fill() {
    use super::order_wait_ms;
    assert_eq!(order_wait_ms(10_000, Some(7_600)), Some(2_400));
    assert_eq!(order_wait_ms(10_000, None), None);
    assert_eq!(order_wait_ms(10_000, Some(0)), None);
    assert_eq!(order_wait_ms(10_000, Some(-5)), None);
    assert_eq!(order_wait_ms(10_000, Some(12_000)), None);
    assert_eq!(order_wait_ms(10_000, Some(10_000)), None);
}

/// Printing a 90s pause as minutes would hide the seconds the next try honours, and printing
/// a 10 minute pause as raw seconds would read as a stuck counter. Past 90s the caption uses
/// the search estimate, so 91s keeps its leftover seconds and ten minutes stays a whole-minute
/// figure. The one-second floor of the search estimate must not turn a deadline that has just
/// passed into "1 s".
#[test]
fn a_venue_wait_stays_in_seconds_through_ninety_then_reads_as_minutes() {
    let _locale = crate::test_locale::force("en");
    assert_eq!(fetch_wait_text(Duration::from_secs(0)), "0 s");
    assert_eq!(fetch_wait_text(Duration::from_secs(30)), "30 s");
    assert_eq!(fetch_wait_text(Duration::from_secs(90)), "90 s");
    assert_eq!(fetch_wait_text(Duration::from_secs(91)), "1 min 31 s");
    assert_eq!(fetch_wait_text(Duration::from_secs(600)), "10 min");
    assert_eq!(
        fetch_wait_text(Duration::from_secs(3 * 3600 + 5 * 60)),
        "3 h 5 min"
    );
    assert_eq!(fetch_wait_text(Duration::from_millis(500)), "1 s");
}
