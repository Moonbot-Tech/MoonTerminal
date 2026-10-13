//! Column projection for the flat Core Status table.

use std::collections::{HashMap, HashSet};

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, Styled, Window, div, px,
};
use moon_core::feed::{ConnStatus, CoreTimeOffsetStatus};
use moon_core::session::{CoreStartupStatus, CoreSysStatus};
use moon_ui::{MoonDataTable, MoonDataTableWidthPolicy, MoonPalette, MoonRect, v_flex};

use super::super::columns_fit;
use super::super::layout_fixture::{self, LOCALES, WIDTHS};
use super::super::same_line::same_line;
use super::super::uniform::UniformFact;
use super::{
    cell_text_for, default_column_width, flat_measure_host, projected_column_keys,
    visible_column_keys,
};
use crate::backend::core_warn::LatencySeverity;
use crate::design;
use crate::panels::core_status::model::{ApiKeyState, CoreStatusRow, ServerKey};

/// Dropping a uniform column, and putting it back when nothing is uniform, is the whole
/// job of the extra argument. A later edit that ignores the list leaves a repeated value
/// in the table; one that drops columns when the list is empty hides a column the user
/// can still tell apart.
#[test]
fn visible_column_keys_drops_uniform_keys_and_keeps_them_when_none_are_uniform() {
    let hidden = HashSet::new();
    let kept = visible_column_keys(&[], &hidden, &[]);
    assert!(kept.contains(&"cpus"), "an empty uniform list keeps cpus");
    assert!(
        kept.contains(&"version"),
        "an empty uniform list keeps version"
    );
    assert!(kept.contains(&"ping"), "an empty uniform list keeps ping");

    let dropped = visible_column_keys(&[], &hidden, &["cpus", "version"]);
    assert!(
        !dropped.contains(&"cpus"),
        "cpus leaves while it is uniform"
    );
    assert!(
        !dropped.contains(&"version"),
        "version leaves while it is uniform"
    );
    assert!(
        dropped.contains(&"ping"),
        "a column that is not uniform stays"
    );
    assert_eq!(
        dropped.len(),
        kept.len() - 2,
        "only the named uniform keys leave"
    );
}

/// Offline and failed cores show an em dash for every numeric column. A ready core shows the
/// figure. The dash is not the ASCII marker a missing sample uses, so the two cannot be confused.
/// The clock column's never-measured marker is already that em dash, so this fixture measures an
/// offset. An unmeasured clock and an offline clock still read the same.
///
/// Breaks when a dropped core keeps painting its last CPU or ping.
#[test]
fn cell_text_for_dashes_numeric_keys_when_the_core_is_offline() {
    let live = sample_row(ConnStatus::Ready);
    let down = sample_row(ConnStatus::Failed("down".to_string()));
    let numeric = [
        "ping",
        "ping_exch",
        "cpu_proc",
        "cpu_sys",
        "mem_used",
        "free_phys",
        "cpus",
        "api_quota",
        "startup",
        "tz_off",
    ];
    for key in numeric {
        assert_eq!(cell_text_for(&down, key), "—", "{key} of a failed core");
        assert_ne!(
            cell_text_for(&live, key),
            "—",
            "{key} of a ready core is a real figure"
        );
    }
    assert_eq!(cell_text_for(&live, "cpu_proc"), "7%");
    assert_eq!(cell_text_for(&live, "cpu_sys"), "11%");
    assert_eq!(cell_text_for(&live, "cpus"), "4");
    assert_eq!(cell_text_for(&live, "api_quota"), "50");
    assert_eq!(cell_text_for(&live, "tz_off"), "UTC+02:00");
}

/// A connecting core is not offline. Its numbers stay, including a core that is only warning.
///
/// Breaks when every non-ready status is dashed: a core still coming up would look dead.
#[test]
fn cell_text_for_keeps_numbers_while_the_core_is_connecting() {
    let connecting = sample_row(ConnStatus::Connecting);
    assert_eq!(cell_text_for(&connecting, "cpu_proc"), "7%");
    assert_eq!(
        cell_text_for(&connecting, "ping"),
        cell_text_for(&sample_row(ConnStatus::Ready), "ping")
    );
}

/// A narrow measured width removes the drop tiers in order. Ping, status and the name stay.
/// Width 0 has not been measured and removes nothing.
///
/// Breaks when the render helper drops the name, or strips columns before the canvas has a width.
#[test]
fn projected_column_keys_follow_drop_tiers_at_a_narrow_width() {
    let hidden = HashSet::new();
    let dragged = HashMap::new();
    let floors = HashMap::new();
    let wide = projected_column_keys(&[], &hidden, &[], &dragged, 100_000.0, &floors);
    let unmeasured = projected_column_keys(&[], &hidden, &[], &dragged, 0.0, &floors);
    assert_eq!(unmeasured, wide);

    let sum: f32 = wide.iter().map(|key| default_column_width(key)).sum();
    let tier0: f32 = ["cpu_sys", "startup"]
        .into_iter()
        .map(default_column_width)
        .sum();
    let tier1: f32 = ["free_phys", "ping_exch"]
        .into_iter()
        .map(default_column_width)
        .sum();
    let available = sum - tier0 - tier1 + 1.0;
    let narrow = projected_column_keys(&[], &hidden, &[], &dragged, available, &floors);
    let expected: Vec<&str> = wide
        .iter()
        .copied()
        .filter(|key| !matches!(*key, "cpu_sys" | "startup" | "free_phys" | "ping_exch"))
        .collect();
    assert_eq!(narrow, expected);
    for kept in ["ping", "status", "core"] {
        assert!(narrow.contains(&kept), "{kept} never drops");
    }
}

/// Quiet row with known telemetry. The connection is the only field the caller changes.
fn sample_row(status: ConnStatus) -> CoreStatusRow {
    CoreStatusRow {
        fault: None,
        mode_suggestion: None,
        id: 1,
        name: "Core 1".to_string(),
        status,
        sys: CoreSysStatus {
            process_cpu_percent: Some(7),
            system_cpu_percent: Some(11),
            used_memory_mb: Some(40),
            free_physical_memory_mb: Some(800),
            round_trip_ms: Some(12),
            order_api_latency_ms: Some(20),
            logical_cpu_count: Some(4),
            ..CoreSysStatus::default()
        },
        endpoint: None,
        ping_warn: false,
        ping_sev: LatencySeverity::Normal,
        exch_sev: LatencySeverity::Normal,
        api_key: ApiKeyState::Unknown,
        api_warn: false,
        api_notice: false,
        api_quota: Some(50),
        api_quota_warn: false,
        startup: CoreStartupStatus::default(),
        time_offset: CoreTimeOffsetStatus {
            offset_secs: Some(7_200),
            ..CoreTimeOffsetStatus::default()
        },
        server_version: None,
        server_version_suffix: None,
        version_behind: None,
        update: None,
    }
}

/// Production flat cells inside `MoonDataTable`, plus the same-for-all line.
struct ProductionProbe {
    /// Fixture rows. One ready core carries a five-digit ping.
    rows: Vec<CoreStatusRow>,
    /// Column ids after the drop for this width.
    keys: Vec<&'static str>,
    /// Content floors the descriptors are compressed against.
    floors: HashMap<String, f32>,
    /// Dock width. Also the table's Fit viewport.
    width: f32,
    /// Shared facts. Each one is shorter than the narrow dock; the set is not.
    facts: Vec<UniformFact>,
}

impl Render for ProductionProbe {
    /// Paint production row cells. A missing backend leaves the update button off.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = MoonPalette::active(cx);
        let keys = self.keys.clone();
        let rows = self.rows.clone();
        let names: HashMap<ServerKey, String> = HashMap::new();
        let columns =
            super::respect_content_floors(super::columns(&self.keys), &self.floors, self.width);
        let width = self.width;
        let table = MoonDataTable::new("cs-flat-layout", rows.len(), move |index, _, app| {
            super::core_status_row(&rows[index], &keys, &names, None, false, palette, app)
        })
        .bounds(MoonRect::new(0.0, 0.0, width, 360.0))
        .columns(columns)
        .width_policy(MoonDataTableWidthPolicy::Fit);
        let line = same_line(&self.facts, cx).expect("facts");
        v_flex()
            .size_full()
            .min_w(px(0.0))
            .debug_selector(|| "cs-flat-probe".into())
            .child(
                div()
                    .debug_selector(|| "cs-same-host".into())
                    .w_full()
                    .min_w(px(0.0))
                    .child(line),
            )
            .child(
                div()
                    .debug_selector(|| "cs-flat-host".into())
                    .w(px(width))
                    .h(px(360.0))
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .child(table),
            )
    }
}

/// The measuring host must stay at the dock width when its child is wider than that dock.
struct MeasureProbe {
    /// Width of the child, deliberately past the tier boundary.
    content_w: f32,
}

impl Render for MeasureProbe {
    /// Paint the production measure host around a child wider than the window.
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let content_w = self.content_w;
        flat_measure_host(
            div()
                .w(px(content_w))
                .h(px(40.0))
                .debug_selector(|| "cs-flat-content".into()),
            |_, _| {},
        )
    }
}

/// Columns still on screen at this dock width, in display order.
///
/// 1640 fits every declared column. 1100 and 720 have shed all three drop tiers. The lists are
/// written out: a helper that walked the tiers again would stay green after the order changed.
fn expected_keys(width: f32) -> &'static [&'static str] {
    if width >= 1_640.0 {
        &[
            "status",
            "core",
            "server",
            "version",
            "ping",
            "ping_exch",
            "cpu_proc",
            "cpu_sys",
            "cpus",
            "mem_used",
            "free_phys",
            "api_key",
            "api_quota",
            "startup",
            "tz_off",
        ]
    } else {
        &[
            "status",
            "core",
            "version",
            "ping",
            "cpu_proc",
            "cpus",
            "api_key",
            "api_quota",
            "tz_off",
        ]
    }
}

fn shared_facts() -> Vec<UniformFact> {
    vec![
        UniformFact {
            key: "tz_off",
            text: "+02:00".to_string(),
            tip: None,
        },
        UniformFact {
            key: "cpus",
            text: "8".to_string(),
            tip: None,
        },
        UniformFact {
            key: "api_key",
            text: "45".to_string(),
            tip: None,
        },
        UniformFact {
            key: "api_quota",
            text: "1200".to_string(),
            tip: None,
        },
        UniformFact {
            key: "version",
            text: "7.71".to_string(),
            tip: None,
        },
    ]
}

/// Dropped columns follow the tier order, figures stay inside their columns, and the same-for-all
/// line wraps instead of clipping. A second measure of a too-wide child does not restore a tier.
#[gpui::test]
fn flat_columns_and_same_line_stay_inside_every_locale(cx: &mut gpui::TestAppContext) {
    layout_fixture::install_dark(cx);
    let mut rows = layout_fixture::cores();
    rows.iter_mut()
        .find(|row| row.id == 2)
        .expect("ready core")
        .sys
        .round_trip_ms = Some(12_345);
    let hidden = HashSet::new();
    let dragged = HashMap::new();
    let empty_floors = HashMap::new();
    let wide = projected_column_keys(&rows, &hidden, &[], &dragged, 100_000.0, &empty_floors);
    let mut line_h: HashMap<(&str, i32), f32> = HashMap::new();

    for code in LOCALES {
        let _locale = crate::test_locale::force(code);
        for width in WIDTHS {
            let floors = cx.update(|app| super::content_floors(&rows, app));
            let keys = projected_column_keys(&rows, &hidden, &[], &dragged, width, &floors);
            assert_eq!(keys, expected_keys(width), "{code} @ {width}");
            let ping_text = cell_text_for(
                rows.iter().find(|row| row.id == 2).expect("ready core"),
                "ping",
            );
            let mut visual = layout_fixture::open(cx, width, 420.0, {
                let keys = keys.clone();
                let rows = rows.clone();
                let floors = floors.clone();
                let facts = shared_facts();
                move |_, _| ProductionProbe {
                    rows,
                    keys,
                    floors,
                    width,
                    facts,
                }
            });
            layout_fixture::draw(&mut visual);
            let at = format!("{code} @ {width}");
            let host = visual.debug_bounds("cs-flat-host").expect("column host");
            let probe = visual.debug_bounds("cs-flat-probe").expect("probe");
            assert!(
                layout_fixture::inside(host, probe),
                "column host left the probe at {at}"
            );
            let ping = visual.debug_bounds("cs-ping-2").expect("ping cell");
            let measured =
                visual.update(|_, app| design::ui_text_width(app, &ping_text, 10.5, 400.0, true));
            assert!(
                layout_fixture::inside(ping, host)
                    && layout_fixture::width_of(ping) + 0.5 >= measured,
                "ping clipped at {at}: measured {measured}, cell {ping:?}, host {host:?}"
            );
            let line = visual.debug_bounds("cs-same").expect("same line");
            let line_host = visual.debug_bounds("cs-same-host").expect("same host");
            assert!(
                layout_fixture::inside(line, line_host),
                "same line left its host at {at}: {line:?} / {line_host:?}"
            );
            let title = visual.debug_bounds("cs-same-title").expect("same title");
            assert!(
                layout_fixture::inside(title, line),
                "same-line title clipped at {at}"
            );
            for fact in shared_facts() {
                let bounds = visual
                    .debug_bounds(layout_fixture::leak_selector(format!(
                        "cs-same-{}",
                        fact.key
                    )))
                    .unwrap_or_else(|| panic!("fact {} at {at}", fact.key));
                let measured =
                    visual.update(|_, app| design::mono_caption_text_width(app, &fact.text, 400.0));
                assert!(
                    layout_fixture::inside(bounds, line)
                        && layout_fixture::width_of(bounds) + 0.5 >= measured,
                    "fact {} clipped at {at}: measured {measured}, box {bounds:?}",
                    fact.key
                );
            }
            line_h.insert((code, width as i32), layout_fixture::height_of(line));
        }
    }

    // Caption text fits the five facts on one line at 720. A 480-wide host is still narrower
    // than that line, so the row has to wrap instead of clipping a fact.
    for code in LOCALES {
        let _locale = crate::test_locale::force(code);
        let floors = cx.update(|app| super::content_floors(&rows, app));
        let keys = projected_column_keys(&rows, &hidden, &[], &dragged, 480.0, &floors);
        let mut visual = layout_fixture::open(cx, 480.0, 420.0, {
            let rows = rows.clone();
            let floors = floors.clone();
            let facts = shared_facts();
            move |_, _| ProductionProbe {
                rows,
                keys,
                floors,
                width: 480.0,
                facts,
            }
        });
        layout_fixture::draw(&mut visual);
        let line = visual.debug_bounds("cs-same").expect("same line");
        let line_host = visual.debug_bounds("cs-same-host").expect("same host");
        assert!(
            layout_fixture::inside(line, line_host),
            "{code}: same line left its host at 480px: {line:?} / {line_host:?}"
        );
        for fact in shared_facts() {
            let bounds = visual
                .debug_bounds(layout_fixture::leak_selector(format!(
                    "cs-same-{}",
                    fact.key
                )))
                .unwrap_or_else(|| panic!("fact {} at 480 {code}", fact.key));
            assert!(
                layout_fixture::inside(bounds, line),
                "{code}: fact {} clipped at 480px: {bounds:?} / {line:?}",
                fact.key
            );
        }
        let wrapped = layout_fixture::height_of(line);
        let wide_h = line_h[&(code, 1640)];
        assert!(
            wrapped > wide_h + 2.0,
            "{code}: the same-for-all line did not wrap at 480px ({wrapped} vs {wide_h})"
        );
    }

    let _locale = crate::test_locale::force("en");
    let shown0: f32 = wide.iter().copied().map(default_column_width).sum();
    let tier0: f32 = ["cpu_sys", "startup"]
        .into_iter()
        .map(default_column_width)
        .sum();
    let boundary = shown0 - tier0 + 8.0;
    let measured: Vec<(&str, f32)> = wide
        .iter()
        .copied()
        .map(|key| (key, default_column_width(key)))
        .collect();
    assert!(shown0 > boundary + 0.5, "level 0 must not fit");
    assert!(
        shown0 - tier0 <= boundary + 0.5,
        "level 1 must fit just across the boundary"
    );
    let mut visual = layout_fixture::open(cx, boundary, 120.0, move |_, _| MeasureProbe {
        content_w: shown0,
    });
    layout_fixture::draw(&mut visual);
    let host = visual
        .debug_bounds("cs-flat-measure")
        .expect("measure host");
    let content = visual.debug_bounds("cs-flat-content").expect("wide child");
    let host_w = layout_fixture::width_of(host);
    assert!(
        layout_fixture::width_of(content) > host_w + 20.0,
        "the child must be wider than the host or the cap is untested: content {} host {host_w}",
        layout_fixture::width_of(content)
    );
    assert!(
        (host_w - boundary).abs() < 2.0,
        "measure host grew toward the child: host {host_w}, dock {boundary}"
    );
    let level = columns_fit::drop_level(&measured, host_w);
    assert_eq!(level, 1, "boundary width must sit on drop level 1");
    layout_fixture::draw(&mut visual);
    let again = visual.debug_bounds("cs-flat-measure").expect("second pass");
    let again_w = layout_fixture::width_of(again);
    assert!(
        (again_w - host_w).abs() <= 0.5,
        "second layout pass changed the host width from {host_w} to {again_w}"
    );
    assert_eq!(
        columns_fit::drop_level(&measured, again_w),
        level,
        "drop level oscillated on the second pass"
    );
}
