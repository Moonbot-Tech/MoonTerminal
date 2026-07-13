//! Полоска-настройки ленты детектов: строка сверху (как у Ордеров — контролы + разделитель)
//! с кнопкой ⚙, раскрывающей меню отображения (размер карточки + видимые поля). Меню —
//! `MoonDropdown` (оверлей-слой moonui: не прячется под own-pass чартом и позиционируется
//! сам). Persist — per-group в `layout.detect_view_by_group` (лента одна на окно-группу).

use gpui::*;
use moon_ui::{
    MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonMenuItem, MoonMenuSize, MoonPalette, h_flex,
};
use moon_core::config::detect_view::{DETECT_SIZE_LARGE, DETECT_SIZE_MEDIUM, DETECT_SIZE_MINI};
use moon_core::config::DetectViewCfg;

use super::DetectsPanel;
use crate::design;
use crate::panels::{radio_items, RadioMark};

const SIZES: [(u8, &str); 3] = [
    (DETECT_SIZE_MINI, "Мини"),
    (DETECT_SIZE_MEDIUM, "Средний"),
    (DETECT_SIZE_LARGE, "Крупный"),
];

/// Правка cfg группы: текущее → мутатор → persist в layout.
fn write_cfg(entity: &Entity<DetectsPanel>, app: &mut App, f: impl FnOnce(&mut DetectViewCfg)) {
    entity.update(app, |panel, cx| {
        let group = panel.group.clone();
        panel.backend.update(cx, |b, _| {
            let mut cfg = b
                .layout
                .detect_view_by_group
                .get(&group)
                .copied()
                .unwrap_or_default();
            f(&mut cfg);
            b.layout.detect_view_by_group.insert(group, cfg);
            b.layout.save();
        });
        cx.notify();
    });
}

/// Пункт-чекбокс одного поля (тогл видимости). Меню не закрывается на клик.
fn field_item(
    entity: &Entity<DetectsPanel>,
    key: &'static str,
    label: &str,
    checked: bool,
    set: fn(&mut DetectViewCfg, bool),
) -> MoonMenuItem {
    let e = entity.clone();
    let next = !checked;
    MoonMenuItem::with_key(key, label.to_string())
        .checked(checked)
        .on_click(move |_, _, app| write_cfg(&e, app, move |c| set(c, next)))
}

/// Меню отображения (размер — radio; поля — чекбоксы; меню не закрывается на клик).
fn settings_menu(cfg: &DetectViewCfg, entity: Entity<DetectsPanel>) -> impl IntoElement {
    let cur = cfg.size_clamped();
    let size_items = radio_items(
        SIZES
            .iter()
            .map(|(sz, label)| (*sz, format!("sz-{sz}").into(), label.to_string().into())),
        cur,
        RadioMark::Check,
        {
            let e = entity.clone();
            move |app, sz| write_cfg(&e, app, |c| c.size = sz)
        },
    );
    MoonDropdown::new("detects-view")
        .label("⚙")
        .trigger_variant(MoonButtonVariant::Ghost)
        // Micro — как кнопка ⚙ полосы вкладок Main (иначе крупнее соседних).
        .trigger_size(MoonButtonSize::Micro)
        .trigger_width(28.0)
        .menu_width(180.0)
        .menu_size(MoonMenuSize::Compact)
        .close_on_select(false)
        .items(size_items)
        .item(MoonMenuItem::separator())
        .item(field_item(&entity, "f-time", "Время", cfg.show_time, |c, v| {
            c.show_time = v
        }))
        .item(field_item(&entity, "f-core", "Ядро", cfg.show_core, |c, v| {
            c.show_core = v
        }))
        .item(field_item(
            &entity,
            "f-badge",
            "Бейдж",
            cfg.show_badge,
            |c, v| c.show_badge = v,
        ))
        .item(field_item(
            &entity,
            "f-chart",
            "Чарт",
            cfg.show_chart,
            |c, v| c.show_chart = v,
        ))
        .item(field_item(
            &entity,
            "f-line",
            "Линия (вместо свечей)",
            cfg.line_mode,
            |c, v| c.line_mode = v,
        ))
        .item(field_item(
            &entity,
            "f-d24",
            "Дельта 24ч (в линии)",
            cfg.show_delta_24h,
            |c, v| c.show_delta_24h = v,
        ))
        .item(field_item(
            &entity,
            "f-d1",
            "Дельта 1ч (в линии)",
            cfg.show_delta_1h,
            |c, v| c.show_delta_1h = v,
        ))
        .item(field_item(
            &entity,
            "f-exch",
            "Биржа",
            cfg.show_exchange,
            |c, v| c.show_exchange = v,
        ))
        .item(field_item(
            &entity,
            "f-exch-kind",
            "Тип биржи",
            cfg.show_exchange_kind,
            |c, v| c.show_exchange_kind = v,
        ))
}

/// Полоска сверху панели: меню ⚙ СЛЕВА (строка как у Ордеров; разделитель снизу добавляет
/// [`super`]). Слева — чтобы кнопка не улетала за правый край при узкой панели на загрузке.
/// `cx` берётся иммутабельно — меню держит только `Entity`.
pub(super) fn toolbar(cfg: &DetectViewCfg, p: MoonPalette, cx: &Context<DetectsPanel>) -> Div {
    let menu = settings_menu(cfg, cx.entity());
    // Высота — как у полосы вкладок Main (chart_tab_strip_h = fit_height(28,13,7.5)); цвет —
    // как у нижней док-полосы (tab_bar палитры), а не фон тела панели.
    h_flex()
        .w_full()
        .flex_none()
        .h(design::fit_h_px(cx, 28.0, 13.0, 7.5))
        .items_center()
        .px_2()
        .bg(rgb(p.tabbar))
        .child(menu)
}
