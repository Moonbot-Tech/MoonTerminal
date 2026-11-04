//! Layout coverage for the header update button's reservation in the manual-strategy fit (#1191).

use gpui::{InteractiveElement, IntoElement, ParentElement, Render, Styled, div, px};
use moon_core::config::UiThemeMode;
use moon_core::update::ReleaseVersion;

use super::{cluster, update_button_label, update_cluster_reserve_w};
use crate::design;
use crate::update::UpdateState;

/// Synthetic offered version; any stable triple works.
const VERSION: ReleaseVersion = ReleaseVersion {
    major: 0,
    minor: 99,
    patch: 12,
};

/// Width standing right of the update cluster (ticker, quiet, clock, window controls) in the
/// probe: a fixed block, since only the boundary on the button's LEFT is under test.
const RIGHT_REST_W: f32 = 320.0;

/// Every face the button can show, one per distinct label.
fn states() -> [UpdateState; 4] {
    [
        UpdateState::Available(VERSION),
        UpdateState::Installing(VERSION),
        UpdateState::Failed {
            version: VERSION,
            message: "synthetic".into(),
        },
        UpdateState::Opened(VERSION),
    ]
}

/// Font deltas the terminal ships: the default and the presentation mapping's.
fn font_deltas() -> [f32; 2] {
    [
        0.0,
        crate::startup::moon_theme_config_for_presentation(UiThemeMode::Light, 1.0)
            .light
            .scale
            .font_delta,
    ]
}

/// One header-shaped row: the strategy names take exactly what the fit ladder allows them once
/// the update reservation is spent (the ladder's guarantee is `base + row <= chrome_width`),
/// then the drag spacer, the real update cluster and the right-hand readouts.
struct HeaderProbe {
    state: UpdateState,
    width: f32,
}

impl Render for HeaderProbe {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let p = moon_ui::MoonPalette::active(cx);
        let reserve = update_cluster_reserve_w(cx, &self.state);
        let names_w = (self.width - reserve - RIGHT_REST_W).max(0.0);
        let label = update_button_label(&self.state).expect("visible state has a label");
        moon_ui::h_flex()
            .w(px(self.width))
            .h(design::header_height_px(cx))
            .gap(design::ui_px(cx, design::CHROME_GAP))
            .child(
                div()
                    .debug_selector(|| "probe-names".into())
                    .flex_none()
                    .w(px(names_w - 2.0 * design::ui_value(cx, design::CHROME_GAP)))
                    .h_full(),
            )
            .child(div().flex_1().min_w_0())
            .child(cluster(
                label,
                &self.state,
                String::new(),
                p,
                cx,
                |_, _, _| {},
            ))
            .child(
                div()
                    .debug_selector(|| "probe-right".into())
                    .flex_none()
                    .w(px(RIGHT_REST_W - design::ui_value(cx, design::CHROME_GAP)))
                    .h_full(),
            )
    }
}

/// `update_button.rs:update_cluster_reserve_w` must cover the button as MoonUI renders it at every
/// label, locale and shipped font delta, and a strategy row that fits its ladder must then stop at
/// the button's left edge. Dropping the reservation (or measuring a shorter label than the one on
/// the button, e.g. always `update.available`) puts the names under `Установка обновления...`.
#[gpui::test]
fn update_button_never_sits_under_the_strategy_names(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        moon_ui::MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Light, 1.0),
            cx,
        )
    });
    for code in ["ru", "en"] {
        let _locale = crate::test_locale::force(code);
        for delta in font_deltas() {
            cx.update(|cx| moon_ui::MoonTheme::global_mut(cx).scale.font_delta = delta);
            for state in states() {
                for width in [1000.0_f32, 1100.0, 1200.0] {
                    let probe_state = state.clone();
                    let window =
                        cx.open_window(gpui::size(px(width), px(200.0)), |_, _| HeaderProbe {
                            state: probe_state,
                            width,
                        });
                    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                    for _ in 0..3 {
                        visual.update(|window, cx| {
                            let _ = window.draw(cx);
                        });
                    }
                    let button = visual.debug_bounds("terminal-update").unwrap();
                    let names = visual.debug_bounds("probe-names").unwrap();
                    let right = visual.debug_bounds("probe-right").unwrap();
                    let ctx = format!("{code} delta {delta} {state:?} at {width}px");
                    assert!(
                        names.right() <= button.left(),
                        "{ctx}: names end at {:?}, button starts at {:?}",
                        names.right(),
                        button.left()
                    );
                    assert!(
                        button.right() <= right.left(),
                        "{ctx}: button over the ticker"
                    );
                    assert!(
                        button.left() >= px(0.0) && button.right() <= px(width),
                        "{ctx}: button outside the header"
                    );
                    let reserve = visual.update(|_, cx| update_cluster_reserve_w(cx, &state));
                    let gap = visual.update(|_, cx| design::ui_value(cx, design::CHROME_GAP));
                    assert!(
                        button.size.width.as_f32() + 2.0 * gap + 1.0 <= reserve,
                        "{ctx}: reserve {reserve} under rendered button {:?}",
                        button.size.width
                    );
                }
            }
        }
    }
}

/// With no version offered the cluster is absent and claims nothing, so the strategy names keep
/// every pixel they had before #1191.
#[gpui::test]
fn hidden_update_reserves_nothing(cx: &mut gpui::TestAppContext) {
    assert!(update_button_label(&UpdateState::Hidden).is_none());
    let reserve = cx.update(|cx| update_cluster_reserve_w(cx, &UpdateState::Hidden));
    assert_eq!(reserve, 0.0);
}

/// The reservation follows the label on the button NOW: the busy face is wider than the offer.
#[gpui::test]
fn reservation_follows_the_current_label(cx: &mut gpui::TestAppContext) {
    let _locale = crate::test_locale::force("ru");
    let (offer, busy) = cx.update(|cx| {
        moon_ui::MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Light, 1.0),
            cx,
        );
        (
            update_cluster_reserve_w(cx, &UpdateState::Available(VERSION)),
            update_cluster_reserve_w(cx, &UpdateState::Installing(VERSION)),
        )
    });
    assert!(
        busy > offer,
        "installing {busy} must reserve more than offer {offer}"
    );
}
