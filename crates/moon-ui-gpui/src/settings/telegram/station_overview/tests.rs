use super::{Tile, install_labels, install_step, install_steps_el, tiles_el};
use crate::settings::layout_probe::{ProbeSpec, sweep};
use crate::settings::telegram::tests::Host;
use gpui::{IntoElement, ParentElement, Styled};

/// The install walks access, then the server check once its key was read, then the install
/// while a job runs on it; a running job before any key does not skip the access step.
#[test]
fn the_install_steps_follow_the_form() {
    assert_eq!(install_step(false, false), 0);
    assert_eq!(install_step(false, true), 0);
    assert_eq!(install_step(true, false), 1);
    assert_eq!(install_step(true, true), 2);
    let _locale = crate::test_locale::force("ru");
    assert_eq!(
        install_labels(),
        ["Доступ к серверу", "Проверка сервера", "Установка станции"]
    );
}

/// Synthetic tiles: the longest captions with a long version and a bar.
fn tiles() -> Vec<Tile> {
    vec![
        Tile {
            label: rust_i18n::t!("station.over.status").to_string(),
            value: rust_i18n::t!("station.over.offline").to_string(),
            progress: None,
        },
        Tile {
            label: rust_i18n::t!("station.over.version").to_string(),
            value: "9.99.999-synthetic".to_string(),
            progress: None,
        },
        Tile {
            label: rust_i18n::t!("station.over.cores").to_string(),
            value: "7 / 12".to_string(),
            progress: Some(58.0),
        },
        Tile {
            label: rust_i18n::t!("station.over.bot").to_string(),
            value: rust_i18n::t!("telegram.server.bot_unpaired").to_string(),
            progress: None,
        },
    ]
}

/// The tiles and the install steps stay inside the body and the tiles never overlap, in every
/// language at the default and the minimum window, with and without a station.
#[gpui::test]
fn the_overview_head_fits_the_body(cx: &mut gpui::TestAppContext) {
    sweep(
        cx,
        |_, _, (width, _)| Host {
            width,
            scroll: gpui::ScrollHandle::new(),
            build: std::rc::Rc::new(|_, cx| {
                gpui::div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(gpui::px(12.0))
                    .child(tiles_el(tiles(), cx))
                    .child(install_steps_el(1))
                    .into_any_element()
            }),
        },
        |_| ProbeSpec {
            container: "probe-body",
            inside: (0..4)
                .map(|i| format!("station-tile-{i}"))
                .chain(["station-tiles".into(), "station-install-steps".into()])
                .collect(),
            no_overlap: vec![
                (0..4).map(|i| format!("station-tile-{i}")).collect(),
                vec!["station-tiles".into(), "station-install-steps".into()],
            ],
            last: Some("station-install-steps".into()),
            ..Default::default()
        },
    );
}
