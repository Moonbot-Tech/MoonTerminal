//! Копировать/Вставить настройки вкладки (обмен между пользователями): текст = РОВНО
//! содержимое переносимого файла вкладки (Интерфейс = theme.toml, Линии = orders.toml,
//! Бейджи = badges.json, Хоткеи = hotkeys.toml). Скопированное можно сохранить файлом,
//! и наоборот — вставить содержимое файла, присланного другим пользователем.
//! Вставка пишет в draft (живое превью); на диск попадает по «Сохранить».
//! Формат/валидация — в moon-core (`to_share_string`/`parse_share` у структур файлов).

use super::{SettingsView, Tab, badges, interface, lines};
use gpui::*;
use moon_core::config::{AppConfig, BadgesConfig, ChartThemeSet, HotkeysConfig, OrdersStyleSet};

impl SettingsView {
    /// У вкладки есть свой переносимый файл → показываем Копировать/Вставить.
    pub(super) fn shareable(&self) -> bool {
        matches!(
            self.active,
            Tab::Interface | Tab::Lines | Tab::Badges | Tab::Hotkeys
        )
    }

    /// «Копировать»: сериализовать draft-срез активной вкладки в формате её файла → буфер.
    pub(super) fn copy_tab(&mut self, cx: &mut Context<Self>) {
        let text = {
            let b = self.backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            match self.active {
                Tab::Interface => d.theme.to_share_string(),
                Tab::Lines => d.orders.to_share_string(),
                Tab::Hotkeys => d.hotkeys.to_share_string(),
                Tab::Badges => d.badges.to_share_string(),
                _ => None,
            }
        };
        let Some(text) = text else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.status = Some((super::StatusMsg::Key("settings.copied"), false));
        cx.notify();
    }

    /// «Вставить»: разобрать буфер как файл активной вкладки, применить к draft (живое
    /// превью) и пересобрать editor-стейты вкладки (пикеры/слайдеры держат значения с
    /// build). Чужой/битый текст — ошибка в статус, draft не трогаем.
    pub(super) fn paste_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .filter(|t| !t.trim().is_empty())
        else {
            self.status = Some((super::StatusMsg::Key("settings.paste_empty"), true));
            cx.notify();
            return;
        };

        let applied = match self.active {
            Tab::Interface => {
                let cur = self.draft_snapshot(cx, |d| d.theme.clone());
                ChartThemeSet::parse_share(&text, &cur)
                    .map(|set| self.apply_draft(cx, move |d| d.theme = set))
                    .is_some()
            }
            Tab::Lines => {
                let cur = self.draft_snapshot(cx, |d| d.orders.clone());
                OrdersStyleSet::parse_share(&text, &cur)
                    .map(|set| self.apply_draft(cx, move |d| d.orders = set))
                    .is_some()
            }
            Tab::Hotkeys => HotkeysConfig::parse_share(&text)
                .map(|h| self.apply_draft(cx, move |d| d.hotkeys = h))
                .is_some(),
            Tab::Badges => BadgesConfig::parse_share(&text)
                .map(|b| self.apply_draft(cx, move |d| d.badges = b))
                .is_some(),
            _ => false,
        };

        if applied {
            // Editor-стейты инициализируются из draft при build — пересобрать под
            // вставленные значения (паттерн add/del бейджей/серверов).
            match self.active {
                Tab::Interface => self.iface = interface::build(&self.backend, window, cx),
                Tab::Lines => self.lines = lines::build(&self.backend, window, cx),
                Tab::Badges => self.badges = badges::build(&self.backend, window, cx),
                _ => {}
            }
            self.status = Some((super::StatusMsg::Key("settings.pasted"), false));
        } else {
            self.status = Some((super::StatusMsg::Key("settings.paste_wrong"), true));
        }
        cx.notify();
    }

    /// Снимок среза draft (для parse_share, которому нужен текущий набор).
    fn draft_snapshot<T>(&self, cx: &Context<Self>, get: impl Fn(&AppConfig) -> T) -> T {
        let b = self.backend.read(cx);
        get(b.preview.as_ref().unwrap_or(&b.config))
    }

    /// Записать в draft + notify бэкенда (живое превью, как обычные правки вкладок).
    fn apply_draft(&self, cx: &mut Context<Self>, apply: impl FnOnce(&mut AppConfig)) {
        self.backend.update(cx, |b, bcx| {
            if let Some(p) = b.preview.as_mut() {
                apply(p);
                bcx.notify();
            }
        });
    }
}
