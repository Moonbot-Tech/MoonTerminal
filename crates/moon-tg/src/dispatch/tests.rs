//! Source contract on the Mini App session check.

/// Counts station calls without starting transport, sessions, or an updater.
struct StationHost {
    /// Process identity selected by the header regression; existing fixtures stay stations.
    kind: crate::HostKind,
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
            kind: crate::HostKind::Station,
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
        super::run_command(self, chat, command, None, reply);
        receiver.try_recv().expect("fixture command must answer")
    }
}

impl crate::TgHost for StationHost {
    /// Identify the bot's fixture process without starting either host.
    fn kind(&self) -> crate::HostKind {
        self.kind
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

/// Bypassing the shared settings renderer would drop the host header despite helper tests
/// passing. Exercise owner commands through dispatch for each host and every service-free page.
#[test]
fn settings_dispatch_names_the_process_being_edited() {
    let _locale = crate::test_locale::force("en");
    use moon_core::telegram::menu_action::{MenuAction, SettingsAction};
    for (kind, name) in [
        (crate::HostKind::Terminal, "Bot of this terminal"),
        (crate::HostKind::Station, "Station bot"),
    ] {
        let mut host = StationHost::new();
        host.kind = kind;
        for action in [
            SettingsAction::Root,
            SettingsAction::Buttons,
            SettingsAction::View,
            SettingsAction::Basis,
        ] {
            let super::Response::Rich { html, .. } =
                host.command(10, super::ParsedCommand::Menu(MenuAction::Settings(action)))
            else {
                panic!("owner settings must render")
            };
            assert!(html.starts_with(&format!("<p><b>{name}</b></p>")), "{html}");
        }
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
    let refused = host.command(20, settings(SettingsAction::SetView(ReportView::Cores)));
    assert!(matches!(refused, super::Response::Text { .. }));
    assert_eq!(host.config.telegram.bot.report_view, ReportView::Exchanges);
    let shown = host.command(10, settings(SettingsAction::SetView(ReportView::Cores)));
    assert!(matches!(shown, super::Response::Rich { .. }));
    assert_eq!(host.config.telegram.bot.report_view, ReportView::Cores);
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

/// Both host kinds must create rules at pairing, with no historical replay or legacy opt-in.
#[test]
fn pairing_initializes_new_chat_rules_for_both_hosts() {
    use moon_core::telegram::notify::{NotifyFile, NotifySettings};
    use moon_core::telegram::runtime::NotifyStore;
    use std::sync::{Arc, Mutex};
    let _locale = crate::test_locale::force("en");
    for kind in [crate::HostKind::Terminal, crate::HostKind::Station] {
        let root = crate::notify::test_host::TempRoot::new("pair-defaults");
        let mut host = StationHost::new();
        host.kind = kind;
        let mut store = NotifyStore::open(root.notifications()).unwrap();
        let legacy = root.notifications().with_extension("legacy.json");
        std::fs::write(&legacy, r#"{"chats":{"11":{"revision":4}}}"#).unwrap();
        store.file = NotifyFile::load(&legacy).unwrap();
        let store = Arc::new(Mutex::new(store));
        host.state.notify_store_override = Some(Arc::clone(&store));
        for chat in [40, 11] {
            let (reply, receive) = std::sync::mpsc::sync_channel(1);
            super::pair(&mut host, chat, reply);
            assert!(matches!(
                receive.try_recv().unwrap(),
                super::Response::PairSaved { saved: true, .. }
            ));
        }
        let guard = store.lock().unwrap();
        let row = &guard.file.chats[&40];
        assert!(row.settings.trades.on);
        assert!(row.settings.down.on);
        assert_eq!(row.settings.trades.profit_at_least_usd, Some(100.0));
        assert_eq!(row.settings.trades.loss_at_least_usd, Some(100.0));
        assert_eq!(row.revision, 1);
        assert!(row.ledger.trades_enabled_utc.is_some());
        assert!(row.ledger.seen.is_empty());
        assert_eq!(guard.file.chats[&11].settings, NotifySettings::default());
        assert_eq!(guard.file.chats[&11].revision, 4);
        let stored = guard.file.clone();
        assert_eq!(NotifyFile::load(&guard.path).unwrap(), stored);
        drop(guard);
        let rows = crate::notify_rows(&host.state, &host.config.telegram).unwrap();
        assert_eq!(rows[&40].settings, stored.chats[&40].settings);
        assert_eq!(rows[&20].settings, NotifySettings::new_chat());
        assert_eq!(rows[&20].revision, 0);
        assert_eq!(
            crate::mini_app::chat_notify(&host, 11),
            Some(NotifySettings::default())
        );
        assert_eq!(
            crate::mini_app::chat_notify(&host, 20),
            Some(NotifySettings::new_chat())
        );
        let (reply, _) = std::sync::mpsc::sync_channel(1);
        super::pair(&mut host, 40, reply);
        assert_eq!(
            store.lock().unwrap().file,
            stored,
            "repeat pairing must not reset rules or their enable time"
        );
    }
}

/// Mini App, in-chat settings and desktop/station settings must share the initial draft rules.
/// Sparse stored documents must stay off on every route, even after unrelated settings edits.
#[test]
fn first_settings_routes_share_new_rules_for_both_hosts() {
    use moon_core::telegram::init_data::{SignedInitData, SignedUser};
    use moon_core::telegram::notify::NotifySettings;
    use moon_core::telegram::runtime::NotifyStore;
    use moon_core::telegram::web::MiniAppApiRequest;
    use std::sync::{Arc, Mutex};
    let _locale = crate::test_locale::force("en");
    for kind in [crate::HostKind::Terminal, crate::HostKind::Station] {
        let root = crate::notify::test_host::TempRoot::new("settings-defaults");
        let mut host = StationHost::new();
        host.kind = kind;
        host.config.telegram.mini_app_enabled = true;
        host.state.visible_override = Some(vec![]);
        let mut store = NotifyStore::open(root.notifications()).unwrap();
        let legacy = root.notifications().with_extension("legacy.json");
        std::fs::write(&legacy, r#"{"chats":{"10":{"revision":4}}}"#).unwrap();
        store.file = moon_core::telegram::notify::NotifyFile::load(&legacy).unwrap();
        let store = Arc::new(Mutex::new(store));
        host.state.notify_store_override = Some(Arc::clone(&store));
        let identity = |id| SignedInitData {
            auth_date: 0,
            user: SignedUser {
                id,
                first_name: "fixture".into(),
                last_name: None,
                username: None,
                language_code: None,
            },
            chat: None,
            query_id: None,
        };
        for (chat, expected, revision) in [
            (10, NotifySettings::default(), 4),
            (20, NotifySettings::new_chat(), 0),
        ] {
            let (reply, receive) = std::sync::mpsc::sync_channel(1);
            super::mini_request(
                &mut host,
                MiniAppApiRequest::Notify {
                    identity: identity(chat),
                    chat_id: chat,
                    reply,
                },
            );
            let draft = receive.try_recv().unwrap().unwrap();
            assert_eq!(draft.settings, expected);
            assert_eq!(draft.revision, revision);
            assert!(
                !root.notifications().exists(),
                "read-only first open must not create a row"
            );
        }
        let (reply, receive) = std::sync::mpsc::sync_channel(1);
        super::mini_request(
            &mut host,
            MiniAppApiRequest::NotifySave {
                identity: identity(20),
                chat_id: 20,
                settings: NotifySettings::new_chat(),
                revision: 0,
                reply,
            },
        );
        let saved = receive.try_recv().unwrap().unwrap();
        assert!(saved.error.is_none());
        assert_eq!(saved.settings, NotifySettings::new_chat());
        assert_eq!(saved.revision, 1);
        let bot = crate::mini_app::save_chat_notify(&host, 30, |settings| {
            settings.trades.usd_followup = true
        })
        .unwrap();
        assert!(bot.trades.on && bot.down.on && bot.trades.usd_followup);
        assert_eq!(bot.trades.profit_at_least_usd, Some(100.0));
        let mut rows = crate::notify_rows(&host.state, &host.config.telegram).unwrap();
        assert_eq!(rows[&10].settings, NotifySettings::default());
        rows.get_mut(&10).unwrap().settings.charts.on = true;
        crate::save_notify_rows(&host.state, &host.config.telegram, &rows, chrono_tz::UTC).unwrap();
        let guard = store.lock().unwrap();
        assert!(!guard.file.chats[&10].settings.trades.on);
        assert!(!guard.file.chats[&10].settings.down.on);
        for chat in [20, 30] {
            assert!(guard.file.chats[&chat].ledger.trades_enabled_utc.is_some());
            assert!(guard.file.chats[&chat].settings.down.on);
        }
    }
}
