//! Debug/perf окна (gated `debug_assertions`/`moon_profile_debug`/feature `debug-tools`):
//! окно статистики (DebugPerfWindow) + хост debug-чарта (DebugChartHost) + спавн/закрытие
//! пачки debug-чартов. Вынесено из main.rs. `Backend` живёт в крейт-руте (доступ к приватным
//! полям сохраняется по правилу видимости предка из потомка).

use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::*;

use moon_ui::{
    MoonBackgroundPolicy, MoonButton, MoonButtonSize, MoonButtonVariant, MoonPalette,
    MoonWindowFrame, Root, h_flex, v_flex,
};

use moon_core::feed::ConnStatus;
use moon_core::session::CoreId;

use crate::Backend;
use crate::design;
use crate::panels::ChartPanel;
use crate::windowing;

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
struct DebugPerfWindow {
    backend: Entity<Backend>,
    group: String,
    diag_tail: String,
    focus: FocusHandle,
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
struct DebugChartHost {
    panel: Entity<ChartPanel>,
    title: String,
    focus: FocusHandle,
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
impl DebugChartHost {
    fn new(panel: Entity<ChartPanel>, title: String, cx: &mut Context<Self>) -> Self {
        Self {
            panel,
            title,
            focus: cx.focus_handle(),
        }
    }
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
impl Focusable for DebugChartHost {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
impl Render for DebugChartHost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let title = self.title.clone();
        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .child(
                h_flex()
                    .h(design::fit_h_px(cx, 34.0, 13.0, 10.5))
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .pl(design::ui_px(cx, design::titlebar_leading_inset()))
                    .pr(design::ui_px(cx, 6.0))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.shell_high))
                    .child(
                        MoonWindowFrame::debug("debug-chart-title-drag", 0.0)
                            .title_cluster(title, cx)
                            .h_full()
                            .flex_1()
                            .min_w_0()
                            .items_center(),
                    )
                    .when(design::show_custom_window_controls(), |this| {
                        this.child(
                            MoonWindowFrame::debug("debug-chart-window-frame-visual", 0.0)
                                .header_height(34.0)
                                .show_controls(true)
                                .visual_controls(cx),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .child(self.panel.clone()),
            )
    }
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
impl DebugPerfWindow {
    fn new(backend: Entity<Backend>, group: String, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            loop {
                executor.timer(Duration::from_secs(1)).await;
                let diag_tail = latest_render_diag_line();
                let alive = cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        this.diag_tail = diag_tail;
                        cx.notify();
                    })
                    .is_ok()
                });
                if !alive {
                    break;
                }
            }
        })
        .detach();
        Self {
            backend,
            group,
            diag_tail: latest_render_diag_line(),
            focus: cx.focus_handle(),
        }
    }

    fn stat_row(
        label: &'static str,
        value: impl Into<String>,
        p: MoonPalette,
        cx: &App,
    ) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap(design::ui_px(cx, 8.0))
            .child(
                div()
                    .w(px(150.0))
                    .text_color(rgb(p.text_muted))
                    .child(label),
            )
            .child(
                div()
                    .flex_1()
                    .font_family(design::mono())
                    .text_color(rgb(p.text_soft))
                    .child(value.into()),
            )
    }

    fn primary_stat_row(
        label: &'static str,
        value: impl Into<String>,
        p: MoonPalette,
        cx: &App,
    ) -> impl IntoElement {
        h_flex()
            .w_full()
            .items_center()
            .gap(design::ui_px(cx, 12.0))
            .py(design::ui_px(cx, 4.0))
            .child(
                div()
                    .w(px(190.0))
                    .font_family(design::ui_font())
                    .text_size(design::t_title(cx))
                    .text_color(rgb(p.text_muted))
                    .child(label),
            )
            .child(
                div()
                    .flex_1()
                    .font_family(design::ui_font())
                    .text_size(design::t_title(cx))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(p.amber))
                    .child(value.into()),
            )
    }
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
impl Focusable for DebugPerfWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
impl Render for DebugPerfWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let (
            ready,
            total,
            snap,
            desired,
            group_windows,
            detached_chart_windows,
            debug_windows,
            main_chart_shift_hz,
        ) = {
            let b = self.backend.read(cx);
            let store = b.session.store();
            let mut ready = 0;
            let mut total = 0;
            for s in b.session.sessions() {
                total += 1;
                if store
                    .core(s.id)
                    .is_some_and(|core| core.status == ConnStatus::Ready)
                {
                    ready += 1;
                }
            }
            (
                ready,
                total,
                b.snap,
                b.desired.len(),
                b.group_windows.len(),
                b.detached_chart_windows.len(),
                b.debug_chart_windows.len(),
                b.debug_main_chart_shift_hz(&self.group),
            )
        };
        let main_chart_shift_text = main_chart_shift_hz
            .map(|hz| format!("{hz:.1} shifts/s"))
            .unwrap_or_else(|| "no main chart".to_string());
        let diag_tail = self.diag_tail.clone();
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|e| format!("<cwd error: {e}>"));
        let open_backend = self.backend.clone();
        let close_backend = self.backend.clone();
        let fill_backend = self.backend.clone();
        let fill_group = self.group.clone();

        v_flex()
            .id("debug-perf-window")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .gap(design::ui_px(cx, 8.0))
            .p_4()
            .bg(rgb(p.shell))
            .text_size(design::t_body(cx))
            .text_color(rgb(p.text))
            .child(
                h_flex()
                    .h(design::fit_h_px(cx, 34.0, 13.0, 10.5))
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(
                        MoonWindowFrame::debug("debug-perf-title-drag", 0.0)
                            .title_cluster("debug stats", cx)
                            .flex_1()
                            .min_w_0()
                            .items_center(),
                    )
                    .when(design::show_custom_window_controls(), |this| {
                        this.child(
                            MoonWindowFrame::debug("debug-perf-window-frame-visual", 0.0)
                                .header_height(34.0)
                                .show_controls(true)
                                .visual_controls(cx),
                        )
                    }),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 6.0))
                    .child(
                        MoonButton::new("debug-fill-main-cap")
                            .width(150.0)
                            .variant(MoonButtonVariant::Neutral)
                            .size(MoonButtonSize::Toolbar)
                            .label("Fill Main CAP")
                            .on_click(move |_, _, cx| {
                                fill_backend.update(cx, |b, bcx| {
                                    b.debug_fill_main_chart_group = Some(fill_group.clone());
                                    b.debug_fill_main_chart_rev =
                                        b.debug_fill_main_chart_rev.wrapping_add(1);
                                    log::info!(
                                        "debug fill main chart: requested group={} rev={}",
                                        fill_group,
                                        b.debug_fill_main_chart_rev
                                    );
                                    bcx.notify();
                                });
                            })
                            .render(),
                    )
                    .child(
                        MoonButton::new("debug-open-10-btc")
                            .width(210.0)
                            .variant(MoonButtonVariant::Neutral)
                            .size(MoonButtonSize::Toolbar)
                            .label("Открыть 10 BTC графиков")
                            .on_click(move |_, _, cx| {
                                spawn_debug_chart_windows(cx, open_backend.clone());
                            })
                            .render(),
                    )
                    .child(
                        MoonButton::new("debug-close-10-btc")
                            .width(110.0)
                            .variant(MoonButtonVariant::Neutral)
                            .size(MoonButtonSize::Toolbar)
                            .label("Закрыть 10")
                            .on_click(move |_, _, cx| {
                                close_debug_btc_chart_windows(cx, close_backend.clone());
                            })
                            .render(),
                    ),
            )
            .child(Self::primary_stat_row(
                "Main chart shifts/s",
                main_chart_shift_text,
                p,
                cx,
            ))
            .child(Self::stat_row(
                "connections",
                format!("{ready}/{total} ready"),
                p,
                cx,
            ))
            .child(Self::stat_row(
                "cpu",
                format!(
                    "process {:.1}% / system {:.1}%",
                    snap.cpu_process, snap.cpu_system
                ),
                p,
                cx,
            ))
            .child(Self::stat_row(
                "gpu",
                format!("process {:.1}%", snap.gpu_process),
                p,
                cx,
            ))
            .child(Self::stat_row(
                "ram",
                format!("{:.0} MB ({:+.1} MB/5s)", snap.mem_mb, snap.mem_delta_mb),
                p,
                cx,
            ))
            .child(Self::stat_row(
                "desired markets",
                desired.to_string(),
                p,
                cx,
            ))
            .child(Self::stat_row(
                "group windows",
                group_windows.to_string(),
                p,
                cx,
            ))
            .child(Self::stat_row(
                "chart windows",
                format!("{detached_chart_windows} detached / {debug_windows} debug"),
                p,
                cx,
            ))
            .child(Self::stat_row("cwd", cwd, p, cx))
            .child(Self::stat_row(
                "render diag",
                if std::env::var_os("MOON_RENDER_DIAG").is_some() {
                    "MOON_RENDER_DIAG=on"
                } else {
                    "MOON_RENDER_DIAG=off"
                },
                p,
                cx,
            ))
            .child(
                v_flex()
                    .w_full()
                    .gap(design::ui_px(cx, 4.0))
                    .mt(px(4.0))
                    .child(
                        div()
                            .text_color(rgb(p.text_muted))
                            .child("last render_diag.log line"),
                    )
                    .child(
                        div()
                            .w_full()
                            .p_2()
                            .rounded(design::ui_px(cx, 4.0))
                            .bg(rgba(0x00000055))
                            .font_family(design::mono())
                            .text_size(design::t_body(cx))
                            .text_color(rgb(p.text_soft))
                            .child(diag_tail),
                    ),
            )
    }
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
fn latest_render_diag_line() -> String {
    let path = std::path::Path::new("render_diag.log");
    let Ok(text) = std::fs::read_to_string(path) else {
        return "render_diag.log not found in current working directory".to_string();
    };
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("<empty render_diag.log>")
        .to_string()
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
pub(crate) fn open_debug_perf_window(
    cx: &mut App,
    backend: Entity<Backend>,
    group: String,
    owner: Option<AnyWindowHandle>,
) {
    if let Some(handle) = backend.read(cx).debug_window {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }

    let opts = windowing::debug_window_options(
        "MoonTerminal Debug",
        WindowBounds::Windowed(Bounds {
            origin: point(px(140.0), px(140.0)),
            size: size(px(720.0), px(420.0)),
        }),
        Some(size(px(560.0), px(320.0))),
        owner,
        true,
    );
    let b = backend.clone();
    let g = group.clone();
    if let Ok(handle) = cx.open_window(opts, move |window, cx| {
        #[cfg(target_os = "windows")]
        windowing::configure_dwm_window(window);
        windowing::configure_shell_clear_color(window, cx);
        let view = cx.new(|cx| DebugPerfWindow::new(b, g, cx));
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    }) {
        backend.update(cx, |bk, _| {
            bk.debug_window = Some(handle);
        });
    }
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
fn debug_order_market_target(b: &Backend) -> Option<(CoreId, String)> {
    let store = b.session.store();
    for s in b.session.sessions() {
        if let Some(d) = store.core(s.id)
            && let Some(o) = d.orders.iter().find(|o| !o.market.trim().is_empty())
        {
            return Some((s.id, o.market.clone()));
        }
    }
    None
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
fn debug_config_market_target(b: &Backend) -> Option<(CoreId, String)> {
    let env_market = std::env::var("MOON_RENDER_DIAG_MARKET")
        .ok()
        .or_else(|| std::env::var("MOON_CONFIG_PLAINTEXT_MARKET").ok())
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty());
    let store = b.session.store();
    for server in &b.config.servers {
        let Some(session) = b.session.sessions().iter().find(|s| s.id == server.id) else {
            continue;
        };
        let ready = store
            .core(session.id)
            .is_some_and(|core| core.status == ConnStatus::Ready);
        if !ready {
            continue;
        }
        let market = env_market
            .clone()
            .or_else(|| {
                let market = server.market.trim();
                (!market.is_empty()).then(|| market.to_string())
            })
            .unwrap_or_else(|| "BTCUSDT".to_string());
        return Some((session.id, market));
    }
    None
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
pub(crate) fn debug_chart_target(b: &Backend) -> Option<(CoreId, String)> {
    debug_config_market_target(b)
        .or_else(|| debug_order_market_target(b))
        .or_else(|| {
            b.session
                .sessions()
                .first()
                .map(|s| (s.id, "BTCUSDT".to_string()))
        })
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
pub(crate) fn spawn_debug_chart_windows(cx: &mut App, backend: Entity<Backend>) {
    let Some((core, group, market, epoch, theme, owner)) = ({
        let b = backend.read(cx);
        debug_chart_target(&b).map(|(core, market)| {
            let group = b
                .session
                .sessions()
                .iter()
                .find(|s| s.id == core)
                .map(|s| s.group.clone())
                .unwrap_or_else(|| "default".to_string());
            let owner = b.group_windows.get(&group).copied().map(Into::into);
            (core, group, market, b.epoch, b.config.theme.clone(), owner)
        })
    }) else {
        log::warn!("debug charts: no live sessions/markets; cannot open charts");
        return;
    };
    log::info!("debug charts: opening 10 windows for core={core} market={market}");

    let mut opened = Vec::new();
    for i in 0..10 {
        let backend_for_panel = backend.clone();
        let market = market.clone();
        let theme = theme.clone();
        let title = format!("MoonTerminal Debug {market} {}", i + 1);
        let mut opts = windowing::debug_window_options(
            title.clone(),
            WindowBounds::Windowed(Bounds {
                origin: point(px(90.0 + i as f32 * 24.0), px(90.0 + i as f32 * 24.0)),
                size: size(px(920.0), px(560.0)),
            }),
            Some(size(px(520.0), px(340.0))),
            owner,
            true,
        );
        opts.focus = false;
        opts.is_minimizable = false;
        let opened_window = cx.open_window(opts, move |window, cx| {
            #[cfg(target_os = "windows")]
            windowing::configure_dwm_window(window);
            windowing::configure_chart_clear_color(window, cx);
            let panel = cx.new(|cx| {
                ChartPanel::new(
                    backend_for_panel,
                    Some((core, market)),
                    epoch,
                    theme,
                    window,
                    cx,
                )
            });
            let host = cx.new(|cx| DebugChartHost::new(panel, title, cx));
            cx.new(|cx| Root::new(host, window, cx).background_policy(MoonBackgroundPolicy::NoFill))
        });
        match opened_window {
            Ok(handle) => opened.push(handle),
            Err(error) => log::warn!("debug charts: failed to open chart {}: {error}", i + 1),
        }
    }

    if !opened.is_empty() {
        backend.update(cx, |b, bcx| {
            b.debug_chart_windows.extend(opened.iter().copied());
            b.detached_chart_windows
                .extend(opened.into_iter().map(|handle| (group.clone(), handle)));
            bcx.notify();
        });
    }
}

#[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
fn close_debug_btc_chart_windows(cx: &mut App, backend: Entity<Backend>) {
    let handles = backend.update(cx, |b, bcx| {
        let handles = std::mem::take(&mut b.debug_chart_windows);
        let ids = handles
            .iter()
            .map(|handle| handle.window_id())
            .collect::<Vec<_>>();
        b.detached_chart_windows
            .retain(|(_, handle)| !ids.contains(&handle.window_id()));
        bcx.notify();
        handles
    });
    for handle in handles {
        handle
            .update(cx, |_, window, _| window.remove_window())
            .ok();
    }
}
