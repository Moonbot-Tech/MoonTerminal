//! Геометрия пользовательских фигур чарта (слой рисования): фигуры из
//! `moon_core::figures` → инстансы линий own-pass. ДОБАВЛЯЕТ в буферы (не чистит):
//! зовётся после `build_order_geometry`, фигуры едут теми же userdata-слоями.
//!
//! Визуальный язык (чтобы фигуры не путались с ордер-линиями и было видно состояние):
//! - обычная фигура — ТОНКИЙ ПУНКТИР;
//! - заармленная (alert) — ЖИРНЫЙ ПУНКТИР (видно, что «боевая»);
//! - под курсором / выделенная — СПЛОШНАЯ ТОЛСТАЯ + яркая (резко «оживает»),
//!   у выделенной ещё узелки-квадраты на точках, а у горизонтали — ручка-маркер.

use moon_core::figures::{FigNode, Figure, FigureKind, LineKind};

use crate::layers::{LineInstance, MarkerInstance, SegInstance};

/// Полупрозрачность спокойной (не активной) фигуры.
const FIG_IDLE_ALPHA: f32 = 0.85;
/// Множитель толщины активной (hover/selected) фигуры и заармленной.
const FIG_ACTIVE_THICKNESS: f32 = 1.9;
const FIG_ARMED_THICKNESS: f32 = 2.2;
/// Размер узелка/ручки выделенной фигуры, px.
const FIG_KNOT_SIZE: f32 = 4.5;
const SEG_PATTERN_SOLID: f32 = 0.0;
const SEG_PATTERN_DASH: f32 = 1.0; // DashDotDot в шейдере — ближайший «штрих»
const SEG_PATTERN_DOT: f32 = 2.0;

/// Паттерн отрезка (шейдер: 0 solid / 1 dashdotdot / 2 dot) по виду линии Moonbot.
/// Штриховых вариантов в шейдере меньше пяти → Dash/DashDot/DashDotDot дают один штрих.
fn seg_pattern(kind: LineKind) -> f32 {
    match kind {
        LineKind::Solid => SEG_PATTERN_SOLID,
        LineKind::Dot => SEG_PATTERN_DOT,
        _ => SEG_PATTERN_DASH,
    }
}

/// Как рисуется конкретная фигура в этом кадре.
struct FigStyle {
    color: [f32; 4],
    thickness: f32,
    /// Вид линии (или Solid при hover/выделении).
    line_kind: LineKind,
    /// Рисовать узелки/ручку (выделенная).
    knots: bool,
}

fn rgba(c: [u8; 4], alpha_mul: f32) -> [f32; 4] {
    [
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
        (c[3] as f32 / 255.0) * alpha_mul,
    ]
}

/// Собирает геометрию фигур чарта. `draft` — превью рисуемой фигуры; `hovered`/
/// `selected` подсвечиваются (у выделенной отрезка/канала — узлы на концах для драга).
pub fn build_figure_geometry(
    figures: &[Figure],
    draft: Option<&Figure>,
    hovered: Option<u64>,
    selected: Option<u64>,
    epoch_ms: f64,
    hlines: &mut Vec<LineInstance>,
    segs: &mut Vec<SegInstance>,
    markers: &mut Vec<MarkerInstance>,
) {
    for fig in figures {
        let is_sel = selected == Some(fig.id);
        let is_hot = is_sel || hovered == Some(fig.id);
        let style = fig_style(fig, is_hot, is_sel);
        push_figure(fig, &style, epoch_ms, hlines, segs);
        if style.knots {
            push_knots(fig, style.color, epoch_ms, markers);
        }
    }
    if let Some(d) = draft {
        // Превью рисуемой фигуры: ярче и толще, базовый вид линии — из стиля.
        let style = FigStyle {
            color: rgba(d.color, 1.0),
            thickness: d.thickness * FIG_ACTIVE_THICKNESS,
            line_kind: d.line_kind,
            knots: false,
        };
        push_figure(d, &style, epoch_ms, hlines, segs);
    }
}

/// Стиль фигуры по состоянию:
/// - спокойная — тонкий базовый вид (Solid/Dash из `fig.dashed`);
/// - под курсором/выделенная — толстая СПЛОШНАЯ (резко «оживает»), + узлы у выделенной;
/// - заармленная (alert) — толстая, базовый вид (толщина = «боевой» индикатор).
fn fig_style(fig: &Figure, is_hot: bool, is_selected: bool) -> FigStyle {
    if is_hot {
        FigStyle {
            color: rgba(fig.color, 1.0),
            thickness: fig.thickness * FIG_ACTIVE_THICKNESS,
            line_kind: LineKind::Solid,
            knots: is_selected,
        }
    } else if fig.alert {
        FigStyle {
            color: rgba(fig.color, 1.0),
            thickness: fig.thickness * FIG_ARMED_THICKNESS,
            line_kind: fig.line_kind,
            knots: false,
        }
    } else {
        FigStyle {
            color: rgba(fig.color, FIG_IDLE_ALPHA),
            thickness: fig.thickness,
            line_kind: fig.line_kind,
            knots: false,
        }
    }
}

fn push_figure(
    fig: &Figure,
    style: &FigStyle,
    epoch_ms: f64,
    hlines: &mut Vec<LineInstance>,
    segs: &mut Vec<SegInstance>,
) {
    let to_rel = |t_ms: f64| (t_ms - epoch_ms) as f32;
    let pattern = seg_pattern(style.line_kind);
    let mut push_seg = |a: &FigNode, b: &FigNode, dp: f64| {
        segs.push(SegInstance {
            t0_rel: to_rel(a.time_ms),
            p0: (a.price + dp) as f32,
            t1_rel: to_rel(b.time_ms),
            p1: (b.price + dp) as f32,
            thickness: style.thickness,
            pattern,
            extend: 0.0,
            color: style.color,
        });
    };
    let mut push_hline = |price: f64| {
        hlines.push(LineInstance {
            price: price as f32,
            color: style.color,
            style: if style.line_kind.is_solid() { 0.0 } else { 1.0 },
            thickness: style.thickness,
        });
    };
    match &fig.kind {
        FigureKind::HLine { price } => push_hline(*price),
        FigureKind::Segment { a, b } => push_seg(a, b, 0.0),
        FigureKind::Triangle { a, b, c } => {
            // Три ребра: a-b, b-c, c-a.
            push_seg(a, b, 0.0);
            push_seg(b, c, 0.0);
            push_seg(c, a, 0.0);
        }
        FigureKind::Channel { price1, price2 } => {
            push_hline(*price1);
            push_hline(*price2);
        }
    }
}

/// Узлы редактирования выделенной фигуры (концы отрезка/канала — за них тянут).
/// У горизонтали узлов нет: её тянут за любое место (наведи → толстая → драг).
fn push_knots(fig: &Figure, color: [f32; 4], epoch_ms: f64, markers: &mut Vec<MarkerInstance>) {
    let to_rel = |t_ms: f64| (t_ms - epoch_ms) as f32;
    let mut knot = |t_rel: f32, price: f64| {
        markers.push(MarkerInstance {
            t_rel,
            price: price as f32,
            size: FIG_KNOT_SIZE,
            thickness: 1.5,
            shape: 1.0,
            color,
        });
    };
    match &fig.kind {
        // У горизонтали и канала узлов нет — их тянут за линию (наведи → толстая → драг).
        FigureKind::HLine { .. } | FigureKind::Channel { .. } => {}
        FigureKind::Segment { a, b } => {
            knot(to_rel(a.time_ms), a.price);
            knot(to_rel(b.time_ms), b.price);
        }
        FigureKind::Triangle { a, b, c } => {
            knot(to_rel(a.time_ms), a.price);
            knot(to_rel(b.time_ms), b.price);
            knot(to_rel(c.time_ms), c.price);
        }
    }
}
