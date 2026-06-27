//! Окно настроек (порт egui `src/settings/*` + `window/settings_window.rs`).
//! Отдельное ОС-окно, редактирует ЖИВОЙ `Backend.config`: правки темы применяются
//! к чарту сразу (группы-окна читают config каждый кадр и пере-рендерят offscreen),
//! «Сохранить» пишет на диск (`AppConfig::save`).
//!
//! Разбито по вкладкам (как egui-оригинал): [`interface`] (тема), [`general`] (общие),
//! [`lines`] (стиль ордер-линий), [`connections`] (ядра/группы). Здесь — каркас:
//! `SettingsView` (состояние + поля редакторов), таб-бар, футер «Сохранить», общие
//! UI-хелперы (`slider_row`/`section`/`color_row`/`separator`) и `open`. Сами вкладки
//! и их состояние — в подмодулях (`impl SettingsView` расщеплён по файлам).

mod connections;
mod general;
mod hotkeys;
mod interface;
mod lines;

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    IndexPath, MoonBackgroundPolicy, MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox,
    MoonColorPicker, MoonColorPickerEvent, MoonColorPickerState, MoonPalette, MoonSelectEvent,
    MoonSelectItem, MoonSelectState, MoonSlider, MoonSliderEvent, MoonSliderState, MoonWindowFrame,
    Root, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use crate::icons::IconSet;
use crate::{Backend, design};
use moon_core::config::{AppConfig, Language};
use moon_core::market::MarketDataMode;

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
}

impl Tab {
    const ALL: [Tab; 5] = [
        Tab::Connections,
        Tab::General,
        Tab::Hotkeys,
        Tab::Interface,
        Tab::Lines,
    ];
    /// Стабильный id вкладки (для `MoonButton::new`/ключей) — НЕ переводим.
    fn id(self) -> &'static str {
        match self {
            Tab::Connections => "Подключения",
            Tab::General => "Общие",
            Tab::Hotkeys => "Хоткеи",
            Tab::Interface => "Интерфейс",
            Tab::Lines => "Линии",
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
        }
        .to_string()
    }
}

/// Hsla (из color-picker) → sRGB [u8;3] для ChartTheme/OrdersStyle.
pub(super) fn hsla_u8(h: Hsla) -> [u8; 3] {
    let c: Rgba = h.into();
    [
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8,
    ]
}

/// Строка слайдера (порт egui `Slider::new(..).text(label)`): сам слайдер, справа —
/// подпись и текущее значение. Инлайн, на высоту одного ряда (как на стенде).
pub(super) fn slider_row(label: &str, st: &Entity<MoonSliderState>, cx: &App) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    let val = st.read(cx).value().end();
    h_flex()
        .w_full()
        .min_h(design::fit_h_px(cx, 28.0, 14.0, 7.0))
        .gap(design::ui_px(cx, 10.0))
        .items_center()
        .child(
            div()
                .w(px(180.0))
                .child(MoonSlider::new(st).height(design::ui_value(cx, 22.0))),
        )
        .child(
            div()
                .w(px(210.0))
                .min_w_0()
                .truncate()
                .text_color(rgba_from(p.text_soft, 1.0))
                .child(label.to_string()),
        )
        .child(
            div()
                .w(px(58.0))
                .text_right()
                .text_color(rgba_from(p.text_muted, 1.0))
                .child(format!("{val:.2}")),
        )
}

/// Разделитель секций (порт egui `ui.separator()`).
pub(super) fn separator(p: MoonPalette, cx: &App) -> impl IntoElement {
    div()
        .my(design::ui_px(cx, 8.0))
        .h(px(1.0))
        .bg(rgba_from(p.border, 1.0))
}

/// Секционный заголовок (порт egui `section()`): жирная подпись с отступом сверху.
pub(super) fn section(title: &str, p: MoonPalette, cx: &App) -> impl IntoElement {
    div()
        .mt(design::ui_px(cx, 10.0))
        .mb(design::ui_px(cx, 4.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgba_from(p.text, 1.0))
        .child(title.to_string())
}

/// Строка цвета (порт egui `color_row`): свотч-пикер, затем подпись справа.
pub(super) fn color_row(
    label: &str,
    st: &Entity<MoonColorPickerState>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .min_h(design::fit_h_px(cx, 28.0, 14.0, 7.0))
        .gap(design::ui_px(cx, 10.0))
        .items_center()
        .child(MoonColorPicker::new(st))
        .child(
            div()
                .text_color(rgba_from(p.text_soft, 1.0))
                .child(label.to_string()),
        )
}

/// Общий color-picker draft-настроек: init = переданное значение, на `Change` — пишет в живой
/// `Backend.preview` через `apply` (он же делает проверку «изменилось ли» и возвращает результат) и
/// нотифаит бэкенд. `apply` — замыкание (может захватывать индекс сервера и т.п.). Общий для вкладок
/// Интерфейс/Линии/Подключения (тонкие обёртки делегируют сюда).
pub(super) fn draft_color(
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    init: [u8; 3],
    apply: impl Fn(&mut AppConfig, [u8; 3]) -> bool + 'static,
) -> Entity<MoonColorPickerState> {
    let st = cx.new(|cx| {
        MoonColorPickerState::new(window, cx).default_value(rgb(design::rgb_to_u32(init)).into())
    });
    cx.subscribe(&st, move |this, _emitter, ev: &MoonColorPickerEvent, cx| {
        let MoonColorPickerEvent::Change(h) = ev;
        let c = hsla_u8(*h);
        this.backend.update(cx, |b, bcx| {
            if let Some(p) = b.preview.as_mut() {
                if apply(p, c) {
                    bcx.notify();
                }
            }
        });
    })
    .detach();
    st
}

/// Общий слайдер f32 draft-настроек: init = переданное значение, на `Change` — пишет в живой
/// `Backend.preview` через `apply` (проверка изменения + сам сеттер; `&mut Context<Backend>` нужен
/// тем полям, что переустанавливают тему). Нотифаит бэкенд, если `apply` вернул true.
pub(super) fn draft_slider(
    cx: &mut Context<SettingsView>,
    min: f32,
    max: f32,
    step: f32,
    init: f32,
    apply: impl Fn(&mut AppConfig, f32, &mut Context<Backend>) -> bool + 'static,
) -> Entity<MoonSliderState> {
    let st = cx.new(|_| {
        MoonSliderState::new()
            .min(min)
            .max(max)
            .step(step)
            .default_value(init)
    });
    cx.subscribe(&st, move |this, _emitter, ev: &MoonSliderEvent, cx| {
        let MoonSliderEvent::Change(f) = ev else {
            return;
        };
        let f = f.end();
        this.backend.update(cx, |b, bcx| {
            if let Some(p) = b.preview.as_mut() {
                if apply(p, f, bcx) {
                    bcx.notify();
                }
            }
        });
    })
    .detach();
    st
}

/// Режимы источника данных (вкладка «Подключения») — стабильный i18n-ключ + режим;
/// подпись локализуется на use-сайте (`conn.market_dedup`/`conn.market_percore`).
const MODE_LABELS: [(&str, MarketDataMode); 2] = [
    ("conn.market_dedup", MarketDataMode::Dedup),
    ("conn.market_percore", MarketDataMode::PerCore),
];

pub struct SettingsView {
    backend: Entity<Backend>,
    active: Tab,
    /// Статус сохранения: (текст, ошибка?).
    status: Option<(String, bool)>,
    iface: Iface,
    lines: Lines,
    /// Per-server editor-стейты (вкладка «Подключения»); пересоздаётся при add/del.
    conn: Vec<ConnRow>,
    /// Выпадающий выбор языка (вкладка «Общие»).
    lang: Entity<MoonSelectState<Language>>,
    /// Выпадающий выбор источника данных (вкладка «Подключения»).
    mode: Entity<MoonSelectState<MarketDataMode>>,
    /// Какие блоки-линии раскрыты (вкладка «Линии», порт CollapsingHeader).
    open_lines: HashSet<&'static str>,
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
        let conn = connections::build_conn(&backend, window, cx);

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
            conn,
            lang,
            mode,
            open_lines: HashSet::new(),
            icons: IconSet::discover(),
            picking: None,
            last_sig: initial_sig,
        }
    }

    /// Коммит draft → config + запись на диск (валидация внутри AppConfig::save).
    /// draft остаётся (правки продолжаются), как egui (Save не закрывает окно). При
    /// успехе — применяем изменения (порт egui `App::render_settings`).
    fn save(&mut self, cx: &mut Context<Self>) {
        // Снимок «до» для diff (структура/режим/язык/лог/чарты). Коммитим draft, пишем
        // на диск (save может выровнять uid'ы), затем сравниваем с актуальным config.
        let before = self.backend.read(cx).config.clone();
        let res = self.backend.update(cx, |b, _| {
            if let Some(p) = &b.preview {
                b.config = p.clone();
            }
            b.config.save()
        });
        match res {
            Ok(()) => {
                self.status = Some((t!("settings.saved").to_string(), false));
                self.apply_settings(&before, cx);
            }
            Err(e) => self.status = Some((e.to_string(), true)),
        }
        cx.notify();
    }

    /// Применить сохранённые настройки (порт egui `App::render_settings` хвост).
    /// • лог-настройки — живо (set_file_logging + чистка);
    /// • структурные изменения серверов/групп → рестарт `SessionManager` + пересоздание
    ///   окон групп; • смена режима рынка — живо (`set_market_mode`); • смена «чарт на
    ///   ядро» без структурных изменений → тоже пересборка окон (новые чарт-вкладки).
    /// • смена языка — живо: ставим локаль rust-i18n и помечаем ВСЕ окна на перерисовку
    ///   (`refresh_windows`). Окна/подключения/раскладка не трогаются — строки через `t!`
    ///   читают локаль на рендере, поэтому достаточно одного redraw.
    fn apply_settings(&mut self, before: &AppConfig, cx: &mut Context<Self>) {
        let after = self.backend.read(cx).config.clone();

        // Язык — применяем живо: глобальная локаль + перерисовка всех окон (БЕЗ пересоздания
        // окон и рестарта сессий). `t!` подхватит новую локаль на ближайшем рендере.
        if before.language != after.language {
            rust_i18n::set_locale(after.language.code());
            cx.refresh_windows();
        }

        // Файловый лог — применяем живо: включили запись или сократили срок → чистим.
        if before.log_to_file != after.log_to_file
            || before.log_retention_days != after.log_retention_days
        {
            moon_core::applog::set_file_logging(after.log_to_file, after.log_retention_days);
            moon_core::applog::purge_old();
        }

        let struct_changed = before.structural_sig() != after.structural_sig();
        let mode_changed = before.market_mode != after.market_mode;
        let split_changed = before.charts_split_by_core != after.charts_split_by_core;
        // Смена чарт-связки (`chart_bundle`) у ядра меняет состав чарт-вкладок, но НЕ требует
        // реконнекта — как split, только пересобираем окна групп (без рестарта сессий).
        let bundle_sig = |c: &AppConfig| {
            let mut v: Vec<(u64, String)> = c
                .servers
                .iter()
                .map(|s| (s.uid, s.chart_bundle.clone()))
                .collect();
            v.sort();
            v
        };
        let bundle_changed = bundle_sig(before) != bundle_sig(&after);
        let ui_theme_changed = before.ui_font_delta != after.ui_font_delta
            || before.ui_theme_mode != after.ui_theme_mode
            || before.ui_scale != after.ui_scale;

        if ui_theme_changed {
            crate::install_moon_theme_for_config(&after, cx);
        }

        if struct_changed {
            // Инкрементальный реконсайл сессий по новому конфигу (НЕ полный рестарт):
            // добавляем новые ядра, гасим удалённые, переподнимаем только изменённые —
            // неизменные ядра не дёргаем. epoch/market_mode сохраняем. chart_market_refs
            // НЕ сбрасываем: пережившие окна сохраняют свои подписки, закрытые освободят их
            // через on_release панелей, новые — зарегистрируют при открытии.
            self.backend.update(cx, |b, _| {
                let reports = b.reports.as_ref().map(|h| &h.tx);
                b.session.reconcile(&b.config, reports);
                b.session.set_market_mode(b.config.market_mode);
            });
            self.reconcile_group_windows(cx);
        } else if mode_changed {
            // Режим рынка — живо: ядра остаются на связи, координатор пере-выберет
            // провайдеров на следующем тике.
            self.backend
                .update(cx, |b, _| b.session.set_market_mode(b.config.market_mode));
        }

        // Сменили «отдельная чарт-вкладка на ядро» (без структурного ребилда, который и
        // так всё пересоздаёт) → пересобираем окна, чтобы чарт-вкладки собрались в новом
        // режиме (egui чистил chart-tabs; в GPUI вкладки живут в окне — пересоздаём окно).
        if !struct_changed && (split_changed || bundle_changed) {
            self.rebuild_group_windows(cx);
        }
    }

    /// Закрыть все окна групп и открыть заново по актуальному конфигу (порт egui
    /// `needs_rebuild`). Геометрия восстановится из сохранённой раскладки.
    ///
    /// Также закрываем ВСЕ откреп-окна чарт-вкладок и снимаем у спек `detached`: при
    /// смене групп их состав/ключи (bucket) меняются — старые окна иначе зависают дублями
    /// и сыплют «window not found» по протухшим хэндлам. Вкладки вернутся в стрип нового
    /// окна группы по детектам (а не повторно откроются off-screen окнами).
    fn rebuild_group_windows(&mut self, cx: &mut Context<Self>) {
        let (handles, chart_handles, cfg, epoch, layout) = self.backend.update(cx, |b, _| {
            let handles: Vec<WindowHandle<Root>> = b.group_windows.values().copied().collect();
            b.group_windows.clear();
            let chart_handles: Vec<WindowHandle<Root>> =
                b.detached_chart_windows.drain(..).map(|(_, h)| h).collect();
            // Вернуть откреп-вкладки в стрип: снять detached у всех спек, чтобы свежие
            // окна групп не открыли их повторно (иначе дубли).
            for s in b.chart_specs.iter_mut() {
                s.detached = None;
            }
            b.chart_specs_dirty = true;
            (
                handles,
                chart_handles,
                b.config.clone(),
                b.epoch,
                b.layout.clone(),
            )
        });
        for h in handles {
            let _ = h.update(cx, |_, window, _| window.remove_window());
        }
        for h in chart_handles {
            let _ = h.update(cx, |_, window, _| window.remove_window());
        }
        for (i, g) in crate::group_window::groups(&cfg).into_iter().enumerate() {
            crate::group_window::spawn_group_window(
                cx,
                &self.backend,
                &cfg,
                g,
                epoch,
                &layout,
                i as f32 * 40.0,
            );
        }
    }

    /// Инкрементальный реконсайл окон групп (вместо разрушительного `rebuild_group_windows`):
    /// закрывает окна ТОЛЬКО исчезнувших групп (и их откреп-чарты), открывает окна ТОЛЬКО
    /// новых групп, а окна сохранившихся групп НЕ трогает — их `ChartTabs` сами подхватят
    /// добавленные/убранные ядра через сигнатуру. Так открытые вкладки и раскладка переживают
    /// добавление/удаление серверов (фикс: раньше любое изменение состава сносило все окна).
    fn reconcile_group_windows(&mut self, cx: &mut Context<Self>) {
        let (close_group, close_detached, spawn_groups, cfg, epoch, layout) =
            self.backend.update(cx, |b, _| {
                let want = crate::group_window::groups(&b.config);
                let want_set: HashSet<&str> = want.iter().map(String::as_str).collect();
                // Окна исчезнувших групп → закрыть.
                let close_group: Vec<WindowHandle<Root>> = b
                    .group_windows
                    .iter()
                    .filter(|(g, _)| !want_set.contains(g.as_str()))
                    .map(|(_, h)| *h)
                    .collect();
                let gone: HashSet<String> = b
                    .group_windows
                    .keys()
                    .filter(|g| !want_set.contains(g.as_str()))
                    .cloned()
                    .collect();
                b.group_windows.retain(|g, _| want_set.contains(g.as_str()));
                // Откреп-чарты исчезнувших групп → закрыть (их группы больше нет).
                let close_detached: Vec<WindowHandle<Root>> = b
                    .detached_chart_windows
                    .iter()
                    .filter(|(g, _)| gone.contains(g))
                    .map(|(_, h)| *h)
                    .collect();
                b.detached_chart_windows.retain(|(g, _)| !gone.contains(g));
                // Новые группы (в want, окна ещё нет) → открыть. Сохранившиеся пропускаем.
                let spawn_groups: Vec<String> = want
                    .iter()
                    .filter(|g| !b.group_windows.contains_key(g.as_str()))
                    .cloned()
                    .collect();
                (
                    close_group,
                    close_detached,
                    spawn_groups,
                    b.config.clone(),
                    b.epoch,
                    b.layout.clone(),
                )
            });
        for h in close_group {
            let _ = h.update(cx, |_, window, _| window.remove_window());
        }
        for h in close_detached {
            let _ = h.update(cx, |_, window, _| window.remove_window());
        }
        for (i, g) in spawn_groups.into_iter().enumerate() {
            crate::group_window::spawn_group_window(
                cx,
                &self.backend,
                &cfg,
                g,
                epoch,
                &layout,
                i as f32 * 40.0,
            );
        }
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let chrome_width = f32::from(window.viewport_size().width);

        // ── Полоска вкладок ─────────────────────────────────────────────────
        let mut tabs = h_flex()
            .w_full()
            .h(design::fit_h_px(cx, 34.0, 13.0, 10.5))
            .gap(design::ui_px(cx, 6.0))
            .px(design::ui_px(cx, 8.0))
            .bg(rgba_from(p.shell_high, 1.0))
            .border_b_1()
            .border_color(rgba_from(p.border, 1.0));
        for t in Tab::ALL {
            let on = self.active == t;
            tabs = tabs.child(
                MoonButton::new(t.id())
                    .variant(if on {
                        MoonButtonVariant::Blue
                    } else {
                        MoonButtonVariant::Ghost
                    })
                    .size(MoonButtonSize::Custom {
                        height: 24.0,
                        radius: 4.0,
                        font_size: 10.5,
                        line_height: 13.0,
                        gap: 5.0,
                    })
                    .width(118.0)
                    .selected(on)
                    .label(t.title())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.active = t;
                        cx.notify();
                    }))
                    .render(),
            );
        }

        // ── Тело активной вкладки ───────────────────────────────────────────
        let content = match self.active {
            Tab::Interface => self.interface_tab(cx).into_any_element(),
            Tab::General => self.general_tab(cx).into_any_element(),
            Tab::Hotkeys => self.hotkeys_tab(cx).into_any_element(),
            Tab::Lines => self.lines_tab(cx).into_any_element(),
            Tab::Connections => self.connections_tab(cx).into_any_element(),
        };
        // Тело прокручивается (вкладки выше высоты окна): stateful div + overflow_y_scroll.
        let body = div()
            .id("settings-body")
            .flex_1()
            .w_full()
            .overflow_y_scroll()
            .bg(rgba_from(p.shell, 1.0))
            .child(
                v_flex()
                    .w_full()
                    .p(design::ui_px(cx, 18.0))
                    .gap(design::ui_px(cx, 10.0))
                    .child(content),
            );

        // ── Подвал: Сохранить + статус ──────────────────────────────────────
        let status_el = match &self.status {
            Some((msg, err)) => div()
                .text_color(rgba_from(if *err { p.red } else { p.green }, 1.0))
                .child(msg.clone()),
            None => div(),
        };
        let footer = h_flex()
            .w_full()
            .h(design::fit_h_px(cx, 42.0, 14.0, 14.0))
            .gap(design::ui_px(cx, 10.0))
            .px(design::ui_px(cx, 10.0))
            .items_center()
            .bg(rgba_from(p.shell_high, 1.0))
            .border_t_1()
            .border_color(rgba_from(p.border, 1.0))
            .child(
                MoonButton::new("save")
                    .primary()
                    .small()
                    .width(110.0)
                    .label(t!("settings.save").to_string())
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx)))
                    .render(),
            )
            .child(status_el);

        v_flex()
            .size_full()
            .relative()
            .bg(rgba_from(p.shell, 1.0))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0))
            .text_color(rgba_from(p.text, 1.0))
            .child(settings_header(p, cx))
            .child(tabs)
            .child(body)
            .child(footer)
            .child(
                MoonWindowFrame::tool("settings-window-frame-hit", chrome_width)
                    .header_height(SETTINGS_HEADER_H)
                    .leading_inset(design::titlebar_leading_inset())
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            )
    }
}

fn settings_header(p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .id("settings-window-header")
        .relative()
        .flex_none()
        .w_full()
        .h(design::fit_h_px(cx, SETTINGS_HEADER_H, 14.0, 8.0))
        .justify_between()
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .bg(rgba_from(p.shell_high, 1.0))
        .border_b(px(1.0))
        .border_color(rgba_from(p.border, 1.0))
        .child(
            MoonWindowFrame::tool("settings-titlebar-title", 0.0)
                .title_cluster(t!("settings.title").to_string(), cx)
                .h_full()
                .flex_1()
                .min_w_0(),
        )
        .when(design::show_custom_window_controls(), |this| {
            this.child(
                MoonWindowFrame::tool("settings-window-frame-visual", 0.0)
                    .header_height(SETTINGS_HEADER_H)
                    .show_controls(true)
                    .visual_controls(cx),
            )
        })
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
pub fn open(backend: Entity<Backend>, owner: Option<AnyWindowHandle>, cx: &mut App) {
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
    // откатывается на дефолт — ищем монитор, содержащий сохранённую точку.
    let display_id = saved.and_then(|g| {
        let origin = point(px(g.x as f32), px(g.y as f32));
        cx.displays()
            .into_iter()
            .find(|d| d.bounds().contains(&origin))
            .map(|d| d.id())
    });
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
