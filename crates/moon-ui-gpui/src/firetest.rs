//! Built-in diagnostic scenario runner.
//!
//! `moonterminal --debug-script chart-smoke` opens a chart, injects a short native mouse storm
//! over it and fails the process if cursor movement wakes expensive GPUI paths or burns CPU.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use gpui::{Context, IntoElement, ParentElement, div, px};
use moon_core::config::{ChartBucket, Language};
use moon_core::feed::CoreLogLine;
use moon_core::metrics::MetricsSnapshot;
use moon_core::session::order_lines::OrderCloseReason;
use moon_core::util::now_unix_ms_i64;
use moon_ui::MoonNotification;

use crate::{Backend, diag};

const DEFAULT_MARKET: &str = "BTCUSDT";
const START_DELAY: Duration = Duration::from_millis(1000);
const SETTLE: Duration = Duration::from_millis(5000);
const BASELINE: Duration = Duration::from_millis(5000);
const BASELINE_WARMUP: Duration = Duration::from_millis(1500);
const COOLDOWN: Duration = Duration::from_millis(1200);
const TEXT_WARMUP: Duration = Duration::from_millis(2500);
const ORDER_CANCEL_TIMEOUT: Duration = Duration::from_millis(15_000);
const OPEN_TIMEOUT: Duration = Duration::from_millis(10_000);
const PROBE_TIMEOUT: Duration = Duration::from_millis(10_000);
const DEFAULT_MOUSE_HZ: f64 = 5000.0;
const DEFAULT_STORM: Duration = Duration::from_millis(5000);
const STATIC_TEXT_LABELS: usize = 10_000;
const STAGE_GAP: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    WaitStartup,
    WaitOpen,
    WaitProbe,
    Settle,
    Baseline,
    Storm,
    StaticTextGap,
    StaticTextWarmup,
    StaticTextStorm,
    CommandErrorContract,
    ToolWindowsOpen,
    ToolWindowsVerifyOpen,
    ToolWindowsDedup,
    ToolWindowsVerifyDedup,
    RootOverlayContract,
    LocaleSwitch,
    LocaleSwitchVerify,
    PriceScale50,
    PriceScale20,
    PriceScaleAuto,
    PriceScaleVerifyAuto,
    OrderCancelLag,
    Cooldown,
    Done,
    // Keep this last: cargo tests use it to catch unplanned FireTest phases.
    #[allow(dead_code)]
    StageCount,
}

#[cfg(test)]
const STAGE_PLAN: [Phase; 24] = [
    Phase::WaitStartup,
    Phase::WaitOpen,
    Phase::WaitProbe,
    Phase::Settle,
    Phase::Baseline,
    Phase::Storm,
    Phase::StaticTextGap,
    Phase::StaticTextWarmup,
    Phase::StaticTextStorm,
    Phase::CommandErrorContract,
    Phase::ToolWindowsOpen,
    Phase::ToolWindowsVerifyOpen,
    Phase::ToolWindowsDedup,
    Phase::ToolWindowsVerifyDedup,
    Phase::RootOverlayContract,
    Phase::LocaleSwitch,
    Phase::LocaleSwitchVerify,
    Phase::PriceScale50,
    Phase::PriceScale20,
    Phase::PriceScaleAuto,
    Phase::PriceScaleVerifyAuto,
    Phase::OrderCancelLag,
    Phase::Cooldown,
    Phase::Done,
];

impl Phase {
    fn stage_name(self) -> &'static str {
        match self {
            Phase::WaitStartup => "start",
            Phase::WaitOpen => "open_chart",
            Phase::WaitProbe => "wait_chart_probe",
            Phase::Settle => "settle_live_chart",
            Phase::Baseline => "baseline",
            Phase::Storm => "mouse_storm",
            Phase::StaticTextGap => "static_text_gap",
            Phase::StaticTextWarmup => "static_text_warmup",
            Phase::StaticTextStorm => "static_text_storm",
            Phase::CommandErrorContract => "command_error_contract",
            Phase::ToolWindowsOpen => "tool_windows_open",
            Phase::ToolWindowsVerifyOpen => "tool_windows_verify_open",
            Phase::ToolWindowsDedup => "tool_windows_dedup",
            Phase::ToolWindowsVerifyDedup => "tool_windows_verify_dedup",
            Phase::RootOverlayContract => "root_overlay_contract",
            Phase::LocaleSwitch => "locale_switch",
            Phase::LocaleSwitchVerify => "locale_switch_verify",
            Phase::PriceScale50 => "price_scale_50",
            Phase::PriceScale20 => "price_scale_20",
            Phase::PriceScaleAuto => "price_scale_auto",
            Phase::PriceScaleVerifyAuto => "price_scale_verify_auto",
            Phase::OrderCancelLag => "order_cancel_lag",
            Phase::Cooldown => "cooldown",
            Phase::Done => "result",
            Phase::StageCount => "__invalid_count",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Script {
    ChartSmoke,
    OrderCancelLag,
}

#[cfg(test)]
const ORDER_CANCEL_LAG_STAGE_PLAN: &[Phase] = &[
    Phase::WaitStartup,
    Phase::WaitOpen,
    Phase::WaitProbe,
    Phase::Settle,
    Phase::OrderCancelLag,
    Phase::Cooldown,
    Phase::Done,
];

fn script_enables_order_cancel(script: Script, env_enabled: bool) -> bool {
    env_enabled || matches!(script, Script::OrderCancelLag)
}

fn phase_after_settle(script: Script) -> Phase {
    match script {
        Script::ChartSmoke => Phase::Baseline,
        Script::OrderCancelLag => Phase::OrderCancelLag,
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Config {
    script: Script,
    market: String,
    storm: Duration,
    mouse_hz: f64,
    text_labels: usize,
    order_cancel_lag: bool,
    order_cancel_size: Option<f64>,
    order_cancel_quote_size: Option<f64>,
    order_cancel_price_mult: f64,
    order_cancel_max_display_lag_ms: f64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ChartProbe {
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    hwnd: Option<isize>,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    screen_left: f32,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    screen_top: f32,
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    scale_factor: f32,
}

#[derive(Clone)]
struct Sample {
    phase: Phase,
    rates: Vec<diag::DiagRate>,
    metrics: MetricsSnapshot,
    gpu_frame_ms: f64,
}

struct MouseStorm {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OrderCancelStep {
    WaitOrder,
    WaitClosed,
}

struct OrderCancelRun {
    core: u64,
    market: String,
    before_uids: HashSet<u64>,
    price: f64,
    size: f64,
    place_submit_ms: i64,
    uid: Option<u64>,
    order_seen_ms: Option<i64>,
    cancel_submit_ms: Option<i64>,
    closed_store_ms: Option<i64>,
    closed_order_lines_rev: Option<u64>,
    closed_reason: Option<OrderCloseReason>,
    server_log: Option<CoreLogLine>,
    step: OrderCancelStep,
}

pub(crate) struct Runtime {
    config: Config,
    started: Instant,
    phase: Phase,
    phase_since: Instant,
    probe: Option<ChartProbe>,
    samples: Vec<Sample>,
    storm: Option<MouseStorm>,
    opened_group: Option<String>,
    tool_window_ids: Option<(String, String, String)>,
    locale_switch: Option<(Language, Language)>,
    order_cancel: Option<OrderCancelRun>,
    text_overlay_enabled: bool,
    present_pressure_enabled: bool,
    last_wait_log: Instant,
}

impl Config {
    pub(crate) fn from_args<I>(args: I) -> anyhow::Result<Option<Self>>
    where
        I: IntoIterator<Item = String>,
    {
        let mut args = args.into_iter();
        let mut script = None;
        while let Some(arg) = args.next() {
            if arg != "--debug-script" {
                continue;
            }
            let Some(value) = args.next() else {
                anyhow::bail!("--debug-script requires a script name");
            };
            script = Some(match value.as_str() {
                "chart-smoke" => Script::ChartSmoke,
                "order-cancel-lag" => Script::OrderCancelLag,
                other => anyhow::bail!("unknown --debug-script {other:?}"),
            });
        }

        let Some(script) = script else {
            return Ok(None);
        };
        let market = std::env::var("MOON_FIRETEST_MARKET")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_MARKET.to_string());
        let mouse_hz = std::env::var("MOON_FIRETEST_MOUSE_HZ")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 100.0)
            .unwrap_or(DEFAULT_MOUSE_HZ);
        let storm = std::env::var("MOON_FIRETEST_STORM_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(Duration::from_millis)
            .filter(|v| *v >= Duration::from_millis(1000))
            .unwrap_or(DEFAULT_STORM);
        let text_labels = std::env::var("MOON_FIRETEST_TEXT_LABELS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(STATIC_TEXT_LABELS);
        let order_cancel_lag =
            script_enables_order_cancel(script, env_flag("MOON_FIRETEST_ORDER_CANCEL"));
        let order_cancel_size = std::env::var("MOON_FIRETEST_ORDER_SIZE")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.0);
        let order_cancel_quote_size = std::env::var("MOON_FIRETEST_ORDER_QUOTE_SIZE")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.0);
        let order_cancel_price_mult = std::env::var("MOON_FIRETEST_ORDER_PRICE_MULT")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.0 && *v < 1.0)
            .unwrap_or(0.98);
        let order_cancel_max_display_lag_ms =
            std::env::var("MOON_FIRETEST_ORDER_CANCEL_MAX_DISPLAY_MS")
                .ok()
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v > 0.0)
                .unwrap_or(750.0);
        Ok(Some(Self {
            script,
            market,
            storm,
            mouse_hz,
            text_labels,
            order_cancel_lag,
            order_cancel_size,
            order_cancel_quote_size,
            order_cancel_price_mult,
            order_cancel_max_display_lag_ms,
        }))
    }
}

impl ChartProbe {
    pub(crate) fn new(
        hwnd: Option<isize>,
        screen_left: f32,
        screen_top: f32,
        left: f32,
        top: f32,
        width: f32,
        height: f32,
        scale_factor: f32,
    ) -> Option<Self> {
        (width >= 80.0
            && height >= 80.0
            && scale_factor > 0.0
            && screen_left.is_finite()
            && screen_top.is_finite())
        .then_some(Self {
            hwnd,
            screen_left,
            screen_top,
            left,
            top,
            width,
            height,
            scale_factor,
        })
    }
}

impl Runtime {
    pub(crate) fn new(config: Config) -> Self {
        diag::force_enable();
        let now = Instant::now();
        firetest_info(&format!(
            "[firetest] script={:?} market={} storm_ms={} mouse_hz={:.0} text_labels={} order_cancel_lag={}",
            config.script,
            config.market,
            config.storm.as_millis(),
            config.mouse_hz,
            config.text_labels,
            config.order_cancel_lag
        ));
        firetest_info("[firetest] stage=start");
        Self {
            config,
            started: now,
            phase: Phase::WaitStartup,
            phase_since: now,
            probe: None,
            samples: Vec::new(),
            storm: None,
            opened_group: None,
            tool_window_ids: None,
            locale_switch: None,
            order_cancel: None,
            text_overlay_enabled: false,
            present_pressure_enabled: false,
            last_wait_log: now,
        }
    }

    fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
        self.phase_since = Instant::now();
        firetest_info(&format!("[firetest] stage={}", self.stage_name()));
    }

    fn stage_name(&self) -> String {
        self.phase.stage_name().to_string()
    }

    fn observe_probe(&mut self, probe: ChartProbe) {
        if matches!(
            self.phase,
            Phase::WaitProbe
                | Phase::Settle
                | Phase::Baseline
                | Phase::Storm
                | Phase::StaticTextGap
                | Phase::StaticTextWarmup
                | Phase::StaticTextStorm
        ) {
            self.probe = Some(probe);
        }
    }

    fn record_sample(
        &mut self,
        _elapsed_ms: f64,
        rates: &[diag::DiagRate],
        metrics: MetricsSnapshot,
        gpu_frame_ms: f64,
    ) {
        if self.phase == Phase::Done {
            return;
        }
        if self.phase == Phase::Baseline && self.phase_since.elapsed() < BASELINE_WARMUP {
            return;
        }
        self.samples.push(Sample {
            phase: self.phase,
            rates: rates.to_vec(),
            metrics,
            gpu_frame_ms,
        });
    }

    fn tick(&mut self, backend: &mut Backend, cx: &mut Context<Backend>) {
        match self.phase {
            Phase::WaitStartup => {
                if self.started.elapsed() >= START_DELAY {
                    self.set_phase(Phase::WaitOpen);
                }
            }
            Phase::WaitOpen => {
                if self.try_open_chart(backend, cx) {
                    self.set_phase(Phase::WaitProbe);
                } else if self.phase_since.elapsed() >= OPEN_TIMEOUT {
                    self.fail("no active visible core/window to open chart");
                } else {
                    self.wait_log("waiting for active visible core/window");
                }
            }
            Phase::WaitProbe => {
                if self.probe.is_some() {
                    self.set_phase(Phase::Settle);
                } else if self.phase_since.elapsed() >= PROBE_TIMEOUT {
                    self.fail("chart opened but no chart bounds probe arrived");
                } else {
                    self.wait_log("waiting for chart bounds probe");
                }
            }
            Phase::Settle => {
                self.set_present_pressure(backend, true);
                if self.phase_since.elapsed() >= SETTLE {
                    let next_phase = phase_after_settle(self.config.script);
                    if matches!(self.config.script, Script::OrderCancelLag) {
                        self.set_present_pressure(backend, false);
                    }
                    self.set_phase(next_phase);
                }
            }
            Phase::Baseline => {
                self.set_present_pressure(backend, true);
                if self.phase_since.elapsed() >= BASELINE {
                    match self.start_mouse_storm() {
                        Ok(storm) => {
                            self.storm = Some(storm);
                            self.set_phase(Phase::Storm);
                        }
                        Err(err) => self.fail(&err),
                    }
                }
            }
            Phase::Storm => {
                self.set_present_pressure(backend, true);
                let done = self.storm.as_ref().is_some_and(MouseStorm::is_done);
                if done || self.phase_since.elapsed() >= self.config.storm {
                    self.stop_storm();
                    self.set_phase(Phase::StaticTextGap);
                }
            }
            Phase::StaticTextGap => {
                self.set_present_pressure(backend, false);
                if self.phase_since.elapsed() >= STAGE_GAP {
                    let text_applied = self.enable_text_overlay(backend, cx);
                    if text_applied == 0 {
                        self.fail("chart opened but static text stress overlay did not attach");
                        return;
                    }
                    self.set_phase(Phase::StaticTextWarmup);
                }
            }
            Phase::StaticTextWarmup => {
                self.set_present_pressure(backend, true);
                if self.phase_since.elapsed() >= TEXT_WARMUP {
                    match self.start_mouse_storm() {
                        Ok(storm) => {
                            self.storm = Some(storm);
                            self.set_phase(Phase::StaticTextStorm);
                        }
                        Err(err) => self.fail(&err),
                    }
                }
            }
            Phase::StaticTextStorm => {
                self.set_present_pressure(backend, true);
                let done = self.storm.as_ref().is_some_and(MouseStorm::is_done);
                if done || self.phase_since.elapsed() >= self.config.storm {
                    self.stop_storm();
                    self.set_present_pressure(backend, false);
                    self.set_phase(Phase::CommandErrorContract);
                }
            }
            Phase::CommandErrorContract => {
                if self.phase_since.elapsed() >= STAGE_GAP {
                    if let Err(error) = self.verify_command_error_contract(backend) {
                        self.fail(&error);
                    } else {
                        self.set_phase(Phase::ToolWindowsOpen);
                    }
                }
            }
            Phase::ToolWindowsOpen => {
                if self.phase_since.elapsed() >= STAGE_GAP {
                    self.request_tool_windows_open(cx);
                    self.set_phase(Phase::ToolWindowsVerifyOpen);
                }
            }
            Phase::ToolWindowsVerifyOpen => {
                if self.phase_since.elapsed() >= STAGE_GAP {
                    if let Err(error) = self.verify_tool_windows_open(backend) {
                        self.fail(&error);
                    } else {
                        self.request_tool_windows_open(cx);
                        self.set_phase(Phase::ToolWindowsDedup);
                    }
                }
            }
            Phase::ToolWindowsDedup => {
                if self.phase_since.elapsed() >= STAGE_GAP {
                    self.set_phase(Phase::ToolWindowsVerifyDedup);
                }
            }
            Phase::ToolWindowsVerifyDedup => {
                if self.phase_since.elapsed() >= STAGE_GAP {
                    if let Err(error) = self.verify_tool_windows_dedup(backend) {
                        self.fail(&error);
                    } else {
                        self.set_phase(Phase::RootOverlayContract);
                    }
                }
            }
            Phase::RootOverlayContract => {
                if self.phase_since.elapsed() >= STAGE_GAP {
                    if let Err(error) = self.verify_root_overlay_contract(backend, cx) {
                        self.fail(&error);
                    } else {
                        self.set_phase(Phase::LocaleSwitch);
                    }
                }
            }
            Phase::LocaleSwitch => {
                if self.phase_since.elapsed() >= STAGE_GAP {
                    self.request_locale_switch(backend, cx);
                    self.set_phase(Phase::LocaleSwitchVerify);
                }
            }
            Phase::LocaleSwitchVerify => {
                if self.phase_since.elapsed() >= STAGE_GAP {
                    let result = self.verify_locale_switch(backend);
                    self.restore_locale(backend, cx);
                    if let Err(error) = result {
                        self.fail(&error);
                    } else {
                        self.set_phase(Phase::PriceScale50);
                    }
                }
            }
            Phase::PriceScale50 => {
                if self.phase_since.elapsed() >= STAGE_GAP {
                    self.request_price_scale(backend, Some(0.50), cx);
                    self.set_phase(Phase::PriceScale20);
                }
            }
            Phase::PriceScale20 => {
                if self.phase_since.elapsed() >= STAGE_GAP {
                    if let Err(error) = self.verify_price_scale(backend, Some(0.50)) {
                        self.fail(&error);
                    } else {
                        self.request_price_scale(backend, Some(0.20), cx);
                        self.set_phase(Phase::PriceScaleAuto);
                    }
                }
            }
            Phase::PriceScaleAuto => {
                if self.phase_since.elapsed() >= STAGE_GAP {
                    if let Err(error) = self.verify_price_scale(backend, Some(0.20)) {
                        self.fail(&error);
                    } else {
                        self.request_price_scale(backend, None, cx);
                        self.set_phase(Phase::PriceScaleVerifyAuto);
                    }
                }
            }
            Phase::PriceScaleVerifyAuto => {
                if self.phase_since.elapsed() >= STAGE_GAP {
                    if let Err(error) = self.verify_price_scale(backend, None) {
                        self.fail(&error);
                    } else {
                        self.set_phase(Phase::OrderCancelLag);
                    }
                }
            }
            Phase::OrderCancelLag => {
                if self.phase_since.elapsed() >= ORDER_CANCEL_TIMEOUT {
                    self.fail("order_cancel_lag timed out");
                    return;
                }
                match self.tick_order_cancel_lag(backend) {
                    Ok(true) => self.set_phase(Phase::Cooldown),
                    Ok(false) => {}
                    Err(error) => self.fail(&error),
                }
            }
            Phase::Cooldown => {
                self.set_present_pressure(backend, false);
                if self.phase_since.elapsed() >= COOLDOWN {
                    self.evaluate_and_exit();
                }
            }
            Phase::Done => {}
            Phase::StageCount => {
                unreachable!("firetest phase count sentinel is not a runtime phase")
            }
        }
    }

    fn start_mouse_storm(&mut self) -> Result<MouseStorm, String> {
        let Some(probe) = self.probe else {
            return Err("missing chart probe before mouse storm".to_string());
        };
        start_mouse_storm(probe, self.config.storm, self.config.mouse_hz)
    }

    fn request_tool_windows_open(&self, cx: &mut Context<Backend>) {
        let backend_entity = cx.entity();
        cx.defer(move |cx| {
            crate::settings::open(backend_entity.clone(), None, cx);
            crate::strategies::open(backend_entity.clone(), None, cx);
            crate::panels::open_assets_window(backend_entity, None, cx);
        });
        firetest_info("[firetest] tool_windows_open deferred=settings,strategies,assets");
    }

    fn tool_window_ids(backend: &Backend) -> Result<(String, String, String), String> {
        let Some(settings) = backend
            .settings_window
            .map(|h| format!("{:?}", h.window_id()))
        else {
            return Err("settings tool window did not open".into());
        };
        let Some(strategies) = backend
            .strategies_window
            .map(|h| format!("{:?}", h.window_id()))
        else {
            return Err("strategies tool window did not open".into());
        };
        let Some(assets) = backend
            .assets_window
            .map(|h| format!("{:?}", h.window_id()))
        else {
            return Err("assets tool window did not open".into());
        };
        Ok((settings, strategies, assets))
    }

    fn verify_tool_windows_open(&mut self, backend: &Backend) -> Result<(), String> {
        let ids = Self::tool_window_ids(backend)?;
        firetest_info(&format!(
            "[firetest] tool_windows_open settings={} strategies={} assets={}",
            ids.0, ids.1, ids.2
        ));
        self.tool_window_ids = Some(ids);
        Ok(())
    }

    fn verify_tool_windows_dedup(&self, backend: &Backend) -> Result<(), String> {
        let before = self
            .tool_window_ids
            .as_ref()
            .ok_or_else(|| "tool window dedup has no baseline ids".to_string())?;
        let after = Self::tool_window_ids(backend)?;

        if before.0 != after.0 {
            return Err("settings tool window dedup created a new window".into());
        }
        if before.1 != after.1 {
            return Err("strategies tool window dedup created a new window".into());
        }
        if before.2 != after.2 {
            return Err("assets tool window dedup created a new window".into());
        }

        firetest_info(&format!(
            "[firetest] tool_windows_dedup settings={} strategies={} assets={}",
            after.0, after.1, after.2
        ));
        Ok(())
    }

    fn verify_root_overlay_contract(
        &self,
        backend: &mut Backend,
        cx: &mut Context<Backend>,
    ) -> Result<(), String> {
        let Some(handle) = backend.strategies_window else {
            return Err("strategies tool window is required for Root overlay contract".into());
        };
        handle
            .update(cx, |root, window, cx| {
                root.close_context_menu(window, cx);
                root.close_all_dialogs(window, cx);
                root.clear_notifications(window, cx);

                root.open_context_menu(|_window, _cx| div().into_any_element(), window, cx);
                if !root.has_active_context_menu() {
                    return Err("Root context menu did not become active".to_string());
                }

                root.open_unique_moon_dialog(
                    "firetest-root-dialog",
                    |dialog, _window, _cx| {
                        dialog
                            .w(px(260.0))
                            .title(div().child("FireTest dialog"))
                            .content(|content, _window, _cx| {
                                content.child(div().child("Root-owned overlay"))
                            })
                    },
                    window,
                    cx,
                );
                if root.has_active_context_menu() {
                    return Err("Root context menu stayed active after dialog open".to_string());
                }
                if root.active_dialog_count() != 1 {
                    return Err(format!(
                        "Root unique dialog count after first open is {}, expected 1",
                        root.active_dialog_count()
                    ));
                }

                root.open_unique_moon_dialog(
                    "firetest-root-dialog",
                    |dialog, _window, _cx| {
                        dialog
                            .w(px(260.0))
                            .title(div().child("FireTest dialog replacement"))
                            .content(|content, _window, _cx| {
                                content.child(div().child("Replacement"))
                            })
                    },
                    window,
                    cx,
                );
                if root.active_dialog_count() != 1 {
                    return Err(format!(
                        "Root unique dialog replacement created {} dialogs",
                        root.active_dialog_count()
                    ));
                }

                root.push_notification(
                    MoonNotification::error("FireTest root notification").autohide(false),
                    window,
                    cx,
                );
                if root.notification_count(cx) != 1 {
                    return Err(format!(
                        "Root notification count is {}, expected 1",
                        root.notification_count(cx)
                    ));
                }

                root.close_all_dialogs(window, cx);
                root.clear_notifications(window, cx);
                if root.active_dialog_count() != 0 || root.notification_count(cx) != 0 {
                    return Err("Root overlay cleanup left dialog or notification active".into());
                }

                firetest_info(
                    "[firetest] root_overlay_contract context_menu dialog notification ok",
                );
                Ok(())
            })
            .map_err(|error| format!("Root overlay contract window update failed: {error}"))?
    }

    fn verify_command_error_contract(&self, backend: &Backend) -> Result<(), String> {
        let missing_core = u64::MAX;
        let session_exists = backend
            .session
            .sessions()
            .iter()
            .any(|session| session.id == missing_core);
        if session_exists {
            return Err("firetest missing-core sentinel unexpectedly exists".into());
        }

        match backend.session.refresh_transfer_assets(missing_core) {
            Ok(()) => Err(
                "session command to missing core returned Ok; UI could close dialog silently"
                    .into(),
            ),
            Err(error) => {
                firetest_info(&format!(
                    "[firetest] command_error_contract missing_core={missing_core} err={error}"
                ));
                Ok(())
            }
        }
    }

    fn start_order_cancel_lag(&self, backend: &Backend) -> Result<OrderCancelRun, String> {
        let group = self
            .opened_group
            .as_ref()
            .ok_or_else(|| "order_cancel_lag has no opened chart group".to_string())?;
        let (core, market) = backend.main_chart_target(group).ok_or_else(|| {
            format!("order_cancel_lag has no main chart target for group={group}")
        })?;
        let latest_price = backend
            .session
            .market_source()
            .latest_price(core, &market)
            .map_err(|reason| {
                format!("order_cancel_lag has no live-correct latest price for {market}: {reason}")
            })?;
        let price = (latest_price as f64 * self.config.order_cancel_price_mult).max(1e-8);
        let size = self.config.order_cancel_size.unwrap_or_else(|| {
            self.config
                .order_cancel_quote_size
                .map(|quote| quote / price)
                .unwrap_or_else(|| backend.manual_order_size(core))
        });
        if !(size.is_finite() && size > 0.0) {
            return Err(format!("order_cancel_lag invalid order size {size}"));
        }
        let before_uids: HashSet<u64> = backend
            .session
            .store()
            .core(core)
            .map(|core| core.orders.iter().map(|order| order.uid).collect())
            .unwrap_or_default();
        let feed_log_enabled = backend
            .config
            .servers
            .iter()
            .find(|server| server.id == core)
            .is_some_and(|server| server.feed.log);
        if !feed_log_enabled {
            firetest_info(&format!(
                "[firetest] order_cancel_lag warning core={core} feed.log=false server_log_metrics=missing"
            ));
        }
        let place_submit_ms = now_unix_ms_i64();
        backend
            .session
            .place_order(core, market.clone(), false, price, size, None)
            .map_err(|error| format!("order_cancel_lag place order failed: {error:#}"))?;
        firetest_info(&format!(
            "[firetest] order_cancel_lag place core={core} market={market} price={price:.8} size={size:.8} quote_size={} latest_price={latest_price:.8}",
            opt_f64(self.config.order_cancel_quote_size)
        ));
        Ok(OrderCancelRun {
            core,
            market,
            before_uids,
            price,
            size,
            place_submit_ms,
            uid: None,
            order_seen_ms: None,
            cancel_submit_ms: None,
            closed_store_ms: None,
            closed_order_lines_rev: None,
            closed_reason: None,
            server_log: None,
            step: OrderCancelStep::WaitOrder,
        })
    }

    fn tick_order_cancel_lag(&mut self, backend: &mut Backend) -> Result<bool, String> {
        if !self.config.order_cancel_lag {
            firetest_info(
                "[firetest] order_cancel_lag skipped (set MOON_FIRETEST_ORDER_CANCEL=1 to enable real order test)",
            );
            return Ok(true);
        }

        let mut run = match self.order_cancel.take() {
            Some(run) => run,
            None => {
                self.order_cancel = Some(self.start_order_cancel_lag(backend)?);
                return Ok(false);
            }
        };
        match run.step {
            OrderCancelStep::WaitOrder => {
                let Some(core) = backend.session.store().core(run.core) else {
                    return Err(format!("order_cancel_lag core={} disappeared", run.core));
                };
                let found = core
                    .orders
                    .iter()
                    .filter(|order| {
                        order.market == run.market
                            && !run.before_uids.contains(&order.uid)
                            && !order.is_short
                            && !order.job_is_done
                            && (order.buy_price - run.price).abs()
                                <= run.price.abs().mul_add(0.03, 1e-8)
                    })
                    .max_by_key(|order| order.uid)
                    .map(|order| order.uid);
                let Some(uid) = found else {
                    self.wait_log("order_cancel_lag waiting for placed order snapshot");
                    self.order_cancel = Some(run);
                    return Ok(false);
                };
                let now = now_unix_ms_i64();
                backend
                    .session
                    .cancel_order(run.core, uid)
                    .map_err(|error| {
                        format!("order_cancel_lag cancel order {uid} failed: {error:#}")
                    })?;
                run.uid = Some(uid);
                run.order_seen_ms = Some(now);
                run.cancel_submit_ms = Some(now_unix_ms_i64());
                run.step = OrderCancelStep::WaitClosed;
                firetest_info(&format!(
                    "[firetest] order_cancel_lag cancel uid={uid} place_to_seen_ms={} core={} market={}",
                    now - run.place_submit_ms,
                    run.core,
                    run.market
                ));
            }
            OrderCancelStep::WaitClosed => {
                let uid = run
                    .uid
                    .ok_or_else(|| "order_cancel_lag waiting closed without uid".to_string())?;
                let Some(core) = backend.session.store().core(run.core) else {
                    return Err(format!("order_cancel_lag core={} disappeared", run.core));
                };
                if run.server_log.is_none() {
                    run.server_log = find_order_cancel_log(
                        core.raw_server_log_snapshot(300),
                        uid,
                        run.cancel_submit_ms.unwrap_or_default(),
                    );
                }
                if run.closed_store_ms.is_none() {
                    if let Some(state) = core.order_lines.order_state(uid) {
                        if let (Some(closed_store_ms), Some(closed_rev)) =
                            (state.closed_store_ms, state.closed_rev)
                        {
                            let closed_ms_i64 = closed_store_ms.round() as i64;
                            run.closed_store_ms = Some(closed_ms_i64);
                            run.closed_order_lines_rev = Some(closed_rev);
                            run.closed_reason = state.closed_reason;
                            firetest_info(&format!(
                                "[firetest] order_cancel_lag closed uid={uid} order_lines_rev={} reason={:?} cancel_to_order_lines_ms={}",
                                closed_rev,
                                state.closed_reason,
                                closed_ms_i64 - run.cancel_submit_ms.unwrap_or(closed_ms_i64)
                            ));
                        }
                    }
                }
                let Some(closed_rev) = run.closed_order_lines_rev else {
                    self.wait_log("order_cancel_lag waiting for cancelled order snapshot");
                    self.order_cancel = Some(run);
                    return Ok(false);
                };
                let Some(group) = self.opened_group.as_ref() else {
                    return Err("order_cancel_lag has no opened group".into());
                };
                #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
                let probe = backend
                    .debug_main_chart_handles
                    .get(group)
                    .and_then(|chart| chart.order_render_probe(run.core, &run.market));
                #[cfg(not(any(debug_assertions, moon_profile_debug, feature = "debug-tools")))]
                let probe = {
                    let _ = (backend, group);
                    None
                };
                let Some(probe) = probe else {
                    self.wait_log("order_cancel_lag waiting for chart order render probe");
                    self.order_cancel = Some(run);
                    return Ok(false);
                };
                if probe.gpu_rev != closed_rev {
                    self.wait_log("order_cancel_lag waiting for chart GPU userdata revision");
                    self.order_cancel = Some(run);
                    return Ok(false);
                }
                if probe.present_rev != closed_rev {
                    self.wait_log(
                        "order_cancel_lag waiting for chart present after order revision",
                    );
                    self.order_cancel = Some(run);
                    return Ok(false);
                }
                if run.closed_reason != Some(OrderCloseReason::Cancel) {
                    return Err(format!(
                        "order_cancel_lag uid={uid} closed with {:?}, expected explicit Cancel",
                        run.closed_reason
                    ));
                }

                let closed_store_ms = run.closed_store_ms.unwrap_or_default() as f64;
                let display_lag_ms = (probe.present_ms - closed_store_ms).max(0.0);
                let sync_to_gpu_ms = (probe.gpu_ms - probe.order_lines_sync_ms).max(0.0);
                let gpu_to_present_ms = (probe.present_ms - probe.gpu_ms).max(0.0);
                let cancel_to_chart_ms =
                    (probe.present_ms - run.cancel_submit_ms.unwrap_or_default() as f64).max(0.0);
                let server_log = run.server_log.as_ref();
                let server_to_recv_ms = server_log.map(|line| line.recv_ms - line.time_ms);
                let log_recv_to_chart_ms =
                    server_log.map(|line| (probe.gpu_ms - line.recv_ms as f64).max(0.0));
                firetest_info(&format!(
                    "[firetest] order_cancel_lag result uid={uid} core={} market={} price={:.8} size={} closed_order_lines_rev={} probe_order_lines_rev={} display_lag_ms={display_lag_ms:.1} sync_to_gpu_ms={sync_to_gpu_ms:.1} gpu_to_present_ms={gpu_to_present_ms:.1} cancel_to_visible_ms={cancel_to_chart_ms:.1} server_to_recv_ms={} log_recv_to_chart_ms={} server_log={}",
                    run.core,
                    run.market,
                    run.price,
                    run.size,
                    closed_rev,
                    probe.order_lines_rev,
                    opt_i64(server_to_recv_ms),
                    opt_f64(log_recv_to_chart_ms),
                    server_log
                        .map(|line| line.msg.replace('\n', " ⏎ "))
                        .unwrap_or_else(|| "missing".to_string())
                ));
                if display_lag_ms > self.config.order_cancel_max_display_lag_ms {
                    return Err(format!(
                        "order_cancel_lag display_lag_ms {display_lag_ms:.1} > {:.1}",
                        self.config.order_cancel_max_display_lag_ms
                    ));
                }
                return Ok(true);
            }
        }
        self.order_cancel = Some(run);
        Ok(false)
    }

    fn wait_log(&mut self, msg: &str) {
        if self.last_wait_log.elapsed() < Duration::from_millis(1000) {
            return;
        }
        self.last_wait_log = Instant::now();
        firetest_info(&format!("[firetest] {msg}"));
    }

    fn try_open_chart(&mut self, backend: &mut Backend, cx: &mut Context<Backend>) -> bool {
        if backend.open_request.is_some() || backend.group_windows.is_empty() {
            return false;
        }

        let target_market = self.config.market.trim();
        let candidate = backend.config.servers.iter().find_map(|server| {
            let session_exists = backend
                .session
                .sessions()
                .iter()
                .any(|session| session.id == server.id && session.group == server.group);
            (server.active
                && server.show_window
                && backend.config.group(&server.group).active
                && backend.group_windows.contains_key(&server.group)
                && session_exists)
                .then(|| (server.id, server.group.clone(), server.name.clone()))
        });
        let Some((core, group, name)) = candidate else {
            return false;
        };

        let market = target_market.to_string();
        backend.open_request = Some((core, market.clone()));
        backend.open_request_rev = backend.open_request_rev.wrapping_add(1);
        backend.open_request_activate = false;
        backend.follow = true;
        self.opened_group = Some(group.clone());
        firetest_info(&format!(
            "[firetest] open chart: core={core} group={group} name={name} market={market}"
        ));
        cx.notify();
        true
    }

    fn request_price_scale(
        &self,
        backend: &mut Backend,
        scale: Option<f32>,
        cx: &mut Context<Backend>,
    ) {
        backend.price_scale = scale;
        backend.price_scale_group = self.opened_group.clone();
        backend.price_scale_rev = backend.price_scale_rev.wrapping_add(1);
        firetest_info(&format!(
            "[firetest] price_scale_request value={}",
            scale_label(scale)
        ));
        cx.notify();
    }

    fn verify_price_scale(&self, backend: &Backend, expected: Option<f32>) -> Result<(), String> {
        let group = self
            .opened_group
            .as_ref()
            .ok_or_else(|| "price scale contract has no opened chart group".to_string())?;
        let actual = backend
            .chart_specs
            .iter()
            .find(|spec| {
                spec.group == *group && spec.num == 0 && spec.bucket() == ChartBucket::Shared
            })
            .and_then(|spec| spec.scale);
        if actual != expected {
            return Err(format!(
                "price scale did not reach active chart: expected {}, got {}",
                scale_label(expected),
                scale_label(actual)
            ));
        }
        firetest_info(&format!(
            "[firetest] price_scale_verify group={group} value={}",
            scale_label(actual)
        ));
        Ok(())
    }

    /// Сменить язык интерфейса тем же live-apply путём, что и Settings::apply_settings:
    /// глобальная локаль rust-i18n + `refresh_windows()` (БЕЗ пересоздания окон). Цель —
    /// любой язык, отличный от текущего; исходный запоминаем для восстановления.
    fn request_locale_switch(&mut self, backend: &mut Backend, cx: &mut Context<Backend>) {
        let original = backend.config.language;
        let target = if original == Language::En {
            Language::Ru
        } else {
            Language::En
        };
        self.locale_switch = Some((original, target));
        backend.config.language = target;
        rust_i18n::set_locale(target.code());
        cx.refresh_windows();
        cx.notify();
        firetest_info(&format!(
            "[firetest] locale_switch from={} to={}",
            original.code(),
            target.code()
        ));
    }

    /// Долёт смены языка: глобальная локаль обязана стать целевой, а tool-окна — остаться
    /// теми же (смена языка живая, окна не пересоздаются — иначе dedup/раскладка ломаются).
    fn verify_locale_switch(&self, backend: &Backend) -> Result<(), String> {
        let (_, target) = self
            .locale_switch
            .ok_or_else(|| "locale switch contract has no recorded target".to_string())?;
        let active = rust_i18n::locale();
        if &*active != target.code() {
            return Err(format!(
                "locale switch did not reach rust-i18n: expected {}, got {}",
                target.code(),
                &*active
            ));
        }
        let before = self
            .tool_window_ids
            .as_ref()
            .ok_or_else(|| "locale switch has no tool window baseline ids".to_string())?;
        let after = Self::tool_window_ids(backend)?;
        if *before != after {
            return Err("locale switch recreated a tool window instead of redrawing it".into());
        }
        firetest_info(&format!(
            "[firetest] locale_switch_verify locale={} windows_stable=true",
            target.code()
        ));
        Ok(())
    }

    /// Вернуть исходный язык, чтобы стадия не отравляла локаль для остального прогона/логов.
    fn restore_locale(&mut self, backend: &mut Backend, cx: &mut Context<Backend>) {
        if let Some((original, _)) = self.locale_switch.take() {
            backend.config.language = original;
            rust_i18n::set_locale(original.code());
            cx.refresh_windows();
            cx.notify();
        }
    }

    fn set_present_pressure(&mut self, backend: &mut Backend, enabled: bool) {
        if self.present_pressure_enabled == enabled {
            return;
        }
        self.present_pressure_enabled = enabled;
        for chart in backend.live_chart_consumers() {
            chart.set_firetest_force_present(enabled);
        }
    }

    fn enable_text_overlay(&mut self, backend: &mut Backend, cx: &mut Context<Backend>) -> usize {
        if self.text_overlay_enabled {
            return 0;
        }
        self.text_overlay_enabled = true;
        let count = self.config.text_labels;
        if count == 0 {
            firetest_info("[firetest] text overlay disabled");
            return 0;
        }
        let mut applied = 0usize;
        if let Some(chart) = backend.live_chart_consumers().into_iter().next() {
            if chart.set_firetest_text_labels(count) {
                applied += 1;
            }
        }
        firetest_info(&format!(
            "[firetest] text overlay labels={count} applied_to={applied}"
        ));
        if applied > 0 {
            cx.notify();
        }
        applied
    }

    fn stop_storm(&mut self) {
        if let Some(storm) = self.storm.take() {
            storm.stop();
        }
    }

    fn evaluate_and_exit(&mut self) {
        self.set_phase(Phase::Done);
        if matches!(self.config.script, Script::OrderCancelLag) {
            firetest_info("[firetest] result=PASS FIRETEST PASS order_cancel_lag");
            std::process::exit(0);
        }
        let baseline: Vec<&Sample> = self
            .samples
            .iter()
            .filter(|s| s.phase == Phase::Baseline)
            .collect();
        let clean_storm: Vec<&Sample> = self
            .samples
            .iter()
            .filter(|s| s.phase == Phase::Storm)
            .collect();
        let static_text_storm: Vec<&Sample> = self
            .samples
            .iter()
            .filter(|s| s.phase == Phase::StaticTextStorm)
            .collect();
        let storm: Vec<&Sample> = clean_storm
            .iter()
            .copied()
            .chain(static_text_storm.iter().copied())
            .collect();
        if storm.is_empty() {
            self.fail("no storm diag samples");
            return;
        }
        if clean_storm.is_empty() {
            self.fail("no clean mouse storm diag samples");
            return;
        }
        if static_text_storm.is_empty() {
            self.fail("no static text storm diag samples");
            return;
        }

        let avg_rate = |label: &str| -> f64 {
            storm.iter().map(|s| rate(s, label)).sum::<f64>() / storm.len() as f64
        };
        let static_text_avg_rate = |label: &str| -> f64 {
            static_text_storm
                .iter()
                .map(|s| rate(s, label))
                .sum::<f64>()
                / static_text_storm.len() as f64
        };
        let static_text_max_rate = |label: &str| -> f64 {
            static_text_storm
                .iter()
                .map(|s| rate(s, label))
                .fold(0.0_f64, f64::max)
        };
        let baseline_max_rate = |label: &str| -> f64 {
            baseline
                .iter()
                .map(|s| rate(s, label))
                .fold(0.0_f64, f64::max)
        };
        let rate_delta =
            |label: &str| -> f64 { (avg_rate(label) - baseline_max_rate(label)).max(0.0) };
        let max_rate =
            |label: &str| -> f64 { storm.iter().map(|s| rate(s, label)).fold(0.0_f64, f64::max) };
        let avg_cpu = storm
            .iter()
            .map(|s| s.metrics.cpu_process as f64)
            .sum::<f64>()
            / storm.len() as f64;
        let max_cpu = storm
            .iter()
            .map(|s| s.metrics.cpu_process as f64)
            .fold(0.0_f64, f64::max);
        let baseline_cpu = if baseline.is_empty() {
            0.0
        } else {
            baseline
                .iter()
                .map(|s| s.metrics.cpu_process as f64)
                .sum::<f64>()
                / baseline.len() as f64
        };
        let cpu_delta = (avg_cpu - baseline_cpu).max(0.0);
        let static_text_avg_cpu = static_text_storm
            .iter()
            .map(|s| s.metrics.cpu_process as f64)
            .sum::<f64>()
            / static_text_storm.len() as f64;
        let static_text_max_cpu = static_text_storm
            .iter()
            .map(|s| s.metrics.cpu_process as f64)
            .fold(0.0_f64, f64::max);
        let static_text_cpu_delta = (static_text_avg_cpu - baseline_cpu).max(0.0);
        let avg_gpu_process = storm
            .iter()
            .map(|s| s.metrics.gpu_process as f64)
            .sum::<f64>()
            / storm.len() as f64;
        let max_gpu_process = storm
            .iter()
            .map(|s| s.metrics.gpu_process as f64)
            .fold(0.0_f64, f64::max);
        let baseline_gpu_process = if baseline.is_empty() {
            0.0
        } else {
            baseline
                .iter()
                .map(|s| s.metrics.gpu_process as f64)
                .sum::<f64>()
                / baseline.len() as f64
        };
        let gpu_process_delta = (avg_gpu_process - baseline_gpu_process).max(0.0);
        let static_text_avg_gpu_process = static_text_storm
            .iter()
            .map(|s| s.metrics.gpu_process as f64)
            .sum::<f64>()
            / static_text_storm.len() as f64;
        let static_text_max_gpu_process = static_text_storm
            .iter()
            .map(|s| s.metrics.gpu_process as f64)
            .fold(0.0_f64, f64::max);
        let static_text_gpu_process_delta =
            (static_text_avg_gpu_process - baseline_gpu_process).max(0.0);
        let avg_gpu_frame_ms =
            storm.iter().map(|s| s.gpu_frame_ms).sum::<f64>() / storm.len() as f64;
        let max_gpu_frame_ms = storm.iter().map(|s| s.gpu_frame_ms).fold(0.0_f64, f64::max);
        let static_text_avg_gpu_frame_ms = static_text_storm
            .iter()
            .map(|s| s.gpu_frame_ms)
            .sum::<f64>()
            / static_text_storm.len() as f64;
        let static_text_max_gpu_frame_ms = static_text_storm
            .iter()
            .map(|s| s.gpu_frame_ms)
            .fold(0.0_f64, f64::max);
        let mem_values: Vec<f64> = storm
            .iter()
            .map(|s| s.metrics.mem_mb as f64)
            .filter(|m| *m > 1.0)
            .collect();
        let mem_growth = if mem_values.len() >= 2 {
            let mem_min = mem_values.iter().copied().fold(f64::INFINITY, f64::min);
            let mem_max = mem_values.iter().copied().fold(0.0_f64, f64::max);
            (mem_max - mem_min).max(0.0)
        } else {
            0.0
        };
        let chart_mouse_min = chart_mouse_min_hz(avg_rate("chart_present"));
        let static_text_chart_mouse_min = chart_mouse_min_hz(static_text_avg_rate("chart_present"));

        let mut fail = Vec::new();
        check_min(
            &mut fail,
            "firetest_mouse_sent",
            avg_rate("firetest_mouse_sent"),
            1000.0,
        );
        check_min(
            &mut fail,
            "chart_mouse_move",
            avg_rate("chart_mouse_move"),
            chart_mouse_min,
        );
        let chart_mouse = avg_rate("chart_mouse_move");
        let fast_mouse = avg_rate("chart_mouse_move_fast");
        if chart_mouse > 1.0 {
            check_min(
                &mut fail,
                "chart_mouse_fast_coverage",
                fast_mouse / chart_mouse,
                0.90,
            );
        }
        check_max(
            &mut fail,
            "chart_mouse_move_entity",
            max_rate("chart_mouse_move_entity"),
            5.0,
        );
        check_max(&mut fail, "shell_render", max_rate("shell_render"), 10.0);
        check_max(&mut fail, "orders_render", max_rate("orders_render"), 10.0);
        check_max(&mut fail, "chart_render", max_rate("chart_render"), 10.0);
        check_max(
            &mut fail,
            "chart_input_notify",
            max_rate("chart_input_notify"),
            5.0,
        );
        check_max(
            &mut fail,
            "chart_canvas_notify",
            max_rate("chart_canvas_notify"),
            5.0,
        );
        check_max(
            &mut fail,
            "chart_gpu_prepare_delta",
            rate_delta("chart_gpu_prepare"),
            8.0,
        );
        check_max(&mut fail, "bg_draw_delta", rate_delta("bg_draw"), 12.0);
        check_max(&mut fail, "grid_draw_delta", rate_delta("grid_draw"), 12.0);
        check_combo_draw_delta(&mut fail, rate_delta("combo_draw"));
        check_max(
            &mut fail,
            "userdata_draw_delta",
            rate_delta("userdata_draw"),
            12.0,
        );
        check_max(&mut fail, "base_bake_delta", rate_delta("base_bake"), 8.0);
        check_max(&mut fail, "combo_bake_delta", rate_delta("combo_bake"), 8.0);
        check_max(
            &mut fail,
            "orderbook_bake_delta",
            rate_delta("orderbook_bake"),
            8.0,
        );
        if self.config.text_labels > 0 {
            check_max(
                &mut fail,
                "firetest_text_cold",
                max_rate("firetest_text_cold"),
                100.0,
            );
            check_min(
                &mut fail,
                "static_text_firetest_mouse_sent",
                static_text_avg_rate("firetest_mouse_sent"),
                1000.0,
            );
            check_min(
                &mut fail,
                "static_text_chart_mouse_move",
                static_text_avg_rate("chart_mouse_move"),
                static_text_chart_mouse_min,
            );
            let static_text_chart_mouse = static_text_avg_rate("chart_mouse_move");
            let static_text_fast_mouse = static_text_avg_rate("chart_mouse_move_fast");
            if static_text_chart_mouse > 1.0 {
                check_min(
                    &mut fail,
                    "static_text_chart_mouse_fast_coverage",
                    static_text_fast_mouse / static_text_chart_mouse,
                    0.90,
                );
            }
            check_max(
                &mut fail,
                "static_text_chart_mouse_move_entity",
                static_text_max_rate("chart_mouse_move_entity"),
                5.0,
            );
            check_max(
                &mut fail,
                "static_text_chart_input_notify",
                static_text_max_rate("chart_input_notify"),
                5.0,
            );
            check_max(
                &mut fail,
                "static_text_chart_canvas_notify",
                static_text_max_rate("chart_canvas_notify"),
                5.0,
            );
            check_max(
                &mut fail,
                "static_text_cpu_process_avg",
                static_text_avg_cpu,
                25.0,
            );
            check_max(
                &mut fail,
                "static_text_cpu_process_delta",
                static_text_cpu_delta,
                12.0,
            );
            check_max(
                &mut fail,
                "static_text_cpu_process_max",
                static_text_max_cpu,
                40.0,
            );
            if static_text_max_gpu_process > 0.1 {
                check_max(
                    &mut fail,
                    "static_text_gpu_process_avg",
                    static_text_avg_gpu_process,
                    35.0,
                );
                check_max(
                    &mut fail,
                    "static_text_gpu_process_delta",
                    static_text_gpu_process_delta,
                    25.0,
                );
                check_max(
                    &mut fail,
                    "static_text_gpu_process_max",
                    static_text_max_gpu_process,
                    70.0,
                );
            }
            if static_text_max_gpu_frame_ms > 0.01 {
                check_max(
                    &mut fail,
                    "static_text_gpu_frame_ms_avg",
                    static_text_avg_gpu_frame_ms,
                    6.0,
                );
                check_max(
                    &mut fail,
                    "static_text_gpu_frame_ms_max",
                    static_text_max_gpu_frame_ms,
                    16.0,
                );
            }
        }
        check_max(&mut fail, "cpu_process_avg", avg_cpu, 25.0);
        check_max(&mut fail, "cpu_process_delta", cpu_delta, 12.0);
        check_max(&mut fail, "cpu_process_max", max_cpu, 40.0);
        if max_gpu_process > 0.1 {
            check_max(&mut fail, "gpu_process_avg", avg_gpu_process, 35.0);
            check_max(&mut fail, "gpu_process_delta", gpu_process_delta, 25.0);
            check_max(&mut fail, "gpu_process_max", max_gpu_process, 70.0);
        }
        if max_gpu_frame_ms > 0.01 {
            check_max(&mut fail, "gpu_frame_ms_avg", avg_gpu_frame_ms, 6.0);
            check_max(&mut fail, "gpu_frame_ms_max", max_gpu_frame_ms, 16.0);
        }
        check_max(&mut fail, "mem_growth_mb", mem_growth, 96.0);

        let summary = format!(
            "mouse_sent={:.0}/s chart_mouse={:.0}/s fast={:.0}/s entity={:.0}/s fast_stop={:.0}/s shell={:.0}/s orders={:.0}/s chart_render={:.0}/s input_notify={:.0}/s text_draw={:.0}/s text_cold={:.0}/s static_text_labels={} static_text_chart_mouse={:.0}/s static_text_fast={:.0}/s static_text_cpu_avg={:.1}% static_text_gpu_proc_avg={:.1}% static_text_text_draw={:.0}/s static_text_text_cold={:.0}/s cpu_avg={:.1}% cpu_delta={:.1}% gpu_proc_avg={:.1}% gpu_proc_delta={:.1}% gpu_proc_max={:.1}% gpu_frame_avg={:.3}ms gpu_frame_max={:.3}ms mem_growth={:.1}MB present={:.0}/s cam_step={:.0}/s gpu_prepare={:.0}/s(+{:.0}) bg_draw={:.0}/s(+{:.0}) combo_draw={:.0}/s(+{:.0}) base_bake={:.0}/s(+{:.0}) combo_bake={:.0}/s(+{:.0}) book_bake={:.0}/s(+{:.0})",
            avg_rate("firetest_mouse_sent"),
            avg_rate("chart_mouse_move"),
            avg_rate("chart_mouse_move_fast"),
            avg_rate("chart_mouse_move_entity"),
            avg_rate("chart_mouse_fast_stop"),
            avg_rate("shell_render"),
            avg_rate("orders_render"),
            avg_rate("chart_render"),
            avg_rate("chart_input_notify"),
            avg_rate("firetest_text_draw"),
            avg_rate("firetest_text_cold"),
            self.config.text_labels,
            static_text_avg_rate("chart_mouse_move"),
            static_text_avg_rate("chart_mouse_move_fast"),
            static_text_avg_cpu,
            static_text_avg_gpu_process,
            static_text_avg_rate("firetest_text_draw"),
            static_text_avg_rate("firetest_text_cold"),
            avg_cpu,
            cpu_delta,
            avg_gpu_process,
            gpu_process_delta,
            max_gpu_process,
            avg_gpu_frame_ms,
            max_gpu_frame_ms,
            mem_growth,
            avg_rate("chart_present"),
            avg_rate("chart_cam_step"),
            avg_rate("chart_gpu_prepare"),
            rate_delta("chart_gpu_prepare"),
            avg_rate("bg_draw"),
            rate_delta("bg_draw"),
            avg_rate("combo_draw"),
            rate_delta("combo_draw"),
            avg_rate("base_bake"),
            rate_delta("base_bake"),
            avg_rate("combo_bake"),
            rate_delta("combo_bake"),
            avg_rate("orderbook_bake"),
            rate_delta("orderbook_bake"),
        );
        if fail.is_empty() {
            firetest_info(&format!("[firetest] result=PASS FIRETEST PASS {summary}"));
            std::process::exit(0);
        }
        firetest_error(&format!(
            "[firetest] result=FAIL FIRETEST FAIL {summary} reasons={}",
            fail.join("; ")
        ));
        std::process::exit(2);
    }

    fn fail(&mut self, reason: &str) {
        self.set_phase(Phase::Done);
        self.stop_storm();
        firetest_error(&format!(
            "[firetest] result=FAIL FIRETEST FAIL reason={reason}"
        ));
        std::process::exit(2);
    }
}

fn firetest_info(line: &str) {
    log::info!("{line}");
    write_firetest_line(line);
}

fn firetest_error(line: &str) {
    log::error!("{line}");
    write_firetest_line(line);
}

fn write_firetest_line(line: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("firetest.log")
    {
        let _ = writeln!(f, "{line}");
    }
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn find_order_cancel_log(lines: Vec<CoreLogLine>, uid: u64, since_ms: i64) -> Option<CoreLogLine> {
    let uid_text = uid.to_string();
    lines.into_iter().rev().find(|line| {
        if line.recv_ms < since_ms.saturating_sub(500) {
            return false;
        }
        let msg = line.msg.to_ascii_lowercase();
        line.msg.contains(&uid_text) || msg.contains("cancel") || msg.contains("отмен")
    })
}

fn opt_i64(value: Option<i64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "NA".to_string())
}

fn opt_f64(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.1}"))
        .unwrap_or_else(|| "NA".to_string())
}

impl MouseStorm {
    fn is_done(&self) -> bool {
        self.done.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn stop(self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

fn rate(sample: &Sample, label: &str) -> f64 {
    sample
        .rates
        .iter()
        .find(|r| r.label == label)
        .map(|r| r.hz)
        .unwrap_or(0.0)
}

fn scale_label(scale: Option<f32>) -> &'static str {
    if scale.is_none() {
        "Auto"
    } else if scale == Some(0.50) {
        "50%"
    } else if scale == Some(0.20) {
        "20%"
    } else if scale == Some(0.10) {
        "10%"
    } else if scale == Some(0.05) {
        "5%"
    } else if scale == Some(0.02) {
        "2%"
    } else {
        "custom"
    }
}

fn chart_mouse_min_hz(present_hz: f64) -> f64 {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        (present_hz * 0.5).clamp(20.0, 60.0)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = present_hz;
        100.0
    }
}

fn check_combo_draw_delta(fail: &mut Vec<String>, combo_draw_delta: f64) {
    check_max(fail, "combo_draw_delta", combo_draw_delta, 12.0);
}

fn check_min(fail: &mut Vec<String>, label: &str, got: f64, min: f64) {
    if got < min {
        fail.push(format!("{label} {got:.1} < {min:.1}"));
    }
}

fn check_max(fail: &mut Vec<String>, label: &str, got: f64, max: f64) {
    if got > max {
        fail.push(format!("{label} {got:.1} > {max:.1}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chart_smoke_stage_plan_covers_every_runtime_phase() {
        assert_eq!(
            STAGE_PLAN.len(),
            Phase::StageCount as usize,
            "FireTest Phase changed; update STAGE_PLAN so chart-smoke stays one explicit scenario"
        );
        assert!(
            !STAGE_PLAN.contains(&Phase::StageCount),
            "phase count sentinel must never be part of the runtime stage plan"
        );
    }

    #[test]
    fn chart_smoke_stage_plan_is_one_contiguous_scenario() {
        let names: Vec<&'static str> = STAGE_PLAN.iter().map(|phase| phase.stage_name()).collect();
        assert_eq!(
            names,
            vec![
                "start",
                "open_chart",
                "wait_chart_probe",
                "settle_live_chart",
                "baseline",
                "mouse_storm",
                "static_text_gap",
                "static_text_warmup",
                "static_text_storm",
                "command_error_contract",
                "tool_windows_open",
                "tool_windows_verify_open",
                "tool_windows_dedup",
                "tool_windows_verify_dedup",
                "root_overlay_contract",
                "locale_switch",
                "locale_switch_verify",
                "price_scale_50",
                "price_scale_20",
                "price_scale_auto",
                "price_scale_verify_auto",
                "order_cancel_lag",
                "cooldown",
                "result",
            ],
            "chart-smoke must remain one ordered run; do not add side tests outside this stage plan"
        );
    }

    #[test]
    fn order_cancel_lag_script_is_a_narrow_order_only_run() {
        let config = Config::from_args([
            "moonterminal".to_string(),
            "--debug-script".to_string(),
            "order-cancel-lag".to_string(),
        ])
        .expect("order-cancel-lag args must parse")
        .expect("order-cancel-lag must create FireTest config");

        assert_eq!(config.script, Script::OrderCancelLag);
        assert!(
            config.order_cancel_lag,
            "order-cancel-lag must enable the real order-lag stage itself"
        );
        assert_eq!(
            phase_after_settle(config.script),
            Phase::OrderCancelLag,
            "order-cancel-lag must skip mouse/static/tool-window stages"
        );

        let names: Vec<&'static str> = ORDER_CANCEL_LAG_STAGE_PLAN
            .iter()
            .map(|phase| phase.stage_name())
            .collect();
        assert_eq!(
            names,
            vec![
                "start",
                "open_chart",
                "wait_chart_probe",
                "settle_live_chart",
                "order_cancel_lag",
                "cooldown",
                "result",
            ],
            "order-cancel-lag must remain a focused order-path diagnostic"
        );
        assert!(
            !ORDER_CANCEL_LAG_STAGE_PLAN.contains(&Phase::Storm)
                && !ORDER_CANCEL_LAG_STAGE_PLAN.contains(&Phase::StaticTextStorm)
                && !ORDER_CANCEL_LAG_STAGE_PLAN.contains(&Phase::ToolWindowsOpen),
            "order-cancel-lag must not pull in unrelated cursor/text/tool-window stages"
        );
    }
}

pub(crate) fn tick_backend(backend: &mut Backend, cx: &mut Context<Backend>) {
    let Some(mut runtime) = backend.firetest.take() else {
        return;
    };
    runtime.tick(backend, cx);
    backend.firetest = Some(runtime);
}

pub(crate) fn observe_chart_probe(backend: &mut Backend, probe: ChartProbe) {
    if let Some(runtime) = backend.firetest.as_mut() {
        runtime.observe_probe(probe);
    }
}

pub(crate) fn record_diag_sample(backend: &mut Backend, elapsed_ms: f64, rates: &[diag::DiagRate]) {
    let metrics = backend.snap;
    let gpu_frame_ms = diag::take_gpu_frame_ms();
    if let Some(runtime) = backend.firetest.as_mut() {
        runtime.record_sample(elapsed_ms, rates, metrics, gpu_frame_ms);
    }
}

#[cfg(target_os = "windows")]
fn start_mouse_storm(
    probe: ChartProbe,
    duration: Duration,
    mouse_hz: f64,
) -> Result<MouseStorm, String> {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, HWND_TOP, PostMessageW, SW_RESTORE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
        SetCursorPos, SetForegroundWindow, SetWindowPos, ShowWindow, WM_MOUSEMOVE,
    };

    let hwnd = probe
        .hwnd
        .ok_or_else(|| "Windows mouse storm needs a Win32 HWND probe".to_string())?;
    firetest_info(&format!(
        "[firetest] mouse_storm target hwnd={hwnd:?} client_rect=({:.1},{:.1},{:.1},{:.1}) scale={:.3}",
        probe.left, probe.top, probe.width, probe.height, probe.scale_factor
    ));

    let stop = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread_done = done.clone();
    std::thread::Builder::new()
        .name("moon-firetest-mouse".to_string())
        .spawn(move || {
            let start = Instant::now();
            let hwnd = HWND(hwnd as *mut _);
            unsafe {
                let _ = ShowWindow(hwnd, SW_RESTORE);
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOP),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
                );
                let _ = SetForegroundWindow(hwnd);
            }
            let mut restore = POINT { x: 0, y: 0 };
            let restore_cursor = unsafe { GetCursorPos(&mut restore).is_ok() };
            let left = probe.left;
            let top = probe.top;
            let width = probe.width;
            let height = probe.height;
            let cx = left + width * 0.5;
            let cy = top + height * 0.5;
            let r = (width.min(height) * 0.35).max(12.0);
            let step = (2.0 * std::f32::consts::PI) / 96.0;
            let mut sent = 0_u64;
            while start.elapsed() < duration && !thread_stop.load(Ordering::Relaxed) {
                let angle = sent as f32 * step;
                let x = (cx + angle.cos() * r).round() as i32;
                let y = (cy + angle.sin() * r).round() as i32;
                let mut point = POINT { x, y };
                let moved = unsafe {
                    let posted = PostMessageW(
                        Some(hwnd),
                        WM_MOUSEMOVE,
                        WPARAM(0),
                        LPARAM(((y as isize) << 16) | (x as u16 as isize)),
                    )
                    .is_ok();
                    let cursor_moved = ClientToScreen(hwnd, &mut point).as_bool()
                        && SetCursorPos(point.x, point.y).is_ok();
                    posted || cursor_moved
                };
                if moved {
                    diag::bump(&diag::FIRETEST_MOUSE_SENT);
                } else {
                    diag::bump(&diag::FIRETEST_MOUSE_POST_FAIL);
                }
                sent = sent.wrapping_add(1);
                let target = Duration::from_secs_f64(sent as f64 / mouse_hz.max(1.0));
                let elapsed = start.elapsed();
                if target > elapsed {
                    std::thread::sleep(target - elapsed);
                } else if sent % 128 == 0 {
                    std::thread::yield_now();
                }
            }
            if restore_cursor {
                unsafe {
                    let _ = SetCursorPos(restore.x, restore.y);
                }
            }
            thread_done.store(true, Ordering::Relaxed);
        })
        .map_err(|e| format!("failed to spawn mouse storm thread: {e}"))?;
    Ok(MouseStorm { stop, done })
}

#[cfg(target_os = "macos")]
fn start_mouse_storm(
    probe: ChartProbe,
    duration: Duration,
    mouse_hz: f64,
) -> Result<MouseStorm, String> {
    use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use core_graphics::geometry::CGPoint;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let stop = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread_done = done.clone();
    std::thread::Builder::new()
        .name("moon-firetest-mouse".to_string())
        .spawn(move || {
            let Ok(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) else {
                diag::bump(&diag::FIRETEST_MOUSE_POST_FAIL);
                thread_done.store(true, Ordering::Relaxed);
                return;
            };
            let start = Instant::now();
            let left = probe.screen_left + probe.left;
            let top = probe.screen_top + probe.top;
            let width = probe.width;
            let height = probe.height;
            let cx = left + width * 0.5;
            let cy = top + height * 0.5;
            let r = (width.min(height) * 0.35).max(12.0);
            let step = (2.0 * std::f32::consts::PI) / 96.0;
            let mut sent = 0_u64;
            while start.elapsed() < duration && !thread_stop.load(Ordering::Relaxed) {
                let angle = sent as f32 * step;
                let point =
                    CGPoint::new((cx + angle.cos() * r) as f64, (cy + angle.sin() * r) as f64);
                match CGEvent::new_mouse_event(
                    source.clone(),
                    CGEventType::MouseMoved,
                    point,
                    CGMouseButton::Left,
                ) {
                    Ok(event) => {
                        event.post(CGEventTapLocation::HID);
                        diag::bump(&diag::FIRETEST_MOUSE_SENT);
                    }
                    Err(_) => diag::bump(&diag::FIRETEST_MOUSE_POST_FAIL),
                }
                sent = sent.wrapping_add(1);
                let target = Duration::from_secs_f64(sent as f64 / mouse_hz.max(1.0));
                let elapsed = start.elapsed();
                if target > elapsed {
                    std::thread::sleep(target - elapsed);
                } else if sent % 128 == 0 {
                    std::thread::yield_now();
                }
            }
            thread_done.store(true, Ordering::Relaxed);
        })
        .map_err(|e| format!("failed to spawn mouse storm thread: {e}"))?;
    Ok(MouseStorm { stop, done })
}

#[cfg(target_os = "linux")]
fn start_mouse_storm(
    probe: ChartProbe,
    duration: Duration,
    mouse_hz: f64,
) -> Result<MouseStorm, String> {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use x11::{xlib, xtest};

    firetest_info(&format!(
        "[firetest] mouse_storm target x11 root_rect=({:.1},{:.1},{:.1},{:.1}) scale={:.3}",
        probe.screen_left + probe.left,
        probe.screen_top + probe.top,
        probe.width,
        probe.height,
        probe.scale_factor
    ));

    let stop = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread_done = done.clone();
    std::thread::Builder::new()
        .name("moon-firetest-mouse".to_string())
        .spawn(move || {
            let start = Instant::now();
            let left = probe.screen_left + probe.left;
            let top = probe.screen_top + probe.top;
            let width = probe.width;
            let height = probe.height;
            let cx = left + width * 0.5;
            let cy = top + height * 0.5;
            let r = (width.min(height) * 0.35).max(12.0);
            let step = (2.0 * std::f32::consts::PI) / 96.0;
            let mut sent = 0_u64;

            unsafe {
                let display = xlib::XOpenDisplay(std::ptr::null());
                if display.is_null() {
                    diag::bump(&diag::FIRETEST_MOUSE_POST_FAIL);
                    thread_done.store(true, Ordering::Relaxed);
                    return;
                }

                let mut event_base = 0;
                let mut error_base = 0;
                let mut major = 0;
                let mut minor = 0;
                if xtest::XTestQueryExtension(
                    display,
                    &mut event_base,
                    &mut error_base,
                    &mut major,
                    &mut minor,
                ) == 0
                {
                    diag::bump(&diag::FIRETEST_MOUSE_POST_FAIL);
                    xlib::XCloseDisplay(display);
                    thread_done.store(true, Ordering::Relaxed);
                    return;
                }

                let screen = xlib::XDefaultScreen(display);
                let root = xlib::XDefaultRootWindow(display);
                let mut restore_root = 0;
                let mut restore_child = 0;
                let mut restore_x = 0;
                let mut restore_y = 0;
                let mut win_x = 0;
                let mut win_y = 0;
                let mut mask = 0;
                let restore_cursor = xlib::XQueryPointer(
                    display,
                    root,
                    &mut restore_root,
                    &mut restore_child,
                    &mut restore_x,
                    &mut restore_y,
                    &mut win_x,
                    &mut win_y,
                    &mut mask,
                ) != 0;

                while start.elapsed() < duration && !thread_stop.load(Ordering::Relaxed) {
                    let angle = sent as f32 * step;
                    let x = (cx + angle.cos() * r).round() as i32;
                    let y = (cy + angle.sin() * r).round() as i32;
                    if xtest::XTestFakeMotionEvent(display, screen, x, y, 0) != 0 {
                        diag::bump(&diag::FIRETEST_MOUSE_SENT);
                    } else {
                        diag::bump(&diag::FIRETEST_MOUSE_POST_FAIL);
                    }
                    sent = sent.wrapping_add(1);
                    if sent % 16 == 0 {
                        xlib::XFlush(display);
                    }
                    let target = Duration::from_secs_f64(sent as f64 / mouse_hz.max(1.0));
                    let elapsed = start.elapsed();
                    if target > elapsed {
                        std::thread::sleep(target - elapsed);
                    } else if sent % 128 == 0 {
                        std::thread::yield_now();
                    }
                }

                if restore_cursor {
                    xtest::XTestFakeMotionEvent(display, screen, restore_x, restore_y, 0);
                }
                xlib::XSync(display, 0);
                xlib::XCloseDisplay(display);
            }

            thread_done.store(true, Ordering::Relaxed);
        })
        .map_err(|e| format!("failed to spawn mouse storm thread: {e}"))?;
    Ok(MouseStorm { stop, done })
}
