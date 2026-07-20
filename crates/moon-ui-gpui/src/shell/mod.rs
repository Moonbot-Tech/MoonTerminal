//! Оболочка одной группы (Shell): одно ОС-окно = header + единый `DockArea` + статус-бар.
//! Вынесено из main.rs. `Backend` живёт в крейт-руте — доступ к его приватным полям из
//! этого модуля сохраняется (правило: потомок видит приватное предка).
//!
//! Разнесено по подмодулям:
//! - [`init`] — конструктор `Shell::new` (сборка дока + observe/subscribe-плумбинг);
//! - [`actions`] — дренаж запросов правок/тостов Engine-действий и хоткеи окна;
//! - [`render`] — сборка кадра (`impl Render`) и контент открытого попапа метрики;
//! - [`metrics`] — попапы торговых метрик тулбара (TP/SL/Lev) и коммит правок в ядро;
//! - [`docks`] — отцепление/возврат панелей и персист геометрии ОС-окна;
//! - [`status_bar`] — нижняя строка состояния (соединение/лицензия/диагностика).

mod actions;
mod core_settings;
mod docks;
mod init;
mod metrics;
mod render;
mod status_bar;
mod ticker;

use std::time::Instant;

use gpui::*;

use moon_ui::{DockArea, MoonInputState, MoonSliderState};

use moon_core::session::CoreId;

use crate::{Backend, controls};

/// Оболочка одной группы (= одно ОС-окно): header + единый `DockArea` + статус.
/// Весь контент — Dock-панели (чарт=center, детекты/ордер=right, нижние вкладки=
/// bottom), перетаскиваемые/отцепляемые. Header/статус — фикс. полосы вокруг дока.
pub(crate) struct Shell {
    backend: Entity<Backend>,
    group: String,
    dock: Entity<DockArea>,
    /// Время прошлого кадра и сглаженный fps рендера — для статус-бара (как egui host).
    last_frame: Option<Instant>,
    fps: f32,
    /// Троттл observe-notify бэкенда: Shell-рендер обновляет лишь статус-бар (book/cpu/
    /// fps), его дёргать чаще ~4 Гц человеку незачем, а он тащит top-down тяжёлый Orders.
    last_notify: Option<Instant>,
    /// Прошлое виденное значение follow (Live/Пауза). Смена = клик юзера → отражаем кнопку
    /// тулбара мгновенно, мимо 250мс-троттла (иначе Live↔Пауза «залипает» до ¼с).
    last_follow: bool,
    /// Прошлое виденное значение масштаба. Это тоже клик юзера, а не фоновая телеметрия:
    /// тулбар должен менять подпись сразу, даже при троттле Shell observe.
    last_price_scale: Option<f32>,
    /// Прошлая виденная ревизия выбора размера ордера (F1-F6). Клик юзера → выбранную
    /// кнопку отражаем мгновенно, мимо 250мс-троттла (иначе selected «залипает» до ¼с).
    last_order_size_rev: u64,
    /// Handle своего ОС-окна. Нужен event/observe callbacks, где нет `&mut Window`,
    /// но нельзя переносить window-bound операции в `render()`.
    window_handle: AnyWindowHandle,
    /// Инпут инлайн-редактирования значения кнопки размера ордера (дабл-клик в тулбаре).
    /// Один на Shell, переиспользуется для любой F-кнопки.
    size_input: Entity<MoonInputState>,
    /// Что сейчас редактируется в тулбаре: `(ядро, индекс F1-F6)`. None = не редактируем.
    size_edit: Option<(CoreId, usize)>,
    /// Инпут инлайн-редактирования процента fixed-sell пресета (дабл-клик по S-кнопке) + что
    /// редактируется `(ядро, индекс S1-S6)`. По Blur/Enter шлём `SetFixedSellPct` в ядро.
    sell_input: Entity<MoonInputState>,
    sell_edit: Option<(CoreId, usize)>,
    /// Слайдер+поле попапов торговых метрик (TP/SL/Lev). Персистентны (значения переживают
    /// рендеры; при открытии попапа сидируются значением активного ядра). Коммит в ядро —
    /// подписками в `new`. Один набор на окно: одновременно открыт лишь один попап. У TP два
    /// слайдера (1..100 и 100..900) под флаг `x_tmode` — границы в рантайме не меняются.
    tp_slider_normal: Entity<MoonSliderState>,
    tp_slider_ext: Entity<MoonSliderState>,
    /// Файн-слайдер TP (суб-процент через scalp). Пересоздаётся при открытии TP-попапа с
    /// диапазоном 0..основной_TP (границы слайдера в рантайме не меняются).
    tp_fine_slider: Entity<MoonSliderState>,
    sl_slider: Entity<MoonSliderState>,
    lev_slider: Entity<MoonSliderState>,
    tp_input: Entity<MoonInputState>,
    sl_input: Entity<MoonInputState>,
    lev_input: Entity<MoonInputState>,
    /// Слайдеры попапа настроек ядра: паника (= price_drop_level), глобальный TP, трейлинг, V-Stop.
    /// Границы фиксированы (CORE_*_BOUNDS); сидируются при открытии. Коммит — подписками в `new`.
    gtp_slider: Entity<MoonSliderState>,
    trailing_slider: Entity<MoonSliderState>,
    vstop_slider: Entity<MoonSliderState>,
    /// Числовые поля попапа настроек ядра: глобальный TP, %, трейлинг, %, V-Stop (целое %) и
    /// текст чёрного списка. Сидируются при открытии, коммит — подписками в `new` (Blur/Enter).
    gtp_input: Entity<MoonInputState>,
    trailing_input: Entity<MoonInputState>,
    vstop_input: Entity<MoonInputState>,
    blacklist_input: Entity<MoonInputState>,
    /// Отдельный multi-line стейт развёрнутого редактора чёрного списка (кнопка «…»).
    /// НЕ общий с `blacklist_input`: textarea необратимо переводит стейт в multi-line
    /// (single-line поле после этого рендерится узкой полоской). Текст синкается в тогле.
    blacklist_area: Entity<MoonInputState>,
    /// Поле поиска стратегии в списке «Стратегия алертов» попапа настроек ядра (сотни
    /// стратегий → фильтр + скролл; выпадашка внутри поповера ловится как клик-вне).
    def_strategy_input: Entity<MoonInputState>,
    /// The open toolbar-metric popup and the address from which its slider and field were seeded.
    ///
    /// One field keeps TP, SL, and leverage mutually exclusive. The address travels with the open
    /// metric because handlers resolve their live target when an event fires, not when the popup
    /// opened; comparing both prevents a stale editor from continuing to write to an old core or
    /// market after the visible context moves.
    /// `None` means all three anchored popovers are closed.
    open_metric_popup: Option<(controls::TradeMetric, controls::MetricTarget)>,
    /// Фокус корня окна — чтобы хоткеи (`on_key_down` на корне) ловились даже когда ничего
    /// другого не сфокусировано (пустой Main). Фокусируем на старте; клик по чарту/инпуту
    /// уводит фокус туда, но F-клавиши всплывают обратно к корню.
    focus: FocusHandle,
    /// Активно ли (в фокусе) это ОС-окно. Авто-закрытие Main по неактивности обновляет
    /// «активность» (`Backend::note_main_input`) ТОЛЬКО когда окно активно: иначе движение
    /// мыши над неактивным окном не должно сбрасывать таймер. Ставится observe_window_activation.
    window_active: bool,
    /// Открыт ли попап настроек ядра (MoonPopover у кнопки ⚙; контролируемый open —
    /// закрытие по клику вне делает сам popover, открытие сидирует поля).
    core_settings_open: bool,
    /// Стадия подтверждения «Отменить все ордера»: первый клик ставит флаг, второй — шлёт.
    core_settings_cancel_confirm: bool,
    /// Развёрнуто ли поле чёрного списка монет в попапе настроек ядра (кнопка «…»):
    /// свёрнуто — одна строка, развёрнуто — многострочный редактор фикс. высоты.
    core_settings_bl_expanded: bool,
    /// Открыт ли попап выбора источника тикера курса (клик по «1 BTC = …» в шапке).
    ticker_popup_open: bool,
    /// Был ли курсор уже над попапом тикера (авто-выход по уводу только после захода).
    ticker_popup_hovered: bool,
    /// Поле поиска монеты в попапе тикера (список «BTC - Bybit1» строится по значению).
    ticker_input: Entity<MoonInputState>,
}
