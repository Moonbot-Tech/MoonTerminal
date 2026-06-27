//! Прототип варианта 1: рисуем тиковую линию + крестик НАТИВНЫМИ примитивами GPUI
//! (PathBuilder::stroke → paint_path, quad-крестик), БЕЗ offscreen-wgpu и readback.
//! Цель — замерить, держит ли GPUI 60fps на реальной плотности тиков.
//!
//! Запуск: `cargo run -p moon-ui-gpui --example chart_proto --release`
//! Плотность тиков: env `CHART_N` (дефолт 3000), число графиков: `CHART_PANES` (дефолт 1).
//! FPS пишется в лог раз в секунду и рисуется в углу. Каждый кадр полилиния
//! ПЕРЕСТРАИВАЕТСЯ заново (худший случай — живой скролл), курсор едет за мышью.

use std::time::Instant;

use gpui::{
    App, Bounds, Context, MouseMoveEvent, Pixels, Point, Render, SharedString, TextAlign, TextRun,
    TitlebarOptions, Window, WindowBounds, WindowOptions, canvas, div, fill, point, prelude::*, px,
    rgb, size,
};

struct Proto {
    /// Трейды: (цена 0..1, is_buy). Рисуем КРЕСТИКАМИ (как реальный тиковый график).
    trades: Vec<(f32, bool)>,
    panes: usize,
    cursor: Option<Point<Pixels>>,
    frame: u64,
    frames_since_log: u32,
    last_log: Instant,
    fps: f32,
}

impl Proto {
    fn new() -> Self {
        let n: usize = std::env::var("CHART_N")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3000);
        let panes: usize = std::env::var("CHART_PANES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);
        // Детерминированный random-walk (LCG), чтобы не тянуть rand.
        let mut seed: u64 = 0x1234_5678_9abc_def0;
        let mut v = 0.5f32;
        let mut trades = Vec::with_capacity(n);
        for _ in 0..n {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let r = ((seed >> 33) as f32 / (1u64 << 31) as f32) - 1.0; // -1..1
            v = (v + r * 0.02).clamp(0.02, 0.98);
            let is_buy = (seed >> 17) & 1 == 0;
            trades.push((v, is_buy));
        }
        eprintln!("chart_proto: N={n} трейдов (крестики), {panes} граф(а/ов)");
        Self {
            trades,
            panes,
            cursor: None,
            frame: 0,
            frames_since_log: 0,
            last_log: Instant::now(),
            fps: 0.0,
        }
    }
}

impl Render for Proto {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Гоним кадры на максимум (vsync) — меряем реальный потолок.
        window.request_animation_frame();

        // FPS.
        self.frame += 1;
        self.frames_since_log += 1;
        let dt = self.last_log.elapsed();
        if dt.as_secs_f32() >= 1.0 {
            self.fps = self.frames_since_log as f32 / dt.as_secs_f32();
            log::info!(
                "chart_proto FPS={:.1} (N={}, panes={})",
                self.fps,
                self.trades.len(),
                self.panes
            );
            eprintln!(
                "chart_proto FPS={:.1} (N={}, panes={})",
                self.fps,
                self.trades.len(),
                self.panes
            );
            self.frames_since_log = 0;
            self.last_log = Instant::now();
        }

        let trades = self.trades.clone();
        let panes = self.panes;
        let frame = self.frame;
        let cursor = self.cursor;
        let fps = self.fps;

        div()
            .size_full()
            .bg(rgb(0x131416))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _w, cx| {
                this.cursor = Some(e.position);
                cx.notify();
            }))
            .child(
                canvas(
                    move |_, _, _| {},
                    move |bounds, _, window, cx| {
                        draw(window, cx, bounds, &trades, panes, frame, cursor, fps);
                    },
                )
                .size_full(),
            )
    }
}

#[allow(clippy::too_many_arguments)]
fn draw(
    window: &mut Window,
    cx: &mut App,
    bounds: Bounds<Pixels>,
    trades: &[(f32, bool)],
    panes: usize,
    frame: u64,
    cursor: Option<Point<Pixels>>,
    fps: f32,
) {
    let left = f32::from(bounds.origin.x);
    let top = f32::from(bounds.origin.y);
    let width = f32::from(bounds.size.width);
    let total_h = f32::from(bounds.size.height);
    let pane_h = total_h / panes as f32;
    let n = trades.len().max(2);
    // Лёгкая анимация: всё «едет» по кадру (живой скролл) — quad'ы пересобираются каждый кадр.
    let phase = (frame as f32) * 0.01;
    // Размер крестика (полудлина плеча, px) и толщина.
    const R: f32 = 3.0;
    const TH: f32 = 1.0;
    let green = rgb(0x2fa85c);
    let red = rgb(0xff4a4a);

    for p in 0..panes {
        let p_top = top + p as f32 * pane_h;
        let plot_h = pane_h - 24.0;
        // КАЖДЫЙ трейд — крестик «+»: горизонт. + вертик. quad. Цвет по стороне.
        for (i, &(val, is_buy)) in trades.iter().enumerate() {
            let x = left + (i as f32 / (n - 1) as f32) * width;
            let wob = (i as f32 * 0.05 + phase + p as f32).sin() * 0.01;
            let y = p_top + (1.0 - (val + wob).clamp(0.0, 1.0)) * plot_h;
            let col = if is_buy { green } else { red };
            if std::env::var("CHART_DOT").is_ok() {
                // 1 quad — маленький квадрат-маркер.
                window.paint_quad(fill(
                    Bounds::new(point(px(x - R), px(y - R)), size(px(2.0 * R), px(2.0 * R))),
                    col,
                ));
            } else {
                // 2 quad'а — крестик «+».
                window.paint_quad(fill(
                    Bounds::new(
                        point(px(x - R), px(y - TH * 0.5)),
                        size(px(2.0 * R), px(TH)),
                    ),
                    col,
                ));
                window.paint_quad(fill(
                    Bounds::new(
                        point(px(x - TH * 0.5), px(y - R)),
                        size(px(TH), px(2.0 * R)),
                    ),
                    col,
                ));
            }
        }
        // Пара горизонтальных «ордер-линий» (quad'ы по 1px).
        for k in 1..=2 {
            let y = p_top + plot_h * (0.3 * k as f32);
            window.paint_quad(fill(
                Bounds::new(point(px(left), px(y)), size(px(width), px(1.0))),
                rgb(0x7fc9ff),
            ));
        }
    }

    // Крестик — у курсора (2 quad'а на всю зону), как в тиковом графике.
    if let Some(c) = cursor {
        let cx_px = f32::from(c.x);
        let cy_px = f32::from(c.y);
        window.paint_quad(fill(
            Bounds::new(point(px(cx_px - 0.5), px(top)), size(px(1.0), px(total_h))),
            rgb(0xe8e4dc),
        ));
        window.paint_quad(fill(
            Bounds::new(point(px(left), px(cy_px - 0.5)), size(px(width), px(1.0))),
            rgb(0xe8e4dc),
        ));
    }

    // FPS-плашка (текст).
    let text = SharedString::from(format!("FPS {fps:.0}  N {n}  panes {panes}"));
    let font = window.text_style().font();
    let run = TextRun {
        len: text.len(),
        font,
        color: rgb(0xffffff).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window
        .text_system()
        .shape_line(text, px(14.0), &[run], None);
    let _ = line.paint(
        point(px(left + 8.0), px(top + 6.0)),
        px(18.0),
        TextAlign::Left,
        None,
        window,
        cx,
    );
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    gpui_platform::application().run(|cx: &mut App| {
        cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("chart_proto".into()),
                    ..Default::default()
                }),
                focus: true,
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1100.0), px(700.0)),
                    cx,
                ))),
                ..Default::default()
            },
            |_window, cx| cx.new(|_| Proto::new()),
        )
        .unwrap();
        cx.activate(true);
    });
}
