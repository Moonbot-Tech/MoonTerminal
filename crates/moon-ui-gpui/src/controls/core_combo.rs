//! Общее поле-список выбора ядер (мультивыбор чекбоксами) — «Ордера», «Отчёт», «Активы».
//!
//! Три панели рисовали один и тот же комбобокс с точностью до подписей, ширины меню и id.
//! Разница в типе ключа была мнимой: `CoreId = u64` (алиас, см. `moon_core::session::store`),
//! поэтому builder работает с `u64` и подходит всем троим. Что делает тумблер и как зовутся
//! подписи — решает вызывающий (у каждой панели свои locale-ключи и свой `toggle_core`).

use std::collections::HashSet;

use gpui::App;

use crate::design;
use moon_ui::{MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonMenuItem, MoonMenuSize};

/// Поле-список ядер: МУЛЬТИВЫБОР (чекбоксы, меню НЕ закрывается на клик — можно отметить
/// сразу несколько). Подпись триггера: `all_label` (пусто / выбраны все) / имя единственного
/// выбранного / `cores_n(N)`. `on_toggle(None, app)` — пункт «Все» (тумблер снять все / выбрать
/// все), `on_toggle(Some(id), app)` — тогл одного ядра.
///
/// `id` — id дропдауна; от него же образуются ключи пунктов (`{id}-all` / `{id}-{i}`),
/// уникальные в пределах своего меню.
pub(crate) fn core_combo<F>(
    cx: &App,
    id: &'static str,
    cores: &[(u64, String)],
    selected: &HashSet<u64>,
    all_label: String,
    cores_n: impl Fn(usize) -> String,
    menu_width: f32,
    on_toggle: F,
) -> MoonDropdown
where
    F: Fn(Option<u64>, &mut App) + Clone + 'static,
{
    let all_selected = !cores.is_empty() && selected.len() == cores.len();
    let all_on = selected.is_empty() || all_selected;
    let cur = match selected.len() {
        0 => all_label.clone(),
        _ if all_selected => all_label.clone(),
        1 => {
            let sel = *selected.iter().next().unwrap();
            cores
                .iter()
                .find(|(c, _)| *c == sel)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| cores_n(1))
        }
        n => cores_n(n),
    };
    let toggle_all = on_toggle.clone();
    let mut menu = MoonDropdown::new(id)
        .label(format!("{cur} ▾"))
        .trigger_variant(MoonButtonVariant::Soft)
        .trigger_size(MoonButtonSize::Action)
        .trigger_width(design::font_w(cx, 118.0))
        .menu_width(design::font_w(cx, menu_width))
        .menu_max_height(360.0)
        .menu_size(MoonMenuSize::Compact)
        .close_on_select(false)
        .item(
            // «Все» — тумблер: снять все / выбрать все ядра.
            MoonMenuItem::with_key(format!("{id}-all"), all_label)
                .checked(all_on)
                .selected(all_on)
                .on_click(move |_, _, app| toggle_all(None, app)),
        );
    for (i, (core, name)) in cores.iter().enumerate() {
        let core = *core;
        let on = selected.contains(&core);
        let on_toggle = on_toggle.clone();
        menu = menu.item(
            MoonMenuItem::with_key(format!("{id}-{i}"), name.clone())
                .checked(on)
                .selected(on)
                .on_click(move |_, _, app| on_toggle(Some(core), app)),
        );
    }
    menu
}
