//! Session-only navigation must retain both editors and stay outside persisted configuration.

use gpui::{AppContext, Context, IntoElement, Render, Window, div};
use moon_ui::MoonInputState;

use super::core_section::CoreTelegramEd;
use super::{TelegramEd, TelegramSegment};

/// A headless editor host with no backend, credentials, service, or filesystem access.
struct EditorFixture(TelegramEd);

impl Render for EditorFixture {
    /// Keep editor entities alive without rendering the production application.
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Build recognizable editor values so resetting either editor during navigation is observable.
fn fixture(window: &mut Window, cx: &mut Context<EditorFixture>) -> EditorFixture {
    let input = cx.new(|cx| MoonInputState::new(window, cx).default_value("unsaved fixture"));
    EditorFixture(TelegramEd {
        segment: TelegramSegment::default(),
        token: input.clone(),
        active_chat: Some(19),
        pending_owner: Some(23),
        name: input.clone(),
        search: input.clone(),
        history_cores: vec![(71, "archived fixture".into())],
        history_loaded: true,
        history_loading: false,
        history_failed: false,
        core: CoreTelegramEd {
            picked: Some(71),
            expanded: true,
            proxy_kind: 1,
            proxy_host: input.clone(),
            proxy_port: input.clone(),
            proxy_user: input.clone(),
            proxy_password: input.clone(),
            mtproto_secret: input.clone(),
            pending: None,
            phone: input.clone(),
            code: input.clone(),
            password: input.clone(),
            email: input.clone(),
            email_code: input.clone(),
            first_name: input.clone(),
            last_name: input,
            terms_accepted: Some((71, "fixture terms".into())),
            resend_pulse_armed: false,
        },
    })
}

/// Resetting picked/expanded or replacing input entities in select_segment loses in-progress work.
#[gpui::test]
fn segment_switch_retains_core_selection_and_bot_editor(cx: &mut gpui::TestAppContext) {
    let window = cx.add_window(fixture);
    window
        .update(cx, |host, _, cx| {
            let ed = &mut host.0;
            assert_eq!(ed.segment, TelegramSegment::TerminalBot);
            let token = ed.token.entity_id();
            for next in [TelegramSegment::CoreReader, TelegramSegment::TerminalBot] {
                ed.select_segment(next);
                assert_eq!(ed.segment, next);
                assert_eq!(ed.core.picked, Some(71));
                assert!(ed.core.expanded);
                assert_eq!(ed.core.proxy_kind, 1);
                assert_eq!(ed.token.entity_id(), token);
                assert_eq!(ed.token.read(cx).value().to_string(), "unsaved fixture");
                assert_eq!(ed.active_chat, Some(19));
                assert_eq!(ed.pending_owner, Some(23));
                assert_eq!(ed.history_cores, vec![(71, "archived fixture".into())]);
                assert_eq!(ed.core.terms_accepted, Some((71, "fixture terms".into())));
                assert!(ed.core.pending.is_none());
            }
        })
        .expect("headless editor window remains open");
}

/// Moving sub-tab navigation into the file schema or its serializer would persist a session choice.
/// The config schema is private to moon-core, so this boundary check reads those sources.
#[test]
fn segment_navigation_is_absent_from_settings_file_and_dump() {
    for source in [
        include_str!("../../../../moon-core/src/config/schema.rs"),
        include_str!("../../../../moon-core/src/config/reconcile.rs"),
        include_str!("../mod.rs"),
    ] {
        assert!(!source.contains("TelegramSegment"));
        assert!(!source.contains("telegram.segment"));
        assert!(!source.contains("telegram_segment"));
    }
}
