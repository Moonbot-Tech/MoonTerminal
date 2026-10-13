//! Card order: a warned server first, then natural display name.
//!
//! Explicit imports (no `use super::*`): the parent module glob-imports `gpui`.

use std::collections::HashMap;
use std::net::IpAddr;

use gpui::{Context, IntoElement, ParentElement, Render, Styled, WeakEntity, Window};
use moon_core::feed::ConnStatus;
use moon_core::session::CoreId;
use moon_core::session::{CoreStartupStatus, CoreSysStatus};
use moon_ui::v_flex;

use super::super::CoreStatusView;
use super::super::layout_fixture::{self, LOCALES, WIDTHS};
use super::super::presentation::{cpu_load, memory_free};
use super::{card_core_order, card_order, ordered_server_grid};
use crate::backend::core_warn::LatencySeverity;
use crate::controls::row_selection::RowSelection;
use crate::design;
use crate::panels::core_status::model::{
    ApiKeyState, CoreStatusRow, GroupVersion, ServerKey, ServerStatusGroup,
};

fn core(id: CoreId) -> CoreStatusRow {
    CoreStatusRow {
        fault: None,
        mode_suggestion: None,
        id,
        name: format!("c{id}"),
        status: ConnStatus::Ready,
        sys: CoreSysStatus::default(),
        endpoint: None,
        ping_warn: false,
        ping_sev: LatencySeverity::Normal,
        exch_sev: LatencySeverity::Normal,
        api_key: ApiKeyState::Unknown,
        api_warn: false,
        api_notice: false,
        api_quota: None,
        api_quota_warn: false,
        startup: CoreStartupStatus::default(),
        time_offset: moon_core::feed::CoreTimeOffsetStatus::default(),
        server_version: None,
        server_version_suffix: None,
        version_behind: None,
        update: None,
    }
}

fn server(name: &str, id: CoreId, warned: bool) -> ServerStatusGroup {
    ServerStatusGroup {
        key: ServerKey::Unknown(id),
        display_name: name.to_string(),
        cpu_warn: warned,
        mem_warn: false,
        conn_warn: false,
        ping_warn: false,
        exch_warn: false,
        api_warn: false,
        api_notice: false,
        api_key: ApiKeyState::Unknown,
        version: GroupVersion::Absent,
        version_behind: None,
        address: None,
        cores: vec![core(id)],
        system_cpu_percent: None,
        process_memory_mb: None,
        free_physical_memory_mb: None,
        logical_cpu_count: None,
    }
}

#[test]
fn servers_order_puts_warned_first_then_natural_name() {
    let calm_2 = server("Server 2", 2, false);
    let calm_10 = server("Server 10", 10, false);
    let warned_10 = server("Server 10", 11, true);
    let groups = vec![calm_10.clone(), warned_10.clone(), calm_2.clone()];

    let names: Vec<&str> = card_order(&groups)
        .iter()
        .map(|group| group.display_name.as_str())
        .collect();
    assert_eq!(
        names,
        ["Server 10", "Server 2", "Server 10"],
        "a warned machine precedes a calm one, and calm names stay natural (2 before 10)"
    );
    assert!(card_order(&groups)[0].cpu_warn);
}

#[test]
fn card_core_order_follows_the_cards() {
    let calm_2 = server("Server 2", 2, false);
    let calm_10 = server("Server 10", 10, false);
    let warned = server("Warn", 7, true);
    let groups = vec![calm_10, calm_2, warned];

    assert_eq!(
        card_core_order(&groups),
        vec![Some(7), Some(2), Some(10)],
        "chips follow the cards: warned core, then natural name"
    );
    let names: Vec<&str> = card_order(&groups)
        .iter()
        .map(|group| group.display_name.as_str())
        .collect();
    assert_eq!(names, ["Warn", "Server 2", "Server 10"]);
}

/// Production server cards at one dock width. The view handle cannot upgrade, so clicks no-op.
struct GridProbe {
    /// Cards, construction order. The grid sorts them.
    groups: Vec<ServerStatusGroup>,
    /// Two-point CPU series keyed by address. A shorter series paints no line.
    history: HashMap<IpAddr, Vec<(f32, f32)>>,
}

impl Render for GridProbe {
    /// Paint the production grid.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = WeakEntity::<CoreStatusView>::new_invalid();
        v_flex().size_full().child(ordered_server_grid(
            card_order(&self.groups),
            false,
            None,
            None,
            None,
            None,
            &self.history,
            &RowSelection::default(),
            0.0,
            weak,
            cx,
        ))
    }
}

/// Cards do not overlap. Bars, the sparkline, the chip row, and each chip stay inside the card.
///
/// At 720 px a card's box is inside the grid, so a row never has to hold a card wider than the dock.
#[gpui::test]
fn server_cards_keep_bars_sparkline_and_chips_inside(cx: &mut gpui::TestAppContext) {
    layout_fixture::install_dark(cx);
    let (groups, history) = layout_fixture::servers();
    assert!(
        groups.len() >= 2,
        "the fixture needs two cards to test a row"
    );

    for code in LOCALES {
        let _locale = crate::test_locale::force(code);
        for width in WIDTHS {
            let mut visual = layout_fixture::open(cx, width, 760.0, {
                let groups = groups.clone();
                let history = history.clone();
                move |_, _| GridProbe { groups, history }
            });
            layout_fixture::draw(&mut visual);
            let at = format!("{code} @ {width}");
            let grid = visual.debug_bounds("cs-servers").expect("grid");
            let mut cards = Vec::new();
            for group in &groups {
                let tree = group.key.tree_id();
                let card = visual
                    .debug_bounds(layout_fixture::leak_selector(format!("cs-server-{tree}")))
                    .unwrap_or_else(|| panic!("card {tree} at {at}"));
                assert!(
                    layout_fixture::inside(card, grid),
                    "card {tree} left the grid at {at}: {card:?} / {grid:?}"
                );
                let cpu = visual
                    .debug_bounds(layout_fixture::leak_selector(format!("cs-card-cpu-{tree}")))
                    .unwrap_or_else(|| panic!("cpu bar {tree}"));
                let ram = visual
                    .debug_bounds(layout_fixture::leak_selector(format!("cs-card-ram-{tree}")))
                    .unwrap_or_else(|| panic!("ram bar {tree}"));
                let spark = visual
                    .debug_bounds(layout_fixture::leak_selector(format!("cs-spark-{tree}")))
                    .unwrap_or_else(|| panic!("spark {tree}"));
                let chips = visual
                    .debug_bounds(layout_fixture::leak_selector(format!("cs-cores-{tree}")))
                    .unwrap_or_else(|| panic!("chips {tree}"));
                for (name, part) in [
                    ("cpu", cpu),
                    ("ram", ram),
                    ("spark", spark),
                    ("chips", chips),
                ] {
                    assert!(
                        layout_fixture::inside(part, card),
                        "{name} left card {tree} at {at}: {part:?} / {card:?}"
                    );
                }
                let cpu_text = cpu_load(group.system_cpu_percent, group.logical_cpu_count);
                let ram_text = memory_free(group.process_memory_mb, group.free_physical_memory_mb);
                let cpu_fig = visual
                    .debug_bounds(layout_fixture::leak_selector(format!(
                        "cs-fig-cs-card-cpu-{tree}"
                    )))
                    .unwrap_or_else(|| panic!("cpu figure {tree}"));
                let ram_fig = visual
                    .debug_bounds(layout_fixture::leak_selector(format!(
                        "cs-fig-cs-card-ram-{tree}"
                    )))
                    .unwrap_or_else(|| panic!("ram figure {tree}"));
                let (cpu_w, ram_w) = visual.update(|_, app| {
                    (
                        design::mono_caption_text_width(app, &cpu_text, 400.0),
                        design::mono_caption_text_width(app, &ram_text, 400.0),
                    )
                });
                assert!(
                    layout_fixture::inside(cpu_fig, card)
                        && layout_fixture::width_of(cpu_fig) + 0.5 >= cpu_w,
                    "cpu figure clipped on {tree} at {at}: measured {cpu_w}, box {cpu_fig:?}"
                );
                assert!(
                    layout_fixture::inside(ram_fig, card)
                        && layout_fixture::width_of(ram_fig) + 0.5 >= ram_w,
                    "ram figure clipped on {tree} at {at}: measured {ram_w}, box {ram_fig:?}"
                );
                for core in &group.cores {
                    let chip = visual
                        .debug_bounds(layout_fixture::leak_selector(format!(
                            "cs-chip-{}",
                            core.id
                        )))
                        .unwrap_or_else(|| panic!("chip {}", core.id));
                    assert!(
                        layout_fixture::inside(chip, chips),
                        "chip {} left the chip row at {at}",
                        core.id
                    );
                }
                cards.push(card);
            }
            assert!(cards.len() >= 2, "both cards laid out at {at}");
            for left in 0..cards.len() {
                for right in (left + 1)..cards.len() {
                    assert!(
                        !layout_fixture::overlaps(cards[left], cards[right]),
                        "cards overlap at {at}: {:?} / {:?}",
                        cards[left],
                        cards[right]
                    );
                }
            }
        }
    }
}
