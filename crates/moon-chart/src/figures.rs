//! Геометрия пользовательских фигур чарта (слой рисования): фигуры из
//! `moon_core::figures` → инстансы линий own-pass. ДОБАВЛЯЕТ в буферы (не чистит):
//! зовётся после `build_order_geometry`, фигуры едут теми же userdata-слоями.

use moon_core::figures::{FigNode, Figure, FigureKind};

use crate::layers::{LineInstance, MarkerInstance, SegInstance};

/// Полупрозрачность обычной (не выделенной) фигуры.
const FIG_ALPHA: f32 = 0.9;
/// Подсветка hover/selected: множитель толщины.
const FIG_HOVER_THICKNESS: f32 = 1.6;
/// Размер узелка выделенной фигуры, px.
const FIG_KNOT_SIZE: f32 = 4.0;

fn rgba(c: [u8; 4], alpha_mul: f32) -> [f32; 4] {
    [
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
        (c[3] as f32 / 255.0) * alpha_mul,
    ]
}

/// Собирает геометрию фигур чарта. `draft` — фигура в процессе рисования
/// (превью за курсором, рисуем пунктиром); `hovered`/`selected` — подсветка и
/// узелки редактирования.
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
        let hot = hovered == Some(fig.id) || selected == Some(fig.id);
        push_figure(fig, hot, false, epoch_ms, hlines, segs);
        if selected == Some(fig.id) {
            push_knots(fig, epoch_ms, markers);
        }
    }
    if let Some(d) = draft {
        // Превью рисуемой фигуры: всегда пунктир, без узелков.
        push_figure(d, true, true, epoch_ms, hlines, segs);
    }
}

fn push_figure(
    fig: &Figure,
    hot: bool,
    force_dashed: bool,
    epoch_ms: f64,
    hlines: &mut Vec<LineInstance>,
    segs: &mut Vec<SegInstance>,
) {
    let alpha = if hot { 1.0 } else { FIG_ALPHA };
    let color = rgba(fig.color, alpha);
    let thickness = if hot {
        fig.thickness * FIG_HOVER_THICKNESS
    } else {
        fig.thickness
    };
    let dashed = fig.dashed || force_dashed;
    let to_rel = |t_ms: f64| (t_ms - epoch_ms) as f32;
    let mut push_seg = |a: &FigNode, b: &FigNode, dp: f64| {
        segs.push(SegInstance {
            t0_rel: to_rel(a.time_ms),
            p0: (a.price + dp) as f32,
            t1_rel: to_rel(b.time_ms),
            p1: (b.price + dp) as f32,
            thickness,
            pattern: if dashed { 2.0 } else { 0.0 },
            extend: 0.0,
            color,
        });
    };
    match &fig.kind {
        FigureKind::HLine { price } => hlines.push(LineInstance {
            price: *price as f32,
            color,
            style: if dashed { 1.0 } else { 0.0 },
            thickness,
        }),
        FigureKind::Segment { a, b } => push_seg(a, b, 0.0),
        FigureKind::Channel { a, b, dprice } => {
            push_seg(a, b, 0.0);
            push_seg(a, b, *dprice);
        }
    }
}

/// Узелки редактирования по узлам выделенной фигуры.
fn push_knots(fig: &Figure, epoch_ms: f64, markers: &mut Vec<MarkerInstance>) {
    let color = rgba(fig.color, 1.0);
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
        // Узел горизонтали ставим на «сейчас»-край не зная его: у горизонтали
        // узелков нет — она редактируется драгом всей линии.
        FigureKind::HLine { .. } => {}
        FigureKind::Segment { a, b } => {
            knot(to_rel(a.time_ms), a.price);
            knot(to_rel(b.time_ms), b.price);
        }
        FigureKind::Channel { a, b, dprice } => {
            knot(to_rel(a.time_ms), a.price);
            knot(to_rel(b.time_ms), b.price);
            // Узел ширины канала — на середине второй линии.
            knot(
                to_rel((a.time_ms + b.time_ms) * 0.5),
                (a.price + b.price) * 0.5 + dprice,
            );
        }
    }
}
