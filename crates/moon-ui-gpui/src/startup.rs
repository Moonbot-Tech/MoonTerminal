//! Старт приложения: логгер + паник/SEH-хуки, загрузка конфига, запуск GPUI `App`,
//! создание общего [`Backend`], фоновые циклы (feed-wake + координация) и окна групп.
//! Вынесено из `main.rs` точь-в-точь (тело старого `main()` = [`run`]).

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui::*;

use moon_ui::{MoonTheme, MoonThemeConfig, Root, ThemeMode, init as init_moon_ui};

use moon_core::config::{AppConfig, UiThemeMode, WindowLayout};
use moon_core::metrics::{Metrics, MetricsSnapshot};
use moon_core::session::{CoreId, SessionManager};

use crate::{Backend, chart_persist, crash, detached, diag, dock_persist, firetest};

fn embedded_fonts() -> Vec<Cow<'static, [u8]>> {
    vec![
        include_bytes!("../../../assets/fonts/Inter-400.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/Inter-500.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/Inter-600.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/GeistMono-400.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/GeistMono-500.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/GeistMono-600.ttf")
            .as_slice()
            .into(),
    ]
}

pub(crate) fn moon_theme_config_for(cfg: &AppConfig) -> MoonThemeConfig {
    let mut theme = match cfg.ui_theme_mode {
        UiThemeMode::Dark => MoonThemeConfig::moon_terminal(),
        UiThemeMode::Light => MoonThemeConfig::moon_light(),
    };
    theme.mode = match cfg.ui_theme_mode {
        UiThemeMode::Dark => ThemeMode::Dark,
        UiThemeMode::Light => ThemeMode::Light,
    };
    theme
        .with_font_delta(cfg.ui_font_delta)
        .with_ui_scale(cfg.ui_scale)
}

pub(crate) fn install_moon_theme_for_config(cfg: &AppConfig, cx: &mut App) {
    MoonTheme::install_config(moon_theme_config_for(cfg), cx);
}

pub(crate) fn run() -> anyhow::Result<()> {
    // Строим env_logger как Logger (не .init()) и оборачиваем в TeeLogger — он
    // дублирует напечатанные записи в in-memory кольцо вкладки «Лог» (порт egui main).
    let env = env_logger::Builder::from_env(
        env_logger::Env::default()
            .default_filter_or("warn,moon_ui_gpui=info,moon_gpui=info,moon_core=info"),
    )
    .build();
    log::set_max_level(env.filter());
    if let Err(e) = log::set_boxed_logger(Box::new(moon_core::applog::TeeLogger::new(env))) {
        eprintln!("не удалось установить логгер: {e}");
    }
    log::info!(
        "build: moonterminal={} moonui={}",
        option_env!("MOONTERMINAL_GIT_REV").unwrap_or("unknown"),
        option_env!("MOONUI_GIT_REV").unwrap_or("unknown")
    );
    let firetest_config = firetest::Config::from_args(std::env::args())?;
    if firetest_config.is_some() {
        diag::force_enable();
    }

    // Нативные краши (access violation в DirectX/GPUI-форке, напр. present по протухшему
    // дескриптору окна при реконнекте) идут МИМО Rust-паник-хука — процесс умирает молча,
    // `panic.log` пуст. Ставим SEH-фильтр верхнего уровня, чтобы такой краш тоже попал в
    // `panic.log` с кодом/адресом/бэктрейсом. Раньше всего — до создания окон.
    crash::install_native_handler();

    // Паник-хук: GUI-приложение без консоли → stderr с сообщением паники теряется (и при
    // panic=abort это выглядит как нативный краш 0xc0000409 в ucrtbase). Пишем место+сообщение
    // паники в `panic.log` (cwd) и в общий лог ДО аборта — чтобы видеть точный source-локейшн.
    {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let loc = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "?".into());
            let payload = info
                .payload()
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| info.payload().downcast_ref::<String>().map(|s| s.as_str()))
                .unwrap_or("<non-string>");
            // Бэктрейс (force — без RUST_BACKTRACE): location у clamp-паник = внутренность core,
            // а нам нужен ВЫЗЫВАЮЩИЙ кадр в нашем коде.
            let bt = std::backtrace::Backtrace::force_capture();
            let line = format!("PANIC at {loc}: {payload}\n--- backtrace ---\n{bt}\n--- end ---");
            log::error!("PANIC at {loc}: {payload}");
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("panic.log")
            {
                let _ = writeln!(f, "{line}");
            }
            default_hook(info);
        }));
    }

    let cfg = AppConfig::load()?;
    // Язык интерфейса из конфига → глобальная локаль rust-i18n (для t! здесь и в MoonUI).
    rust_i18n::set_locale(cfg.language.code());
    // Файловый лог: режим из конфига + одноразовая чистка старых файлов при старте.
    moon_core::applog::set_file_logging(cfg.log_to_file, cfg.log_retention_days);
    moon_core::applog::purge_old();
    let group_list = crate::group_window::groups(&cfg);
    log::info!("groups: {group_list:?} (servers: {})", cfg.servers.len());

    // Единая точка отсчёта времени для сессий и чарт-вью (как epoch_ms в egui).
    let epoch = moon_chart::paint::now_unix_ms();

    // Регистрируем встроенные SVG-иконки MoonUI как AssetSource — без этого `IconName::*`
    // (напр. крестик очистки `cleanable` = CircleX) не находят svg и рисуются пустыми.
    let app = gpui_platform::application().with_assets(moon_ui::MoonAssets);
    app.run(move |cx| {
        init_moon_ui(cx);
        install_moon_theme_for_config(&cfg, cx);
        cx.text_system()
            .add_fonts(embedded_fonts())
            .expect("failed to add embedded MoonBot fonts");

        let layout = WindowLayout::load();
        let dock_states = dock_persist::load_all();
        let detached = detached::load_all();

        // Одноразовый ремап charts.json: до v11 схемы вкладки хранили ПОЗИЦИОННЫЕ CoreId,
        // а теперь CoreId = стабильный uid. Перепривязываем, пока порядок серверов тот же,
        // что был при записи файла (флаг взводится только при апгрейде со старой версии).
        let chart_specs = {
            let mut specs = chart_persist::load_all();
            if cfg.chart_core_remap_needed {
                chart_persist::remap_core_ids(&mut specs, &cfg.servers);
                chart_persist::save_all(&specs);
            }
            specs
        };

        // БД отчётов: поднимаем writer (как egui App). Его `tx` отдаём сессии (ядро
        // шлёт close-report → запись в SQLite), `generation` живёт в Backend для окна
        // «Отчёт». None = БД недоступна (окно отчётов покажет пусто).
        let reports = moon_core::db::spawn_writer();
        let (feed_wake_tx, feed_wake_rx) = std::sync::mpsc::channel::<()>();

        let backend = cx.new(|_| Backend {
            session: SessionManager::start(
                &cfg,
                epoch,
                reports.as_ref().map(|h| &h.tx),
                Some(feed_wake_tx.clone()),
            ),
            epoch,
            reports,
            metrics: Metrics::new(),
            snap: MetricsSnapshot::default(),
            // open = рынки ОТКРЫТЫХ чарт-панелей (как App::about_to_wait в egui).
            // Пусто на старте; наполнится при открытии монеты (порт чарт-панелей).
            // set_open всё равно избирает провайдера/биржу на старте → subscribe_all_trades
            // (ретейн всех трейдов биржи — как было; ради мгновенного открытия монеты).
            desired: Vec::new(),
            chart_market_refs: HashMap::new(),
            chart_market_refs_epoch: 0,
            chart_orderbook_refs: HashMap::new(),
            desired_orderbook: Vec::new(),
            desired_open_dirty: true,
            last_open_sync: Instant::now() - Duration::from_secs(10),
            main_chart_targets: HashMap::new(),
            main_open_markets: HashMap::new(),
            trade_core_override: HashMap::new(),
            config: cfg.clone(),
            preview: None,
            open_request: None,
            open_request_rev: 0,
            open_request_activate: false,
            open_compare_request: None,
            open_compare_request_rev: 0,
            diag_open_first_market: std::env::var_os("MOON_RENDER_DIAG_OPEN_FIRST_MARKET")
                .is_some(),
            diag_open_done: false,
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            diag_open_10_btc: std::env::var_os("MOON_RENDER_DIAG_OPEN_10_BTC").is_some(),
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            diag_open_10_btc_done: false,
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            debug_fill_main_chart_group: None,
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            debug_fill_main_chart_rev: 0,
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            debug_main_chart_handles: HashMap::new(),
            layout: layout.clone(),
            layout_dirty: false,
            header_ticker_default: None,
            last_header_ticker_refresh: None,
            dock_states,
            dock_dirty: false,
            price_scale: None,
            price_scale_group: None,
            price_scale_rev: 0,
            follow: true,
            order_size_sel: HashMap::new(),
            order_size_rev: 0,
            order_size_edit_req: None,
            sell_edit_req: None,
            sell_pct_local: HashMap::new(),
            sell_slot_local: HashMap::new(),
            panic_armed: HashSet::new(),
            backend_dirty_since_notify: false,
            last_backend_notify: None,
            reconnect_request: Vec::new(),
            show_group_request: Vec::new(),
            group_windows: HashMap::new(),
            settings_window: None,
            strategies_window: None,
            assets_window: None,
            screener_window: None,
            firetest: firetest_config.clone().map(firetest::Runtime::new),
            hovered_chart: None,
            detached,
            detached_dirty: false,
            repin_request: Vec::new(),
            chart_repin_request: Vec::new(),
            chart_apply_all: Vec::new(),
            detached_chart_windows: Vec::new(),
            last_main_input: std::collections::HashMap::new(),
            exclude_bl_delta: std::collections::HashMap::new(),
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            debug_window: None,
            #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
            debug_chart_windows: Vec::new(),
            chart_consumers: Vec::new(),
            chart_specs,
            chart_specs_dirty: false,
            figures: std::rc::Rc::new(std::cell::RefCell::new(
                moon_core::figures::FigureStore::load(),
            )),
            fig_draw_mode: true,
            fig_tool: moon_core::figures::FigureTool::HLine,
            fig_style: moon_core::figures::DrawStyle::default(),
            fig_selected: None,
            last_chart_alerts_activity: 0,
            last_detect_seq: std::collections::HashMap::new(),
            default_alert_sound: "ding1".to_string(),
            config_dirty: false,
            quitting: false,
        });
        backend.update(cx, |b, _| b.refresh_header_ticker_default(true));

        // Фабрики панелей для восстановления раскладки доков (PanelRegistry — глобален).
        dock_persist::register_panels(cx, backend.clone(), epoch);

        // Закрытие ГЛАВНОГО (группового) окна = полный выход: убираем закрытое окно из
        // group_windows, и если групповых окон не осталось — quit (закроет и откреплённые
        // чарт-окна). Детач-чарт окна сами quit не вызывают (их id нет в group_windows).
        let quit_backend = backend.clone();
        cx.on_window_closed(move |app, closed_id| {
            // Возвращаем (откреп-окна_на_закрытие, надо_ли_выйти).
            let (to_close, quit) = quit_backend.update(app, |b, _| {
                // Это окно группы? (его group, если да)
                let group = b
                    .group_windows
                    .iter()
                    .find(|(_, h)| h.window_id() == closed_id)
                    .map(|(g, _)| g.clone());
                if let Some(group) = group {
                    b.group_windows.remove(&group);
                    if b.group_windows.is_empty() {
                        // Последнее окно группы → полный выход (quit закроет всё, вкл. откреп).
                        return (Vec::new(), true);
                    }
                    // Иначе закрыть откреп-чарты ИМЕННО этой группы.
                    let close: Vec<WindowHandle<Root>> = b
                        .detached_chart_windows
                        .iter()
                        .filter(|(g, _)| *g == group)
                        .map(|(_, h)| *h)
                        .collect();
                    b.detached_chart_windows.retain(|(g, _)| *g != group);
                    (close, false)
                } else {
                    // Закрыли откреп-чарт-окно (или иное) — вычистить из трекинга.
                    b.detached_chart_windows
                        .retain(|(_, h)| h.window_id() != closed_id);
                    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
                    {
                        if b.debug_window
                            .as_ref()
                            .is_some_and(|h| h.window_id() == closed_id)
                        {
                            b.debug_window = None;
                        }
                        b.debug_chart_windows.retain(|h| h.window_id() != closed_id);
                    }
                    (Vec::new(), false)
                }
            });
            for h in to_close {
                h.update(app, |_, window, _| window.remove_window()).ok();
            }
            if quit {
                app.quit();
            }
        })
        .detach();

        // На выходе из приложения: пометить quitting и СРАЗУ сохранить charts.json. На старте
        // quit окна ещё не снесены → detached=Some; без этого закрытие откреп-окон при выходе
        // репинит их (detached→None) и они не восстанавливаются. quitting также глушит дренаж
        // репина (drain_chart_repin), чтобы он не сбросил detached.
        let app_quit_backend = backend.clone();
        cx.on_app_quit(move |cx| {
            moon_core::detect_diag::line("[quit] on_app_quit → сохраняю charts.json");
            app_quit_backend.update(cx, |b, _| {
                b.quitting = true;
                if b.config_dirty {
                    if let Err(e) = b.config.save() {
                        log::warn!("config save (quit) failed: {e}");
                    } else {
                        b.config_dirty = false;
                    }
                }
                chart_persist::save_all(&b.chart_specs);
            });
            async move {}
        })
        .detach();

        // Feed event path: feed threads send causal wakes after real MoonProto events.
        // Market-only wakes update MarketDataSource/store; visible charts pull it from
        // gpu_canvas.frame() without dirtying Backend/Shell. Account/order wakes still notify
        // Backend through the slow gate and update only chart order overlays here.
        let data_backend = backend.clone();
        cx.spawn(async move |cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let mut feed_wake_rx = feed_wake_rx;
            loop {
                let (rx, woke) = executor
                    .spawn(async move {
                        let woke = feed_wake_rx.recv().is_ok();
                        (feed_wake_rx, woke)
                    })
                    .await;
                feed_wake_rx = rx;
                if !woke {
                    break;
                }
                while feed_wake_rx.try_recv().is_ok() {}

                cx.update(|cx| {
                    data_backend.update(cx, |b, cx| {
                        let drain = b.session.drain();
                        if !drain.any {
                            return;
                        }
                        // Серверный набор chart-алертов изменился → пере-декодировать
                        // remote-фигуры (алерты, созданные в ядре/MoonBot). Гейт по activity,
                        // чтобы не декодить blob'ы на каждый ui_state-тик.
                        let alerts_activity = b.session.store().chart_alerts_activity();
                        if alerts_activity != b.last_chart_alerts_activity {
                            b.last_chart_alerts_activity = alerts_activity;
                            b.sync_remote_alerts();
                        }
                        // Звук детектов/алертов ядер (по новым детектам со звуком).
                        b.play_detect_sounds();
                        if drain.order_lines_data {
                            let chart_consumers = b.live_chart_consumers();
                            for chart in chart_consumers {
                                chart.sync_orders_if_visible(&b.session, false);
                            }
                        }
                        if drain.ui_state {
                            b.mark_backend_dirty(cx);
                        }
                    });
                });
            }
        })
        .detach();

        // Slow coordination path: provider roles, metrics, reconnects and persistence. This may
        // wake the GPUI tree through Backend notify, but it never stages high-rate chart pixels.
        let coord_backend = backend.clone();
        let coord_cfg = cfg.clone();
        let coord_layout = layout.clone();
        cx.spawn(async move |cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let mut last_report = Instant::now();
            // Сумма assets_rev по всем ядрам на прошлом сэмпле — для дельты assets_apply.
            let mut last_assets_rev_sum: u64 = 0;
            loop {
                executor.timer(Duration::from_millis(100)).await;
                cx.update(|cx| {
                    let (show_reqs, open_debug_10) = coord_backend.update(cx, |b, cx| {
                        b.maybe_diag_open_first_market(cx);
                        b.refresh_header_ticker_default(false);
                        b.sync_open_markets_if_due();
                        b.snap = b.metrics.sample(Instant::now());
                        crate::firetest::tick_backend(b, cx);

                        let recon: Vec<CoreId> = b.reconnect_request.drain(..).collect();
                        for id in recon {
                            b.session
                                .reconnect(id, &b.config, b.reports.as_ref().map(|h| &h.tx));
                        }
                        if b.layout_dirty {
                            b.layout.save();
                            b.layout_dirty = false;
                        }
                        if b.dock_dirty {
                            dock_persist::save_all(&b.dock_states);
                            b.dock_dirty = false;
                        }
                        if b.detached_dirty {
                            detached::save_all(&b.detached);
                            b.detached_dirty = false;
                        }
                        if b.chart_specs_dirty {
                            chart_persist::save_all(&b.chart_specs);
                            b.chart_specs_dirty = false;
                        }
                        if b.figures.borrow().dirty {
                            b.figures.borrow_mut().save();
                        }
                        if b.config_dirty {
                            // Дебаунс-сейв конфига (правка размеров колесом мыши пишет в память
                            // часто; на диск — раз за дренаж-тик, а не на каждый тик колеса).
                            if let Err(e) = b.config.save() {
                                log::warn!("config save (debounced) failed: {e}");
                            }
                            b.config_dirty = false;
                        }
                        b.flush_backend_notify(cx);
                        let reqs = std::mem::take(&mut b.show_group_request);
                        #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
                        let open_debug_10 = b.take_diag_open_10_btc();
                        #[cfg(not(any(
                            debug_assertions,
                            moon_profile_debug,
                            feature = "debug-tools"
                        )))]
                        let open_debug_10 = false;
                        (reqs, open_debug_10)
                    });

                    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
                    if open_debug_10 {
                        log::info!("diag auto-open: spawning 10 live-market chart windows");
                        crate::debug_window::spawn_debug_chart_windows(cx, coord_backend.clone());
                    }
                    for g in show_reqs {
                        crate::group_window::spawn_group_window(
                            cx,
                            &coord_backend,
                            &coord_cfg,
                            g,
                            epoch,
                            &coord_layout,
                            0.0,
                        );
                    }
                });
                if last_report.elapsed().as_millis() >= 1000 {
                    let ms = last_report.elapsed().as_secs_f64() * 1000.0;
                    last_report = Instant::now();
                    // Контекст момента для строки диагностики: CPU процесса/системы, число
                    // окон и открытых чарт-панелей + дельта assets_rev (снапшоты «Активов»,
                    // собранные feed-потоками за интервал — работа идёт и без открытого окна).
                    // Считаем ДО take_sample, чтобы assets_apply попал в этот же сэмпл.
                    let ctx = if crate::diag::is_enabled() {
                        cx.update(|cx| {
                            let windows = cx.windows().len();
                            coord_backend.update(cx, |b, _| {
                                let charts = b.live_chart_consumers().len();
                                let rev_sum: u64 = b
                                    .session
                                    .store()
                                    .cores()
                                    .map(|(_, d)| d.assets_rev)
                                    .sum();
                                // Ядро могло переподключиться (rev обнулился) — тогда дельта
                                // не определена, берём сумму заново без бампа.
                                if rev_sum >= last_assets_rev_sum {
                                    crate::diag::bump_by(
                                        &crate::diag::ASSETS_APPLY,
                                        rev_sum - last_assets_rev_sum,
                                    );
                                }
                                last_assets_rev_sum = rev_sum;
                                format!(
                                    "cpu={:.1} sys={:.1} windows={} charts={}",
                                    b.snap.cpu_process, b.snap.cpu_system, windows, charts
                                )
                            })
                        })
                    } else {
                        String::new()
                    };
                    if let Some(sample) = crate::diag::take_sample(ms) {
                        crate::diag::write_sample(ms, &sample, &ctx);
                        cx.update(|cx| {
                            coord_backend.update(cx, |b, _| {
                                crate::firetest::record_diag_sample(b, ms, &sample);
                            });
                        });
                    }
                }
            }
        })
        .detach();
        // По окну на группу (тем же helper'ом, что и кнопка 👁 «показать группу»).
        for (i, group) in group_list.into_iter().enumerate() {
            crate::group_window::spawn_group_window(
                cx,
                &backend,
                &cfg,
                group,
                epoch,
                &layout,
                i as f32 * 40.0,
            );
        }

        // Восстановить окна откреплённых панелей (панель уже не в доке — она была убрана
        // при откреплении, и dock_persist сохранил док без неё). Порт egui-восстановления
        // detached на старте.
        let specs = backend.read(cx).detached.clone();
        for spec in &specs {
            if let Err(err) = detached::spawn(cx, &backend, spec, None) {
                log::warn!(
                    "restore detached panel failed group={} panel={}: {err:#}",
                    spec.group,
                    spec.panel
                );
            }
        }
    });
    Ok(())
}
