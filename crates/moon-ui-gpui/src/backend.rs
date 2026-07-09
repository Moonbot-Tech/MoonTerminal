//! Методы общего backend-состояния приложения ([`Backend`]). Сам struct объявлен в
//! `main.rs` (крейт-рут), чтобы его приватные поля были видны всем модулям крейта
//! (правило: потомок видит приватное предка). Вынесено из `main.rs` точь-в-точь;
//! методы получили `pub(crate)` (в корне приватное = видно всему крейту, здесь — нет).

use std::time::{Duration, Instant};

use gpui::Context;

use crate::Backend;
use crate::chartdx::ChartDataHandle;
use moon_core::session::CoreId;

impl Backend {
    pub(crate) fn manual_order_size_state(&self, core: CoreId) -> ([f64; 6], usize) {
        const DEFAULT_SEL: usize = 2;

        let base = self.session.core_base(core).unwrap_or("");
        let server = self.config.servers.iter().find(|s| s.id == core);
        let sizes = server
            .map(|s| s.order_sizes_or_default(base))
            .unwrap_or_else(|| moon_core::config::servers::default_order_sizes(base));
        // Выбор: рантайм-карта → персист конфига (последний выбор прошлого запуска) → F3.
        let sel = self
            .order_size_sel
            .get(&core)
            .copied()
            .or_else(|| server.and_then(|s| s.order_size_sel))
            .unwrap_or(DEFAULT_SEL)
            .min(sizes.len().saturating_sub(1));
        (sizes, sel)
    }

    /// Выбрать пресет размера ордера (клик F1-F6 / хоткей): рантайм-карта + персист в
    /// конфиг сервера (дебаунс-сейв дренажа через `config_dirty`, как значения пресетов).
    pub(crate) fn set_order_size_sel(&mut self, core: CoreId, ix: usize) {
        if ix >= 6 {
            return;
        }
        self.order_size_sel.insert(core, ix);
        if let Some(s) = self.config.servers.iter_mut().find(|s| s.id == core) {
            if s.order_size_sel != Some(ix) {
                s.order_size_sel = Some(ix);
                self.config_dirty = true;
            }
        }
    }

    pub(crate) fn manual_order_size(&self, core: CoreId) -> f64 {
        let (sizes, sel) = self.manual_order_size_state(core);
        sizes[sel]
    }

    /// Прогнозный размер ручного ордера (s1-s6 активного ядра) в USD: размер в базовой валюте
    /// аккаунта × курс базы→USD. None — нет ядра/размера/курса. Для подписи на перекрестии чарта.
    pub(crate) fn prospective_order_usd(&self, core: CoreId) -> Option<f64> {
        let size = self.manual_order_size(core);
        if !(size > 0.0) {
            return None;
        }
        let base = self.session.core_base(core).unwrap_or("");
        let rate = self.session.market_source().currency_usd_rate(core, base)?;
        (rate > 0.0).then_some(size * rate)
    }

    /// Значение пресета размера `ix` (F1-F6) ядра — из конфига (или дефолт по базе).
    pub(crate) fn order_size_value(&self, core: CoreId, ix: usize) -> f64 {
        let (sizes, _) = self.manual_order_size_state(core);
        sizes[ix.min(sizes.len().saturating_sub(1))]
    }

    /// Записать значение пресета размера `ix` ядра в конфиг (правка колесом/инпутом). На диск
    /// НЕ сохраняем сразу — ставим `config_dirty`, дренаж сделает дебаунс-сейв.
    pub(crate) fn set_order_size_value(&mut self, core: CoreId, ix: usize, v: f64) {
        if ix >= 6 || !(v > 0.0) {
            return;
        }
        let base = self.session.core_base(core).unwrap_or("").to_string();
        if let Some(s) = self.config.servers.iter_mut().find(|s| s.id == core) {
            let mut arr = s
                .order_sizes
                .unwrap_or_else(|| moon_core::config::servers::default_order_sizes(&base));
            arr[ix] = v;
            s.order_sizes = Some(arr);
            self.config_dirty = true;
        }
    }

    /// Текущий видимый процент fixed-sell пресета `ix` (S1-S6) ядра: оптимистичный локальный
    /// кэш, если есть (свежая правка колесом/инпутом), иначе значение из снимка ClientSettings.
    pub(crate) fn fixed_sell_pct(&self, core: CoreId, ix: usize) -> f64 {
        if let Some(v) = self.sell_pct_local.get(&(core, ix)) {
            return *v;
        }
        self.session
            .store()
            .core(core)
            .and_then(|d| d.client_settings.as_ref())
            .map(|s| s.fixed_sell_pcts[ix.min(5)])
            .unwrap_or(0.0)
    }

    /// Записать оптимистичный локальный процент fixed-sell (живой дисплей до эха ядра).
    pub(crate) fn set_fixed_sell_pct_local(&mut self, core: CoreId, ix: usize, v: f64) {
        self.sell_pct_local.insert((core, ix), v);
    }

    /// Локальный кэш `(core,ix)`, иначе `fallback` (значение ядра) — для дисплея sell-полосы.
    pub(crate) fn fixed_sell_pct_with(&self, core: CoreId, ix: usize, fallback: f64) -> f64 {
        self.sell_pct_local
            .get(&(core, ix))
            .copied()
            .unwrap_or(fallback)
    }

    pub(crate) fn set_fixed_sell_slot_local(&mut self, core: CoreId, slot: Option<usize>) {
        self.sell_slot_local.insert(core, slot);
    }

    pub(crate) fn fixed_sell_slot_with(
        &self,
        core: CoreId,
        fallback: Option<usize>,
    ) -> Option<usize> {
        self.sell_slot_local.get(&core).copied().unwrap_or(fallback)
    }

    pub(crate) fn fixed_sell_mode_with(&self, core: CoreId, fallback: bool) -> bool {
        self.sell_slot_local
            .get(&core)
            .map(|slot| slot.is_some())
            .unwrap_or(fallback)
    }

    /// Оптимистичный локальный выбор ручной стратегии (живой отклик до echo ядра).
    pub(crate) fn set_manual_strat_local(&mut self, core: CoreId, on: bool, id: u64) {
        self.manual_strat_local.insert(core, (on, id));
    }

    /// Эффективное состояние ручной стратегии `(вкл, id)` ядра: локальный кэш поверх
    /// снимка ClientSettings; ни того ни другого нет → (false, 0).
    pub(crate) fn manual_strat_state(&self, core: CoreId) -> (bool, u64) {
        if let Some(v) = self.manual_strat_local.get(&core) {
            return *v;
        }
        self.session
            .store()
            .core(core)
            .and_then(|d| d.client_settings.as_ref())
            .map(|s| (s.use_manual_strategy, s.manual_strategy_id))
            .unwrap_or((false, 0))
    }

    /// Взведён ли «паник-селл» по (ядро, рынок) — для подсветки кнопки Panic Sell.
    pub(crate) fn is_panic_armed(&self, core: CoreId, market: &str) -> bool {
        let snapshot_armed = self.session.store().core(core).is_some_and(|data| {
            data.order_lines
                .iter_market(market)
                .any(|order| order.closed_ms.is_none() && order.panic_sell)
        });
        if snapshot_armed {
            return true;
        }
        self.panic_armed.contains(&(core, market.to_string()))
    }

    /// Тоггл «паник-селл» по рынку: текущее состояние берём из снапшота ордеров, локальный флаг —
    /// только optimistic echo до прихода следующего апдейта ядра.
    pub(crate) fn toggle_panic_sell(&mut self, core: CoreId, market: String) -> bool {
        let key = (core, market.clone());
        let on = !self.is_panic_armed(core, &market);
        if let Err(error) = self.session.panic_sell_market(core, market, on) {
            log::warn!("panic sell market failed: {error:#}");
            return !on;
        }
        if on {
            self.panic_armed.insert(key);
        } else {
            self.panic_armed.remove(&key);
        }
        on
    }

    /// Отменить ожидающие buy-ордера по ВСЕМ рынкам ядра (хоткей «cancel all buys»). Берём
    /// удержанный снимок ордеров, отбираем рынки с pending buy (не шорт, не исполнен, не
    /// закрыт) и шлём по каждому `cancel_market_buys`. Возвращает число задействованных рынков.
    pub(crate) fn cancel_all_buys_for_core(&self, core: CoreId) -> usize {
        let markets: Vec<String> = self
            .session
            .store()
            .core(core)
            .map(|cd| {
                let mut set = std::collections::BTreeSet::new();
                for o in &cd.orders {
                    if !o.is_short && o.pending && !o.job_is_done {
                        set.insert(o.market.clone());
                    }
                }
                set.into_iter().collect()
            })
            .unwrap_or_default();
        let mut n = 0;
        for m in markets {
            n += self.cancel_buy_orders(core, &m);
        }
        n
    }

    /// Сторона позиции рынка (для join_sells): true = short. Берём из удержанного снимка
    /// ордеров рынка (первый ордер рынка), иначе — long по умолчанию.
    pub(crate) fn market_position_short(&self, core: CoreId, market: &str) -> bool {
        self.session
            .store()
            .core(core)
            .and_then(|cd| cd.orders.iter().find(|o| o.market == market).map(|o| o.is_short))
            .unwrap_or(false)
    }

    pub(crate) fn cancel_buy_orders(&self, core: CoreId, market: &str) -> usize {
        match self.session.cancel_market_buys(core, market.to_string()) {
            Ok(()) => {
                log::info!("cancel buy: requested market buys for core={core} market={market}");
                1
            }
            Err(err) => {
                log::warn!("cancel buy failed: core={core} market={market}: {err:#}");
                0
            }
        }
    }

    pub(crate) fn register_chart_consumer(&mut self, chart: ChartDataHandle) {
        if self
            .chart_consumers
            .iter()
            .any(|existing| existing == &chart)
        {
            return;
        }
        self.chart_consumers.push(chart);
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub(crate) fn register_debug_main_chart(&mut self, group: String, chart: ChartDataHandle) {
        self.debug_main_chart_handles.insert(group, chart);
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub(crate) fn debug_main_chart_shift_hz(&self, group: &str) -> Option<f32> {
        self.debug_main_chart_handles
            .get(group)
            .filter(|chart| chart.is_alive())
            .and_then(ChartDataHandle::camera_shift_hz)
    }

    pub(crate) fn live_chart_consumers(&mut self) -> Vec<ChartDataHandle> {
        self.chart_consumers.retain(ChartDataHandle::is_alive);
        self.chart_consumers.clone()
    }

    pub(crate) fn set_main_chart_target(&mut self, group: &str, target: Option<(CoreId, String)>) {
        // Открытие фуллскрином чарта ДРУГОГО ядра = «явная смена» → сбрасываем sticky-override,
        // чтобы шапка вернулась к авто-следованию за фуллскрином. Тот же core / снятие фуллскрина
        // override не трогают.
        if let Some((new_core, _)) = &target {
            let prev_core = self.main_chart_targets.get(group).map(|(c, _)| *c);
            if prev_core != Some(*new_core) {
                self.trade_core_override.remove(group);
            }
        }
        match target {
            Some(target) => {
                self.main_chart_targets.insert(group.to_string(), target);
            }
            None => {
                self.main_chart_targets.remove(group);
            }
        }
    }

    pub(crate) fn main_chart_target(&self, group: &str) -> Option<(CoreId, String)> {
        self.main_chart_targets.get(group).cloned()
    }

    /// Опубликовать монеты, открытые в стеке Main группы (из `MainChartStack`).
    pub(crate) fn set_main_open_markets(&mut self, group: &str, markets: Vec<(CoreId, String)>) {
        if markets.is_empty() {
            self.main_open_markets.remove(group);
        } else {
            self.main_open_markets.insert(group.to_string(), markets);
        }
    }

    /// Монеты, открытые в стеке Main группы (для подсветки/сортировки в «Ордерах»).
    pub(crate) fn main_open_markets(&self, group: &str) -> &[(CoreId, String)] {
        self.main_open_markets
            .get(group)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Отметить «активный» ввод в главном окне группы (движение мыши при сфокусированном окне).
    /// Сбрасывает таймер авто-закрытия Main по неактивности. Зовётся из Shell on_mouse_move
    /// ТОЛЬКО когда окно активно.
    pub(crate) fn note_main_input(&mut self, group: &str) {
        self.last_main_input
            .insert(group.to_string(), std::time::Instant::now());
    }

    /// Время последнего активного ввода в главном окне группы (для авто-закрытия по неактивности).
    pub(crate) fn main_input_at(&self, group: &str) -> Option<std::time::Instant> {
        self.last_main_input.get(group).copied()
    }

    /// Локальный тоггл «исключить ЧС из дельт» активного ядра (см. поле `exclude_bl_delta`).
    pub(crate) fn exclude_bl_delta(&self, core: CoreId) -> bool {
        self.exclude_bl_delta.get(&core).copied().unwrap_or(false)
    }

    /// Запомнить выбор «исключить ЧС из дельт» для ядра (отправку команды делает вызывающий).
    pub(crate) fn set_exclude_bl_delta(&mut self, core: CoreId, on: bool) {
        self.exclude_bl_delta.insert(core, on);
    }

    /// Авто-закрытие Main по неактивности, сек (config; 0 = выключено).
    pub(crate) fn main_idle_close_secs(&self) -> u32 {
        self.preview
            .as_ref()
            .unwrap_or(&self.config)
            .main_idle_close_secs
    }

    /// Активное торговое ядро группы для шапки/тулбара: sticky-override (ручной выбор в
    /// шапке), если он ещё валиден (ядро в группе), иначе ядро фуллскрин-чарта, иначе первое
    /// ядро группы. Тулбар не должен превращаться в прочерки только потому, что чарт ещё не открыт.
    pub(crate) fn active_trade_core(&self, group: &str) -> Option<CoreId> {
        if let Some(&core) = self.trade_core_override.get(group) {
            let in_group = self
                .session
                .sessions()
                .iter()
                .any(|s| s.id == core && s.group == group);
            if in_group {
                return Some(core);
            }
        }
        self.main_chart_target(group)
            .map(|(core, _)| core)
            .or_else(|| {
                self.session
                    .sessions()
                    .iter()
                    .find(|s| s.group == group)
                    .map(|s| s.id)
            })
    }

    /// Записать ручной выбор активного торгового ядра (клик в селекторе шапки).
    pub(crate) fn set_trade_core_override(&mut self, group: &str, core: CoreId) {
        self.trade_core_override.insert(group.to_string(), core);
    }

    pub(crate) fn refresh_header_ticker_default(&mut self, force: bool) {
        if self.layout.header_ticker.is_some() {
            return;
        }
        if let Some((core, _)) = &self.header_ticker_default {
            if self.session.sessions().iter().any(|s| s.id == *core) {
                return;
            }
        }
        let now = Instant::now();
        if !force
            && self
                .last_header_ticker_refresh
                .is_some_and(|last| now.duration_since(last) < Duration::from_secs(1))
        {
            return;
        }
        self.last_header_ticker_refresh = Some(now);
        let Some(core) = self.session.sessions().first().map(|s| s.id) else {
            self.header_ticker_default = None;
            return;
        };
        let ms = self.session.market_source();
        let market = ["BTCUSDT", "UBTCUSDC"]
            .iter()
            .find(|cand| ms.search_markets(core, cand, 2).iter().any(|m| m == *cand))
            .map(|c| c.to_string())
            .or_else(|| ms.search_markets(core, "BTC", 1).into_iter().next());
        self.header_ticker_default = market.map(|market| (core, market));
    }

    /// Источник тикера курса в шапке: сохранённый выбор (layout, по стабильному uid ядра),
    /// если ядро ещё подключено; иначе готовый дефолтный кэш. Render не ищет рынки и не
    /// мутирует backend.
    pub(crate) fn header_ticker(&self) -> Option<(CoreId, String)> {
        if let Some(sel) = &self.layout.header_ticker {
            let core = self
                .config
                .servers
                .iter()
                .find(|s| s.uid == sel.core_uid)
                .map(|s| s.id);
            if let Some(core) = core {
                if self.session.sessions().iter().any(|s| s.id == core) {
                    return Some((core, sel.market.clone()));
                }
            }
        }
        self.header_ticker_default
            .as_ref()
            .filter(|(core, _)| self.session.sessions().iter().any(|s| s.id == *core))
            .cloned()
    }

    /// Записать выбор тикера шапки (клик в попапе поиска) + персист в layout по uid ядра.
    pub(crate) fn set_header_ticker(&mut self, core: CoreId, market: String) {
        let Some(uid) = self
            .config
            .servers
            .iter()
            .find(|s| s.id == core)
            .map(|s| s.uid)
        else {
            return;
        };
        let sel = moon_core::config::layout::HeaderTicker {
            core_uid: uid,
            market: market.clone(),
        };
        if self.layout.header_ticker.as_ref() != Some(&sel) {
            self.layout.header_ticker = Some(sel);
            self.layout_dirty = true;
        }
    }

    /// Смещение часов шапки (минуты от UTC). Дефолт 0 = UTC.
    pub(crate) fn header_clock_offset_min(&self) -> i32 {
        self.layout.header_clock_offset_min
    }

    /// Записать смещение часов шапки (клик в попапе выбора пояса) + персист в layout.
    pub(crate) fn set_header_clock_offset_min(&mut self, off_min: i32) {
        if self.layout.header_clock_offset_min != off_min {
            self.layout.header_clock_offset_min = off_min;
            self.layout_dirty = true;
        }
    }

    /// Ядра группы (id, имя) для селектора в шапке. Порядок — как в конфиге/сессиях.
    pub(crate) fn group_cores(&self, group: &str) -> Vec<(CoreId, String)> {
        self.session
            .sessions()
            .iter()
            .filter(|s| s.group == group)
            .map(|s| (s.id, s.name.clone()))
            .collect()
    }

    pub(crate) fn retain_chart_market(&mut self, core: CoreId, market: &str) {
        let key = (core, market.to_string());
        *self.chart_market_refs.entry(key).or_insert(0) += 1;
        self.rebuild_desired_markets();
    }

    pub(crate) fn release_chart_market(&mut self, core: CoreId, market: &str) {
        let key = (core, market.to_string());
        let mut remove = false;
        if let Some(count) = self.chart_market_refs.get_mut(&key) {
            debug_assert!(*count > 0, "chart market refcount over-release");
            *count = count.saturating_sub(1);
            remove = *count == 0;
        } else {
            debug_assert!(false, "chart market refcount release without owner");
        }
        if remove {
            self.chart_market_refs.remove(&key);
        }
        self.rebuild_desired_markets();
    }

    pub(crate) fn retain_chart_orderbook(&mut self, core: CoreId, market: &str) {
        let key = (core, market.to_string());
        *self.chart_orderbook_refs.entry(key).or_insert(0) += 1;
        self.rebuild_orderbook_wanted();
    }

    pub(crate) fn release_chart_orderbook(&mut self, core: CoreId, market: &str) {
        let key = (core, market.to_string());
        let mut remove = false;
        if let Some(count) = self.chart_orderbook_refs.get_mut(&key) {
            *count = count.saturating_sub(1);
            remove = *count == 0;
        }
        if remove {
            self.chart_orderbook_refs.remove(&key);
        }
        self.rebuild_orderbook_wanted();
    }

    /// Пересобрать `desired_orderbook` (рынки с ≥1 включённым стаканом). Меняется → dirty (re-send).
    pub(crate) fn rebuild_orderbook_wanted(&mut self) {
        let mut want: Vec<(CoreId, String)> = self
            .chart_orderbook_refs
            .iter()
            .filter_map(|((core, market), count)| (*count > 0).then(|| (*core, market.clone())))
            .collect();
        want.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        if self.desired_orderbook != want {
            self.desired_orderbook = want;
            self.desired_open_dirty = true;
        }
    }

    pub(crate) fn rebuild_desired_markets(&mut self) {
        let mut desired: Vec<(CoreId, String)> = self
            .chart_market_refs
            .iter()
            .filter_map(|((core, market), count)| (*count > 0).then(|| (*core, market.clone())))
            .collect();
        desired.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        if self.desired != desired {
            self.desired = desired;
            self.desired_open_dirty = true;
        }
    }

    pub(crate) fn sync_open_markets_if_due(&mut self) {
        let now = Instant::now();
        // The 1s fallback is intentional: provider-side linger/drop/failover is
        // wall-clock based. The hot path itself is the boolean dirty flag; we no
        // longer hash the whole desired market list every 100ms.
        let due = now.duration_since(self.last_open_sync) >= Duration::from_secs(1);
        if self.desired_open_dirty || due {
            self.desired_open_dirty = false;
            self.last_open_sync = now;
            self.session
                .set_open(&self.desired, &self.desired_orderbook);
        }
    }

    pub(crate) fn mark_backend_dirty(&mut self, cx: &mut Context<Self>) {
        self.backend_dirty_since_notify = true;
        self.flush_backend_notify(cx);
    }

    pub(crate) fn flush_backend_notify(&mut self, cx: &mut Context<Self>) {
        if !self.backend_dirty_since_notify {
            return;
        }
        let due = self
            .last_backend_notify
            .is_none_or(|last| last.elapsed() >= Duration::from_millis(250));
        if !due {
            return;
        }
        self.backend_dirty_since_notify = false;
        self.last_backend_notify = Some(Instant::now());
        crate::diag::bump(&crate::diag::BACKEND_NOTIFY);
        cx.notify();
    }

    pub(crate) fn maybe_diag_open_first_market(&mut self, cx: &mut Context<Self>) {
        if !self.diag_open_first_market || self.diag_open_done || self.open_request.is_some() {
            return;
        }
        if self.group_windows.is_empty() {
            return;
        }

        let candidate = self.config.servers.iter().find_map(|server| {
            let market = server.market.trim();
            let session_exists = self
                .session
                .sessions()
                .iter()
                .any(|session| session.id == server.id && session.group == server.group);
            (server.active
                && server.show_window
                && self.config.group(&server.group).active
                && self.group_windows.contains_key(&server.group)
                && !market.is_empty()
                && session_exists)
                .then(|| (server.id, market.to_string(), server.name.clone()))
        });

        let Some((core, market, name)) = candidate else {
            self.diag_open_done = true;
            log::warn!("diag auto-open: no active visible server with default market");
            return;
        };

        self.diag_open_done = true;
        self.open_request = Some((core, market.clone()));
        self.open_request_rev = self.open_request_rev.wrapping_add(1);
        self.open_request_activate = false;
        if std::env::var_os("MOON_RENDER_DIAG_PAUSE_AFTER_OPEN").is_some() {
            self.follow = false;
        }
        log::info!("diag auto-open: core={core} name={name} market={market}");
        cx.notify();
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub(crate) fn take_diag_open_10_btc(&mut self) -> bool {
        if !self.diag_open_10_btc || self.diag_open_10_btc_done {
            return false;
        }
        // Debug perf windows only need a live core id/group, not the main group window.
        // On headless Linux/X11 the main window can exist while the bookkeeping gate is
        // still false during early startup, which made MOON_RENDER_DIAG_OPEN_10_BTC
        // silently do nothing and broke automated perf runs.
        if self.session.sessions().is_empty() {
            return false;
        }
        if crate::debug_window::debug_chart_target(self).is_none() {
            return false;
        }
        self.diag_open_10_btc_done = true;
        true
    }
}
