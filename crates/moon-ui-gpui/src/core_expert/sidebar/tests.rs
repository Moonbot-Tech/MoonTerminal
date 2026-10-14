//! Production rail geometry with realistic names, freshness states and every translation.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use gpui::{Bounds, Context, IntoElement, ParentElement, Pixels, Render, Styled, Window, px};
use moon_core::config::{Language, UiThemeMode};
use moon_core::session::CoreRunState;
use moon_core::venue::CoreVenue;
use moon_ui::{MoonTheme, h_flex};
use rust_i18n::t;

use super::{RailAction, RailPresentation, RailWidthCache, rail};
use crate::controls::row_selection::RowSelection;
use crate::core_expert::cores::{CoreRoster, RosterRow, RosterSection};
use crate::design;

/// Production rail input: twenty long names, a connected wait and an offline retained page.
fn roster() -> CoreRoster {
    CoreRoster::from_sections(
        [4, 2]
            .into_iter()
            .enumerate()
            .map(|(section, code)| {
                let venue = CoreVenue::identify(code, "", None);
                RosterSection {
                    label: crate::controls::venue_section_label(Some(&venue)).into(),
                    venue: Some(venue),
                    rows: (1..=10)
                        .map(|index| {
                            let core = section as u64 * 10 + index;
                            RosterRow {
                                core,
                                name: format!(
                                    "CORE_ALPHA$22 ~ EX-XX / UNIT_LONG_NAME (SUB_01) TOKEN_{core:02}"
                                )
                                .into(),
                                has_page: core != 2,
                                connected: core != 3,
                                failed: false,
                                run: CoreRunState {
                                    online: core != 3,
                                    trading: Some(core.is_multiple_of(2)),
                                    trading_confirmed: true,
                                    ..CoreRunState::default()
                                },
                            }
                        })
                        .collect(),
                }
            })
            .collect(),
        0,
    )
}

/// Fixture view calls the complete production builder, including MoonUI badges and disclosure.
struct RailProbe {
    width_cache: RailWidthCache,
    roster: CoreRoster,
    selection: RowSelection<u64>,
    collapsed: HashSet<Option<moon_core::feed::ExchangeId>>,
    action: Rc<dyn Fn(RailAction, &mut gpui::App)>,
}

impl Render for RailProbe {
    /// Render production chrome at the window's actual zoom and theme.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex().size_full().child(rail(
            RailPresentation {
                width_cache: &self.width_cache,
                viewport_width: crate::window::windowing::responsive_width(window),
                roster: &self.roster,
                selection: &self.selection,
                anchor: Some(2),
                collapsed: &self.collapsed,
                logos_ready: true,
                action: self.action.clone(),
            },
            MoonTheme::active_tokens(cx).palette,
            cx,
        ))
    }
}

/// Bounds containment with the same half-pixel rounding tolerance as the crate's layout tests.
fn inside(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    f32::from(inner.origin.x) + 0.5 >= f32::from(outer.origin.x)
        && f32::from(inner.right()) <= f32::from(outer.right()) + 0.5
        && f32::from(inner.origin.y) + 0.5 >= f32::from(outer.origin.y)
        && f32::from(inner.bottom()) <= f32::from(outer.bottom()) + 0.5
}

/// Read a production debug selector without retaining a mutable borrow across text measurement.
fn bounds(visual: &mut gpui::VisualTestContext, id: &'static str) -> Bounds<Pixels> {
    visual
        .debug_bounds(id)
        .unwrap_or_else(|| panic!("missing {id}"))
}

/// A cached width must grow after a rename and shrink again when that long name disappears.
#[gpui::test]
fn expert_rail_width_invalidates_after_rename(cx: &mut gpui::TestAppContext) {
    let _locale = crate::test_locale::force("en");
    cx.update(|app| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Dark, 1.0),
            app,
        );
        let cache = RailWidthCache::default();
        let selection = RowSelection::default();
        let collapsed = HashSet::new();
        let measure = |roster: &CoreRoster| {
            cache.width(
                &RailPresentation {
                    width_cache: &cache,
                    viewport_width: 860.0,
                    roster,
                    selection: &selection,
                    anchor: None,
                    collapsed: &collapsed,
                    logos_ready: true,
                    action: Rc::new(|_, _| {}),
                },
                &[],
                app,
            )
        };
        let mut input = roster();
        let original = measure(&input);
        let previous = input.sections[0].rows[0].name.clone();
        input.sections[0].rows[0].name =
            format!("{previous} EXTRA_DISTINGUISHING_ACCOUNT_TAIL").into();
        let renamed = measure(&input);
        assert!(renamed > original, "renamed core retained stale geometry");
        input.sections[0].rows[0].name = previous;
        assert_eq!(
            measure(&input),
            original,
            "removed long name retained excess width"
        );
    });
}

/// Fixed rail widths, ellipsised tails, shrinking chips or missing zoom must fail here.
#[gpui::test]
fn expert_rail_keeps_names_chips_and_chrome_inside_in_every_locale(cx: &mut gpui::TestAppContext) {
    crate::media::exchange_logos::prewarm();
    for locale in Language::ALL {
        let _locale = crate::test_locale::force(locale.code());
        for theme in [
            UiThemeMode::Light,
            UiThemeMode::Dark,
            UiThemeMode::Graphite,
            UiThemeMode::LightExperimental,
            UiThemeMode::DarkExperimental,
        ] {
            for zoom in [0.75, 1.0, 2.0] {
                cx.update(|app| {
                    MoonTheme::install_config(
                        crate::startup::moon_theme_config_for_presentation(theme, zoom),
                        app,
                    )
                });
                let handle =
                    cx.open_window(gpui::size(px(860.0 * zoom), px(900.0 * zoom)), |_, _| {
                        let mut selection = RowSelection::default();
                        selection.select_only(Some(2));
                        RailProbe {
                            width_cache: RailWidthCache::default(),
                            roster: roster(),
                            selection,
                            collapsed: HashSet::new(),
                            action: Rc::new(|_, _| {}),
                        }
                    });
                let mut visual = gpui::VisualTestContext::from_window(handle.into(), cx);
                // Production Root installs zoom on the window, not on token sizes. This probe
                // supplies the same host contract without constructing a backend or live app.
                visual.update(|window, app| window.set_content_zoom(zoom, app));
                for _ in 0..3 {
                    visual.update(|window, app| {
                        let _ = window.draw(app);
                    });
                }
                let rail_box = bounds(&mut visual, "expert-rail");
                let actual_zoom = visual.update(|window, _| window.content_zoom());
                assert_eq!(actual_zoom, zoom);
                assert!(
                    f32::from(rail_box.size.width) >= 300.0,
                    "rail lost its width floor: {rail_box:?}"
                );
                let viewport = bounds(&mut visual, "expert-rail-viewport");
                assert!(f32::from(viewport.size.width) <= 860.0 * 0.45 + 0.5);
                assert!(
                    viewport.size.width < rail_box.size.width,
                    "long names need horizontal access"
                );
                let header = bounds(&mut visual, "expert-header");
                let summary = bounds(&mut visual, "expert-summary");
                let list = bounds(&mut visual, "expert-list");
                let footer = bounds(&mut visual, "expert-footer");
                for element in [header, summary, list, footer] {
                    assert!(
                        inside(element, rail_box),
                        "chrome overflow {locale:?}/{zoom}: {element:?}"
                    );
                }
                assert!(
                    f32::from(header.bottom()) <= f32::from(summary.origin.y) + 0.5
                        && f32::from(summary.bottom()) <= f32::from(list.origin.y) + 0.5
                        && f32::from(list.bottom()) <= f32::from(footer.origin.y) + 0.5,
                    "chrome overlaps in {locale:?}/{theme:?}/{zoom}: header {header:?}, summary {summary:?}, list {list:?}, footer {footer:?}"
                );
                for (id, text, weight, mono, body) in [
                    (
                        "expert-title",
                        t!("core_expert.cores_title").to_string(),
                        600.0,
                        false,
                        true,
                    ),
                    (
                        "expert-selected",
                        t!("core_expert.cores_selected", n = 1).to_string(),
                        400.0,
                        true,
                        false,
                    ),
                    (
                        "expert-summary-cores",
                        t!("workspace.summary_cores", n = 20).to_string(),
                        400.0,
                        false,
                        false,
                    ),
                    (
                        "expert-summary-ready",
                        t!("workspace.summary_ready", n = 18).to_string(),
                        400.0,
                        false,
                        false,
                    ),
                    (
                        "expert-summary-problem",
                        t!("workspace.summary_problem", n = 2).to_string(),
                        400.0,
                        false,
                        false,
                    ),
                ] {
                    let text_box = bounds(&mut visual, id);
                    assert!(inside(text_box, rail_box));
                    let width = visual.update(|_, app| {
                        if mono {
                            design::mono_caption_text_width(app, &text, weight)
                        } else if body {
                            design::ui_body_text_width(app, &text, weight)
                        } else {
                            design::ui_caption_text_width(app, &text, weight)
                        }
                    });
                    assert!(
                        width <= f32::from(text_box.size.width) + 0.5,
                        "{id} clips in {locale:?}: {width} / {text_box:?}"
                    );
                }
                let footer_text = bounds(&mut visual, "expert-footer-text");
                assert!(inside(footer_text, footer));
                let hint = t!("core_expert.rail_hint").to_string();
                let required_height = visual.update(|window, app| {
                    let runs = [gpui::TextRun {
                        len: hint.len(),
                        font: gpui::font(design::ui_font()),
                        ..gpui::TextRun::default()
                    }];
                    window
                        .text_system()
                        .shape_text(
                            hint.clone().into(),
                            design::t_caption(app),
                            &runs,
                            Some(footer_text.size.width),
                            None,
                        )
                        .expect("footer shaping")
                        .iter()
                        .map(|line| f32::from(line.size(px(18.0)).height))
                        .sum::<f32>()
                });
                assert!(
                    required_height <= f32::from(footer_text.size.height) + 0.5,
                    "footer text clipped in {locale:?}: needs {required_height}, has {footer_text:?}"
                );
                for word in hint.split_whitespace() {
                    let width =
                        visual.update(|_, app| design::ui_caption_text_width(app, word, 400.0));
                    assert!(
                        width <= f32::from(footer_text.size.width) + 0.5,
                        "unwrappable footer word"
                    );
                }
                let mut previous = None;
                for core in 1..=20 {
                    let row = bounds(
                        &mut visual,
                        Box::leak(format!("expert-row-{core}").into_boxed_str()),
                    );
                    let name = bounds(
                        &mut visual,
                        Box::leak(format!("expert-name-{core}").into_boxed_str()),
                    );
                    assert!(
                        inside(row, rail_box) && inside(name, row),
                        "row/name overflow: {core}"
                    );
                    let text =
                        format!("CORE_ALPHA$22 ~ EX-XX / UNIT_LONG_NAME (SUB_01) TOKEN_{core:02}");
                    let required = visual.update(|window, app| {
                        let runs = [gpui::TextRun {
                            len: text.len(),
                            font: gpui::Font {
                                weight: if core == 2 {
                                    gpui::FontWeight::SEMIBOLD
                                } else {
                                    gpui::FontWeight::NORMAL
                                },
                                ..gpui::font(design::mono())
                            },
                            ..gpui::TextRun::default()
                        }];
                        window
                            .text_system()
                            .shape_line(text.into(), design::t_caption(app), &runs, None)
                            .width
                    });
                    assert!(
                        required <= name.size.width + px(0.5),
                        "tail clipped: {locale:?}/{core}"
                    );
                    assert!(
                        (f32::from(row.size.height) * actual_zoom - 30.0 * zoom).abs() < 0.5,
                        "row shrank"
                    );
                    if let Some(previous) = previous {
                        assert!(
                            f32::from(row.origin.y) + 0.5 >= f32::from(previous),
                            "rows overlap in {locale:?}/{theme:?}/{zoom} at {core}: {row:?}, previous bottom {previous:?}"
                        );
                    }
                    previous = Some(row.bottom());
                    if [2, 3].contains(&core) {
                        let chip = bounds(
                            &mut visual,
                            Box::leak(format!("expert-chip-{core}").into_boxed_str()),
                        );
                        assert!(
                            inside(chip, row) && name.right() <= chip.origin.x,
                            "name under chip: {locale:?}/{core}"
                        );
                    }
                }
                // Drive horizontal access: the distinguishing tail must be reachable at minimum width.
                visual.simulate_event(gpui::ScrollWheelEvent {
                    position: gpui::point(viewport.center().x, header.center().y),
                    delta: gpui::ScrollDelta::Pixels(gpui::point(px(-10000.0), px(0.0))),
                    ..Default::default()
                });
                visual.update(|window, app| {
                    let _ = window.draw(app);
                });
                let scrolled = bounds(&mut visual, "expert-rail");
                assert!(scrolled.origin.x < rail_box.origin.x, "rail did not scroll");
                for id in ["expert-name-1", "expert-name-2"] {
                    let name = bounds(&mut visual, id);
                    assert!(
                        name.right() <= viewport.right() + px(0.5),
                        "tail unreachable: {id}"
                    );
                }
                visual.simulate_resize(gpui::size(px(2000.0 * zoom), px(900.0 * zoom)));
                visual.update(|window, app| {
                    let _ = window.draw(app);
                });
                let wide = bounds(&mut visual, "expert-rail-viewport");
                let content = bounds(&mut visual, "expert-rail");
                assert_eq!(
                    wide.size.width, content.size.width,
                    "wide window still clips names"
                );
            }
        }
    }
}

/// Nested scrolling must never pan names on a vertical wheel or move rows on a horizontal wheel.
#[gpui::test]
fn expert_rail_nested_wheels_stay_on_their_own_axis(cx: &mut gpui::TestAppContext) {
    let _locale = crate::test_locale::force("en");
    cx.update(|app| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Dark, 1.0),
            app,
        )
    });
    let handle = cx.open_window(gpui::size(px(860.0), px(400.0)), |_, _| RailProbe {
        width_cache: RailWidthCache::default(),
        roster: roster(),
        selection: RowSelection::default(),
        collapsed: HashSet::new(),
        action: Rc::new(|_, _| {}),
    });
    let mut visual = gpui::VisualTestContext::from_window(handle.into(), cx);
    for _ in 0..3 {
        visual.update(|window, app| {
            let _ = window.draw(app);
        });
    }
    let before = bounds(&mut visual, "expert-name-2");
    let row = bounds(&mut visual, "expert-row-5");
    let position = gpui::point(px(100.0), row.center().y);
    visual.simulate_event(gpui::ScrollWheelEvent {
        position,
        delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(-30.0))),
        ..Default::default()
    });
    visual.update(|window, app| {
        let _ = window.draw(app);
    });
    let vertical = bounds(&mut visual, "expert-name-2");
    assert_eq!(
        vertical.origin.x, before.origin.x,
        "vertical wheel panned core names"
    );
    assert!(
        vertical.origin.y < before.origin.y,
        "vertical list did not scroll"
    );
    visual.simulate_event(gpui::ScrollWheelEvent {
        position,
        delta: gpui::ScrollDelta::Pixels(gpui::point(px(-30.0), px(0.0))),
        ..Default::default()
    });
    visual.update(|window, app| {
        let _ = window.draw(app);
    });
    let horizontal = bounds(&mut visual, "expert-name-2");
    assert!(
        horizontal.origin.x < vertical.origin.x,
        "horizontal rail did not scroll"
    );
    assert_eq!(
        horizontal.origin.y, vertical.origin.y,
        "horizontal wheel moved core rows"
    );
}

/// A disclosure click must never select a venue; row clicks retain their native modifiers.
#[gpui::test]
fn expert_rail_caret_and_selection_have_separate_click_targets(cx: &mut gpui::TestAppContext) {
    let _locale = crate::test_locale::force("en");
    cx.update(|app| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Dark, 1.0),
            app,
        )
    });
    let events: Rc<RefCell<Vec<RailAction>>> = Rc::default();
    let sink = events.clone();
    let handle = cx.open_window(gpui::size(px(860.0), px(900.0)), |_, _| RailProbe {
        width_cache: RailWidthCache::default(),
        roster: roster(),
        selection: RowSelection::default(),
        collapsed: HashSet::new(),
        action: Rc::new(move |action, _| sink.borrow_mut().push(action)),
    });
    let mut visual = gpui::VisualTestContext::from_window(handle.into(), cx);
    for _ in 0..3 {
        visual.update(|window, app| {
            let _ = window.draw(app);
        });
    }
    for selector in [
        "expert-venue-4-0-caret",
        "expert-venue-4-0-label",
        "expert-name-2",
    ] {
        let position = bounds(&mut visual, selector).center();
        visual.simulate_click(position, gpui::Modifiers::none());
    }
    let events = events.borrow();
    assert_eq!(events.len(), 3, "a child click leaked to the other target");
    assert!(matches!(events[0], RailAction::Collapse(Some(id), false) if id.code == 4));
    assert!(matches!(events[1], RailAction::Venue(Some(id)) if id.code == 4));
    assert!(
        matches!(events[2], RailAction::Core(2, modifiers) if !modifiers.shift && !modifiers.secondary())
    );
}
