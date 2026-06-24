//! Окна групп: список активных групп, фокус-монета по умолчанию и открытие/фокус
//! окна группы (порт egui `App::show_group`). Вынесено из main.rs.

use gpui::*;

use moon_ui::{MoonBackgroundPolicy, Root};

use moon_core::config::{AppConfig, WindowLayout};
use moon_core::session::CoreId;

use crate::Backend;
use crate::shell::Shell;
use crate::windowing;

/// Активные группы конфига (уникальные, в порядке появления). Нет — одна "default".
pub(crate) fn groups(cfg: &AppConfig) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in &cfg.servers {
        if s.active && cfg.group(&s.group).active && !out.contains(&s.group) {
            out.push(s.group.clone());
        }
    }
    if out.is_empty() {
        out.push("default".into());
    }
    out
}

/// Открыть (или сфокусировать, если уже открыто) окно группы. Используется на старте
/// по окну на группу и по кнопке 👁 «показать группу» в настройках (порт egui
/// `App::show_group`). Геометрия — из сохранённой раскладки, иначе каскад по `offset`.
pub(crate) fn spawn_group_window(
    cx: &mut App,
    backend: &Entity<Backend>,
    cfg: &AppConfig,
    group: String,
    epoch: f64,
    layout: &WindowLayout,
    offset: f32,
) {
    // Уже открыто → сфокусировать (handle.update вернёт Err, если окно закрыли).
    if let Some(handle) = backend.read(cx).group_windows.get(&group).copied() {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    // НЕ открываем монету на Main автоматически при старте — Main стартует пустым (юзер сам
    // открывает монету). (Раньше брали `server.market`, дефолт BTCUSDT.)
    let focus: Option<(CoreId, String)> = None;
    let saved = layout.groups.get(&group);
    let win_bounds = match saved {
        Some(g) => Bounds {
            origin: point(px(g.x as f32), px(g.y as f32)),
            size: size(px(g.w as f32), px(g.h as f32)),
        },
        None => Bounds {
            origin: point(px(80.0 + offset), px(80.0 + offset)),
            size: size(px(1280.0), px(720.0)),
        },
    };
    // Монитор по сохранённому origin — чтобы окно открылось на ТОМ дисплее, с которого
    // снимали bounds. Без display_id GPUI восстанавливает по scale primary-монитора, и на
    // мониторе с другим DPI окно открывается смещённым/сжатым. MoonUI GPUI берёт scale
    // целевого display ТОЛЬКО когда display_id задан. Round-trip как у detached-окон.
    let origin = win_bounds.origin;
    let display_id = cx
        .displays()
        .into_iter()
        .find(|d| d.bounds().contains(&origin))
        .map(|d| d.id());
    let window_bounds = if saved.map(|g| g.maximized).unwrap_or(false) {
        WindowBounds::Maximized(win_bounds)
    } else {
        WindowBounds::Windowed(win_bounds)
    };
    // Значок окна группы из конфига (`GroupConfig.icon` → assets/icons/<id>.png, вшит в exe).
    let icon_id = cfg.group(&group).icon;
    let mut opts = windowing::trading_window_options(
        "MoonTerminal",
        &group,
        icon_id,
        window_bounds,
        display_id,
        Some(size(px(520.0), px(340.0))),
    );
    opts.window_background = WindowBackgroundAppearance::Opaque;
    // Цвет clear из темы (фон чарта): иначе не закрытые сценой пиксели = белые (дефолт рендерера),
    // что мелькает при старте/ресайзе и под чартом (own-pass UnderScene нельзя перекрывать фоном).
    let cbg = cfg.theme.bg;
    opts.window_clear_color = Some(gpui::rgb(
        ((cbg[0] as u32) << 16) | ((cbg[1] as u32) << 8) | cbg[2] as u32,
    ));
    let theme = cfg.theme.clone();
    let b = backend.clone();
    let g = group.clone();
    if let Ok(handle) = cx.open_window(opts, move |window, cx| {
        windowing::configure_dwm_window(window);
        windowing::configure_shell_clear_color(window, cx);
        windowing::set_group_window_icon(window, icon_id);
        let view = cx.new(|cx| Shell::new(b, g, focus, epoch, theme, window, cx));
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::NoFill))
    }) {
        backend.update(cx, |bk, _| {
            bk.group_windows.insert(group, handle);
        });
    }
}
