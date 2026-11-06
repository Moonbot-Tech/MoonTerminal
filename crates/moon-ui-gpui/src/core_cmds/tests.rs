//! Draw the real command window over neutral facts in every language, theme, and page state.

use super::pages::Confirm;
use super::view::CoreCmdsView;
use super::{DEFAULT_SIZE, MIN_SIZE};
use crate::backend::core_cmds::{CmdError, CmdPhase};
use gpui::{FontWeight, px};
use moon_core::config::{Language, UiThemeMode};
use moon_remote::core_cmds::{Cmd, LogTail, ServiceInfo};
use moon_remote::core_new::{CoreState, Exchange};
use std::cell::RefCell;
use std::collections::BTreeMap;

/// A decorated SSH target must still find its login, including before locate succeeds.
#[test]
fn ssh_display_addresses_keep_the_login_lookup_host() {
    for target in [
        "root@203.0.113.7:22",
        "203.0.113.7:2222",
        "root@203.0.113.7",
        "203.0.113.7",
    ] {
        assert_eq!(super::view::target_host(target), "203.0.113.7");
    }
    assert_eq!(
        super::view::target_host("root@[2001:db8::7]:22"),
        "2001:db8::7"
    );
    assert_eq!(super::view::target_host("2001:db8::7"), "2001:db8::7");
}

/// Navigation must preserve disclosure ownership so closing can erase an earlier shown key.
#[gpui::test]
fn navigation_keeps_disclosure_until_explicit_hide(cx: &mut gpui::TestAppContext) {
    let _locale = crate::test_locale::force("en");
    cx.update(moon_ui::init);
    let handle = cx.open_window(gpui::size(px(MIN_SIZE.0), px(MIN_SIZE.1)), |window, cx| {
        let mut view = CoreCmdsView::controls(1, Cmd::ConnectionKey, window, cx);
        view.revealed = true;
        view
    });
    cx.update(|app| {
        handle
            .update(app, |view, window, cx| {
                view.select(Cmd::Status, false, window, cx);
                assert!(
                    view.revealed,
                    "navigation lost the disclosed key's close-time cleanup"
                );
                view.act(super::pages::Action::HideKey, window, cx);
                assert!(!view.revealed, "Hide left the key visible");
                window.remove_window();
            })
            .expect("command window")
    });
}

/// A hot switch must open its promised key form once, not on every later repaint.
#[gpui::test]
fn completed_exchange_switch_opens_api_keys_once(cx: &mut gpui::TestAppContext) {
    let _locale = crate::test_locale::force("en");
    cx.update(moon_ui::init);
    let handle = cx.open_window(gpui::size(px(MIN_SIZE.0), px(MIN_SIZE.1)), |window, cx| {
        let mut view = CoreCmdsView::controls(1, Cmd::SwitchExchange, window, cx);
        let mut facts = super::pages::tests::facts();
        facts.phases.insert(Cmd::SwitchExchange, CmdPhase::Done);
        facts.exchange_switched = true;
        view.fixture = Some(facts);
        view.follow_exchange_switch(cx);
        assert_eq!(view.cmd, Cmd::ApiKeys);

        view.cmd = Cmd::SwitchExchange;
        view.follow_exchange_switch(cx);
        assert_eq!(view.cmd, Cmd::SwitchExchange);
        view
    });
    cx.update(|app| {
        let _ = handle.update(app, |_, window, _| window.remove_window());
    });
}

/// Recreating the window must not replace the completed retry's duration with an unknown value.
#[gpui::test]
fn retry_state_uses_its_completed_phase_duration(cx: &mut gpui::TestAppContext) {
    let _locale = crate::test_locale::force("en");
    cx.update(moon_ui::init);
    let handle = cx.open_window(gpui::size(px(MIN_SIZE.0), px(MIN_SIZE.1)), |window, cx| {
        let mut view = CoreCmdsView::controls(1, Cmd::RetryLogin, window, cx);
        let mut facts = super::pages::tests::facts();
        facts.phases.insert(Cmd::RetryLogin, CmdPhase::Done);
        facts.retry_elapsed = Some(std::time::Duration::from_secs(42));
        let state = view.state(&facts, cx);
        assert_eq!(state.elapsed, "0:42");
        view.fixture = Some(facts);
        view
    });
    cx.update(|app| {
        let _ = handle.update(app, |_, window, _| window.remove_window());
    });
}

thread_local! {
    /// Text emitted by the real builders, keyed by its layout selector.
    static TEXTS: RefCell<BTreeMap<String, ProbeText>> = const { RefCell::new(BTreeMap::new()) };
}

/// A text measurement independent of the layout builder's height calculation.
pub(super) struct ProbeText {
    text: String,
    size: f32,
    weight: FontWeight,
}

/// Record production text only in test builds, following the Settings layout-probe seam.
pub(super) fn record(selector: String, text: String, size: f32, weight: FontWeight) {
    TEXTS.with(|texts| {
        texts
            .borrow_mut()
            .insert(selector, ProbeText { text, size, weight });
    });
}

/// Required state or confirmation text for each scenario that promises it.
fn required_text_selectors(scenario: usize) -> &'static [&'static str] {
    match scenario {
        13 | 17 | 25 => &["cc-confirm-title", "cc-confirm-why"],
        0..=4
        | 6..=9
        | 11
        | 14..=16
        | 18..=20
        | 22
        | 23
        | 26
        | 29
        | 30
        | 33
        | 35
        | 36
        | 40
        | 43
        | 44 => &["cc-state-title"],
        _ => &[],
    }
}

/// Test the real view, including narrow navigation, forms, secrets, and failure footers.
/// A wider translated footer or a missing flex constraint must fail this sweep.
#[gpui::test]
fn all_command_states_fit_locales_themes_and_window_sizes(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        for theme in [
            UiThemeMode::Light,
            UiThemeMode::Dark,
            UiThemeMode::Graphite,
            UiThemeMode::LightExperimental,
            UiThemeMode::DarkExperimental,
        ] {
            cx.update(|app| {
                moon_ui::MoonTheme::install_config(
                    crate::startup::moon_theme_config_for_presentation(theme, 1.0),
                    app,
                )
            });
            for (w, h) in [MIN_SIZE, DEFAULT_SIZE] {
                for scenario in 0..45 {
                    TEXTS.with(|texts| texts.borrow_mut().clear());
                    let handle = cx.open_window(gpui::size(px(w), px(h)), |window, cx| {
                        let (cmd, facts, confirm, revealed) = scenario_facts(scenario);
                        let mut view = CoreCmdsView::controls(1, cmd, window, cx);
                        view.fixture = Some(facts);
                        view.confirm = confirm;
                        view.revealed = revealed;
                        if matches!(scenario, 15 | 19 | 23) {
                            view.service_attempt = Some(cmd);
                        }
                        if scenario == 37 || scenario == 38 {
                            view.fixture_version =
                                Some(("7.84".into(), (scenario == 38).then(|| "7.85".into())));
                        }
                        if scenario == 28 || scenario == 41 {
                            view.key_input.update(cx, |input, cx| {
                                input.set_value("synthetic-key", window, cx)
                            });
                        }
                        if scenario == 41 {
                            view.secret_input.update(cx, |input, cx| {
                                input.set_value("synthetic-secret", window, cx)
                            });
                        }
                        if scenario == 42 {
                            view.port_input
                                .update(cx, |input, cx| input.set_value("0", window, cx));
                        }
                        if scenario == 39 {
                            view.fixture_form = Some(crate::backend::core_cmds::MenuForm::NoLogin);
                        }
                        view
                    });
                    let mut visual = gpui::VisualTestContext::from_window(handle.into(), cx);
                    for _ in 0..3 {
                        visual.update(|window, app| {
                            let _ = window.draw(app);
                        });
                    }
                    let context = format!("{language:?}/{theme:?}/{w}x{h}/state-{scenario}");
                    let header = visual.debug_bounds("cc-header").expect("header");
                    let body = visual.debug_bounds("cc-page").expect("page body");
                    let footer = visual.debug_bounds("cc-footer").expect("footer");
                    for bounds in [header, body, footer] {
                        assert!(
                            bounds.origin.x >= px(-0.5)
                                && bounds.origin.y >= px(-0.5)
                                && bounds.right() <= px(w + 0.5)
                                && bounds.bottom() <= px(h + 0.5),
                            "{context}: frame outside window: {bounds:?}"
                        );
                    }
                    assert!(
                        header.bottom() <= body.origin.y && body.bottom() <= footer.origin.y,
                        "{context}: chrome overlaps"
                    );
                    if w == MIN_SIZE.0 {
                        let target = visual.debug_bounds("cc-ssh-target").expect("ssh target");
                        assert!(
                            target.size.width > px(0.0)
                                && target.origin.x >= header.origin.x
                                && target.right() <= header.right(),
                            "{context}: ssh target hidden or clipped: {target:?}"
                        );
                    }
                    if w >= 640.0 {
                        let rail = visual.debug_bounds("cc-rail").expect("rail");
                        assert!(
                            rail.right() <= body.origin.x,
                            "{context}: rail overlaps page"
                        );
                        for i in [1, 2, 3, 5, 6, 7, 9, 10, 11, 12, 13] {
                            let selector = Box::leak(format!("cc-rail-{i}").into_boxed_str());
                            let item = visual.debug_bounds(selector).expect("rail item");
                            assert!(
                                item.origin.x >= rail.origin.x
                                    && item.right() <= rail.right()
                                    && item.origin.y >= rail.origin.y
                                    && item.bottom() <= rail.bottom(),
                                "{context}: rail item clipped: {item:?}"
                            );
                        }
                    }
                    for i in 0..3 {
                        let selector = Box::leak(format!("cc-footer-button-{i}").into_boxed_str());
                        if let Some(button) = visual.debug_bounds(selector) {
                            assert!(
                                button.origin.x >= footer.origin.x
                                    && button.right() <= footer.right()
                                    && button.origin.y >= footer.origin.y
                                    && button.bottom() <= footer.bottom(),
                                "{context}: footer button clipped: {button:?}"
                            );
                        }
                    }
                    let title = visual.debug_bounds("cc-h1").expect("page title");
                    assert!(
                        title.origin.x >= body.origin.x && title.right() <= body.right(),
                        "{context}: title outside page"
                    );
                    let (need, available) = visual.update(|window, app| {
                        let (cmd, _, _, _) = scenario_facts(scenario);
                        let text = rust_i18n::t!(super::pages::title(cmd)).to_string();
                        let runs = [gpui::TextRun {
                            len: text.len(),
                            font: gpui::Font {
                                weight: FontWeight::SEMIBOLD,
                                ..gpui::font(crate::design::ui_font())
                            },
                            ..gpui::TextRun::default()
                        }];
                        let need = window
                            .text_system()
                            .shape_text(text.into(), px(16.0), &runs, Some(title.size.width), None)
                            .expect("title shaping")
                            .iter()
                            .map(|line| f32::from(line.size(px(19.0)).height))
                            .sum::<f32>();
                        let _ = app;
                        (need, f32::from(title.size.height))
                    });
                    assert!(need <= available + 0.5, "{context}: title clipped");
                    let texts = TEXTS.with(|texts| std::mem::take(&mut *texts.borrow_mut()));
                    for selector in required_text_selectors(scenario) {
                        assert!(
                            texts.contains_key(*selector),
                            "{context}/{selector}: required text was not built"
                        );
                        assert!(
                            visual.debug_bounds(selector).is_some(),
                            "{context}/{selector}: required text is not visible"
                        );
                    }
                    for (selector, spec) in texts {
                        let selector = Box::leak(selector.into_boxed_str());
                        let bounds = visual
                            .debug_bounds(selector)
                            .unwrap_or_else(|| panic!("{context}/{selector}: text has no bounds"));
                        let need = visual.update(|window, _| {
                            let runs = [gpui::TextRun {
                                len: spec.text.len(),
                                font: gpui::Font {
                                    weight: spec.weight,
                                    ..gpui::font(crate::design::ui_font())
                                },
                                ..gpui::TextRun::default()
                            }];
                            window
                                .text_system()
                                .shape_text(
                                    spec.text.into(),
                                    px(spec.size),
                                    &runs,
                                    Some(bounds.size.width),
                                    None,
                                )
                                .expect("body text shaping")
                                .iter()
                                .map(|line| f32::from(line.size(px(19.0)).height))
                                .sum::<f32>()
                        });
                        assert!(
                            need <= f32::from(bounds.size.height) + 0.5,
                            "{context}/{selector}: body text clipped ({need} > {})",
                            f32::from(bounds.size.height)
                        );
                        assert!(
                            bounds.origin.x >= body.origin.x && bounds.right() <= body.right(),
                            "{context}/{selector}: body text outside page"
                        );
                    }
                    visual.update(|window, _| window.remove_window());
                }
            }
        }
    }
}

/// Generate only synthetic, neutral facts; no request can reach a real backend from this view.
fn scenario_facts(
    n: usize,
) -> (
    Cmd,
    crate::backend::core_cmds::CoreFacts,
    Option<Confirm>,
    bool,
) {
    let mut facts = super::pages::tests::facts();
    let located = facts.located.clone().expect("location");
    facts.info = Some(ServiceInfo {
        located,
        active: "active".into(),
        uptime_s: Some(47_040),
        memory: Some(26 * 1024 * 1024),
    });
    facts.log = Some(LogTail {
        lines: vec![
            "06:34:02 Out of memory: process killed".into(),
            "06:34:05 MoonBot started".into(),
            "06:34:10 AuthCheck failed: 429".into(),
            "06:34:10 Login stopped: state login_error".into(),
        ],
    });
    facts.exchanges = Some(vec![
        Exchange {
            id: "xalpha".into(),
            name: "XALPHA".into(),
            required_passphrase: String::new(),
            rsa_supported: false,
        },
        Exchange {
            id: "xbeta".into(),
            name: "XBETA".into(),
            required_passphrase: "passphrase".into(),
            rsa_supported: false,
        },
    ]);
    let mut confirm = None;
    let mut revealed = false;
    let cmd = match n {
        0..=4 => Cmd::Status,
        5..=9 => Cmd::RetryLogin,
        10..=12 => Cmd::Log,
        13..=16 => Cmd::Restart,
        17..=20 => Cmd::Stop,
        21..=23 => Cmd::Start,
        24..=27 => Cmd::SwitchExchange,
        28..=30 => Cmd::ApiKeys,
        31..=33 => Cmd::ConnectionKey,
        34..=36 => Cmd::MoonProtoAddress,
        39 | 40 => Cmd::Status,
        41 | 43 => Cmd::ApiKeys,
        42 | 44 => Cmd::MoonProtoAddress,
        _ => Cmd::UpdateCore,
    };
    match n {
        1 => {
            let s = facts.status.as_mut().expect("status");
            s.state = CoreState::LoginError;
            s.login_error = "429 Too Many Requests".into();
        }
        2 | 5 | 11 | 14 | 18 | 22 | 26 | 29 | 33 | 35 => {
            facts.phases.insert(
                cmd,
                CmdPhase::Running {
                    since: std::time::Instant::now(),
                    seen: Some(CoreState::Initializing),
                },
            );
        }
        3 => {
            facts.status.as_mut().expect("status").state = CoreState::Other("waiting".into());
        }
        4 => {
            facts
                .phases
                .insert(cmd, CmdPhase::Failed(CmdError::NoCore { port: 3302 }));
        }
        6 | 15 | 19 | 23 | 30 | 36 => {
            facts.phases.insert(cmd, CmdPhase::Done);
        }
        7 => {
            facts.phases.insert(
                cmd,
                CmdPhase::Failed(CmdError::Refused("429 Too Many Requests".into())),
            );
        }
        8 => {
            facts
                .phases
                .insert(cmd, CmdPhase::Failed(CmdError::Timeout));
        }
        9 | 16 | 20 => {
            facts
                .phases
                .insert(cmd, CmdPhase::Failed(CmdError::Ssh("offline".into())));
        }
        12 => facts.log = Some(LogTail::default()),
        13 | 17 => confirm = Some(Confirm::Service(cmd)),
        25 => {
            confirm = Some(Confirm::Exchange {
                id: "xbeta".into(),
                name: "XBETA".into(),
            })
        }
        27 => {
            facts.located.as_mut().expect("location").ours = false;
        }
        32 => {
            revealed = true;
            facts.key = Some(zeroize::Zeroizing::new("synthetic-key".into()));
        }
        40 => {
            facts.status = None;
        }
        43 | 44 => {
            facts
                .phases
                .insert(cmd, CmdPhase::Failed(CmdError::Ssh("offline".into())));
        }
        _ => {}
    }
    (cmd, facts, confirm, revealed)
}
