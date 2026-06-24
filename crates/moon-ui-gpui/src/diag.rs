//! ПОСТОЯННАЯ диагностика частоты перерисовок. Глобальные атомарные счётчики дёргаются в
//! render/observe/notify; раз в ~1с дренаж-цикл снимает их и пишет строку в `render_diag.log`
//! (Hz по каждому пункту) + в log::info. Вопрос «где рисуем чаще чем надо» = РАНТАЙМ → читаем
//! этот лог, не гадаем по коду.
//!
//! ⚠️ ГРАБЛИ: гейт — env `MOON_RENDER_DIAG`, а НЕ `#[cfg(debug_assertions)]`! В этом проекте
//! `[profile.dev] debug-assertions = false` (Cargo.toml — снимают DX12 validation-слой ради
//! плавности), т.е. в рабочей dev-сборке `cfg(debug_assertions)` = false и счётчики бы исчезли.
//! Поэтому off-by-default через env (см. `enabled()`): инертно в dev и release, включаешь явно
//! `MOON_RENDER_DIAG=1` на время отладки — публичная сборка чистая, файл не пишется.
//!
//! NB: это РУЧНАЯ инструментация по узлам (забывается на новом узле). Целевое — чокпоинт во
//! фреймворке (render каждой вьюхи по имени типа) + own-pass-слои руками. Пока — вот так.

use std::sync::atomic::{AtomicBool, AtomicU64};

macro_rules! diag_counters {
    ($($name:ident => $label:literal),* $(,)?) => {
        $( pub static $name: AtomicU64 = AtomicU64::new(0); )*
        fn snapshot_and_reset() -> Vec<(&'static str, u64)> {
            use std::sync::atomic::Ordering;
            vec![ $( ($label, $name.swap(0, Ordering::Relaxed)) ),* ]
        }
    };
}

diag_counters!(
    ORDERS_RENDER     => "orders_render",
    SHELL_RENDER      => "shell_render",
    CHART_RENDER      => "chart_render",
    DETACHED_RENDER   => "detached_render",
    BACKEND_NOTIFY    => "backend_notify",
    CHART_PREPARE     => "chart_prepare",
    CHART_FRAME       => "chart_frame",
    CHART_FRAME_REQUEST => "chart_frame_request",
    CHART_FRAME_SKIP_NOT_PRESENTABLE => "chart_frame_skip_not_presentable",
    CHART_FRAME_SKIP_IDLE => "chart_frame_skip_idle",
    CHART_GPU_PREPARE => "chart_gpu_prepare",
    // gpu_canvas present (реальная частота показа чарта) — раньше СЛЕПАЯ зона: present-rate
    // не измерялся вообще. CHART_CAM_STEP = сколько present'ов реально сдвинули камеру на
    // ≥1 пиксель ("рабочие" кадры). Соотношение CAM_STEP/PRESENT = экономия пиксельного
    // рубильника (адаптивна к зуму: на мелком масштабе почти все кадры пропускаются).
    CHART_PRESENT     => "chart_present",
    CHART_CAM_STEP    => "chart_cam_step",
    // ПОСЛОЙНЫЕ счётчики gpu_canvas (мандат AGENTS.md «UI Render Diagnostics»): canvas ВНЕ
    // GPUI-рендера → считаем руками в одной точке-чокпоинте (backend::render_d3d). *_DRAW =
    // отрисовка/блит слоя (раз на present); *_BAKE = перепекание текстуры-кэша (combo/стакан),
    // должно быть РЕДКО (по приходу данных/смене вида). BAKE ≈ DRAW = кэш не работает.
    CHART_BG_DRAW     => "bg_draw",
    CHART_GRID_DRAW   => "grid_draw",
    CHART_CURSOR_DRAW => "cursor_draw",
    CHART_BASE_BAKE   => "base_bake",
    CHART_BASE_BLIT   => "base_blit",
    CHART_COMBO_DRAW  => "combo_draw",
    CHART_COMBO_BAKE  => "combo_bake",
    CHART_HISTORY_RESET_ROWS => "history_reset_rows",
    CHART_HISTORY_RESET_MS => "history_reset_ms",
    CHART_COMBO_UPLOAD_LEN => "combo_upload_len",
    CHART_PRICE_LINE_UPLOAD_LEN => "price_line_upload_len",
    CHART_BOOK_DRAW   => "orderbook_draw",
    CHART_BOOK_BAKE   => "orderbook_bake",
    CHART_USER_DRAW   => "userdata_draw",
    ORDERS_OBS_FIRE   => "orders_obs_fire",
    ORDERS_OBS_NOTIFY => "orders_obs_notify",
    SHELL_OBS_FIRE    => "shell_obs_fire",
    SHELL_OBS_NOTIFY  => "shell_obs_notify",
    CHART_OBS_FIRE    => "chart_obs_fire",
    CHART_OBS_NOTIFY  => "chart_obs_notify",
    CHART_OPEN_NOTIFY => "chart_open_notify",
    CHART_TTL_NOTIFY  => "chart_ttl_notify",
    CHART_INPUT_NOTIFY => "chart_input_notify",
    CHART_CANVAS_NOTIFY => "chart_canvas_notify",
    CHART_MOUSE_MOVE => "chart_mouse_move",
    CHART_MOUSE_MOVE_FAST => "chart_mouse_move_fast",
    CHART_MOUSE_MOVE_ENTITY => "chart_mouse_move_entity",
    CHART_MOUSE_FAST_STOP => "chart_mouse_fast_stop",
    CHART_CURSOR_UPDATE => "chart_cursor_update",
    FIRETEST_MOUSE_SENT => "firetest_mouse_sent",
    FIRETEST_MOUSE_POST_FAIL => "firetest_mouse_post_fail",
    FIRETEST_TEXT_DRAW => "firetest_text_draw",
    FIRETEST_TEXT_COLD => "firetest_text_cold",
);

#[derive(Clone, Debug)]
pub struct DiagRate {
    pub label: &'static str,
    pub hz: f64,
}

static FORCE_ON: AtomicBool = AtomicBool::new(false);
static GPU_FRAME_US_SUM: AtomicU64 = AtomicU64::new(0);
static GPU_FRAME_COUNT: AtomicU64 = AtomicU64::new(0);

/// Диагностика включается ТОЛЬКО при заданной env `MOON_RENDER_DIAG` (любое значение). По
/// умолчанию инертна в ЛЮБОЙ сборке (dev и release): ни счётчиков, ни файла render_diag.log —
/// включаешь явно на время отладки. (`cfg(debug_assertions)` тут НЕ годится: dev-профиль ставит
/// `debug-assertions = false` ради DX12 validation, см. шапку.) Читается один раз через OnceLock.
fn enabled() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    FORCE_ON.load(std::sync::atomic::Ordering::Relaxed)
        || *ON.get_or_init(|| std::env::var_os("MOON_RENDER_DIAG").is_some())
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn is_enabled() -> bool {
    enabled()
}

pub fn force_enable() {
    FORCE_ON.store(true, std::sync::atomic::Ordering::Relaxed);
}

#[inline]
pub fn bump(c: &AtomicU64) {
    if enabled() {
        c.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

#[inline]
pub fn bump_by(c: &AtomicU64, n: u64) {
    if enabled() && n > 0 {
        c.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Снять счётчики за прошедший интервал (no-op без env/force_enable).
pub fn take_sample(elapsed_ms: f64) -> Option<Vec<DiagRate>> {
    if !enabled() {
        return None;
    }
    let snap = snapshot_and_reset();
    let hz = |c: u64| c as f64 * 1000.0 / elapsed_ms.max(1.0);
    Some(
        snap.into_iter()
            .map(|(label, count)| DiagRate {
                label,
                hz: hz(count),
            })
            .collect(),
    )
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn record_gpu_frame_ms(ms: f64) {
    if !enabled() || !ms.is_finite() || ms <= 0.0 {
        return;
    }
    let us = (ms * 1000.0).round().clamp(1.0, u64::MAX as f64) as u64;
    GPU_FRAME_US_SUM.fetch_add(us, std::sync::atomic::Ordering::Relaxed);
    GPU_FRAME_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

pub fn take_gpu_frame_ms() -> f64 {
    let sum = GPU_FRAME_US_SUM.swap(0, std::sync::atomic::Ordering::Relaxed);
    let count = GPU_FRAME_COUNT.swap(0, std::sync::atomic::Ordering::Relaxed);
    if count == 0 {
        0.0
    } else {
        sum as f64 / count as f64 / 1000.0
    }
}

pub fn format_sample(elapsed_ms: f64, sample: &[DiagRate]) -> String {
    let mut line = format!("[diag {:.0}ms]", elapsed_ms);
    for rate in sample {
        line.push_str(&format!(" {}={:.0}", rate.label, rate.hz));
    }
    line
}

pub fn write_sample(elapsed_ms: f64, sample: &[DiagRate]) {
    use std::io::Write;
    let line = format_sample(elapsed_ms, sample);
    log::info!("{line}");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("render_diag.log")
    {
        let _ = writeln!(f, "{line}");
    }
}
