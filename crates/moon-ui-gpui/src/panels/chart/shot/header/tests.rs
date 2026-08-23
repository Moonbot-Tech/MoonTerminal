//! Unit coverage for the burnt-in header strip.

use chrono_tz::UTC;

use super::{
    centred_start_x, fit_tail, header_strip, Gaps, HeaderInputs, LeadGap, Measured, RunStyle,
};

/// Build a complete independently chosen snapshot, so the expected strip contents do not reuse
/// values produced by the builder under test.
fn inputs() -> HeaderInputs {
    HeaderInputs {
        coin: Some("BTCUSDT".to_string()),
        venue: "Binance".to_string(),
        when_ms: 1_704_164_645_000,
        zone: UTC,
        tf_min: 15,
        scale_pct: Some(7),
        delta_3h: Some(3.1),
        delta_1h: Some(2.2),
        delta_15m: Some(1.3),
    }
}

/// Join a field's runs, matching the text that the drawing loop writes without reusing its
/// measurement implementation.
fn field_text(field: &super::StripField) -> String {
    field.runs.iter().map(|run| run.text.as_str()).collect()
}

/// `header.rs:window_field` must use `fmt::pct`, not `fmt::signed_pct`; otherwise a range
/// magnitude is shown as a directional move on a chart image someone can trade from.
#[test]
fn a_positive_window_magnitude_is_printed_without_an_invented_sign() {
    let strip = header_strip(&inputs());
    let tail: Vec<String> = strip.tail.iter().map(field_text).collect();

    assert_eq!(&tail[3..], ["3h 3.1%", "1h 2.2%", "15m 1.3%"]);
}

/// `header.rs:header_strip` must keep coin and venue in its never-clipped head and retain the
/// three-hour-to-fifteen-minute tail order; otherwise a narrow shared screenshot loses identity
/// or keeps the least useful movement while dropping the longer context first.
#[test]
fn identity_stays_in_the_head_and_fifteen_minutes_is_the_first_window_to_clip() {
    let strip = header_strip(&inputs());
    let head: Vec<String> = strip.head.iter().map(field_text).collect();
    let tail: Vec<String> = strip.tail.iter().map(field_text).collect();
    let gaps = Gaps { field: 2, group: 4 };
    let head_widths = [Measured {
        width: 10,
        lead_gap: LeadGap::Field,
    }];
    let tail_widths = [
        Measured {
            width: 8,
            lead_gap: LeadGap::Group,
        },
        Measured {
            width: 8,
            lead_gap: LeadGap::Field,
        },
        Measured {
            width: 8,
            lead_gap: LeadGap::Field,
        },
    ];

    assert_eq!(head, ["BTCUSDT", "Binance"]);
    assert_eq!(&tail[3..], ["3h 3.1%", "1h 2.2%", "15m 1.3%"]);
    assert_eq!(fit_tail(&head_widths, &tail_widths, gaps, 41), 2);
}

/// `header.rs:header_strip` must omit an unavailable movement field; otherwise an unknown market
/// is presented as a quiet `0.0%` market in the screenshot.
#[test]
fn an_unknown_window_is_omitted_instead_of_becoming_a_zero() {
    let mut snapshot = inputs();
    snapshot.delta_1h = None;

    let strip = header_strip(&snapshot);
    let tail: Vec<String> = strip.tail.iter().map(field_text).collect();

    assert_eq!(&tail[3..], ["3h 3.1%", "15m 1.3%"]);
    assert!(tail.iter().all(|field| !field.starts_with("1h")));
}

/// `header.rs:fit_tail` must retain complete leading fields only; otherwise a narrow shot can
/// show a truncated percentage that states a different market fact.
#[test]
fn clipping_keeps_whole_priority_fields_and_never_a_partial_one() {
    let gaps = Gaps { field: 2, group: 4 };
    let head = [Measured {
        width: 10,
        lead_gap: LeadGap::Field,
    }];
    let tail = [
        Measured {
            width: 8,
            lead_gap: LeadGap::Group,
        },
        Measured {
            width: 8,
            lead_gap: LeadGap::Field,
        },
        Measured {
            width: 8,
            lead_gap: LeadGap::Field,
        },
    ];

    assert_eq!(fit_tail(&head, &tail, gaps, 32), 2);
    assert_eq!(fit_tail(&head, &tail, gaps, 29), 1);
}

/// `header.rs:fit_tail` must charge a group boundary more than an ordinary field gap; otherwise
/// the drawing can clip a token that the measurement claimed would fit.
#[test]
fn a_field_survives_or_clips_when_only_the_group_gap_changes() {
    let head = [Measured {
        width: 10,
        lead_gap: LeadGap::Field,
    }];
    let tail = [
        Measured {
            width: 10,
            lead_gap: LeadGap::Group,
        },
        Measured {
            width: 10,
            lead_gap: LeadGap::Field,
        },
    ];

    assert_eq!(fit_tail(&head, &tail, Gaps { field: 2, group: 2 }, 34), 2);
    assert_eq!(fit_tail(&head, &tail, Gaps { field: 2, group: 5 }, 34), 1);
}

/// `header.rs:header_strip` must put group boundaries before the stamp and three-hour movement
/// only; otherwise the redesigned hierarchy becomes a uniform run of ungrouped text.
#[test]
fn group_boundaries_fall_on_the_stamp_and_three_hour_field_only() {
    let strip = header_strip(&inputs());
    let grouped: Vec<String> = strip
        .tail
        .iter()
        .filter(|field| field.lead_gap == LeadGap::Group)
        .map(field_text)
        .collect();

    assert_eq!(grouped.len(), 2);
    assert!(grouped[0].contains("2024"));
    assert_eq!(grouped[1], "3h 3.1%");
}

/// `header.rs:scale_field` must omit a hidden scale, spell a sub-percent badge as `<1%`, and
/// place the shown scale before the market windows; otherwise the screenshot contradicts its chart.
#[test]
fn the_scale_badge_follows_the_chart_convention_and_view_order() {
    let mut hidden = inputs();
    hidden.scale_pct = None;
    let mut sub_percent = inputs();
    sub_percent.scale_pct = Some(0);

    let hidden_tail: Vec<String> = header_strip(&hidden).tail.iter().map(field_text).collect();
    let sub_percent_tail: Vec<String> = header_strip(&sub_percent)
        .tail
        .iter()
        .map(field_text)
        .collect();
    let shown_tail: Vec<String> = header_strip(&inputs())
        .tail
        .iter()
        .map(field_text)
        .collect();

    assert!(!hidden_tail.iter().any(|field| field == "7%"));
    assert_eq!(sub_percent_tail[2], "<1%");
    assert_eq!(shown_tail[2], "7%");
    assert_eq!(shown_tail[3], "3h 3.1%");
}

/// `header.rs:header_strip` must use spacing rather than separator glyphs; otherwise the compact
/// screenshot wastes width on punctuation and reads as the old undifferentiated header.
#[test]
fn the_builder_emits_no_separator_glyphs() {
    let strip = header_strip(&inputs());
    for field in strip.head.iter().chain(&strip.tail) {
        let text = field_text(field);
        assert!(!text.contains(['|', '\u{00b7}', ',']));
    }
}

/// `header.rs:window_field` must reserve `RunStyle::Primary` for movement figures; otherwise
/// context text competes with the market figures a reader opens the screenshot to scan.
#[test]
fn only_movement_figures_are_primary_runs() {
    let strip = header_strip(&inputs());
    let mut primary_fields = Vec::new();
    for field in strip.head.iter().chain(&strip.tail) {
        if field.runs.iter().any(|run| run.style == RunStyle::Primary) {
            primary_fields.push(field_text(field));
        }
    }

    assert_eq!(primary_fields, ["3h 3.1%", "1h 2.2%", "15m 1.3%"]);
}

/// `header.rs:centred_start_x` must divide the leftover width by two; otherwise a shared chart's
/// header visibly drifts left instead of being centred, with an odd pixel consistently on the right.
#[test]
fn a_narrower_run_is_centred_with_the_odd_pixel_on_the_right() {
    let drawn = [
        Measured {
            width: 100,
            lead_gap: LeadGap::Field,
        },
        Measured {
            width: 100,
            lead_gap: LeadGap::Field,
        },
    ];
    let start = centred_start_x(
        &drawn,
        Gaps {
            field: 20,
            group: 40,
        },
        501,
        20,
    );

    assert_eq!(start, 140);
    assert_eq!(501 - (start + 220), 141);
}

/// `header.rs:centred_start_x` must include one gap between two fields; otherwise the header's
/// measured run is too narrow and its centred position shifts by the missing field gap.
#[test]
fn two_fields_count_one_gap_when_their_run_is_centred() {
    let drawn = [
        Measured {
            width: 100,
            lead_gap: LeadGap::Field,
        },
        Measured {
            width: 100,
            lead_gap: LeadGap::Field,
        },
    ];

    assert_eq!(
        centred_start_x(
            &drawn,
            Gaps {
                field: 20,
                group: 40
            },
            400,
            20
        ),
        90
    );
}

/// `header.rs:centred_start_x` must clamp an over-wide run to the inset; otherwise a narrow chart
/// starts text at a negative coordinate and clips its identity at the wrong edge.
#[test]
fn an_over_wide_run_starts_exactly_at_the_inset() {
    let drawn = [Measured {
        width: 260,
        lead_gap: LeadGap::Field,
    }];

    assert_eq!(
        centred_start_x(
            &drawn,
            Gaps {
                field: 20,
                group: 40
            },
            200,
            20
        ),
        20
    );
}

/// `header.rs:centred_start_x` must remain continuous at the fitting boundary; otherwise a one-
/// pixel width change makes the header jump instead of staying at the strip inset.
#[test]
fn the_inset_clamp_starts_only_after_the_exact_fitting_boundary() {
    let gaps = Gaps {
        field: 20,
        group: 40,
    };
    for width in [159, 160, 161] {
        let drawn = [Measured {
            width,
            lead_gap: LeadGap::Field,
        }];
        assert_eq!(centred_start_x(&drawn, gaps, 200, 20), 20);
    }
}

/// `header.rs:centred_start_x` must give an empty run the inset; otherwise an empty header claims
/// a meaningless midpoint and a removed empty-run guard goes undetected.
#[test]
fn an_empty_run_uses_the_inset_instead_of_the_strip_midpoint() {
    assert_eq!(
        centred_start_x(
            &[],
            Gaps {
                field: 20,
                group: 40
            },
            500,
            23
        ),
        23
    );
}
