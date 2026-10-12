//! Security page layout and the machine revocation's write path.

use gpui::{IntoElement, ParentElement, Styled, div};
use moon_core::config::AppConfig;

use super::{SecurityParts, password_card, security_body, unrecoverable_alert, write_saved};
use crate::settings::general::probe_host::{host, stub};
use crate::settings::layout_probe::{ProbeSpec, sweep};

/// Catches the revocation writing the draft again: with a dirty draft, the write receives the
/// saved configuration without the draft's edit, and the draft is still dirty against what was
/// written.
#[test]
fn forgetting_machines_writes_the_saved_config_and_keeps_the_draft_dirty() {
    let saved = AppConfig::load(None, false).expect("test-binary config must load");
    let mut draft = saved.clone();
    draft.log_retention_days = saved.log_retention_days.wrapping_add(7) % 366;
    assert!(
        crate::settings::draft_dirty(&saved, &draft),
        "the draft must start dirty"
    );

    let mut seen = None;
    let written = write_saved(&saved, |config| {
        seen = Some(config.log_retention_days);
        Ok(())
    })
    .expect("the stub write succeeds");

    assert_eq!(
        seen,
        Some(saved.log_retention_days),
        "the write got a draft edit"
    );
    assert_eq!(written.log_retention_days, saved.log_retention_days);
    assert!(
        crate::settings::draft_dirty(&written, &draft),
        "the draft must stay dirty after the revocation is written"
    );
}

/// A failed write reports its error and hands back nothing to adopt as the saved config.
#[test]
fn a_failed_revocation_write_is_an_error() {
    let saved = AppConfig::load(None, false).expect("test-binary config must load");
    assert!(write_saved(&saved, |_| Err(anyhow::anyhow!("disk full"))).is_err());
}

/// Both password cards open with every row, the alert and the machines row stay inside the body,
/// the cards and the machines section keep apart, and the card hints and alert are not cut off,
/// in every language at the default and the minimum window.
#[gpui::test]
fn security_page_fits_the_body(cx: &mut gpui::TestAppContext) {
    sweep(
        cx,
        host(|_, cx| {
            let field = |id: &'static str| {
                div()
                    .w_full()
                    .flex()
                    .gap_2()
                    .child(div().w(gpui::px(90.0)).flex_none().child("label"))
                    .child(stub(id, 200.0, 28.0))
                    .into_any_element()
            };
            let launch = password_card(
                "security-launch-card",
                rust_i18n::t!("security.launch.title").to_string(),
                rust_i18n::t!("security.launch.hint").to_string(),
                stub("probe-launch-check", 18.0, 18.0),
                vec![field("probe-launch-pw"), field("probe-launch-confirm")],
                cx,
            );
            let file = password_card(
                "security-file-card",
                rust_i18n::t!("security.file.title").to_string(),
                rust_i18n::t!("security.file.hint").to_string(),
                stub("probe-file-check", 18.0, 18.0),
                vec![
                    field("probe-file-pw"),
                    field("probe-file-confirm"),
                    unrecoverable_alert(),
                ],
                cx,
            );
            security_body(
                SecurityParts {
                    not_editable: true,
                    launch,
                    file,
                    machines: stub("probe-machines", 240.0, 28.0),
                    status: None,
                },
                cx,
            )
        }),
        |_| ProbeSpec {
            container: "probe-body",
            inside: [
                "security-launch-card",
                "security-file-card",
                "probe-launch-pw",
                "probe-file-confirm",
                "security-unrecoverable",
                "security-machines",
                "probe-machines",
            ]
            .map(String::from)
            .to_vec(),
            no_overlap: vec![
                [
                    "security-launch-card",
                    "security-file-card",
                    "security-machines",
                ]
                .map(String::from)
                .to_vec(),
                ["probe-launch-pw", "probe-launch-confirm"]
                    .map(String::from)
                    .to_vec(),
            ],
            wraps: vec![
                (
                    "security-launch-card-hint".into(),
                    rust_i18n::t!("security.launch.hint").to_string(),
                ),
                (
                    "security-file-card-hint".into(),
                    rust_i18n::t!("security.file.hint").to_string(),
                ),
            ],
            last: Some("security-machines".into()),
            ..Default::default()
        },
    );
}
