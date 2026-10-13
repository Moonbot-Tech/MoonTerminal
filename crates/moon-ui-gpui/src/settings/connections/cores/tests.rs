//! The Cores page: list tags, selection, the card's writers, delete only through the dialog, and
//! the layout sweep over the list, the card and the empty state.

use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, ScrollHandle, StatefulInteractiveElement, Styled, Window, div, px,
};
use moon_core::config::{
    AppConfig, FeedFlags, Secret, ServerConfig, TotalMode, TransportVersion, WorkspaceMembership,
};
use moon_ui::{MoonColorPickerState, MoonInputState};

use super::super::entries::{
    CoreDot, CoreListRow, CoresModel, ExBlock, GroupBlock, reselect, valid_selection,
};
use super::super::page::set_core_group;
use super::super::table::{FEED_TOTAL, feed_count, feed_set, set_core_active, set_for_station};
use super::{
    ADVANCED_KEY, CardInputs, CoreAct, CoreAction, CoreCard, FEEDS_KEY, ListParts, card_min_w,
    core_card, cores_empty, cores_layout, cores_list, cores_split, deletes, feeds_partial,
    plain_dot,
};
use super::{CARD_LABEL_W, LIST_SHARE};
use crate::settings::layout_probe::{ProbeSpec, Probed, sweep};

/// Synthetic core config.
fn server(name: &str, group: &str) -> ServerConfig {
    ServerConfig {
        id: 1,
        uid: 1,
        name: name.to_string(),
        active: true,
        feed: FeedFlags::default(),
        key: Secret::new(""),
        endpoint_override: String::new(),
        endpoint_to_station: false,
        group: group.to_string(),
        market: "BTCUSDT".to_string(),
        color: [1, 2, 3],
        synthetic: false,
        chart_bundle: String::new(),
        default_alert_strategy: 0,
        own_trade_config: false,
        strat_slots: None,
        manual_strategy: None,
        trade: None,
        transport: None,
        workspace_membership: WorkspaceMembership::default(),
        total_mode: TotalMode::default(),
    }
}

/// Catches the amber tag missing on a core that drops a data kind, or shown on one that receives
/// everything: `log_delivery` is the ninth kind and counts like the eight filters.
#[test]
fn the_feeds_tag_shows_exactly_below_nine() {
    let mut feed = FeedFlags {
        log_delivery: true,
        ..Default::default()
    };
    assert_eq!(feed_count(&feed), FEED_TOTAL);
    assert!(!feeds_partial(feed_count(&feed)));
    feed.log_delivery = false;
    assert_eq!(feed_count(&feed), 8);
    assert!(feeds_partial(feed_count(&feed)), "log_delivery off is 8/9");
    feed.log_delivery = true;
    feed_set(&mut feed, 0, false);
    assert!(feeds_partial(feed_count(&feed)), "a filter off is 8/9");
}

/// Catches a selection that names a deleted row or a stale key after the rows were rebuilt: the
/// same core keeps the selection, a deleted one hands it to the core that took its place, and a
/// render never keeps a key no row has.
#[test]
fn the_selection_always_names_an_existing_core() {
    let keys = [10, 11, 12];
    // Deleting the selected middle row selects the one that moved into its place.
    assert_eq!(reselect(Some(1), Some(1), &keys[..2]), Some(11));
    // Deleting the selected last row selects the new last.
    assert_eq!(reselect(Some(2), Some(2), &keys[..2]), Some(11));
    // Deleting a row above the selection keeps the same core.
    assert_eq!(reselect(Some(2), Some(0), &[20, 21]), Some(21));
    // A rebuild without deletion keeps the index.
    assert_eq!(reselect(Some(1), None, &[30, 31, 32]), Some(31));
    // Nothing selected falls back to the first row; no rows, no selection.
    assert_eq!(reselect(None, None, &keys), Some(10));
    assert_eq!(reselect(Some(0), Some(0), &[]), None);
    assert_eq!(valid_selection(Some(99), &keys), Some(10));
    assert_eq!(valid_selection(Some(12), &keys), Some(12));
    assert_eq!(valid_selection(Some(12), &[]), None);
}

/// Catches a delete that skips the confirmation: the card's button only asks, and only the
/// dialog's confirm deletes.
#[test]
fn only_the_dialog_confirm_deletes_a_core() {
    assert_eq!(deletes(&CoreAction::AskDelete), None);
    assert_eq!(deletes(&CoreAction::DeleteConfirmed(3)), Some(3));
    assert_eq!(deletes(&CoreAction::Select(3)), None);
}

/// Catches the card's "и для станции" box and the table's cloud glyph drifting onto different
/// fields, and the card's switch not writing `ServerConfig.active`.
#[test]
fn the_card_writes_the_same_fields_as_the_table() {
    let mut s = server("ALPHA", "default");
    assert!(set_for_station(&mut s, true));
    assert!(s.endpoint_to_station);
    assert!(!set_for_station(&mut s, true), "no change, no notify");
    assert!(set_for_station(&mut s, false));
    assert!(!s.endpoint_to_station);

    assert!(set_core_active(&mut s, false));
    assert!(!s.active);
    assert!(set_core_active(&mut s, true));
    assert!(s.active);
}

/// Catches a group change that leaves no group row for the new name.
#[test]
fn moving_a_core_creates_its_group_row() {
    let mut config = AppConfig::headless(vec![server("ALPHA", "default")]);
    assert!(set_core_group(&mut config, 0, " Scalp "));
    assert_eq!(config.servers[0].group, "Scalp");
    assert!(config.groups.iter().any(|g| g.name == "Scalp"));
    assert!(
        !set_core_group(&mut config, 0, "  "),
        "an empty name moves nothing"
    );
    assert!(!set_core_group(&mut config, 5, "x"), "no such core");
}

/// A probe host drawing the Cores page body at the probe's body width.
struct Host {
    scroll: ScrollHandle,
    width: f32,
    build: Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>,
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = (self.build)(window, cx);
        div()
            .w(px(self.width))
            .h_full()
            .debug_selector(|| "probe-body".into())
            .child(
                div()
                    .id("probe-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(div().w_full().flex_none().child(content)),
            )
    }
}

impl Probed for Host {
    fn scroll(&self) -> Option<ScrollHandle> {
        Some(self.scroll.clone())
    }
}

/// Synthetic list row.
fn row(key: u64, name: &str, addr: &str, feeds_on: u8, dot: CoreDot) -> CoreListRow {
    CoreListRow {
        row_key: key,
        draft_index: key as usize,
        core_id: key as _,
        name: name.to_string(),
        addr: addr.to_string(),
        feeds_on,
        active: dot != CoreDot::Off,
        dot,
    }
}

/// Three groups, seven cores, one with a key the core rejected.
fn model() -> CoresModel {
    let ex = |caption: &str, rows| ExBlock {
        caption: caption.to_string(),
        rows,
    };
    CoresModel {
        groups: vec![
            GroupBlock {
                name: "default".into(),
                active: true,
                icon: 0,
                exchanges: vec![
                    ex(
                        "Binance Futures",
                        vec![
                            row(0, "ALPHA ~ F-BN / SUB 07", "10.0.4.17:3100", 9, CoreDot::Ok),
                            row(1, "ALPHA ~ F-BN / SUB 08", "10.0.4.18:3100", 9, CoreDot::Ok),
                            row(
                                2,
                                "ORION ~ F-BN / SUB 21",
                                "10.0.4.21:3100",
                                7,
                                CoreDot::Wait,
                            ),
                        ],
                    ),
                    ex(
                        "Bybit Futures",
                        vec![row(
                            3,
                            "VEGA ~ F-BB / SUB 13",
                            "10.0.5.13:3100",
                            9,
                            CoreDot::Bad,
                        )],
                    ),
                ],
            },
            GroupBlock {
                name: "Scalp".into(),
                active: true,
                icon: 0,
                exchanges: vec![ex(
                    "Binance Futures",
                    vec![
                        row(4, "LYRA ~ F-BN / SUB 30", "10.0.6.30:3100", 9, CoreDot::Ok),
                        row(5, "LYRA ~ F-BN / SUB 31", "10.0.6.31:3100", 8, CoreDot::Off),
                    ],
                )],
            },
            GroupBlock {
                name: "Night shift with a long name".into(),
                active: false,
                icon: 0,
                exchanges: vec![ex(
                    "Not identified",
                    vec![row(6, "DRACO ~ S-OK / SUB 02", "", 9, CoreDot::Unknown)],
                )],
            },
        ],
        selected: Some(3),
    }
}

/// The rejected-key core's card, every disclosure unfolded.
fn card() -> CoreCard {
    CoreCard {
        draft_index: 3,
        name: "VEGA ~ F-BB / SUB 13".into(),
        active: true,
        key_bad: true,
        addr_note: Some("Address duplicates core ALPHA".into()),
        addr_invalid: false,
        has_override: true,
        for_station: false,
        group: "default".into(),
        groups: vec!["default".into(), "Scalp".into()],
        feeds: [true, true, true, true, true, true, true, true, false],
        log_ignored: true,
        proto: Some(TransportVersion::ALL[0]),
        preset: WorkspaceMembership::default(),
        total: TotalMode::default(),
    }
}

/// Input states the card binds.
struct Inputs {
    name: Entity<MoonInputState>,
    key: Entity<MoonInputState>,
    address: Entity<MoonInputState>,
    bundle: Entity<MoonInputState>,
    color: Entity<MoonColorPickerState>,
}

fn inputs(window: &mut Window, cx: &mut App) -> Inputs {
    let field = |text: &str, window: &mut Window, cx: &mut App| {
        let text = text.to_string();
        cx.new(|cx| MoonInputState::new(window, cx).default_value(text))
    };
    Inputs {
        name: field("VEGA ~ F-BB / SUB 13", window, cx),
        key: field("synthetic-key", window, cx),
        address: field("10.0.5.13:3100", window, cx),
        bundle: field("", window, cx),
        color: crate::controls::color_picker::shared_color_state([180, 83, 9], window, cx),
    }
}

/// The page body as the live page builds it, from the synthetic model.
fn body(window: &mut Window, cx: &mut App, fields: &Inputs) -> AnyElement {
    let size = window.viewport_size();
    let (width, height) = (f32::from(size.width), f32::from(size.height));
    let narrow = width < crate::settings::nav::NAV_BREAKPOINT;
    let content_w = crate::settings::layout_probe::body_box((width, height), cx).0;
    let layout = cores_layout(
        narrow,
        content_w,
        card_min_w(cx),
        crate::design::ui_value(cx, 1.0),
    );
    let act: CoreAct = Rc::new(|_, _, _| {});
    let dot = |r: &CoreListRow, cx: &App| plain_dot(r.dot, cx);
    let icon = |_: &GroupBlock, _: &App| div().w(px(16.0)).h(px(16.0)).into_any_element();
    let parts = ListParts {
        dot: &dot,
        icon: &icon,
    };
    let model = model();
    let list = cores_list(&model, &parts, &act, cx);
    let open = |key: &'static str| key == FEEDS_KEY || key == ADVANCED_KEY;
    let card = core_card(
        &card(),
        &CardInputs {
            name: &fields.name,
            key: &fields.key,
            address: &fields.address,
            bundle: &fields.bundle,
            color: &fields.color,
            key_ring: None,
        },
        plain_dot(CoreDot::Bad, cx),
        &open,
        &act,
        cx,
    );
    cores_split(list, Some(card), layout, cx)
}

/// The list, the rejected-key card with both disclosures unfolded, and the empty state stay
/// inside the body, keep list and card apart, wrap their prose uncut and keep the card's footer
/// reachable, in every language at the default and the minimum window.
#[gpui::test]
fn the_cores_page_fits_the_body(cx: &mut gpui::TestAppContext) {
    sweep(
        cx,
        |window, cx: &mut Context<Host>, (width, _)| {
            let fields = inputs(window, cx);
            Host {
                scroll: ScrollHandle::new(),
                width,
                build: Rc::new(move |window, cx| {
                    let act: CoreAct = Rc::new(|_, _, _| {});
                    let add = div().w(px(120.0)).h(px(24.0)).into_any_element();
                    gpui::div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .child(body(window, cx, &fields))
                        .child(cores_empty(&act, add, cx))
                        .into_any_element()
                }),
            }
        },
        |_| ProbeSpec {
            container: "probe-body",
            inside: vec![
                "cores-list".into(),
                "core-card".into(),
                "core-card-foot".into(),
                "core-bad-key".into(),
                // The widest card row: a card narrower than its controls pushes these out.
                "core-card-key".into(),
                "core-card-paste".into(),
                "cores-empty".into(),
            ],
            // The list's scroll box, not the list: below the breakpoint the list scrolls inside
            // its capped box and its far rows are clipped, not overlapping.
            no_overlap: vec![vec![
                "cores-list-box".into(),
                "core-card".into(),
                "cores-empty".into(),
            ]],
            wraps: vec![(
                "cores-empty-body".into(),
                rust_i18n::t!("conn.empty.body").to_string(),
            )],
            last: Some("cores-empty".into()),
            ..Default::default()
        },
    );
}

/// Catches the mockup's split regressing in a wide window: the list squeezed to a narrow column
/// that truncates every address, or the card's fields pinned to the far right with a hole after
/// the label. The list takes [`LIST_SHARE`] of the content, each address shows in full, and the
/// key field starts right after the card's label column.
#[gpui::test]
fn a_wide_cores_page_follows_the_mockup_split(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    let _locale = crate::test_locale::force("ru");
    let size = (1600.0_f32, 900.0_f32);
    let window = cx.open_window(gpui::size(px(size.0), px(size.1)), |window, cx| {
        let (width, _) = crate::settings::layout_probe::body_box(size, cx);
        let fields = inputs(window, cx);
        Host {
            scroll: ScrollHandle::new(),
            width,
            build: Rc::new(move |window, cx| body(window, cx, &fields)),
        }
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    for _ in 0..3 {
        visual.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
    let get = |visual: &mut gpui::VisualTestContext, sel: &'static str| {
        visual
            .debug_bounds(sel)
            .unwrap_or_else(|| panic!("{sel} was not laid out"))
    };
    let body_w = f32::from(get(&mut visual, "probe-body").size.width);
    let list_w = f32::from(get(&mut visual, "cores-list-box").size.width);
    assert!(
        (list_w - body_w * LIST_SHARE).abs() < 1.0,
        "list {list_w} of {body_w}"
    );

    for key in [1_u64, 2, 3] {
        let sel = crate::settings::layout_probe::leak(format!("cores-addr-{key}"));
        let addr = get(&mut visual, sel);
        let text = model()
            .groups
            .iter()
            .flat_map(|g| g.exchanges.iter())
            .flat_map(|e| e.rows.iter())
            .find(|r| r.row_key == key)
            .map(|r| r.addr.clone())
            .expect("synthetic row");
        let need = visual.update(|_, cx| crate::design::mono_body_text_width(cx, &text, 400.0));
        assert!(
            f32::from(addr.size.width) + 0.5 >= need,
            "address {key} is cut: {} < {need}",
            f32::from(addr.size.width)
        );
    }

    let card = get(&mut visual, "core-card");
    let key = get(&mut visual, "core-card-key");
    // Card padding 14, label column, row gap 16, plus a pixel of border.
    let lead = visual.update(|_, cx| crate::design::ui_value(cx, 14.0 + CARD_LABEL_W + 16.0)) + 2.0;
    let offset = f32::from(key.origin.x - card.origin.x);
    assert!(offset <= lead, "key field starts {offset} px into the card");
}

/// The card's captions sit under their field, not under the narrow label column, and a caption
/// that fits the field column is one line, in every language at every swept size and a wide
/// window.
///
/// The one-line oracle is the caption's own measured width against its box: when the text is no
/// wider than the box, the box must be shorter than two caption lines.
///
/// Breaks on: the hint moving back under the label, where "Каждая группа открывается в своём
/// главном окне." wrapped to three or four lines in a ~120 px column.
#[gpui::test]
fn card_captions_sit_under_their_field_on_one_line(cx: &mut gpui::TestAppContext) {
    use crate::settings::layout_probe::{SIZES, body_box, leak};
    cx.update(moon_ui::init);
    for language in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        for size in SIZES.into_iter().chain([(1600.0, 900.0)]) {
            let window = cx.open_window(gpui::size(px(size.0), px(size.1)), |window, cx| {
                let (width, _) = body_box(size, cx);
                let fields = inputs(window, cx);
                Host {
                    scroll: ScrollHandle::new(),
                    width,
                    build: Rc::new(move |window, cx| body(window, cx, &fields)),
                }
            });
            let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
            for _ in 0..3 {
                visual.update(|window, cx| {
                    let _ = window.draw(cx);
                });
            }
            let at = format!("{} at {}x{}", language.code(), size.0, size.1);
            let label = visual
                .debug_bounds(leak("core-card-group-label".into()))
                .unwrap_or_else(|| panic!("group label was not laid out ({at})"));
            let hint = visual
                .debug_bounds(leak("core-card-group-hint".into()))
                .unwrap_or_else(|| panic!("group hint was not laid out ({at})"));
            assert!(
                hint.origin.x >= label.right(),
                "the group hint starts under the label column ({at})"
            );
            let text = rust_i18n::t!("conn.card.group_hint").to_string();
            let (text_w, line) = visual.update(|_, cx| {
                (
                    crate::design::ui_caption_text_width(cx, &text, 400.0),
                    f32::from(crate::design::line_px(cx, 12.0)),
                )
            });
            let box_w = f32::from(hint.size.width);
            if text_w <= box_w {
                assert!(
                    f32::from(hint.size.height) < 2.0 * line,
                    "the group hint wraps although it fits {box_w} px: {} tall ({at})",
                    f32::from(hint.size.height)
                );
            }
        }
    }
}
