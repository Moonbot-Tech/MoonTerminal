//! Source contract on the Mini App session check.

/// Counts station calls without starting transport, sessions, or an updater.
struct StationHost {
    config: moon_core::config::AppConfig,
    state: crate::TelegramState,
    status_calls: usize,
    /// Whether the last status was asked from the Settings section.
    status_from_settings: Option<bool>,
    update_calls: usize,
}

impl StationHost {
    /// Paired order deliberately differs from the explicit owner.
    fn new() -> Self {
        let mut config = moon_core::config::AppConfig::headless(Vec::new());
        config.telegram.authorized_chat_ids = vec![20, 10, 30];
        config.telegram.owner_chat_id = Some(10);
        config.telegram.chat_profile_mut(20).core_uids = vec![1];
        let state = crate::TelegramState::new(&config.telegram, crate::HostKind::Station);
        assert!(state.service.is_none());
        Self {
            config,
            state,
            status_calls: 0,
            status_from_settings: None,
            update_calls: 0,
        }
    }

    /// Run the production dispatcher and receive its synchronous fixture answer.
    fn command(&mut self, chat: i64, command: super::ParsedCommand) -> super::Response {
        let (reply, receiver) = std::sync::mpsc::sync_channel(1);
        super::run_command(self, chat, command, reply);
        receiver.try_recv().expect("fixture command must answer")
    }
}

impl crate::TgHost for StationHost {
    /// Identify this fixture as a station.
    fn kind(&self) -> crate::HostKind {
        crate::HostKind::Station
    }
    /// Expose the current saved fixture grant.
    fn config(&self) -> &moon_core::config::AppConfig {
        &self.config
    }
    /// Reject accidental use of live sessions.
    fn session(&self) -> &moon_core::session::SessionManager {
        panic!("no session needed")
    }
    /// Reject accidental mutation of live sessions.
    fn session_mut(&mut self) -> &mut moon_core::session::SessionManager {
        panic!("no session needed")
    }
    /// Expose transport-free fixture state.
    fn state(&self) -> &crate::TelegramState {
        &self.state
    }
    /// Allow the dispatcher to update fixture state.
    fn state_mut(&mut self) -> &mut crate::TelegramState {
        &mut self.state
    }
    /// Keep fixture report times deterministic.
    fn report_zone(&self) -> chrono_tz::Tz {
        chrono_tz::UTC
    }
    /// Point the fixture at a process-local path. The empty token never creates it.
    fn notifications_path(&self) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("moon-tg-notify-{}.json", std::process::id()))
    }
    /// Pair in memory without granting ownership.
    fn save_paired_chat(&mut self, chat: i64) -> bool {
        self.config.telegram.pair_chat(chat);
        true
    }
    /// Clear fixture grants without persistence.
    fn save_cleared_pairing(&mut self) -> bool {
        self.config.telegram.clear_pairing();
        true
    }
    /// Adopt the chat's change in memory.
    fn save_bot_settings(&mut self, bot: moon_core::config::telegram_menu::BotSettings) -> bool {
        self.config.telegram.bot = bot;
        true
    }
    /// Reject money-command reads outside the fixture scope.
    fn is_panic_armed(&self, _: u64, _: &str) -> bool {
        panic!("no money commands")
    }
    /// Reject money-command writes outside the fixture scope.
    fn toggle_panic_sell(&mut self, _: u64, _: String) -> bool {
        panic!("no money commands")
    }
    /// Reject core reconnects outside the fixture scope.
    fn request_reconnect(&mut self, _: u64) {
        panic!("no reconnect")
    }
    /// Reject unexpected background reads.
    fn spawn(&mut self, _: crate::Job) {
        panic!("no background reads")
    }
    /// No display exists in this fixture.
    fn repaint(&mut self) {}
    /// Count a station status call and return a synthetic answer.
    fn station_status(
        &mut self,
        reply: std::sync::mpsc::SyncSender<super::Response>,
        from_settings: bool,
    ) -> bool {
        self.status_calls += 1;
        self.status_from_settings = Some(from_settings);
        super::answer(&reply, "fixture status".into());
        true
    }
    /// Count updater requests without touching a server.
    fn request_station_update(&mut self) -> Option<Result<(), crate::UpdateRefusal>> {
        self.update_calls += 1;
        Some(Ok(()))
    }
}

/// Removing the gate exposes status and restarts the station for viewers with or without cores.
/// Using the first chat instead also denies the explicitly selected owner; no owner means no one.
#[test]
fn station_commands_require_current_owner_before_host_calls() {
    let _locale = crate::test_locale::force("en");
    let mut host = StationHost::new();
    for chat in [20, 30, 99] {
        for command in [
            super::ParsedCommand::StationStatus,
            super::ParsedCommand::StationUpdate,
        ] {
            let super::Response::Text { text, keyboard } = host.command(chat, command) else {
                panic!("expected refusal")
            };
            assert_eq!(text, rust_i18n::t!("telegram.refusal"));
            assert!(keyboard.is_none());
        }
    }
    assert_eq!((host.status_calls, host.update_calls), (0, 0));
    host.command(10, super::ParsedCommand::StationStatus);
    host.command(10, super::ParsedCommand::StationUpdate);
    assert_eq!((host.status_calls, host.update_calls), (1, 1));
    assert!(host.config.telegram.set_owner(20));
    host.command(10, super::ParsedCommand::StationUpdate);
    assert_eq!(host.update_calls, 1);
    host.command(20, super::ParsedCommand::StationUpdate);
    assert_eq!(host.update_calls, 2);
    host.config.telegram.owner_chat_id = None;
    host.command(10, super::ParsedCommand::StationStatus);
    host.command(20, super::ParsedCommand::StationStatus);
    assert_eq!(
        host.status_calls, 1,
        "without an owner no chat is one implicitly"
    );
    host.config.telegram.owner_chat_id = Some(99);
    host.command(20, super::ParsedCommand::StationStatus);
    assert_eq!(host.status_calls, 1);
}

/// Dropping the role on a navigation path would advertise station status to observers.
#[test]
fn viewer_dispatch_navigation_never_offers_station_commands() {
    let _locale = crate::test_locale::force("en");
    let mut host = StationHost::new();
    host.state.report_pending = true;
    for chat in [10, 20, 30] {
        for command in [
            super::ParsedCommand::Help,
            super::ParsedCommand::MiniApp,
            super::ParsedCommand::Unknown,
            super::ParsedCommand::Start,
        ] {
            let markup = match host.command(chat, command) {
                super::Response::Rich { navigation, .. } => navigation.1,
                super::Response::Text {
                    keyboard: Some(markup),
                    ..
                } => markup,
                _ => panic!("expected navigation"),
            };
            assert_status_button(markup, chat == 10);
        }
        let (reply, receiver) = std::sync::mpsc::sync_channel(1);
        super::pair(&mut host, chat, reply);
        let super::Response::PairSaved {
            keyboard: Some(markup),
            ..
        } = receiver.try_recv().unwrap()
        else {
            panic!("expected pairing navigation")
        };
        assert_status_button(markup, chat == 10);
    }
}

/// Parse rendered buttons through transport aliases rather than assuming row positions.
fn assert_status_button(markup: moon_core::telegram::api::ReplyMarkup, expected: bool) {
    let moon_core::telegram::api::ReplyMarkup::Reply(markup) = markup else {
        panic!("expected a reply keyboard")
    };
    let labels = super::telegram_labels(crate::HostKind::Station);
    let commands: Vec<_> = markup
        .keyboard
        .iter()
        .flatten()
        .map(|button| moon_core::telegram::commands::parse_reply_button(&button.text, &labels))
        .collect();
    assert_eq!(
        commands.contains(&super::ParsedCommand::StationStatus),
        expected
    );
    assert!(!commands.contains(&super::ParsedCommand::StationUpdate));
    assert!(commands.contains(&super::ParsedCommand::Help));
}

/// `dispatch.rs:mini_request` must reject both a disabled Mini App and a chat absent from the
/// current paired set, while `telegram::web::App::handle_session` maps that rejection to 403;
/// otherwise a stale bound server can keep returning a successful session after a user disables
/// Mini App access or revokes the chat.
#[test]
fn mini_app_session_guard_rechecks_both_live_authorization_conditions() {
    let dispatch = include_str!("../dispatch.rs").replace("\r\n", "\n");
    let guard = "if !telegram.mini_app_enabled || !telegram.authorized_chat_ids.contains(&chat_id) {\n                let _ = reply.try_send(Err(MiniAppApiError::Rejected));\n                return;\n            }";
    assert!(
        dispatch.contains(guard),
        "the session check must reject a disabled Mini App OR a now-revoked chat before replying OK"
    );

    let http = include_str!("../../../moon-core/src/telegram/web.rs");
    assert!(
        http.contains("Ok(Err(_)) => status_response(StatusCode::FORBIDDEN, \"rejected\")"),
        "a host rejection must become a 403 response rather than the old timeout class"
    );
}

/// The Settings section is the owner's: a viewer is refused, the owner's switch is saved and its
/// screen comes back.
#[test]
fn the_settings_section_is_the_owners() {
    use moon_core::config::telegram_menu::{MenuItem, ReportView};
    use moon_core::telegram::menu_action::{MenuAction, SettingsAction};
    let _locale = crate::test_locale::force("en");
    let mut host = StationHost::new();
    let settings = |action| super::ParsedCommand::Menu(MenuAction::Settings(action));
    let refused = host.command(20, settings(SettingsAction::SetView(ReportView::Days)));
    assert!(matches!(refused, super::Response::Text { .. }));
    assert_eq!(host.config.telegram.bot.report_view, ReportView::Exchanges);
    let shown = host.command(10, settings(SettingsAction::SetView(ReportView::Days)));
    assert!(matches!(shown, super::Response::Rich { .. }));
    assert_eq!(host.config.telegram.bot.report_view, ReportView::Days);
    let shown = |host: &StationHost, item| {
        host.config
            .telegram
            .bot
            .menu
            .keyboard
            .iter()
            .flatten()
            .any(|e| e.item == item && e.show)
    };
    assert!(!shown(&host, MenuItem::Report));
    let show_report = settings(SettingsAction::ShowButton(MenuItem::Report, true));
    host.command(10, show_report.clone());
    assert!(shown(&host, MenuItem::Report));
    // A second press of the same (now stale) button does not undo it.
    host.command(10, show_report);
    assert!(shown(&host, MenuItem::Report));
    // Settings itself stays on: the chat would lose its way back here.
    host.command(
        10,
        settings(SettingsAction::ShowButton(MenuItem::Settings, false)),
    );
    assert!(shown(&host, MenuItem::Settings));
    // The station's Mini App is switched by its administrator, not from the chat.
    assert!(matches!(
        host.command(10, settings(SettingsAction::MiniApp(true))),
        super::Response::Text { .. }
    ));
}

/// The station's status asked from the Settings section is told so (its answer leads back
/// there); asked from the keyboard it is not; a viewer gets neither.
#[test]
fn the_status_knows_where_it_was_asked_from() {
    use moon_core::telegram::menu_action::{MenuAction, SettingsAction};
    let mut host = StationHost::new();
    host.command(
        10,
        super::ParsedCommand::Menu(MenuAction::Settings(SettingsAction::StationStatus)),
    );
    assert_eq!(host.status_from_settings, Some(true));
    host.command(10, super::ParsedCommand::StationStatus);
    assert_eq!(host.status_from_settings, Some(false));
    host.command(
        20,
        super::ParsedCommand::Menu(MenuAction::Settings(SettingsAction::StationStatus)),
    );
    assert_eq!(
        host.status_calls, 2,
        "a viewer is refused before the station is asked"
    );
}
