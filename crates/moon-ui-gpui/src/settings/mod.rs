//! Окно настроек (порт egui `src/settings/*` + `window/settings_window.rs`).
//! Отдельное ОС-окно, редактирует ЖИВОЙ `Backend.config`: правки темы применяются
//! к чарту сразу (группы-окна читают config каждый кадр и пере-рендерят offscreen),
//! «Сохранить» пишет на диск (`AppConfig::save`).
//!
//! Разбито по вкладкам (как egui-оригинал): [`interface`] (тема), [`general`] (общие),
//! [`lines`] (стиль ордер-линий), [`connections`] (ядра/группы). Здесь — каркас:
//! `SettingsView` (состояние + поля редакторов) и `open`. Сами вкладки и их состояние —
//! в подмодулях (`impl SettingsView` расщеплён по файлам): рендер каркаса (таб-бар,
//! футер «Сохранить», шапка) — в [`render`], сохранение/применение — в [`apply`],
//! общие UI-хелперы (`slider_row`/`section`/`color_row`/`separator`/draft-байндеры) —
//! в [`common`] (re-export ниже).

mod apply;
mod badges;
mod common;
mod connections;
mod general;
mod hotkeys;
mod interface;
mod lines;
mod render;
mod share;
mod storage;

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use gpui::*;
use moon_ui::{
    IndexPath, MoonBackgroundPolicy, MoonCheckbox, MoonSelectEvent, MoonSelectItem,
    MoonSelectState, MoonSliderState, Root,
};
use rust_i18n::t;

use crate::icons::IconSet;
use crate::Backend;
use moon_core::config::{AppConfig, Language};
use moon_core::market::MarketDataMode;

use badges::BadgesEd;
use common::{collapse_block, color_row, draft_color, draft_slider, section, separator, slider_row};
use connections::ConnRow;
use interface::Iface;
use lines::Lines;

const SETTINGS_HEADER_H: f32 = 30.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Connections,
    General,
    Hotkeys,
    Interface,
    Lines,
    Badges,
    Storage,
}

impl Tab {
    const ALL: [Tab; 7] = [
        Tab::Connections,
        Tab::General,
        Tab::Hotkeys,
        Tab::Interface,
        Tab::Lines,
        Tab::Badges,
        Tab::Storage,
    ];
    /// Стабильный id вкладки (для `MoonButton::new`/ключей) — НЕ переводим.
    fn id(self) -> &'static str {
        match self {
            Tab::Connections => "Подключения",
            Tab::General => "Общие",
            Tab::Hotkeys => "Хоткеи",
            Tab::Interface => "Интерфейс",
            Tab::Lines => "Линии",
            Tab::Badges => "Бейджи",
            Tab::Storage => "Хранилище",
        }
    }
    /// Локализованная подпись вкладки (порт `tab.*`).
    fn title(self) -> String {
        match self {
            Tab::Connections => t!("tab.connections"),
            Tab::General => t!("tab.general"),
            Tab::Hotkeys => t!("tab.hotkeys"),
            Tab::Interface => t!("tab.interface"),
            Tab::Lines => t!("tab.lines"),
            Tab::Badges => t!("tab.badges"),
            Tab::Storage => t!("tab.storage"),
        }
        .to_string()
    }
}

/// Режимы источника данных (вкладка «Подключения») — стабильный i18n-ключ + режим;
/// подпись локализуется на use-сайте (`conn.market_dedup`/`conn.market_percore`).
const MODE_LABELS: [(&str, MarketDataMode); 2] = [
    ("conn.market_dedup", MarketDataMode::Dedup),
    ("conn.market_percore", MarketDataMode::PerCore),
];

/// Сообщение статуса подвала: ключ i18n (резолвится на РЕНДЕРЕ — не кэшируем
/// готовую строку, иначе после смены языка «Сохранено» оставалось хвостом
/// прошлой локали) либо готовый текст (ошибки I/O, не локализуются).
pub(crate) enum StatusMsg {
    Key(&'static str),
    Text(String),
}

pub struct SettingsView {
    backend: Entity<Backend>,
    active: Tab,
    /// Статус сохранения: (сообщение, ошибка?).
    status: Option<(StatusMsg, bool)>,
    iface: Iface,
    lines: Lines,
    /// Редактор бейджей типов детектов (вкладка «Бейджи»); пересоздаётся при add/del.
    badges: BadgesEd,
    /// Per-server editor-стейты (вкладка «Подключения»); пересоздаётся при add/del.
    conn: Vec<ConnRow>,
    /// Слайдер «Шрифт UI» (`ui_font_delta`, личное из settings.toml) — вкладка «Общие».
    ui_font: Entity<MoonSliderState>,
    /// Выпадающий выбор языка (вкладка «Общие»).
    lang: Entity<MoonSelectState<Language>>,
    /// Выпадающий выбор источника данных (вкладка «Подключения»).
    mode: Entity<MoonSelectState<MarketDataMode>>,
    /// Какие блоки-линии раскрыты (вкладка «Линии», порт CollapsingHeader).
    open_lines: HashSet<&'static str>,
    /// Активная группа вкладки «Хоткеи» (саб-вкладки, как страницы хоткеев Moonbot).
    hotkeys_group: hotkeys::HotkeyGroup,
    /// Вкладка «Хранилище»: конфиг storage.toml + фоновый снимок размеров/счётчиков.
    storage: storage::StorageEd,
    /// Кэш иконок групп (вкладка «Подключения»).
    icons: IconSet,
    /// Для какой группы открыт пикер иконок (None = закрыт). Порт egui `picking`.
    picking: Option<String>,
    /// Сигнатура данных, которые реально читают настройки: draft/config + статусы.
    last_sig: u64,
}

impl SettingsView {
    /// Общий чекбокс draft-настроек: init = переданное значение, на `Change` — пишет в живой
    /// `Backend.preview` через `apply` (проверка изменения + сеттер) и нотифаит бэкенд+view, если
    /// что-то поменялось. Возвращает базовый `MoonCheckbox` — вызывающий навешивает `.label()`/
    /// `.size()`. Общий для вкладок Линии/Подключения/Общие.
    pub(super) fn draft_checkbox(
        &self,
        cx: &Context<Self>,
        id: impl Into<SharedString>,
        init: bool,
        apply: impl Fn(&mut AppConfig, bool) -> bool + 'static,
    ) -> MoonCheckbox {
        MoonCheckbox::new(id.into())
            .checked(init)
            .on_change(cx.listener(move |this, ch: &bool, _w, cx| {
                let v = *ch;
                let changed = this.backend.update(cx, |b, bcx| {
                    let mut changed = false;
                    if let Some(p) = b.preview.as_mut() {
                        if apply(p, v) {
                            bcx.notify();
                            changed = true;
                        }
                    }
                    changed
                });
                if changed {
                    cx.notify();
                }
            }))
    }

    fn new(backend: Entity<Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let iface = interface::build(&backend, window, cx);
        let lines = lines::build(&backend, window, cx);
        let badges = badges::build(&backend, window, cx);
        let conn = connections::build_conn(&backend, window, cx);

        // Слайдер «Шрифт UI» — личная настройка (settings.toml), живёт на вкладке «Общие»;
        // правка переустанавливает MoonUI-тему живьём (масштаб шрифтов всего UI).
        let ui_font = {
            let cur = {
                let b = backend.read(cx);
                b.preview.as_ref().unwrap_or(&b.config).ui_font_delta
            };
            draft_slider(cx, -2.0, 6.0, 1.0, cur, move |p, f, bcx| {
                if p.ui_font_delta != f {
                    p.ui_font_delta = f;
                    crate::install_moon_theme_for_config(p, bcx);
                    true
                } else {
                    false
                }
            })
        };

        // Сохранять положение/размер окна «Настройки» в layout — чтобы открывалось на прежнем
        // месте. Дебаунс-сейв делает дренаж по `layout_dirty` (как у Стратегий/Активов).
        cx.observe_window_bounds(window, |this, window, cx| {
            let Some((x, y, w, h)) = crate::windowing::window_geom(window) else {
                return;
            };
            this.backend.update(cx, |b, _| {
                if b.layout.settings_window.map(|g| (g.x, g.y, g.w, g.h)) != Some((x, y, w, h)) {
                    b.layout.settings_window =
                        Some(moon_core::config::layout::GeomRect { x, y, w, h });
                    b.layout_dirty = true;
                }
            });
        })
        .detach();

        // Язык — выпадающий список (порт egui ComboBox). Init = текущий язык draft.
        let (cur_lang, cur_mode) = {
            let b = backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            (d.language, d.market_mode)
        };
        let lang_items = Language::ALL
            .iter()
            .map(|l| MoonSelectItem::new(*l, l.label()))
            .collect::<Vec<_>>();
        let lang_idx = Language::ALL
            .iter()
            .position(|l| *l == cur_lang)
            .unwrap_or(0);
        let lang = cx
            .new(|cx| MoonSelectState::new(lang_items, Some(IndexPath::new(lang_idx)), window, cx));
        cx.subscribe(&lang, |this, _e, ev: &MoonSelectEvent<Language>, cx| {
            if let MoonSelectEvent::Confirm(Some(language)) = ev {
                let language = *language;
                this.backend.update(cx, |b, bcx| {
                    if let Some(p) = b.preview.as_mut() {
                        p.language = language;
                        bcx.notify();
                    }
                });
            }
        })
        .detach();

        // Источник данных — выпадающий список (порт egui ComboBox).
        let mode_items = MODE_LABELS
            .iter()
            .map(|(key, mode)| MoonSelectItem::new(*mode, t!(*key).to_string()))
            .collect::<Vec<_>>();
        let mode_idx = MODE_LABELS
            .iter()
            .position(|(_, m)| *m == cur_mode)
            .unwrap_or(0);
        let mode = cx
            .new(|cx| MoonSelectState::new(mode_items, Some(IndexPath::new(mode_idx)), window, cx));
        cx.subscribe(
            &mode,
            |this, _e, ev: &MoonSelectEvent<MarketDataMode>, cx| {
                if let MoonSelectEvent::Confirm(Some(mode)) = ev {
                    let mode = *mode;
                    this.backend.update(cx, |b, bcx| {
                        if let Some(p) = b.preview.as_mut() {
                            p.market_mode = mode;
                            bcx.notify();
                        }
                    });
                }
            },
        )
        .detach();

        let initial_sig = settings_sig(backend.read(cx));
        cx.observe(&backend, |this, backend, cx| {
            let sig = settings_sig(backend.read(cx));
            if sig != this.last_sig {
                this.last_sig = sig;
                cx.notify();
            }
        })
        .detach();

        // Закрытие окна (drop view) → сбросить draft: чарт откатывается к config
        // (отмена несохранённых правок) — как egui (draft discarded on close).
        cx.on_release(|this, app| {
            this.backend.update(app, |b, cx| {
                crate::install_moon_theme_for_config(&b.config, cx);
                b.preview = None;
                b.settings_window = None;
                cx.notify();
            });
        })
        .detach();
        Self {
            backend,
            active: Tab::Connections,
            status: None,
            iface,
            lines,
            badges,
            conn,
            ui_font,
            lang,
            mode,
            open_lines: HashSet::new(),
            hotkeys_group: hotkeys::HotkeyGroup::Presets,
            storage: storage::build(),
            icons: IconSet::discover(),
            picking: None,
            last_sig: initial_sig,
        }
    }
}

fn settings_sig(b: &Backend) -> u64 {
    let cfg = b.preview.as_ref().unwrap_or(&b.config);
    let mut h = DefaultHasher::new();

    cfg.language.code().hash(&mut h);
    cfg.market_mode.code().hash(&mut h);
    cfg.charts_split_by_core.hash(&mut h);
    cfg.charts_stack_scroll.hash(&mut h);
    cfg.charts_stack_compress.hash(&mut h);
    cfg.chart_stack_height.hash(&mut h);
    cfg.log_to_file.hash(&mut h);
    cfg.log_retention_days.hash(&mut h);
    cfg.ui_font_delta.to_bits().hash(&mut h);
    cfg.ui_theme_mode.hash(&mut h);
    cfg.ui_scale.to_bits().hash(&mut h);
    cfg.hotkeys.hash(&mut h);
    format!("{:?}", cfg.theme).hash(&mut h);
    format!("{:?}", cfg.orders).hash(&mut h);
    format!("{:?}", cfg.badges).hash(&mut h);

    cfg.servers.len().hash(&mut h);
    for s in &cfg.servers {
        s.id.hash(&mut h);
        s.uid.hash(&mut h);
        s.name.hash(&mut h);
        s.active.hash(&mut h);
        s.show_window.hash(&mut h);
        s.feed.orders.hash(&mut h);
        s.feed.detects.hash(&mut h);
        s.feed.reports.hash(&mut h);
        s.feed.balance.hash(&mut h);
        s.feed.strategies.hash(&mut h);
        s.feed.log.hash(&mut h);
        s.feed.alerts.hash(&mut h);
        s.feed.arb.hash(&mut h);
        // The key input owns its local repaint while typing; only empty/non-empty
        // affects surrounding settings layout.
        s.key.is_empty().hash(&mut h);
        s.group.hash(&mut h);
        s.market.hash(&mut h);
        s.color.hash(&mut h);
        s.synthetic.hash(&mut h);
    }

    cfg.groups.len().hash(&mut h);
    for g in &cfg.groups {
        g.name.hash(&mut h);
        g.active.hash(&mut h);
        g.icon.hash(&mut h);
    }

    let mut statuses = b.session.status_map().into_iter().collect::<Vec<_>>();
    statuses.sort_by_key(|(id, _)| *id);
    for (id, status) in statuses {
        id.hash(&mut h);
        format!("{status:?}").hash(&mut h);
    }

    h.finish()
}

/// Открыть окно настроек (отдельное ОС-окно). Заводит draft = копия config (его
/// правят вкладки, чарт показывает его живьём). Повторный клик при уже открытом
/// окне игнорируем (draft уже есть) — иначе два окна делили бы один draft.
pub fn open(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
    if let Some(handle) = backend.read(cx).settings_window {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    if backend.read(cx).preview.is_some() {
        return;
    }
    backend.update(cx, |b, _| {
        let mut preview = b.config.clone();
        connections::sync_groups_from_servers(&mut preview);
        b.preview = Some(preview);
    });
    // Геометрию восстанавливаем из layout (её сохраняет SettingsView), как у Стратегий/Активов.
    let saved = backend.read(cx).layout.settings_window;
    let bounds = saved.map_or(
        Bounds {
            origin: point(px(160.0), px(120.0)),
            size: size(px(860.0), px(620.0)),
        },
        |g| Bounds {
            origin: point(px(g.x as f32), px(g.y as f32)),
            size: size(px(g.w as f32), px(g.h as f32)),
        },
    );
    // Мультимонитор: без display_id окно создаётся на primary и при bounds вне него gpui
    // откатывается на дефолт — монитор по сохранённой точке (не-мак) либо от владельца.
    let display_id = crate::windowing::saved_or_owner_display_id(
        saved.map(|g| point(px(g.x as f32), px(g.y as f32))),
        owner,
        owner_display,
        cx,
    );
    let mut opts = crate::windowing::tool_window_options(
        t!("settings.window_title").to_string(),
        WindowBounds::Windowed(bounds),
        Some(size(px(620.0), px(420.0))),
        owner,
    );
    opts.display_id = display_id;
    let b = backend.clone();
    match cx.open_window(opts, move |window, cx| {
        crate::windowing::configure_shell_clear_color(window, cx);
        let view = cx.new(|cx| SettingsView::new(b, window, cx));
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    }) {
        Ok(handle) => {
            backend.update(cx, |b, _| b.settings_window = Some(handle));
            crate::windowing::activate_new_window(handle.into(), cx);
        }
        Err(_) => {
            backend.update(cx, |b, cx| {
                b.preview = None;
                b.settings_window = None;
                cx.notify();
            });
        }
    }
}
