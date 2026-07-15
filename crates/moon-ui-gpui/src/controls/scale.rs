//! Дропдауны масштаба цены (Y): для полоски чарт-вкладок и для AddToChart-stack.
//! Вынесено из `controls.rs` точь-в-точь.

use gpui::*;
use rust_i18n::t;

use moon_ui::{
    MoonButtonSegment, MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonMenuItem, MoonMenuSize,
    MoonPalette, MoonTooltipView,
};

/// Пресеты масштаба цены (Y) — 1:1 с egui `dock/controls.rs::SCALES`. `None` = «Авто».
/// Первый элемент пары — СТАБИЛЬНЫЙ ключ пункта меню (не показывается): проценты
/// интернациональны и идут в подпись как есть, «Авто» подставляется из локали.
const SCALES: [(&str, Option<f32>); 6] = [
    ("auto", None),
    ("50%", Some(0.50)),
    ("20%", Some(0.20)),
    ("10%", Some(0.10)),
    ("5%", Some(0.05)),
    ("2%", Some(0.02)),
];

/// Подпись ступени. `None` (и кастомный масштаб после перетаскивания, который не
/// совпал ни с одной ступенью) → «Авто» из локали; проценты — как в `SCALES`.
fn scale_label(scale: Option<f32>) -> String {
    SCALES
        .iter()
        .find(|(_, value)| *value == scale && value.is_some())
        .map(|(label, _)| (*label).to_string())
        .unwrap_or_else(|| t!("toolbar.scale_auto").to_string())
}

/// Следующая ступень масштаба для хоткеев Scale +/− (единый источник порядка — `SCALES`:
/// Авто → 50% → 20% → 10% → 5% → 2%, индекс растёт = зум ВНУТРЬ). `zoom_in=true` (Scale +)
/// идёт к меньшему проценту, `false` (Scale −) — наружу к «Авто». По краям клампится (без
/// wrap). Текущее значение матчится точно; кастомное (после перетаскивания) сводится к
/// ближайшей числовой ступени.
pub(crate) fn step_scale(current: Option<f32>, zoom_in: bool) -> Option<f32> {
    let idx = SCALES
        .iter()
        .position(|(_, v)| *v == current)
        .unwrap_or_else(|| match current {
            None => 0,
            Some(cur) => SCALES
                .iter()
                .enumerate()
                .filter_map(|(i, (_, v))| v.map(|v| (i, (v - cur).abs())))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
                .unwrap_or(0),
        });
    let next = if zoom_in {
        (idx + 1).min(SCALES.len() - 1)
    } else {
        idx.saturating_sub(1)
    };
    SCALES[next].1
}

/// Дропдаун масштаба для полоски чарт-вкладок главного окна: применяет масштаб ТОЛЬКО к
/// АКТИВНОЙ вкладке (Main или конкретный AddToChart), не трогая другие вкладки/окна, и
/// сохраняет (per-вкладочный масштаб). Стоит рядом с кнопкой ⚙ настроек раскладки.
/// Общая сборка дропдауна масштаба: единственные отличия вкладок и AddToChart-stack —
/// набор id, размер триггера (`Micro`/`ToolbarCompact`) и куда писать выбранный масштаб
/// (`on_pick`). Визуал/тултип/лупа/«А» для Авто — общие.
fn scale_dropdown(
    scale: Option<f32>,
    tip_id: &'static str,
    dropdown_id: &'static str,
    item_key_prefix: &'static str,
    trigger_size: MoonButtonSize,
    p: MoonPalette,
    on_pick: impl Fn(Option<f32>, &mut App) + Clone + 'static,
) -> impl IntoElement {
    let selected_label = scale_label(scale);
    let mut items = Vec::with_capacity(SCALES.len());
    for (key, pct) in SCALES {
        let on_pick = on_pick.clone();
        items.push(
            MoonMenuItem::with_key(format!("{item_key_prefix}-{key}"), scale_label(pct))
                .selected(scale == pct)
                .checked(scale == pct)
                .on_click(move |_, _, cx| on_pick(pct, cx)),
        );
    }

    // Лупа вместо слова «МАСШТАБ» + «А» для Авто (компактнее); подсказка «Масштаб» — тултипом.
    let trigger_val = if scale.is_none() {
        t!("toolbar.scale_auto_short").to_string()
    } else {
        selected_label
    };
    div()
        .id(tip_id)
        .tooltip(|_window, cx| {
            cx.new(|_| MoonTooltipView::new(t!("toolbar.scale").to_string()))
                .into()
        })
        .child(
            MoonDropdown::new(dropdown_id)
                .trigger_width(72.0)
                .trigger_variant(MoonButtonVariant::Neutral)
                .trigger_size(trigger_size)
                .menu_width(116.0)
                .menu_size(MoonMenuSize::Compact)
                .segment(
                    MoonButtonSegment::new("🔍")
                        .color(p.text_muted)
                        .weight(400.0),
                )
                .segment(
                    MoonButtonSegment::new(trigger_val)
                        .color(p.text)
                        .weight(500.0),
                )
                .items(items),
        )
}

pub(crate) fn scale_dropdown_for_tabs(
    scale: Option<f32>,
    tabs: Entity<crate::chart_tabs::ChartTabs>,
    p: MoonPalette,
) -> impl IntoElement {
    scale_dropdown(
        scale,
        "tabs-scale-tip",
        "tabs-scale-dropdown",
        "scale-tab",
        MoonButtonSize::Micro,
        p,
        move |pct, cx| {
            tabs.update(cx, |t, tcx| t.pick_active_scale(pct, tcx));
        },
    )
}

/// Дропдаун масштаба для AddToChart-stack: пишет масштаб во все отдельные ChartPanel внутри
/// stack-а. Это сохраняет Delphi-модель "один график = одна сущность", но управление масштабом
/// остаётся единым для окна/вкладки.
pub(crate) fn scale_dropdown_for_add_stack(
    scale: Option<f32>,
    stack: Entity<crate::chart_tabs::AddChartStack>,
    p: MoonPalette,
) -> impl IntoElement {
    scale_dropdown(
        scale,
        "detached-stack-scale-tip",
        "detached-stack-scale-dropdown",
        "scale-stack",
        MoonButtonSize::ToolbarCompact,
        p,
        move |pct, cx| {
            stack.update(cx, |st, scx| st.set_scale(pct, scx));
        },
    )
}
