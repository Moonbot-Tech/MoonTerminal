//! Вкладка «Подключения» — порт egui `settings/connections.rs`: слева таблица ядер
//! (Акт·Окно·Имя·Ключ·Группа·[Данные n/8]·Цвет·Удалить·↻реконнект·●статус), справа
//! панель групп (галка·иконка·имя·👁показать·выбор иконки + пикер). Над ними — источник
//! рыночных данных (выпадающий). Правки идут в draft; статус/реконнект — через `Backend`.
//!
//! Разбито по файлам: здесь — editor-стейты строк (`ConnRow`/`build_conn`) и синхронизация
//! групп из серверов; [`table`] — таблица ядер (строки/колонки/шапка/add/del);
//! [`tab`] — панель групп, пикер иконок и сборка вкладки (`connections_tab`).

mod tab;
mod table;

use gpui::*;
use moon_ui::{MoonColorPickerState, MoonInputEvent, MoonInputState};

use super::SettingsView;
use crate::Backend;
use moon_core::config::{AppConfig, GroupConfig, Secret, ServerConfig};

/// Редактор одной строки сервера: текст-поля + цвет (entity-стейты компонентов).
pub(super) struct ConnRow {
    name: Entity<MoonInputState>,
    key: Entity<MoonInputState>,
    group: Entity<MoonInputState>,
    /// Имя чарт-связки AddToChart (пусто = по глоб. настройке). См. `ServerConfig::chart_bundle`.
    bundle: Entity<MoonInputState>,
    color: Entity<MoonColorPickerState>,
}

pub(super) fn sync_groups_from_servers(cfg: &mut AppConfig) -> bool {
    let mut names: Vec<String> = cfg.servers.iter().map(|s| s.group.clone()).collect();
    names.sort();
    names.dedup();

    let mut changed = false;
    cfg.groups.retain(|g| {
        let keep = names.contains(&g.name);
        changed |= !keep;
        keep
    });
    for name in names {
        if !cfg.groups.iter().any(|g| g.name == name) {
            cfg.groups.push(GroupConfig::new(name));
            changed = true;
        }
    }
    changed
}

/// TextInput, привязанный к полю сервера `servers[i]` (пишет в draft).
fn conn_input(
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    i: usize,
    init: String,
    get: fn(&ServerConfig) -> String,
    set: fn(&mut ServerConfig, String),
    sync_groups: bool,
) -> Entity<MoonInputState> {
    let st = cx.new(|cx| MoonInputState::new(window, cx).default_value(init));
    cx.subscribe(&st, move |this, emitter, ev: &MoonInputEvent, cx| {
        if matches!(ev, MoonInputEvent::Change) {
            let val = emitter.read(cx).value().to_string();
            this.backend.update(cx, |b, bcx| {
                if let Some(p) = b.preview.as_mut() {
                    if let Some(s) = p.servers.get_mut(i) {
                        if get(s) != val {
                            set(s, val);
                            if sync_groups {
                                sync_groups_from_servers(p);
                            }
                            bcx.notify();
                        }
                    }
                }
            });
        }
    })
    .detach();
    st
}

/// Color-picker, привязанный к `servers[i].color` (пишет в draft).
fn conn_color(
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    i: usize,
    init: [u8; 3],
) -> Entity<MoonColorPickerState> {
    super::draft_color(window, cx, init, move |p, c| {
        if let Some(s) = p.servers.get_mut(i) {
            if s.color != c {
                s.color = c;
                return true;
            }
        }
        false
    })
}

/// Построить per-server editor-стейты из draft-серверов. Зовётся в `SettingsView::new`
/// и после add/remove сервера (индексы в подписках свежие).
pub(super) fn build_conn(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> Vec<ConnRow> {
    let servers = {
        let b = backend.read(cx);
        b.preview.as_ref().unwrap_or(&b.config).servers.clone()
    };
    servers
        .iter()
        .enumerate()
        .map(|(i, s)| ConnRow {
            name: conn_input(
                window,
                cx,
                i,
                s.name.clone(),
                |s| s.name.clone(),
                |s, v| s.name = v,
                false,
            ),
            // Ключ — поле пароля (порт egui `.password(true)`): символы скрыты, рядом
            // переключатель видимости (mask_toggle), чтобы при необходимости показать.
            key: {
                let st = conn_input(
                    window,
                    cx,
                    i,
                    s.key.expose().to_string(),
                    |s| s.key.expose().to_string(),
                    |s, v| s.key = Secret::new(v),
                    false,
                );
                st.update(cx, |st, c| st.set_masked(true, window, c));
                st
            },
            group: conn_input(
                window,
                cx,
                i,
                s.group.clone(),
                |s| s.group.clone(),
                |s, v| s.group = v,
                true,
            ),
            bundle: conn_input(
                window,
                cx,
                i,
                s.chart_bundle.clone(),
                |s| s.chart_bundle.clone(),
                |s, v| s.chart_bundle = v,
                false,
            ),
            color: conn_color(window, cx, i, s.color),
        })
        .collect()
}
