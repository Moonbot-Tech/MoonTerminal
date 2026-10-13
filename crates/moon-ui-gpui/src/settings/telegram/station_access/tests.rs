//! The danger zone's re-check at the moment of "Yes".

use super::{access_check, danger_off_reason};

/// A Forget or Remove confirmed while a station job runs, or while a bot hand-over or return is
/// pending, never starts; an idle station with nothing pending lets it run.
#[test]
fn a_confirmed_danger_action_rechecks_the_station() {
    assert_eq!(access_check(false, None), Ok(()));
    assert_eq!(
        access_check(true, None),
        Err("telegram.mini_error_busy_station")
    );
    assert_eq!(
        access_check(false, Some("telegram.server.access_pending_handover")),
        Err("telegram.server.access_pending_handover")
    );
}

/// The Maintenance page's auto-update switch with its note and the danger zone stay inside the
/// body, and Forget and Remove never overlap, in every language at both window sizes.
#[gpui::test]
fn the_maintenance_page_fits_the_body(cx: &mut gpui::TestAppContext) {
    use super::super::server_auto_update::{AutoUpdateView, auto_update_el};
    use super::danger_el;
    use crate::settings::layout_probe::{ProbeSpec, sweep};
    use crate::settings::telegram::tests::Host;
    use gpui::{IntoElement, ParentElement, Styled};
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
                    .child(auto_update_el(
                        AutoUpdateView::OldStation,
                        false,
                        Box::new(|_, _, _| {}),
                        cx,
                    ))
                    .child(danger_el(
                        None,
                        Box::new(|_, _, _| {}),
                        Box::new(|_, _, _| {}),
                        cx,
                    ))
                    .into_any_element()
            }),
        },
        |_| ProbeSpec {
            container: "probe-body",
            inside: [
                "server-auto-update-block",
                "server-auto-update-note",
                "station-danger",
                "station-forget",
                "station-remove",
            ]
            .map(String::from)
            .to_vec(),
            no_overlap: vec![vec!["station-forget".into(), "station-remove".into()]],
            last: Some("station-danger".into()),
            ..Default::default()
        },
    );
}

/// Catches Forget and Remove going grey without saying why: a pending hand-over names its own
/// refusal, a running read the wait, and an idle station leaves them on.
#[test]
fn the_danger_zone_names_why_it_is_off() {
    assert_eq!(danger_off_reason(false, None), None);
    assert_eq!(
        danger_off_reason(true, None),
        Some("telegram.server.wait_station")
    );
    assert_eq!(
        danger_off_reason(true, Some("telegram.server.access_pending_handover")),
        Some("telegram.server.access_pending_handover")
    );
}
