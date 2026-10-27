//! Column projection for the flat Core Status table.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::{
    AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render, Styled,
    Window, div, px,
};
use moon_core::feed::{ConnStatus, CoreTimeOffsetStatus};
use moon_core::session::core_order::ExchangeSection;
use moon_core::session::{CoreStartupStatus, CoreSysStatus};
use moon_core::venue::CoreVenue;
use moon_ui::{
    MoonDataTable, MoonDataTableState, MoonDataTableWidthPolicy, MoonPalette, MoonRect, v_flex,
};

use super::super::columns_fit;
use super::super::layout_fixture::{self, LOCALES, WIDTHS};
use super::super::ordering::{self, FlatLine};
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

/// The API-key column leaves the table when every core in scope shows the same key text, and
/// stays when one core has a different dated key, in every locale.
#[test]
fn api_key_column_follows_the_displayed_key_text() {
    let fleet = |days: [i32; 3]| -> Vec<CoreStatusRow> {
        days.iter()
            .enumerate()
            .map(|(index, days)| {
                let mut row = sample_row(ConnStatus::Ready);
                row.id = index as u64 + 1;
                row.api_key = ApiKeyState::Days(*days);
                row
            })
            .collect()
    };
    let hidden = HashSet::new();
    for code in LOCALES {
        let _locale = crate::test_locale::force(code);
        for (days, shown) in [([30, 30, 30], false), ([30, 30, 9], true)] {
            let rows = fleet(days);
            let facts = super::super::uniform::uniform_columns(&rows);
            let uniform: Vec<&str> = facts.iter().map(|fact| fact.key).collect();
            let keys = visible_column_keys(&rows, &hidden, &uniform);
            assert_eq!(
                keys.contains(&"api_key"),
                shown,
                "{code}: api_key column for {days:?}"
            );
            assert_eq!(
                uniform.contains(&"api_key"),
                !shown,
                "{code}: same-for-all line for {days:?}"
            );
        }
    }
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

/// A narrow measured width removes the drop tiers in order. Ping and the name stay.
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
    for kept in ["ping", "core"] {
        assert!(narrow.contains(&kept), "{kept} never drops");
    }
}

/// Quiet row with known telemetry. The connection is the only field the caller changes.
fn sample_row(status: ConnStatus) -> CoreStatusRow {
    // Startup cells translate; the first lookup must not run on the 2 MiB test stack.
    crate::test_locale::warm();
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

/// Width a vertical scrollbar may take off the right edge of the dock slot.
const SCROLLBAR_SLACK: f32 = 16.0;

/// Core count of the dock-shaped fixture: enough rows that the list must scroll in a 420 px dock.
const DOCK_CORES: u64 = 44;

/// `DOCK_CORES` healthy cores split across two exchanges, laid out by the production
/// [`ordering::flat_lines`] so both exchange headings are real lines.
fn dock_fixture() -> (Vec<CoreStatusRow>, Vec<FlatLine>) {
    let rows: Vec<CoreStatusRow> = (1..=DOCK_CORES)
        .map(|id| {
            let mut row = sample_row(ConnStatus::Ready);
            row.id = id;
            row.name = format!("CORE-{id:02}");
            // The first core of each exchange is dropped, so a red dot and an inline reason are
            // on screen whichever exchange heads the list.
            if id <= 2 {
                row.status = ConnStatus::Disconnected;
            }
            row
        })
        .collect();
    let venues: HashMap<u64, CoreVenue> = rows
        .iter()
        .map(|row| {
            let code = if row.id % 2 == 0 { 2 } else { 13 };
            (row.id, CoreVenue::identify(code, "", None))
        })
        .collect();
    let lines = ordering::flat_lines(&rows, &venues);
    (rows, lines)
}

/// The Flat table mounted the way `render.rs` mounts it: a fixed strip, then the `flex_1`
/// content slot, then [`flat_measure_host`] around [`data_table_host`] around the table.
struct DockProbe {
    rows: Rc<Vec<CoreStatusRow>>,
    lines: Rc<Vec<FlatLine>>,
    keys: Vec<&'static str>,
    /// Table state carrying the persisted drags the probe restores.
    state: Entity<MoonDataTableState>,
}

impl Render for DockProbe {
    /// Paint the production row and heading builders inside the production host chain.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let rows = self.rows.clone();
        let lines = self.lines.clone();
        let (keys, columns) = super::filled_columns(&self.keys, self.state.read(cx));
        let count = keys.len();
        let names: HashMap<ServerKey, String> = HashMap::new();
        let table = MoonDataTable::new(
            "cs-dock-table",
            lines.len(),
            move |ix, _, app| match &lines[ix] {
                FlatLine::Section(section) => {
                    super::section_row(section, false, false, count, p, app)
                }
                FlatLine::Core(row) => {
                    super::core_status_row(&rows[*row], &keys, &names, None, false, p, app)
                }
            },
        )
        .columns(columns)
        .state(&self.state)
        .style(design::table_style(p));
        v_flex()
            .size_full()
            // The panel root sets the mono face; headings inherit it.
            .font_family(design::mono())
            .child(div().w_full().h(px(56.0)).flex_none())
            .child(
                v_flex()
                    .w_full()
                    .min_w(px(0.0))
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .debug_selector(|| "cs-dock-slot".into())
                    .child(flat_measure_host(
                        crate::panels::common::data_table_host(
                            "cs-dock-host",
                            false,
                            String::new(),
                            p,
                            cx,
                            table,
                        ),
                        |_, _| {},
                    )),
            )
    }
}

/// Table state as an older build left it: a drag on the status column that no longer exists.
fn legacy_state() -> MoonDataTableState {
    let mut state = MoonDataTableState::new();
    state.set_column_width("status", 130.0);
    state
}

/// Table state where the user has dragged every visible column narrow, below even 720 px.
fn dragged_state(keys: &[&str]) -> MoonDataTableState {
    let mut state = legacy_state();
    for key in keys {
        state.set_column_width(*key, 50.0);
    }
    state
}

/// The Flat view must lay its rows out with real height inside the dock's content slot.
///
/// The measuring host wrapped `data_table_host` in a block box, so the host's `flex_1` had no
/// flex parent to grow in and the table resolved to zero height: header, headings and rows
/// all vanished while the strip above still counted every core.
#[gpui::test]
fn flat_rows_have_height_inside_the_dock_slot(cx: &mut gpui::TestAppContext) {
    // Section captions translate, and the first lookup must not run on the 2 MiB test stack.
    crate::test_locale::warm();
    layout_fixture::install_dark(cx);
    let (rows, lines) = dock_fixture();
    let sections = lines
        .iter()
        .filter(|line| matches!(line, FlatLine::Section(_)))
        .count();
    assert_eq!(sections, 2, "fixture must span two exchanges");
    let rows = Rc::new(rows);
    let lines = Rc::new(lines);
    let keys = visible_column_keys(&rows, &HashSet::new(), &[]);
    for code in LOCALES {
        let _locale = crate::test_locale::force(code);
        for (width, all_dragged) in WIDTHS
            .into_iter()
            .flat_map(|width| [(width, false), (width, true)])
        {
            let mut visual = layout_fixture::open(cx, width, 420.0, {
                let rows = rows.clone();
                let lines = lines.clone();
                let keys = keys.clone();
                move |_, cx| {
                    let state = if all_dragged {
                        dragged_state(&keys)
                    } else {
                        legacy_state()
                    };
                    DockProbe {
                        rows,
                        lines,
                        keys,
                        state: cx.new(|_| state),
                    }
                }
            });
            layout_fixture::draw(&mut visual);
            let at = format!("{code} @ {width}, all dragged {all_dragged}");
            let slot = visual.debug_bounds("cs-dock-slot").expect("content slot");
            assert!(
                layout_fixture::height_of(slot) > 100.0,
                "content slot collapsed at {at}: {slot:?}"
            );
            // Lines 0 and 1 are the first heading and its first core, both above the fold.
            let first_core = match &lines[1] {
                FlatLine::Core(index) => rows[*index].id,
                FlatLine::Section(_) => panic!("a heading must be followed by its members"),
            };
            let ping = visual
                .debug_bounds(layout_fixture::leak_selector(format!(
                    "cs-ping-{first_core}"
                )))
                .unwrap_or_else(|| panic!("first core row was never laid out at {at}"));
            assert!(
                layout_fixture::height_of(ping) > 4.0 && layout_fixture::inside(ping, slot),
                "first core row has no height or left the slot at {at}: {ping:?} / {slot:?}"
            );
            // The first heading is on screen: its caption and its aggregate draw unclipped.
            let FlatLine::Section(section) = &lines[0] else {
                panic!("the first line must be a heading");
            };
            let key = match section.section {
                ExchangeSection::Venue(id) => format!("cs-section-{}-{}", id.code, id.dex),
                ExchangeSection::Unidentified => "cs-section-unknown".to_string(),
            };
            let banner = visual
                .debug_bounds(layout_fixture::leak_selector(key.clone()))
                .unwrap_or_else(|| panic!("heading was never laid out at {at}"));
            assert!(
                layout_fixture::height_of(banner) > 4.0 && layout_fixture::inside(banner, slot),
                "heading has no height or left the slot at {at}: {banner:?} / {slot:?}"
            );
            // The band is the banner's own fill, so the banner must span the row, not stop
            // where the columns end. A vertical scrollbar may take the slot's last pixels.
            assert!(
                layout_fixture::width_of(banner) + SCROLLBAR_SLACK
                    >= layout_fixture::width_of(slot),
                "heading band stops short of the row at {at}: {banner:?} / {slot:?}"
            );
            // The status is an 8 px dot, not a filled square, and it stays inside the slot.
            let dot = visual
                .debug_bounds(layout_fixture::leak_selector(format!(
                    "cs-status-dot-{first_core}"
                )))
                .unwrap_or_else(|| panic!("status dot at {at}"));
            assert!(
                (layout_fixture::width_of(dot) - 8.0).abs() <= 0.5
                    && (layout_fixture::height_of(dot) - 8.0).abs() <= 0.5
                    && layout_fixture::inside(dot, slot),
                "status dot is not an 8 px disc inside the slot at {at}: {dot:?}"
            );
            // The columns reach the panel edge: the last cell plus its right padding ends where
            // the row does, whether the name column fills or every column is dragged and the
            // last one does.
            let last = visual
                .debug_bounds(layout_fixture::leak_selector(format!(
                    "cs-tz-off-{first_core}"
                )))
                .unwrap_or_else(|| panic!("last cell at {at}"));
            let edge = f32::from(last.right()) + super::CELL_PAD_RIGHT;
            let band_edge = f32::from(banner.right());
            assert!(
                (edge - band_edge).abs() <= 2.0
                    && band_edge + SCROLLBAR_SLACK + 2.0 >= f32::from(slot.right()),
                "columns end at {edge}, band at {band_edge}, panel at {} ({at})",
                f32::from(slot.right())
            );
            // The dot sits right before the name, even with the old status column's width on file.
            let name = visual
                .debug_bounds(layout_fixture::leak_selector(format!(
                    "cs-core-name-{first_core}"
                )))
                .unwrap_or_else(|| panic!("core name at {at}"));
            let gap = f32::from(name.origin.x) - f32::from(dot.right());
            assert!(
                (0.0..=12.0).contains(&gap),
                "dot-name gap is {gap} px at {at}: {dot:?} / {name:?}"
            );
            // The dropped core's reason follows its name, whole.
            let dropped = rows
                .iter()
                .find(|row| row.id == first_core)
                .expect("dropped core");
            let reason_text = super::problem_reason(dropped, None)
                .expect("a dropped core has a reason")
                .0;
            let reason = visual
                .debug_bounds(layout_fixture::leak_selector(format!(
                    "cs-core-reason-{first_core}"
                )))
                .unwrap_or_else(|| panic!("reason text at {at}"));
            let reason_w =
                visual.update(|_, app| design::ui_caption_text_width(app, &reason_text, 400.0));
            // Whole wherever the name column has room; at 720 px or dragged narrow it may ellipsize.
            assert!(
                (all_dragged
                    || width < 1_000.0
                    || layout_fixture::width_of(reason) + 0.5 >= reason_w)
                    && layout_fixture::inside(reason, slot),
                "reason text clipped at {at}: measured {reason_w}, box {reason:?}"
            );
            // The name cell clips its overflow, so the reason must end inside that cell.
            let name_cell = visual
                .debug_bounds(layout_fixture::leak_selector(format!(
                    "cs-core-cell-{first_core}"
                )))
                .unwrap_or_else(|| panic!("name cell at {at}"));
            assert!(
                layout_fixture::inside(reason, name_cell),
                "reason text crosses the name cell edge at {at}: {reason:?} / {name_cell:?}"
            );
            assert!(
                f32::from(reason.origin.x) > f32::from(dot.right()),
                "reason must sit after the dot and the name at {at}: {reason:?} / {dot:?}"
            );
            let summary = ordering::section_aggregate_text(&section.aggregate);
            for (suffix, text, weight) in [
                ("label", section.label.as_str(), 600.0),
                ("agg", summary.as_str(), 400.0),
            ] {
                let bounds = visual
                    .debug_bounds(layout_fixture::leak_selector(format!("{key}-{suffix}")))
                    .unwrap_or_else(|| panic!("heading {suffix} at {at}"));
                let measured =
                    visual.update(|_, app| design::mono_caption_text_width(app, text, weight));
                assert!(
                    layout_fixture::inside(bounds, banner)
                        && layout_fixture::width_of(bounds) + 0.5 >= measured,
                    "heading {suffix} clipped at {at}: measured {measured}, box {bounds:?}, banner {banner:?}"
                );
            }
        }
    }
}

/// A persisted layout narrower than the panel must still reach the panel's right edge.
///
/// Every column carries a saved drag. The name column absorbs the leftover whenever it is
/// visible, so the numeric columns keep their saved widths and stay packed together; only with
/// the name hidden does the last VISIBLE column in display order take it. Mutations this
/// catches: filling the last column while the name is shown, filling the last SOURCE column
/// after the user moved another one to the end, stretching a column away from its saved width,
/// and writing the rendered width back into the table state, which would mark the saved layout
/// dirty on every render.
#[gpui::test]
fn persisted_layout_name_column_absorbs_the_leftover(cx: &mut gpui::TestAppContext) {
    crate::test_locale::warm();
    layout_fixture::install_dark(cx);
    let (rows, lines) = dock_fixture();
    let rows = Rc::new(rows);
    let lines = Rc::new(lines);
    let all_keys = visible_column_keys(&rows, &HashSet::new(), &[]);
    let first_core = match &lines[1] {
        FlatLine::Core(index) => rows[*index].id,
        FlatLine::Section(_) => panic!("a heading must be followed by its members"),
    };
    let without = |gone: &str| -> Vec<&'static str> {
        all_keys
            .iter()
            .copied()
            .filter(|key| *key != gone)
            .collect()
    };
    let mut core_last = without("core");
    core_last.push("core");
    let tz = format!("cs-tz-off-{first_core}");
    let startup = format!("cs-startup-{first_core}");
    let name = format!("cs-core-cell-{first_core}");
    // (case, visible keys, column order, (key, selector) of the cell reaching the edge,
    // (key, selector) of the cell that fills, or `None` when it is the edge cell itself)
    type Cell = (&'static str, String);
    let cases: [(
        &str,
        Vec<&'static str>,
        Vec<&'static str>,
        Cell,
        Option<Cell>,
    ); 4] = [
        (
            "source order",
            all_keys.clone(),
            Vec::new(),
            ("tz_off", tz.clone()),
            Some(("core", name.clone())),
        ),
        (
            "tz_off hidden",
            without("tz_off"),
            Vec::new(),
            ("startup", startup),
            Some(("core", name.clone())),
        ),
        (
            "core moved last",
            all_keys.clone(),
            core_last,
            ("core", name.clone()),
            None,
        ),
        (
            "core hidden",
            without("core"),
            Vec::new(),
            ("tz_off", tz),
            None,
        ),
    ];
    let width = WIDTHS[0];
    for code in LOCALES {
        let _locale = crate::test_locale::force(code);
        for (case, keys, order, (last_key, last_selector), filler) in &cases {
            // Saved widths at 60% of the defaults: well under the panel, like a layout saved
            // on a narrower monitor.
            let mut seeded = MoonDataTableState::new();
            for key in keys {
                seeded.set_column_width(*key, (default_column_width(key) * 0.6).round());
            }
            seeded.set_column_order(order.iter().map(|key| gpui::SharedString::from(*key)));
            let saved = seeded.column_widths.clone();
            let saved_order = seeded.column_order.clone();
            let state_slot: Rc<std::cell::RefCell<Option<Entity<MoonDataTableState>>>> =
                Rc::default();
            let mut visual = layout_fixture::open(cx, width, 420.0, {
                let rows = rows.clone();
                let lines = lines.clone();
                let keys = keys.clone();
                let state_slot = state_slot.clone();
                move |_, cx| {
                    let state = cx.new(|_| seeded);
                    *state_slot.borrow_mut() = Some(state.clone());
                    DockProbe {
                        rows,
                        lines,
                        keys,
                        state,
                    }
                }
            });
            layout_fixture::draw(&mut visual);
            let at = format!("{code}, {case} @ {width}");
            let slot = visual.debug_bounds("cs-dock-slot").expect("content slot");
            let last = visual
                .debug_bounds(layout_fixture::leak_selector(last_selector.clone()))
                .unwrap_or_else(|| panic!("last cell at {at}"));
            let edge = f32::from(last.right()) + super::CELL_PAD_RIGHT;
            let FlatLine::Section(section) = &lines[0] else {
                panic!("the first line must be a heading");
            };
            let banner_key = match section.section {
                ExchangeSection::Venue(id) => format!("cs-section-{}-{}", id.code, id.dex),
                ExchangeSection::Unidentified => "cs-section-unknown".to_string(),
            };
            let banner = visual
                .debug_bounds(layout_fixture::leak_selector(banner_key))
                .unwrap_or_else(|| panic!("heading at {at}"));
            let band_edge = f32::from(banner.right());
            assert!(
                (edge - band_edge).abs() <= 2.0
                    && band_edge + SCROLLBAR_SLACK + 2.0 >= f32::from(slot.right()),
                "last column ends at {edge}, band at {band_edge}, panel at {} ({at})",
                f32::from(slot.right())
            );
            let mut drawn = |selector: &String| {
                let cell = visual
                    .debug_bounds(layout_fixture::leak_selector(selector.clone()))
                    .unwrap_or_else(|| panic!("{selector} at {at}"));
                layout_fixture::width_of(cell) + super::CELL_PAD_LEFT + super::CELL_PAD_RIGHT
            };
            // The edge cell keeps its saved width unless it is the one that fills.
            if let Some((fill_key, fill_selector)) = filler {
                let rendered = drawn(last_selector);
                let saved_last = saved[*last_key];
                assert!(
                    (rendered - saved_last).abs() <= 1.0,
                    "{last_key} drew {rendered} px instead of its saved {saved_last} at {at}"
                );
                let grown = drawn(fill_selector);
                let saved_fill = saved[*fill_key];
                assert!(
                    grown > saved_fill + 2.0,
                    "{fill_key} drew {grown} px, not wider than its saved {saved_fill} at {at}"
                );
            }
            // Rendering wrote nothing back: the saved layout is not dirtied by a paint.
            let state = state_slot.borrow().clone().expect("probe state");
            visual.update(|_, app| {
                let now = state.read(app);
                assert_eq!(
                    now.column_widths, saved,
                    "widths changed by rendering at {at}"
                );
                assert_eq!(
                    now.column_order, saved_order,
                    "order changed by rendering at {at}"
                );
            });
        }
    }
}
