//! Хост-вид ОС-окна откреплённой чарт-вкладки (`DetachedChartHost`): шапка (поиск монеты +
//! масштаб + попап раскладки ⚙ + «закрыть все графики») над панелью чарт-стека. Сам пишет
//! геометрию окна и per-tab настройки в `charts.json` и просит репин по закрытию. Жизненный
//! цикл самого окна (создание/восстановление/репин) живёт в `windows.rs` (`impl ChartTabs`).

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonInput, MoonInputEvent, MoonInputState,
    MoonPalette, MoonWindowFrame, MoonWindowFrameControls, h_flex, v_flex,
};
use rust_i18n::t;
use std::time::Duration;

use super::{AddChartStack, chart_pane_label, coin_search, layout_popup};
use crate::Backend;
use crate::chart_persist::{self, StackLayoutMode};
use crate::design;
use moon_core::config::ChartBucket;
use moon_core::session::CoreId;

/// Хост-вид окна откреплённой чарт-вкладки: шапка (масштаб + «закрыть все графики») + панель.
/// Сам пишет геометрию окна в charts.json (`observe_window_bounds`) и просит репин по закрытию
/// (`on_release` → `chart_repin_request`, дренит ChartTabs).
pub(super) struct DetachedChartHost {
    panel: Entity<AddChartStack>,
    backend: Entity<Backend>,
    group: String,
    num: u32,
    bucket: ChartBucket,
    /// Можно ли сохранять геометрию из `observe_window_bounds`. У ВОССТАНОВЛЕННОГО окна сперва
    /// false: авто-размещение gpui на не-primary DPI читается со сдвигом ×scale, и пересохранять
    /// его НЕЛЬЗЯ (иначе позиция уезжает с каждым запуском). Армируется через ~1.5с — дальше
    /// пишем только реальные перемещения пользователя. У свежего детача — сразу true.
    persist_armed: bool,
    /// Логический размер для коррекции на ПЕРВОМ render восстановленного окна: gpui создаёт окно
    /// на primary, и `WM_DPICHANGED` при переезде на монитор с другим DPI пере-масштабирует
    /// РАЗМЕР (позиция уже верная) → форсим сохранённый логический размер один раз. None у детача.
    restore_size: Option<Size<Pixels>>,
    /// Кнопку окна из таскбара убираем `ITaskbarList::DeleteTab` на первых рендерах (когда окно
    /// уже показано и кнопка создана). Окно при этом остаётся обычным independent → FancyZones его
    /// видит. Несколько тиков — подстраховка от гонки «кнопка ещё не появилась».
    taskbar_hide_ticks: u8,
    /// In-scene попап настроек раскладки этой вкладки (кнопка ⚙). Не отдельное ОС-окно:
    /// chart text теперь лежит ниже обычной GPUI scene.
    layout_popup_open: bool,
    /// Был ли курсор внутри popup-а. Уход после первого входа закрывает popup и коммитит ввод.
    layout_popup_hovered: bool,
    /// Поле высоты режима Fit.
    layout_fit_input: Entity<MoonInputState>,
    /// Поле высоты режима Scroll.
    layout_scroll_input: Entity<MoonInputState>,
    /// Поле имени кастомной вкладки (в попапе ⚙, только если окно — откреплённая Custom-вкладка).
    custom_name_input: Entity<MoonInputState>,
    /// Поле ввода монеты (поиск) шапки окна; набор зависит от ядер bucket-а этого окна.
    coin_input: Entity<MoonInputState>,
    /// Текущий текст в поле монеты (зеркало `coin_input`).
    coin_query: String,
    /// Открыт ли список совпадений монеты.
    coin_popup_open: bool,
}

impl DetachedChartHost {
    pub(super) fn new(
        panel: Entity<AddChartStack>,
        backend: Entity<Backend>,
        group: String,
        num: u32,
        bucket: ChartBucket,
        restored: bool,
        restore_size: Option<Size<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Геометрия окна (causal bounds event) → charts.json («то же место» при загрузке).
        cx.observe_window_bounds(window, |this, window, cx| {
            this.persist_geometry(window, cx);
        })
        .detach();
        // Состав панели изменился (закрыли «×»/добавили монету) → если окно держит откреплённую
        // кастомную вкладку, пере-персист её тикеров (diff внутри, no-op для обычных окон).
        cx.observe(&panel, |this, _panel, cx| {
            this.persist_custom_coins_if_any(cx);
        })
        .detach();
        // Восстановленное окно не пишет стартовые bounds сразу: на не-primary DPI GPUI/Win32
        // могут прислать временную позицию/размер со scale-сдвигом. Это нельзя сохранять, иначе
        // окно будет уезжать на каждом запуске. Через короткое окно стабилизации снова разрешаем
        // обычный persist пользовательских move/resize. Свежий detach сохраняет геометрию сразу.
        if restored {
            cx.spawn(async move |this, cx| {
                let executor = cx.update(|cx| cx.background_executor().clone());
                executor.timer(Duration::from_millis(1500)).await;
                let _ = cx.update(|cx| {
                    this.update(cx, |this, _cx| {
                        this.persist_armed = true;
                        moon_core::detect_diag::line(&format!(
                            "[geom] n={} bucket={:?} persist armed after restore settle",
                            this.num, this.bucket
                        ));
                    })
                    .is_ok()
                });
            })
            .detach();
        }
        // Закрытие окна → репин в стрип (дренит ChartTabs). На выходе приложения запрос не
        // обработается → спека остаётся откреплённой → окно восстановится на след. запуске.
        let (g, n, c) = (group.clone(), num, bucket.clone());
        cx.on_release(move |this, app| {
            this.backend.update(app, |b, cx| {
                b.chart_repin_request.push((g.clone(), n, c.clone()));
                cx.notify();
            });
        })
        .detach();
        // Восстановить сохранённую раскладку + флаг стакана вкладки из charts.json в панель.
        let (group2, num2, bucket2) = (group.clone(), num, bucket.clone());
        let saved = backend.read(cx).chart_specs.iter().find_map(|s| {
            s.matches(&group2, num2, &bucket2).then(|| {
                (
                    s.layout_mode,
                    s.layout_height_fit,
                    s.layout_height_scroll,
                    s.orderbook_enabled,
                    s.show_zone,
                    s.auto_pin,
                    (s.cancel_buy_pos, s.panic_sell_pos),
                    s.price_axis_pos,
                    s.time_axis_visible,
                    s.line_labels,
                    s.cursor_labels,
                )
            })
        });
        if let Some((m, hf, hs, ob, sz, ap, action_pos, axis_pos, time_axis, line_labels, cursor_labels)) =
            saved
        {
            if m.is_some() || hf.is_some() || hs.is_some() {
                panel.update(cx, |p, pcx| p.set_layout(m, hf, hs, pcx));
            }
            if ob.is_some() {
                panel.update(cx, |p, pcx| p.set_orderbook_enabled(ob, pcx));
            }
            if sz.is_some() {
                panel.update(cx, |p, pcx| p.set_show_zone(sz, pcx));
            }
            if ap.is_some() {
                panel.update(cx, |p, pcx| p.set_auto_pin(ap, pcx));
            }
            if action_pos.0.is_some() || action_pos.1.is_some() {
                panel.update(cx, |p, pcx| {
                    p.set_action_btn_pos(action_pos.0, action_pos.1, pcx)
                });
            }
            if axis_pos.is_some() {
                panel.update(cx, |p, pcx| p.set_price_axis_pos(axis_pos, pcx));
            }
            if time_axis.is_some() {
                panel.update(cx, |p, pcx| p.set_time_axis_visible(time_axis, pcx));
            }
            if line_labels.is_some() {
                panel.update(cx, |p, pcx| p.set_line_labels(line_labels, pcx));
            }
            if cursor_labels.is_some() {
                panel.update(cx, |p, pcx| p.set_cursor_labels(cursor_labels, pcx));
            }
        }
        let layout_fit_input = cx.new(|cx| MoonInputState::new(window, cx));
        let layout_scroll_input = cx.new(|cx| MoonInputState::new(window, cx));
        cx.subscribe(
            &layout_fit_input,
            |this, _input, ev: &MoonInputEvent, cx| {
                if this.layout_popup_open
                    && matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. })
                {
                    this.commit_layout_popup(cx);
                }
            },
        )
        .detach();
        cx.subscribe(
            &layout_scroll_input,
            |this, _input, ev: &MoonInputEvent, cx| {
                if this.layout_popup_open
                    && matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. })
                {
                    this.commit_layout_popup(cx);
                }
            },
        )
        .detach();
        // Поле имени кастомной вкладки: коммит переименования по Blur/Enter.
        let custom_name_input = cx.new(|cx| MoonInputState::new(window, cx));
        cx.subscribe(
            &custom_name_input,
            |this, input, ev: &MoonInputEvent, cx| {
                if this.layout_popup_open
                    && matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. })
                {
                    let name = input.read(cx).value().to_string();
                    this.rename_custom(name, cx);
                }
            },
        )
        .detach();
        let coin_input = cx.new(|cx| {
            MoonInputState::new(window, cx).placeholder(t!("chart.coin.search").to_string())
        });
        cx.subscribe(&coin_input, |this, input, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                let value = input.read(cx).value().to_string();
                if this.coin_query != value {
                    this.coin_popup_open = !value.trim().is_empty();
                    this.coin_query = value;
                    cx.notify();
                }
            }
        })
        .detach();
        Self {
            panel,
            backend,
            group,
            num,
            bucket,
            persist_armed: !restored,
            restore_size,
            taskbar_hide_ticks: 8,
            layout_popup_open: false,
            layout_popup_hovered: false,
            layout_fit_input,
            layout_scroll_input,
            custom_name_input,
            coin_input,
            coin_query: String::new(),
            coin_popup_open: false,
        }
    }

    /// Это окно — откреплённая кастомная вкладка? (спек с `custom_coins`).
    fn is_custom(&self, cx: &App) -> bool {
        let (group, num, bucket) = (&self.group, self.num, &self.bucket);
        self.backend.read(cx).chart_specs.iter().any(|s| {
            s.matches(group, num, bucket) && s.custom_coins.is_some()
        })
    }

    /// Переименовать кастомную вкладку этого окна (поле имени в попапе ⚙): пишем `custom_label`
    /// в charts.json. Заголовок окна (через `chart_pane_label`) обновится на следующем render.
    fn rename_custom(&mut self, name: String, cx: &mut Context<Self>) {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        let (group, num, bucket) = (self.group.clone(), self.num, self.bucket.clone());
        self.backend.update(cx, |b, _| {
            if let Some(s) = b
                .chart_specs
                .iter_mut()
                .find(|s| s.matches(&group, num, &bucket))
            {
                s.custom_label = Some(name);
                b.chart_specs_dirty = true;
            }
        });
        cx.notify();
    }

    /// Совпадения поля монеты для этого окна (ядра bucket-а).
    fn coin_results(&self, cx: &App) -> Vec<(CoreId, String, String)> {
        coin_search::search(
            self.backend.read(cx),
            &self.group,
            Some(&self.bucket),
            &self.coin_query,
        )
    }

    /// Открыть выбранную монету в стеке этого окна.
    fn open_coin(&mut self, core: CoreId, market: String, cx: &mut Context<Self>) {
        self.panel.update(cx, |p, c| {
            p.add_coin(core, &market, coin_search::MANUAL_COIN_TTL_MS, c)
        });
        // Если это окно — откреплённая КАСТОМНАЯ вкладка, держим её список тикеров в charts.json
        // синхронным (добавили монету в окне → попадёт в персист и переживёт рестарт).
        self.persist_custom_coins_if_any(cx);
        cx.notify();
    }

    /// Если спек этого окна — кастомная вкладка (`custom_coins.is_some()`), переписать её тикеры
    /// из текущего состава панели — ТОЛЬКО при изменении (observe-колбэк зовётся часто). Для
    /// обычных AddToChart-окон — no-op.
    fn persist_custom_coins_if_any(&self, cx: &mut Context<Self>) {
        let (group, num, bucket) = (self.group.clone(), self.num, self.bucket.clone());
        let is_custom = {
            let specs = &self.backend.read(cx).chart_specs;
            specs.iter().any(|s| {
                s.matches(&group, num, &bucket) && s.custom_coins.is_some()
            })
        };
        if !is_custom {
            return;
        }
        let (coins, anchor, broom) = {
            let p = self.panel.read(cx);
            (p.coins(cx), p.compare_anchor(), p.compare_orderbook_only())
        };
        self.backend.update(cx, |b, _| {
            if let Some(s) = b
                .chart_specs
                .iter_mut()
                .find(|s| s.matches(&group, num, &bucket))
            {
                if s.custom_coins.as_deref() != Some(coins.as_slice())
                    || s.compare_anchor != anchor
                    || s.compare_orderbook_only != broom
                {
                    s.custom_coins = Some(coins);
                    s.compare_anchor = anchor;
                    s.compare_orderbook_only = broom;
                    b.chart_specs_dirty = true;
                }
            }
        });
    }

    fn clear_coin_search(&mut self, cx: &mut Context<Self>) {
        self.coin_query.clear();
        self.coin_popup_open = false;
        cx.notify();
    }

    /// Текущая per-tab раскладка панели этого окна: `(mode, height_fit, height_scroll)`.
    fn panel_layout(&self, cx: &App) -> (Option<StackLayoutMode>, Option<u16>, Option<u16>) {
        let p = self.panel.read(cx);
        (
            p.layout_mode(),
            p.layout_height_fit(),
            p.layout_height_scroll(),
        )
    }

    /// Открыть/закрыть in-scene popup раскладки этой вкладки.
    fn toggle_layout_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.layout_popup_open {
            self.close_layout_popup(true, cx);
        } else {
            self.seed_layout_popup_inputs(window, cx);
            self.layout_popup_open = true;
            self.layout_popup_hovered = false;
            cx.notify();
        }
    }

    fn seed_layout_popup_inputs(&self, window: &mut Window, cx: &mut Context<Self>) {
        // Эффективные значения вместо пустоты при None (Fit→0, Scroll→дефолт) — иначе после
        // рестарта поля высоты пустые, без цифр.
        let (_, hf, hs) = self.panel_layout(cx);
        let fit = hf.unwrap_or(0).to_string();
        let scroll = hs
            .unwrap_or(super::stack::DEFAULT_SCROLL_HEIGHT)
            .to_string();
        self.layout_fit_input
            .update(cx, |input, c| input.set_value(fit, window, c));
        self.layout_scroll_input
            .update(cx, |input, c| input.set_value(scroll, window, c));
        // Имя кастомной вкладки — для поля переименования.
        if self.is_custom(cx) {
            let name = chart_pane_label(&self.backend, &self.group, self.num, &self.bucket, cx);
            self.custom_name_input
                .update(cx, |input, c| input.set_value(name, window, c));
        }
    }

    fn read_layout_height(&self, mode: StackLayoutMode, cx: &App) -> Option<u16> {
        let (_, fit_fallback, scroll_fallback) = self.panel_layout(cx);
        let (input, fallback) = match mode {
            StackLayoutMode::Fit => (&self.layout_fit_input, fit_fallback),
            StackLayoutMode::Scroll => (&self.layout_scroll_input, scroll_fallback),
        };
        let value = input.read(cx).value().to_string();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }
        trimmed
            .parse::<u16>()
            .ok()
            .map(|raw| layout_popup::clamp_height(mode, raw))
            .or(fallback)
    }

    fn commit_layout_popup(&mut self, cx: &mut Context<Self>) {
        let (mode, _, _) = self.panel_layout(cx);
        let hf = self.read_layout_height(StackLayoutMode::Fit, cx);
        let hs = self.read_layout_height(StackLayoutMode::Scroll, cx);
        self.apply_layout(Some(mode.unwrap_or(StackLayoutMode::Fit)), hf, hs, cx);
    }

    fn close_layout_popup(&mut self, commit: bool, cx: &mut Context<Self>) {
        if !self.layout_popup_open {
            return;
        }
        if commit {
            self.commit_layout_popup(cx);
        }
        self.layout_popup_open = false;
        self.layout_popup_hovered = false;
        cx.notify();
    }

    fn apply_layout_to_all_charts(
        &mut self,
        mode: Option<StackLayoutMode>,
        height_fit: Option<u16>,
        height_scroll: Option<u16>,
        cx: &mut Context<Self>,
    ) {
        let group = self.group.clone();
        // Копируем ВСЕ настройки этого окна: + масштаб + галку стакана.
        let scale = self.panel.read(cx).scale();
        let orderbook = Some(self.panel.read(cx).orderbook_enabled().unwrap_or(true));
        let show_zone = Some(self.panel.read(cx).show_zone().unwrap_or(true));
        let auto_pin = Some(self.panel.read(cx).auto_pin().unwrap_or(false));
        let orientation = self.panel.read(cx).layout_orientation();
        let (cancel_pos, panic_pos) = {
            let (c, pp) = self.panel.read(cx).action_btn_pos();
            (Some(c.unwrap_or_default()), Some(pp.unwrap_or_default()))
        };
        let price_axis_pos = Some(self.panel.read(cx).price_axis_pos().unwrap_or_default());
        let time_axis_visible = Some(self.panel.read(cx).time_axis_visible().unwrap_or(true));
        let line_labels = Some(self.panel.read(cx).line_labels().unwrap_or(true));
        let cursor_labels = Some(self.panel.read(cx).cursor_labels().unwrap_or(true));
        self.backend.update(cx, |bk, bcx| {
            bk.chart_apply_all.push(crate::ChartApplyAll {
                group,
                include_main: false,
                mode,
                height_fit,
                height_scroll,
                scale,
                orderbook,
                show_zone,
                auto_pin,
                orientation,
                cancel_pos,
                panic_pos,
                price_axis_pos,
                time_axis_visible,
                line_labels,
                cursor_labels,
            });
            bcx.notify();
        });
    }

    /// Найти спеку этой вкладки в `chart_specs` (по group/num/bucket) и применить `f`; если её
    /// ещё нет — создать заготовку и применить `f` к ней. Везде далее проставляет `dirty`. Один
    /// общий апсёрт для всех `apply_*` этого окна (зеркало `ChartTabs::upsert_spec`).
    fn upsert_spec(
        &self,
        cx: &mut Context<Self>,
        num: u32,
        bucket: &ChartBucket,
        f: impl FnOnce(&mut chart_persist::ChartTabSpec),
    ) {
        let group = self.group.clone();
        self.backend.update(cx, |bk, _| {
            chart_persist::upsert(&mut bk.chart_specs, &group, num, bucket, f);
            bk.chart_specs_dirty = true;
        });
    }

    /// Сменить ориентацию (верт/гор) панели этого окна + persist.
    fn apply_orientation(
        &mut self,
        orientation: crate::chart_persist::StackOrientation,
        cx: &mut Context<Self>,
    ) {
        self.panel
            .update(cx, |p, c| p.set_orientation(Some(orientation), c));
        let bucket = self.bucket.clone();
        self.upsert_spec(cx, self.num, &bucket, move |s| {
            s.layout_orientation = Some(orientation);
        });
        cx.notify();
    }

    /// Применить раскладку к панели вкладки и сохранить в charts.json.
    fn apply_layout(
        &mut self,
        mode: Option<StackLayoutMode>,
        height_fit: Option<u16>,
        height_scroll: Option<u16>,
        cx: &mut Context<Self>,
    ) {
        self.panel
            .update(cx, |p, c| p.set_layout(mode, height_fit, height_scroll, c));
        let bucket = self.bucket.clone();
        self.upsert_spec(cx, self.num, &bucket, move |s| {
            s.layout_mode = mode;
            s.layout_height_fit = height_fit;
            s.layout_height_scroll = height_scroll;
        });
        cx.notify();
    }

    /// Вкл/выкл стакан этой вкладки + persist + пересбор набора рынков, которым нужен стакан.
    fn apply_orderbook(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.panel
            .update(cx, |p, c| p.set_orderbook_enabled(Some(enabled), c));
        let bucket = self.bucket.clone();
        self.upsert_spec(cx, self.num, &bucket, move |s| {
            s.orderbook_enabled = Some(enabled);
        });
        // Пересобрать набор рынков, которым нужен стакан (мог измениться спрос).
        self.backend.update(cx, |b, _| b.rebuild_orderbook_wanted());
        cx.notify();
    }

    /// Вкл/выкл заливку зоны управления этой вкладки + persist.
    fn apply_show_zone(&mut self, show: bool, cx: &mut Context<Self>) {
        self.panel.update(cx, |p, c| p.set_show_zone(Some(show), c));
        let bucket = self.bucket.clone();
        self.upsert_spec(cx, self.num, &bucket, move |s| {
            s.show_zone = Some(show);
        });
        cx.notify();
    }

    /// Вкл/выкл авто-пин при ордере этой вкладки + persist.
    fn apply_auto_pin(&mut self, on: bool, cx: &mut Context<Self>) {
        self.panel.update(cx, |p, c| p.set_auto_pin(Some(on), c));
        let bucket = self.bucket.clone();
        self.upsert_spec(cx, self.num, &bucket, move |s| {
            s.auto_pin = Some(on);
        });
        cx.notify();
    }

    /// Позиция кнопки Cancel Buy этого окна + persist (Panic Sell не трогаем).
    fn apply_cancel_pos(&mut self, pos: chart_persist::ChartBtnPos, cx: &mut Context<Self>) {
        let (_, panic) = self.panel.read(cx).action_btn_pos();
        self.apply_action_pos(Some(pos), panic, cx);
    }

    /// Позиция кнопки Panic Sell этого окна + persist (Cancel Buy не трогаем).
    fn apply_panic_pos(&mut self, pos: chart_persist::ChartBtnPos, cx: &mut Context<Self>) {
        let (cancel, _) = self.panel.read(cx).action_btn_pos();
        self.apply_action_pos(cancel, Some(pos), cx);
    }

    fn apply_action_pos(
        &mut self,
        cancel: Option<chart_persist::ChartBtnPos>,
        panic: Option<chart_persist::ChartBtnPos>,
        cx: &mut Context<Self>,
    ) {
        self.panel
            .update(cx, |p, c| p.set_action_btn_pos(cancel, panic, c));
        let bucket = self.bucket.clone();
        self.upsert_spec(cx, self.num, &bucket, move |s| {
            s.cancel_buy_pos = cancel;
            s.panic_sell_pos = panic;
        });
        cx.notify();
    }

    /// Положение оси цен этого окна + persist.
    fn apply_price_axis_pos(&mut self, pos: chart_persist::PriceAxisPos, cx: &mut Context<Self>) {
        self.panel
            .update(cx, |p, c| p.set_price_axis_pos(Some(pos), c));
        let bucket = self.bucket.clone();
        self.upsert_spec(cx, self.num, &bucket, move |s| {
            s.price_axis_pos = Some(pos);
        });
        cx.notify();
    }

    /// Видимость оси времени этого окна + persist.
    fn apply_time_axis_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.panel
            .update(cx, |p, c| p.set_time_axis_visible(Some(visible), c));
        let bucket = self.bucket.clone();
        self.upsert_spec(cx, self.num, &bucket, move |s| {
            s.time_axis_visible = Some(visible);
        });
        cx.notify();
    }

    /// Видимость подписей у линий этого окна + persist.
    fn apply_line_labels(&mut self, show: bool, cx: &mut Context<Self>) {
        self.panel.update(cx, |p, c| p.set_line_labels(Some(show), c));
        let bucket = self.bucket.clone();
        self.upsert_spec(cx, self.num, &bucket, move |s| {
            s.line_labels = Some(show);
        });
        cx.notify();
    }

    /// Видимость подписей у перекрестия этого окна + persist.
    fn apply_cursor_labels(&mut self, show: bool, cx: &mut Context<Self>) {
        self.panel
            .update(cx, |p, c| p.set_cursor_labels(Some(show), c));
        let bucket = self.bucket.clone();
        self.upsert_spec(cx, self.num, &bucket, move |s| {
            s.cursor_labels = Some(show);
        });
        cx.notify();
    }

    fn persist_geometry(&mut self, window: &Window, cx: &mut Context<Self>) {
        // У восстановленного окна сохранение задержано до `persist_armed`: не даём стартовому
        // авто-размещению GPUI/Win32 перезаписать сохранённую позицию DPI-мусором.
        if !self.persist_armed {
            return;
        }
        let Some((x, y, w, h)) = crate::windowing::window_geom(window) else {
            moon_core::detect_diag::line(&format!(
                "[geom] n={} НЕ Windowed → геометрия не сохранена",
                self.num
            ));
            return;
        };
        let geom = chart_persist::WinGeom { x, y, w, h };
        let (group, num, bucket) = (self.group.clone(), self.num, self.bucket.clone());
        let found = self.backend.update(cx, |bk, _| {
            if let Some(s) = bk
                .chart_specs
                .iter_mut()
                .find(|s| s.matches(&group, num, &bucket))
            {
                let cur = s.detached.map(|g| (g.x, g.y, g.w, g.h));
                if cur != Some((geom.x, geom.y, geom.w, geom.h)) {
                    s.detached = Some(geom);
                    bk.chart_specs_dirty = true;
                }
                true
            } else {
                false
            }
        });
        moon_core::detect_diag::line(&format!(
            "[geom] n={num} bucket={bucket:?} → x={} y={} w={} h={} (spec_found={found})",
            geom.x, geom.y, geom.w, geom.h
        ));
    }
}

impl Render for DetachedChartHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Коррекция размера восстановленного окна (один раз): окно уже на целевом мониторе с
        // верным scale → форсим сохранённый логический размер, перебивая DPICHANGED-сжатие.
        if let Some(sz) = self.restore_size.take() {
            window.resize(sz);
        }
        // Убрать кнопку из таскбара (DeleteTab), оставив окно independent → FancyZones его видит.
        // Несколько первых рендеров — на случай, если кнопка появляется чуть позже показа окна.
        if self.taskbar_hide_ticks > 0 {
            crate::windowing::hide_window_from_taskbar(window);
            self.taskbar_hide_ticks -= 1;
        }
        let p = MoonPalette::active(cx);
        // Масштаб — СВОЙ у этой панели (по-вкладочно), правится прямо в неё.
        let scale = self.panel.read(cx).scale();
        let panel = self.panel.clone();
        let close_all_panel = self.panel.clone();
        let title = chart_pane_label(&self.backend, &self.group, self.num, &self.bucket, cx);
        let frame = MoonWindowFrame::detached_chart("detached-chart-window-frame", 0.0)
            .header_height(34.0)
            .controls(MoonWindowFrameControls::Close)
            .show_controls(design::show_custom_window_controls());
        let popup_open = self.layout_popup_open;
        let layout_popup = self.layout_popup_open.then(|| {
            let mode = self.panel_layout(cx).0.unwrap_or(StackLayoutMode::Fit);
            let orientation = self
                .panel
                .read(cx)
                .layout_orientation()
                .unwrap_or(crate::chart_persist::StackOrientation::Vertical);
            let orderbook_enabled = self.panel.read(cx).orderbook_enabled().unwrap_or(true);
            let show_zone = self.panel.read(cx).show_zone().unwrap_or(true);
            let auto_pin = self.panel.read(cx).auto_pin().unwrap_or(false);
            let (cancel_pos, panic_pos) = {
                let (c, pp) = self.panel.read(cx).action_btn_pos();
                (c.unwrap_or_default(), pp.unwrap_or_default())
            };
            let price_axis_pos = self.panel.read(cx).price_axis_pos().unwrap_or_default();
            let time_axis_visible = self.panel.read(cx).time_axis_visible().unwrap_or(true);
            let line_labels = self.panel.read(cx).line_labels().unwrap_or(true);
            let cursor_labels = self.panel.read(cx).cursor_labels().unwrap_or(true);
            let is_custom = self.is_custom(cx);
            let pick_entity = cx.entity();
            let all_entity = cx.entity();
            let ob_entity = cx.entity();
            let sz_entity = cx.entity();
            let ap_entity = cx.entity();
            let or_entity = cx.entity();
            let cbp_entity = cx.entity();
            let psp_entity = cx.entity();
            let pap_entity = cx.entity();
            let tav_entity = cx.entity();
            let ll_entity = cx.entity();
            let cl_entity = cx.entity();
            let hover_entity = cx.entity();
            let size = layout_popup::content_size(cx, is_custom);
            div()
                .id("detached-chart-layout-popup-scene")
                .absolute()
                .right(px(6.0))
                .top(px(38.0))
                .w(size.width)
                .h(size.height)
                .on_mouse_down(MouseButton::Left, |_, _window, app| {
                    app.stop_propagation();
                })
                .on_hover(move |hovered, _window, app| {
                    hover_entity.update(app, |this, cx| {
                        if *hovered {
                            this.layout_popup_hovered = true;
                        } else if this.layout_popup_hovered {
                            this.close_layout_popup(true, cx);
                        }
                    });
                })
                .child(layout_popup::render_layout_popup(
                    "detached-chart-layout",
                    mode,
                    orientation,
                    is_custom.then_some(&self.custom_name_input),
                    &self.layout_fit_input,
                    &self.layout_scroll_input,
                    orderbook_enabled,
                    show_zone,
                    auto_pin,
                    cancel_pos,
                    panic_pos,
                    price_axis_pos,
                    time_axis_visible,
                    line_labels,
                    cursor_labels,
                    p,
                    cx,
                    move |mode, app| {
                        pick_entity.update(app, |this, cx| {
                            let hf = this.read_layout_height(StackLayoutMode::Fit, cx);
                            let hs = this.read_layout_height(StackLayoutMode::Scroll, cx);
                            this.apply_layout(Some(mode), hf, hs, cx);
                        });
                    },
                    t!("chart.layout.apply_all_charts").to_string(),
                    move |app| {
                        all_entity.update(app, |this, cx| {
                            let (mode, _, _) = this.panel_layout(cx);
                            let hf = this.read_layout_height(StackLayoutMode::Fit, cx);
                            let hs = this.read_layout_height(StackLayoutMode::Scroll, cx);
                            this.apply_layout_to_all_charts(
                                Some(mode.unwrap_or(StackLayoutMode::Fit)),
                                hf,
                                hs,
                                cx,
                            );
                        });
                    },
                    move |checked, app| {
                        ob_entity.update(app, |this, cx| this.apply_orderbook(checked, cx));
                    },
                    move |checked, app| {
                        sz_entity.update(app, |this, cx| this.apply_show_zone(checked, cx));
                    },
                    move |checked, app| {
                        ap_entity.update(app, |this, cx| this.apply_auto_pin(checked, cx));
                    },
                    move |app| {
                        or_entity.update(app, |this, cx| {
                            use crate::chart_persist::StackOrientation as O;
                            let next = match this
                                .panel
                                .read(cx)
                                .layout_orientation()
                                .unwrap_or(O::Vertical)
                            {
                                O::Vertical => O::Horizontal,
                                O::Horizontal => O::Vertical,
                            };
                            this.apply_orientation(next, cx);
                        });
                    },
                    move |pos, app| {
                        cbp_entity.update(app, |this, cx| this.apply_cancel_pos(pos, cx));
                    },
                    move |pos, app| {
                        psp_entity.update(app, |this, cx| this.apply_panic_pos(pos, cx));
                    },
                    move |pos, app| {
                        pap_entity.update(app, |this, cx| this.apply_price_axis_pos(pos, cx));
                    },
                    move |checked, app| {
                        tav_entity.update(app, |this, cx| this.apply_time_axis_visible(checked, cx));
                    },
                    move |checked, app| {
                        ll_entity.update(app, |this, cx| this.apply_line_labels(checked, cx));
                    },
                    move |checked, app| {
                        cl_entity.update(app, |this, cx| this.apply_cursor_labels(checked, cx));
                    },
                ))
        });
        let layout_dismiss = self.layout_popup_open.then(|| {
            let entity = cx.entity();
            div()
                .id("detached-chart-layout-popup-dismiss")
                .absolute()
                .inset_0()
                .on_mouse_down(MouseButton::Left, move |_, _window, app| {
                    entity.update(app, |this, cx| this.close_layout_popup(true, cx));
                    app.stop_propagation();
                })
        });
        // Поле ввода монеты (поиск) шапки + список совпадений. Список рисуем на уровне v_flex
        // (после тела), иначе тело окна (paint-порядок ниже) перекроет выпадашку из шапки.
        let coin_search_el = div().w(px(80.0)).child(
            MoonInput::new("detached-coin-search")
                .state(&self.coin_input)
                .cleanable(true)
                .small(),
        );
        let coin_popup = self.coin_popup_open.then(|| {
            let results = self.coin_results(cx);
            let view = cx.entity();
            let input = self.coin_input.clone();
            coin_search::render_popup(
                "detached-coin",
                results,
                &std::collections::HashSet::new(),
                false,
                p,
                cx,
                move |core, market, window, app| {
                    view.update(app, |this, cx| this.open_coin(core, market, cx));
                    input.update(app, |inp, c| {
                        inp.set_value(SharedString::default(), window, c)
                    });
                    view.update(app, |this, cx| this.clear_coin_search(cx));
                },
                |_core, _market, _app| {},
                |_app| {},
            )
            .absolute()
            .right(px(6.0))
            .top(px(38.0))
        });
        // Перехватчик клика вне списка — только ниже шапки (top 34), чтобы не блокировать само поле.
        let coin_dismiss = self.coin_popup_open.then(|| {
            let entity = cx.entity();
            div()
                .id("detached-coin-dismiss")
                .absolute()
                .top(px(34.0))
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0))
                .on_mouse_down(MouseButton::Left, move |_, _w, app| {
                    entity.update(app, |this, cx| this.clear_coin_search(cx));
                    app.stop_propagation();
                })
        });
        // Шапка — ТОЛЬКО у выносных окон вкладок (в основном доке её нет): масштаб слева,
        // «закрыть все графики» справа.
        v_flex()
            .size_full()
            .relative()
            .child(
                h_flex()
                    .h(design::fit_h_px(cx, 34.0, 13.0, 10.5))
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .pl(design::ui_px(cx, design::titlebar_leading_inset()))
                    .pr(design::ui_px(cx, 6.0))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.shell_high))
                    .child(
                        frame
                            .title_cluster(title, cx)
                            .h_full()
                            .flex_1()
                            .min_w_0()
                            .items_center(),
                    )
                    .child(coin_search_el)
                    .child(crate::controls::scale_dropdown_for_add_stack(
                        scale,
                        panel.clone(),
                        p,
                    ))
                    .child({
                        let entity = cx.entity();
                        div().relative().child(
                            MoonButton::new("detached-layout-settings")
                                .label("⚙")
                                .tooltip(t!("chart.layout.tip").to_string())
                                .size(MoonButtonSize::Micro)
                                .variant(if popup_open {
                                    MoonButtonVariant::Blue
                                } else {
                                    MoonButtonVariant::Ghost
                                })
                                .selected(popup_open)
                                .on_click(move |_, window, app| {
                                    entity.update(app, |this, cx| {
                                        this.toggle_layout_popup(window, cx)
                                    });
                                })
                                .render(),
                        )
                    })
                    .child(
                        MoonButton::new("detached-close-all")
                            .label("🗑")
                            .tooltip(t!("chartwin.clear").to_string())
                            .size(MoonButtonSize::Micro)
                            .variant(MoonButtonVariant::Ghost)
                            .on_click(move |_, _w, app| {
                                close_all_panel.update(app, |p, cx| p.close_all_panes(cx));
                            })
                            .render(),
                    )
                    .when(design::show_custom_window_controls(), |this| {
                        this.child(frame.visual_controls(cx))
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    // БЕЗ .bg(): own-pass чарта и его text layer лежат under-scene, любой
                    // непрозрачный фон тела перекроет график. Подложку под/между чартами закрывает
                    // тёмный clear окна (правка форка MoonUI), белого нет.
                    .child(self.panel.clone()),
            )
            .children(coin_dismiss)
            .children(coin_popup)
            .children(layout_dismiss)
            .children(layout_popup)
    }
}
