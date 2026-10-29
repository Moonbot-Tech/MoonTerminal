//! Network-free interactive pipeline probe with a disk replica and concurrent chart rendering.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crate::{Finish, HostKind, Job, TelegramState, TgHost};
use moon_core::config::{AppConfig, TelegramConfig};
use moon_core::session::SessionManager;
use moon_core::telegram::api::{ReplyMarkup, Update};
use moon_core::telegram::commands::parse_update;
use moon_core::telegram::runtime::{NotifyStore, Response, Work};

/// Stage durations for a single request, written by the owner and its read thread.
#[derive(Default, Clone)]
struct Stages {
    received: Option<Instant>,
    admitted: Option<Instant>,
    started: Option<Instant>,
    read: Option<Instant>,
    finish_started: Option<Instant>,
}

/// Station-shaped owner; only the transport receive and HTTP send boundary are injected.
struct ProbeHost {
    config: AppConfig,
    state: TelegramState,
    session: SessionManager,
    path: PathBuf,
    finished: mpsc::Sender<Finish>,
    stages: Arc<Mutex<Stages>>,
}

impl TgHost for ProbeHost {
    /// Use the station's unchanged report wording and keyboard.
    fn kind(&self) -> HostKind {
        HostKind::Station
    }
    /// Expose only synthetic saved settings.
    fn config(&self) -> &AppConfig {
        &self.config
    }
    /// The empty live session has no networked cores; ten cores live in the replica.
    fn session(&self) -> &SessionManager {
        &self.session
    }
    /// No core control is exercised by this probe.
    fn session_mut(&mut self) -> &mut SessionManager {
        &mut self.session
    }
    /// Return isolated Telegram state.
    fn state(&self) -> &TelegramState {
        &self.state
    }
    /// Apply report admission and completion normally.
    fn state_mut(&mut self) -> &mut TelegramState {
        &mut self.state
    }
    /// Use stable UTC calendar boundaries.
    fn report_zone(&self) -> chrono_tz::Tz {
        chrono_tz::UTC
    }
    /// Keep every notification inside the private fixture root.
    fn notifications_path(&self) -> PathBuf {
        self.path.clone()
    }
    /// Pairing is outside a report request.
    fn save_paired_chat(&mut self, _: i64) -> bool {
        panic!("unexpected pairing")
    }
    /// Revocation is outside this load probe.
    fn save_cleared_pairing(&mut self) -> bool {
        panic!("unexpected revocation")
    }
    /// Report view switches use the real notification store instead.
    fn save_bot_settings(&mut self, _: moon_core::config::telegram_menu::BotSettings) -> bool {
        panic!("unexpected settings save")
    }
    /// No money controls are sent by a report.
    fn is_panic_armed(&self, _: u64, _: &str) -> bool {
        panic!("unexpected control")
    }
    /// No money controls are sent by a report.
    fn toggle_panic_sell(&mut self, _: u64, _: String) -> bool {
        panic!("unexpected control")
    }
    /// Reports never reconnect a core.
    fn request_reconnect(&mut self, _: u64) {
        panic!("unexpected reconnect")
    }
    /// Use the station's one-thread-per-read topology and owner completion channel.
    fn spawn(&mut self, job: Job) {
        let tx = self.finished.clone();
        let stages = self.stages.clone();
        let owner = std::thread::current();
        std::thread::spawn(move || {
            stages.lock().unwrap().started = Some(Instant::now());
            let finish = job();
            stages.lock().unwrap().read = Some(Instant::now());
            tx.send(finish).unwrap();
            owner.unpark();
        });
    }
    /// No display exists in this headless fixture.
    fn repaint(&mut self) {}
}

/// Render real 2000x1200 PNGs while report jobs run on their own threads.
fn chart_until_stopped(stop: &AtomicBool, entered: mpsc::Sender<()>) {
    let chart = crate::deal_chart::DealChart {
        market: "XALPHA",
        base: "XALPHA",
        short: false,
        entry: (1_000, 1.0),
        exit: (36_000, 1.01),
        stop: None,
        entry_line: &[],
        exit_line: &[],
        caption: "CORE_A",
        won: true,
        money: crate::deal_chart::Money::Usdt,
        spent: Some(100.0),
        day: None,
        ticks: &[],
        window: (0, 40_000),
        zone: chrono_tz::UTC,
    };
    entered.send(()).unwrap();
    while !stop.load(Ordering::Acquire) {
        assert!(crate::deal_chart::render(&chart).is_some());
    }
}

/// Extract every observable part of an answer for byte-for-byte parity assertions.
fn rich(response: Response) -> (String, ReplyMarkup, (String, ReplyMarkup)) {
    match response {
        Response::Rich {
            html,
            keyboard,
            navigation,
        } => (html, keyboard, navigation),
        _ => panic!("report must produce a rich answer"),
    }
}

/// Time update receipt through production dispatch/read/render to an injected HTTP send boundary.
/// Run this ignored probe alone: its private data-root override is process-wide, like the app's.
#[test]
#[ignore = "isolated end-to-end load probe; run alone with --exact --ignored --nocapture"]
fn probe_interactive_pipeline() {
    let _locale = crate::test_locale::force("en");
    let root = crate::notify::test_host::TempRoot::new("interactive");
    let path = root.notifications();
    assert!(moon_core::config::paths::set_data_dir_override(
        path.parent().unwrap().to_owned()
    ));
    // Fixed UTC noon puts every fixture close inside Today, independently of the host clock.
    let now = 1_700_000_000_i64.div_euclid(86_400) * 86_400 + 12 * 3600;
    drop(super::scale::source_on(
        rusqlite::Connection::open(moon_core::config::paths::reports_db_path()).unwrap(),
        100_000,
        now - 12 * 3600,
        now,
    ));
    let _permit = moon_core::db::report_recovery::prepare().expect("private replica lease");
    let mut config = AppConfig::headless(vec![]);
    config.telegram.authorized_chat_ids = vec![42];
    config.telegram.owner_chat_id = Some(42);
    let mut state = TelegramState::new(&TelegramConfig::default(), HostKind::Station);
    let mut store = NotifyStore::open(path.clone()).unwrap();
    store
        .update(|file| {
            for card in 0..50 {
                if card % 2 == 0 {
                    moon_core::telegram::runtime::push_outbox(
                        file,
                        42,
                        format!("CORE_A {card}"),
                        Some(vec![1]),
                        now,
                    );
                } else {
                    moon_core::telegram::runtime::push_photo(
                        file,
                        42,
                        format!("CORE_A {card}"),
                        Some(vec![1]),
                        "CORE_A.png".into(),
                        now,
                    );
                }
            }
        })
        .unwrap();
    let expected_cards: Vec<_> = store
        .file
        .outbox
        .iter()
        .map(|row| row.html.clone())
        .collect();
    let store = Arc::new(Mutex::new(store));
    state.notify_store_override = Some(store.clone());
    let stages = Arc::new(Mutex::new(Stages::default()));
    let (finish_tx, finish_rx) = mpsc::channel();
    let mut host = ProbeHost {
        session: SessionManager::start(&config, 0.0, None, None),
        config,
        state,
        path,
        finished: finish_tx,
        stages: stages.clone(),
    };
    let mut request = moon_core::telegram::report::ReportRequest::preset(
        moon_core::telegram::report::Period::Today,
    );
    let (from, to) = request.bounds(now, chrono_tz::UTC).unwrap();
    request.window = Some((from, to));
    let mut resolved = request
        .clone()
        .resolve_view(host.config.telegram.bot.report_view);
    resolved.view_mode = Some(host.config.telegram.bot.report_view);
    let conn = rusqlite::Connection::open(moon_core::config::paths::reports_db_path()).unwrap();
    let page = super::super::read_page_on(
        &conn,
        resolved,
        from,
        to,
        chrono_tz::UTC,
        moon_core::config::telegram_menu::ReportBasis::Close,
        &Default::default(),
        |_| {
            (
                Default::default(),
                moon_core::config::telegram_access::TelegramReportAccess::Owner,
            )
        },
    )
    .unwrap();
    assert_eq!(
        page.total.orders, 100_000,
        "Today must include every fixture deal"
    );
    let expected = rich(super::super::render::render(
        &page,
        HostKind::Station,
        crate::labels::navigation_keyboard(HostKind::Station, true, &host.config.telegram),
    ));
    let (work_tx, work_rx) = mpsc::sync_channel(64);
    let owner_stages = stages.clone();
    let owner = std::thread::spawn(move || {
        loop {
            for finish in finish_rx.try_iter() {
                owner_stages.lock().unwrap().finish_started = Some(Instant::now());
                finish(&mut host);
            }
            match work_rx.try_recv() {
                Ok(Work::Command {
                    chat_id,
                    command,
                    message,
                    reply,
                }) => {
                    owner_stages.lock().unwrap().admitted = Some(Instant::now());
                    crate::dispatch::run_command(&mut host, chat_id, command, message, reply);
                }
                Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) => {}
                _ => panic!("unexpected work"),
            }
            std::thread::park_timeout(Duration::from_millis(100));
        }
    });
    let stop = Arc::new(AtomicBool::new(false));
    let chart_stop = stop.clone();
    let (entered, active) = mpsc::channel();
    let chart = std::thread::spawn(move || chart_until_stopped(&chart_stop, entered));
    active.recv().unwrap();
    let mut times = Vec::new();
    for index in 0..20 {
        let received = Instant::now();
        let mut update = today_update();
        let message = if index % 2 == 0 {
            None
        } else {
            let message = update.message.take().unwrap();
            update.callback_query = Some(moon_core::telegram::api::CallbackQuery {
                id: "fixture".into(),
                from: message.from.clone().unwrap(),
                message: Some(message),
                data: Some(request.callback()),
            });
            Some(1)
        };
        let mut inbound = parse_update(&update, None).unwrap();
        if let moon_core::telegram::commands::ParsedCommand::Report(parsed) = &mut inbound.command {
            // Freeze only the test clock: otherwise each second intentionally changes button data.
            parsed.window = request.window;
        }
        let (reply, rx) = mpsc::sync_channel(1);
        *stages.lock().unwrap() = Stages {
            received: Some(received),
            ..Default::default()
        };
        work_tx
            .send(Work::Command {
                chat_id: inbound.chat_id,
                command: inbound.command,
                message,
                reply,
            })
            .unwrap();
        owner.thread().unpark();
        let answer = rich(rx.recv_timeout(Duration::from_secs(10)).unwrap());
        let issued = Instant::now();
        assert_eq!(
            answer, expected,
            "text and keyboards differ from the direct reader/renderer"
        );
        let stage = stages.lock().unwrap().clone();
        let admitted = stage.admitted.unwrap();
        let started = stage.started.unwrap();
        let read = stage.read.unwrap();
        let finish_started = stage.finish_started.unwrap();
        times.push([
            (admitted - stage.received.unwrap()).as_secs_f64() * 1000.0,
            (started - admitted).as_secs_f64() * 1000.0,
            (read - started).as_secs_f64() * 1000.0,
            (finish_started - read).as_secs_f64() * 1000.0,
            (issued - finish_started).as_secs_f64() * 1000.0,
            (issued - received).as_secs_f64() * 1000.0,
        ]);
    }
    stop.store(true, Ordering::Release);
    chart.join().unwrap();
    drop(work_tx);
    owner.thread().unpark();
    owner.join().unwrap();
    let cards: Vec<_> = store
        .lock()
        .unwrap()
        .file
        .outbox
        .iter()
        .map(|row| row.html.clone())
        .collect();
    assert_eq!(
        cards, expected_cards,
        "interactive replies must neither drop nor reorder cards"
    );
    for (index, name) in [
        "owner wake",
        "worker start",
        "read",
        "completion wake",
        "render/send",
        "total",
    ]
    .iter()
    .enumerate()
    {
        let mut values: Vec<_> = times.iter().map(|row| row[index]).collect();
        values.sort_by(f64::total_cmp);
        eprintln!(
            "interactive pipeline {name}: p50={:.3} p95={:.3} ms",
            values[9], values[18]
        );
        if index == 5 {
            assert!(values[18] < 100.0, "interactive p95 target exceeded");
        }
    }
}

/// Construct the same neutral private /today update a long poll would return, without JSON I/O.
fn today_update() -> Update {
    Update {
        update_id: 1,
        message: Some(moon_core::telegram::api::Message {
            message_id: 1,
            chat: moon_core::telegram::api::Chat {
                id: 42,
                kind: "private".into(),
            },
            from: Some(moon_core::telegram::api::User {
                id: 42,
                ..Default::default()
            }),
            text: Some("/today".into()),
            ..Default::default()
        }),
        callback_query: None,
    }
}
