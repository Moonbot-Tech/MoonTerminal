use super::{
    ALL_TAGS, TgTag, card, card_header, card_row, core_list, core_row, disconnected, error_banner,
    note_box, short_tag_text, split, split_side, tag_text, tg_tag,
};
use crate::design;
use gpui::{AnyElement, App, InteractiveElement, IntoElement, ParentElement, Styled, div, px};
use moon_core::feed::{ConnStatus, CoreTelegramError, CoreTelegramService, CoreTelegramState};
use moon_ui::{MoonPalette, v_flex};
use rust_i18n::t;

fn snapshot(enabled: bool, supported: bool, auth: Option<&str>) -> CoreTelegramState {
    CoreTelegramState {
        enabled,
        service_online: true,
        state_supported: supported,
        service: auth.map(|a| CoreTelegramService {
            auth_state: a.to_string(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[test]
fn a_core_that_is_not_ready_is_unreachable_whatever_it_held() {
    let s = snapshot(true, true, Some("ready"));
    for status in [
        None,
        Some(ConnStatus::Disconnected),
        Some(ConnStatus::Connecting),
        Some(ConnStatus::Failed("synthetic".into())),
    ] {
        assert_eq!(tg_tag(status.as_ref(), true, Some(&s)), TgTag::NoCore);
    }
}

#[test]
fn a_missing_or_stale_snapshot_is_unknown_never_guessed() {
    let ready = Some(&ConnStatus::Ready);
    assert_eq!(tg_tag(ready, false, None), TgTag::Unknown);
    assert_eq!(tg_tag(ready, true, None), TgTag::Unknown);
    let s = snapshot(true, true, Some("ready"));
    assert_eq!(tg_tag(ready, false, Some(&s)), TgTag::Unknown);
    // Reader on but no service snapshot yet: the auth step is not known.
    let s = snapshot(true, true, None);
    assert_eq!(tg_tag(ready, true, Some(&s)), TgTag::Unknown);
    assert_eq!(tag_text(TgTag::Unknown), "—");
}

#[test]
fn each_reader_state_maps_to_its_tag() {
    let ready = Some(&ConnStatus::Ready);
    let tag = |s: CoreTelegramState| tg_tag(ready, true, Some(&s));
    assert_eq!(tag(snapshot(true, true, Some("ready"))), TgTag::LoggedIn);
    assert_eq!(
        tag(snapshot(true, true, Some("wait_code"))),
        TgTag::NotLoggedIn
    );
    assert_eq!(
        tag(snapshot(true, true, Some("starting"))),
        TgTag::NotLoggedIn
    );
    assert_eq!(tag(snapshot(false, true, None)), TgTag::ReaderOff);
    assert_eq!(tag(snapshot(true, false, None)), TgTag::Unsupported);
    assert_eq!(tag(snapshot(false, false, None)), TgTag::Unsupported);
    let mut err = snapshot(true, true, Some("ready"));
    if let Some(svc) = err.service.as_mut() {
        svc.proxy_error = Some(CoreTelegramError {
            code: 1,
            message: "synthetic refused".into(),
        });
    }
    assert_eq!(tag(err.clone()), TgTag::ProxyError);
    // An empty error message is no error.
    if let Some(svc) = err.service.as_mut() {
        svc.proxy_error = Some(CoreTelegramError::default());
    }
    assert_eq!(tag(err), TgTag::LoggedIn);
}

/// A long synthetic core name, so the name truncates and the tag must keep its room.
const LONG_NAME: &str = "SYNTHETIC-CORE-WITH-A-VERY-LONG-NAME-0001";

/// The page as the probe draws it: the list with one row per tag (the longest included), beside
/// or above a card holding a header with the toggle, an error banner, every body row and the
/// action row; under it the disconnected empty state and the unsupported note.
fn probe_page(column_w: f32, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    let rows = ALL_TAGS
        .iter()
        .enumerate()
        .map(|(i, tag)| {
            core_row(i as u64, LONG_NAME.to_string(), p.green, *tag, i == 0, cx).into_any_element()
        })
        .collect();
    let value = |sel: &'static str, text: String| {
        div()
            .debug_selector(move || sel.into())
            .font_family(design::ui_font())
            .child(text)
            .into_any_element()
    };
    let toggle = moon_ui::MoonToggle::new("probe-tg-enable")
        .label(t!("telegram_core.enable").to_string())
        .mono(false)
        .checked(true)
        .into_any_element();
    let body = card(
        vec![
            card_header(LONG_NAME.to_string(), TgTag::ProxyError, Some(toggle), cx),
            error_banner(
                "tgcore-err-proxy".into(),
                t!("telegram_core.proxy_error").to_string(),
                "synthetic connection refused (198.51.100.9:1080)".into(),
                cx,
            ),
            card_row(
                "state",
                t!("telegram_core.client_state").to_string(),
                value(
                    "probe-v-state",
                    t!("telegram_core.account_reader_off").to_string(),
                ),
                cx,
            ),
            card_row(
                "login",
                t!("telegram_core.row_login").to_string(),
                value("probe-v-login", t!("telegram_core.phone_hint").to_string()),
                cx,
            ),
            card_row(
                "proxy",
                t!("tgcore.proxy.title").to_string(),
                value(
                    "probe-v-proxy",
                    t!("telegram_core.proxy_secret_hint").to_string(),
                ),
                cx,
            ),
            card_row(
                "actual",
                t!("telegram_core.proxy_actual").to_string(),
                value(
                    "probe-v-actual",
                    t!("telegram_core.proxy_actual_unknown").to_string(),
                ),
                cx,
            ),
        ],
        cx,
    );
    let reconnect = moon_ui::MoonButton::new("probe-reconnect")
        .ghost()
        .mono(false)
        .label(t!("telegram_core.reconnect").to_string())
        .render()
        .into_any_element();
    let empty = card(
        vec![
            card_header(LONG_NAME.to_string(), TgTag::NoCore, None, cx),
            disconnected(reconnect, cx),
            note_box(
                "tgcore-unsupported",
                t!("telegram_core.unsupported_ver", v = "7.21").to_string(),
                cx,
            ),
        ],
        cx,
    );
    v_flex()
        .w(px(column_w))
        .gap(px(12.0))
        .child(split(core_list(rows, cx), body, column_w, cx))
        .child(div().debug_selector(|| "probe-empty".into()).child(empty))
        .into_any_element()
}

/// The list rows (dot + long name + every tag, the longest included), the card's rows, and the
/// empty and error states stay inside the body, unclipped and apart — every language, every
/// probe window size.
#[gpui::test]
fn the_tgcore_page_fits_in_every_locale(cx: &mut gpui::TestAppContext) {
    use crate::settings::layout_probe::{ProbeSpec, sweep};
    use crate::settings::telegram::tests::Host;
    sweep(
        cx,
        |_, _, (width, _)| Host {
            width,
            scroll: gpui::ScrollHandle::new(),
            build: std::rc::Rc::new(move |_, cx| probe_page(width, cx)),
        },
        |_| {
            let mut inside = vec![
                "tgcore-list".to_string(),
                "tgcore-card".into(),
                "tgcore-head-name".into(),
                "tgcore-head-tag".into(),
                "tgcore-head-toggle".into(),
                "tgcore-err-proxy".into(),
                "tgcore-empty".into(),
                "tgcore-empty-title".into(),
                "tgcore-empty-hint".into(),
                "tgcore-unsupported".into(),
            ];
            let mut no_overlap = vec![vec![
                "tgcore-head-name".to_string(),
                "tgcore-head-tag".into(),
                "tgcore-head-toggle".into(),
            ]];
            for key in ["state", "login", "proxy", "actual"] {
                inside.push(format!("tgcore-l-{key}"));
                inside.push(format!("tgcore-v-{key}"));
                no_overlap.push(vec![format!("tgcore-l-{key}"), format!("tgcore-v-{key}")]);
            }
            let mut wraps = vec![
                (
                    "tgcore-empty-hint".to_string(),
                    t!("telegram_core.disconnected_hint").to_string(),
                ),
                (
                    "probe-v-proxy".to_string(),
                    t!("telegram_core.proxy_secret_hint").to_string(),
                ),
            ];
            for (key, label) in [
                ("state", t!("telegram_core.client_state")),
                ("login", t!("telegram_core.row_login")),
                ("proxy", t!("tgcore.proxy.title")),
                ("actual", t!("telegram_core.proxy_actual")),
            ] {
                wraps.push((format!("tgcore-l-{key}"), label.to_string()));
            }
            for i in 0..ALL_TAGS.len() {
                inside.push(format!("tgcore-row-{i}"));
                for part in ["dot", "name", "tag"] {
                    inside.push(format!("tgcore-{part}-{i}"));
                }
                no_overlap.push(vec![
                    format!("tgcore-dot-{i}"),
                    format!("tgcore-name-{i}"),
                    format!("tgcore-tag-{i}"),
                ]);
            }
            ProbeSpec {
                container: "probe-body",
                inside,
                no_overlap,
                wraps,
                last: Some("probe-empty".into()),
                ..Default::default()
            }
        },
    );
}

/// Every row's tag lies wholly inside its row, so the longest tag is never cut by the list edge.
#[gpui::test]
fn every_tag_sits_inside_its_row(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    for language in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        let window = cx.open_window(gpui::size(px(700.0), px(900.0)), |_, _| {
            crate::settings::telegram::tests::Host {
                width: 600.0,
                scroll: gpui::ScrollHandle::new(),
                build: std::rc::Rc::new(|_, cx| probe_page(600.0, cx)),
            }
        });
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        for _ in 0..3 {
            visual.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
        let leak = crate::settings::layout_probe::leak;
        for i in 0..ALL_TAGS.len() {
            let row = visual
                .debug_bounds(leak(format!("tgcore-row-{i}")))
                .expect("row drawn");
            let tag = visual
                .debug_bounds(leak(format!("tgcore-tag-{i}")))
                .expect("tag drawn");
            let name = visual
                .debug_bounds(leak(format!("tgcore-name-{i}")))
                .expect("name drawn");
            let code = language.code();
            assert!(
                f32::from(tag.right()) <= f32::from(row.right()) + 0.5,
                "{code}: tag {i} spills past its row"
            );
            assert!(
                f32::from(name.right()) <= f32::from(tag.left()) + 0.5,
                "{code}: name {i} runs under its tag"
            );
            assert!(f32::from(tag.size.width) > 0.0, "{code}: tag {i} collapsed");
        }
    }
}

/// A typical 30-character synthetic core name.
const NAME_30: &str = "CORE_A synthetic subacc No 031";

/// The page body as `render` bounds it for this tab: a fixed-height window holding the page
/// column, the list of 63 cores beside a short card.
struct BoundedProbe {
    width: f32,
}

impl gpui::Render for BoundedProbe {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let rows = (0..63u64)
            .map(|i| {
                let tag = ALL_TAGS[i as usize % ALL_TAGS.len()];
                core_row(i, NAME_30.to_string(), p.green, tag, i == 0, cx).into_any_element()
            })
            .collect();
        let body = card(
            vec![note_box(
                "tgcore-pick",
                t!("telegram_core.core_pick_none").to_string(),
                cx,
            )],
            cx,
        );
        v_flex().size_full().child(
            v_flex()
                .w(px(self.width))
                .flex_1()
                .min_h(px(0.0))
                .child(split(core_list(rows, cx), body, self.width, cx)),
        )
    }
}

/// Beside the card the list fills the body's height and grows with the window, and a
/// 30-character name keeps at least 24 characters beside the longest short tag — every language.
#[gpui::test]
fn the_core_list_fills_the_body_and_keeps_names_readable(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    let leak = crate::settings::layout_probe::leak;
    for language in moon_core::config::Language::ALL {
        let code = language.code();
        let _locale = crate::test_locale::force(code);
        let mut heights = Vec::new();
        for height in [600.0, 900.0] {
            let width = 1200.0;
            let window = cx.open_window(gpui::size(px(width), px(height)), |_, _| BoundedProbe {
                width,
            });
            let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
            for _ in 0..3 {
                visual.update(|window, cx| {
                    let _ = window.draw(cx);
                });
            }
            assert!(
                visual.update(|_, cx| split_side(width, cx)),
                "{code}: stacked"
            );
            let list = visual
                .debug_bounds("tgcore-list-scroll")
                .expect("list drawn");
            assert!(
                f32::from(list.size.height) + 1.0 >= height,
                "{code}: the list stops short of the body ({} < {height})",
                f32::from(list.size.height)
            );
            assert!(
                f32::from(list.bottom()) <= height + 0.5,
                "{code}: list overflows"
            );
            heights.push(f32::from(list.size.height));
            let widths: Vec<f32> = visual.update(|_, cx| {
                ALL_TAGS
                    .iter()
                    .map(|t| design::ui_body_text_width(cx, &short_tag_text(*t), 400.0))
                    .collect()
            });
            let longest = (0..widths.len())
                .max_by(|a, b| widths[*a].total_cmp(&widths[*b]))
                .expect("tags");
            let name = visual
                .debug_bounds(leak(format!("tgcore-name-{longest}")))
                .expect("name drawn");
            let tag = visual
                .debug_bounds(leak(format!("tgcore-tag-{longest}")))
                .expect("tag drawn");
            let need =
                visual.update(|_, cx| design::mono_body_text_width(cx, &NAME_30[..24], 400.0));
            assert!(
                f32::from(name.size.width) + 0.5 >= need,
                "{code}: name cut under 24 characters ({} < {need})",
                f32::from(name.size.width)
            );
            assert!(
                name.right() <= tag.left() + px(0.5),
                "{code}: name under its tag"
            );
        }
        assert!(
            heights[1] - heights[0] >= 290.0,
            "{code}: the list did not grow with the window: {heights:?}"
        );
    }
}
