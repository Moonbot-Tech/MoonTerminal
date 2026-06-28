//! `ChartTabs`: поле поиска монеты + кастомные (мульти-монетные) вкладки — мульти-выбор,
//! создание/переименование/персист/восстановление и гейтинг подписок на стаканы по фокусу.
//! Вынесено из `mod.rs`.

use std::time::Duration;

use gpui::*;
use rust_i18n::t;

use super::{AddChartStack, CUSTOM_NUM_BASE, ChartTabs, Tab, coin_search};
use crate::chart_persist::{StackLayoutMode, StackOrientation};
use moon_core::config::ChartBucket;
use moon_core::session::CoreId;

impl ChartTabs {
    /// bucket-а). Кастомная вкладка собирает монеты с разных ядер → ищем по всей группе.
    pub(super) fn coin_results(&self, cx: &App) -> Vec<(CoreId, String, String)> {
        let bucket = match &self.active {
            Tab::Main | Tab::Custom(..) => None,
            Tab::Add(_, b) => Some(b.clone()),
        };
        coin_search::search(
            self.backend.read(cx),
            &self.group,
            bucket.as_ref(),
            &self.coin_query,
        )
    }

    /// Открыть выбранную монету на АКТИВНОЙ вкладке: Main → fullscreen-чарт; Add/Custom → её стек.
    pub(super) fn open_coin_on_active(
        &mut self,
        core: CoreId,
        market: String,
        cx: &mut Context<Self>,
    ) {
        match self.active.clone() {
            Tab::Main => self
                .main
                .update(cx, |m, c| m.open_or_focus(core, market, c)),
            Tab::Add(..) | Tab::Custom(..) => {
                if let Some(panel) = self.active_stack() {
                    panel.update(cx, |p, c| {
                        p.add_coin(core, &market, coin_search::MANUAL_COIN_TTL_MS, c)
                    });
                }
            }
        }
        // Кастомная вкладка изменила состав → пере-персист её тикеров.
        if self.active_is_custom() {
            self.persist_custom_active(cx);
        }
        self.sync_main_chart_target(cx);
        cx.notify();
    }

    /// Тоггл выбора монеты чекбоксом в выпадашке (накапливается для «Открыть в новой вкладке»).
    /// Выбор переживает смену запроса (можно искать BTC → отметить, потом ETH → отметить).
    pub(super) fn toggle_coin_selected(
        &mut self,
        core: CoreId,
        market: String,
        cx: &mut Context<Self>,
    ) {
        let key = (core, market);
        if !self.coin_selected.remove(&key) {
            self.coin_selected.insert(key);
        }
        cx.notify();
    }

    /// Создать кастомную вкладку из отмеченных монет: чарты сразу запинены, горизонтальная
    /// ориентация, фокус переходит на новую вкладку. Персистится (тикеры + имя + раскладка).
    pub(super) fn open_selected_in_new_tab(&mut self, cx: &mut Context<Self>) {
        if self.coin_selected.is_empty() {
            return;
        }
        let coins: Vec<(CoreId, String)> = self.coin_selected.iter().cloned().collect();
        let num = self.next_custom_num;
        self.next_custom_num += 1;
        let label = t!("chart.tab.custom", n = num - CUSTOM_NUM_BASE + 1).to_string();
        let bucket = ChartBucket::Shared;
        let stack = cx.new(|_| {
            AddChartStack::new(
                self.backend.clone(),
                num,
                bucket.clone(),
                self.epoch,
                self.theme.clone(),
            )
        });
        // По умолчанию — горизонтальная ориентация. Кастомная вкладка не держит пустые слоты.
        stack.update(cx, |s, c| {
            s.set_hold_vacated(false);
            s.set_orientation(Some(StackOrientation::Horizontal), c);
        });
        for (core, market) in &coins {
            stack.update(cx, |s, c| {
                s.add_coin(*core, market, coin_search::MANUAL_COIN_TTL_MS, c)
            });
        }
        // Чарты сразу запинены (защита от TTL-закрытия).
        stack.update(cx, |s, c| s.pin_all(c));
        self.custom.push((num, bucket.clone(), stack.clone()));
        self.custom_labels.insert(num, label.clone());
        self.active = Tab::Custom(num, bucket.clone());
        self.persist_custom(cx, num, &bucket, &coins, &label);
        // Следить за составом → пере-персист при закрытии/добавлении чарта.
        self.watch_custom_stack(num, &bucket, &stack, cx);
        // Сброс выбора/поля/попапа.
        self.coin_selected.clear();
        self.coin_query.clear();
        self.coin_popup_open = false;
        self.sync_active_scale(cx);
        self.sync_inactive_chart_visibility(cx);
        self.refresh_orderbook_gates(cx);
        self.sync_main_chart_target(cx);
        cx.notify();
    }

    /// Метка кастомной вкладки (имя пользователя или дефолт «Набор N»).
    pub(super) fn custom_label(&self, n: u32) -> String {
        self.custom_labels
            .get(&n)
            .cloned()
            .unwrap_or_else(|| t!("chart.tab.custom", n = n - CUSTOM_NUM_BASE + 1).to_string())
    }

    /// Переименовать активную кастомную вкладку (поле имени в попапе ⚙) + persist.
    pub(super) fn rename_active_custom(&mut self, name: String, cx: &mut Context<Self>) {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        if let Tab::Custom(n, b) = self.active.clone() {
            self.custom_labels.insert(n, name.clone());
            self.upsert_spec(cx, n, &b, move |s| s.custom_label = Some(name));
            cx.notify();
        }
    }

    /// Записать спек кастомной вкладки (тикеры + имя + гориз. ориентация) в charts.json.
    pub(super) fn persist_custom(
        &self,
        cx: &mut Context<Self>,
        num: u32,
        bucket: &ChartBucket,
        coins: &[(CoreId, String)],
        label: &str,
    ) {
        let coins = coins.to_vec();
        let label = label.to_string();
        self.upsert_spec(cx, num, bucket, move |s| {
            s.custom_coins = Some(coins);
            s.custom_label = Some(label);
            if s.layout_orientation.is_none() {
                s.layout_orientation = Some(StackOrientation::Horizontal);
            }
        });
    }

    /// Удалить спек кастомной вкладки из charts.json (закрытие вкладки = удаление сохранёнки).
    pub(super) fn remove_custom_spec(&self, n: u32, cx: &mut Context<Self>) {
        let group = self.group.clone();
        self.backend.update(cx, |b, _| {
            let before = b.chart_specs.len();
            b.chart_specs
                .retain(|s| !(s.group == group && s.num == n && s.custom_coins.is_some()));
            if b.chart_specs.len() != before {
                b.chart_specs_dirty = true;
            }
        });
    }

    /// Подписаться на изменения кастомного стека → пере-персист тикеров при смене состава
    /// (закрыли «×»/добавили чарт на сохранённой вкладке → обновляем `custom_coins`). Пока стек
    /// откреплён (в `self.detached`), `sync_custom_coins` ничего не пишет (его держит окно-хост);
    /// после репина в стрип эта подписка снова актуальна.
    pub(super) fn watch_custom_stack(
        &self,
        num: u32,
        bucket: &ChartBucket,
        stack: &Entity<AddChartStack>,
        cx: &mut Context<Self>,
    ) {
        let bk = bucket.clone();
        cx.observe(stack, move |this, _stack, cx| {
            this.sync_custom_coins(num, &bk, cx);
            // Якорь сравнения мог смениться (клик замка) → обновить торговый таргет группы
            // (хоткеи/cancel_buy идут на залоченный якорь как на Main-фулскрин).
            this.sync_main_chart_target(cx);
        })
        .detach();
    }

    /// Сверить текущий состав кастомной вкладки (тикеры + якорь сравнения + режим метлы) с
    /// сохранённым; переписать спек ТОЛЬКО при изменении (иначе observe-колбэк на каждый тик
    /// данных писал бы вхолостую).
    fn sync_custom_coins(&mut self, num: u32, bucket: &ChartBucket, cx: &mut Context<Self>) {
        let Some(stack) = self.add_stack(num, bucket) else {
            return;
        };
        let (coins, anchor, broom) = {
            let s = stack.read(cx);
            (s.coins(cx), s.compare_anchor(), s.compare_orderbook_only())
        };
        let changed = {
            let specs = &self.backend.read(cx).chart_specs;
            specs
                .iter()
                .find(|s| s.matches(&self.group, num, bucket))
                .map_or(true, |s| {
                    s.custom_coins.as_deref() != Some(coins.as_slice())
                        || s.compare_anchor != anchor
                        || s.compare_orderbook_only != broom
                })
        };
        if changed {
            let label = self.custom_label(num);
            self.persist_custom(cx, num, bucket, &coins, &label);
            self.upsert_spec(cx, num, bucket, move |s| {
                s.compare_anchor = anchor;
                s.compare_orderbook_only = broom;
            });
        }
    }

    /// Пере-персист тикеров активной кастомной вкладки (после изменения состава).
    pub(super) fn persist_custom_active(&mut self, cx: &mut Context<Self>) {
        if let Tab::Custom(n, b) = self.active.clone() {
            if let Some(stack) = self.add_stack(n, &b) {
                let coins = stack.read(cx).coins(cx);
                let label = self.custom_label(n);
                self.persist_custom(cx, n, &b, &coins, &label);
            }
        }
    }

    /// Восстановить кастомные вкладки из charts.json (спеки с `custom_coins`): создать стек,
    /// залить тикеры (пин), применить раскладку/ориентацию/масштаб, имя. В стрипе (не окном).
    pub(super) fn restore_custom_tabs(&mut self, cx: &mut Context<Self>) {
        #[allow(clippy::type_complexity)]
        let specs: Vec<(
            u32,
            ChartBucket,
            Vec<(CoreId, String)>,
            Option<String>,
            Option<f32>,
            (Option<StackLayoutMode>, Option<u16>, Option<u16>),
            Option<StackOrientation>,
            Option<bool>,
            Option<bool>,
            Option<bool>,
            Option<(CoreId, String)>,
            bool,
            Option<crate::chart_persist::PriceAxisPos>,
            Option<bool>,
            Option<bool>,
            Option<bool>,
        )> = {
            let all = &self.backend.read(cx).chart_specs;
            all.iter()
                .filter(|s| s.group == self.group && s.detached.is_none())
                .filter_map(|s| {
                    s.custom_coins.clone().map(|coins| {
                        (
                            s.num,
                            s.bucket(),
                            coins,
                            s.custom_label.clone(),
                            s.scale,
                            (s.layout_mode, s.layout_height_fit, s.layout_height_scroll),
                            s.layout_orientation,
                            s.orderbook_enabled,
                            s.show_zone,
                            s.auto_pin,
                            s.compare_anchor.clone(),
                            s.compare_orderbook_only,
                            s.price_axis_pos,
                            s.time_axis_visible,
                            s.line_labels,
                            s.cursor_labels,
                        )
                    })
                })
                .collect()
        };
        for (
            num,
            bucket,
            coins,
            label,
            scale,
            layout,
            orientation,
            ob,
            sz,
            ap,
            anchor,
            broom,
            axis_pos,
            time_axis,
            line_labels,
            cursor_labels,
        ) in specs
        {
            let stack = cx.new(|_| {
                AddChartStack::new(
                    self.backend.clone(),
                    num,
                    bucket.clone(),
                    self.epoch,
                    self.theme.clone(),
                )
            });
            stack.update(cx, |s, c| {
                s.set_hold_vacated(false);
                s.set_orientation(Some(orientation.unwrap_or(StackOrientation::Horizontal)), c);
                if scale.is_some() {
                    s.set_scale(scale, c);
                }
                s.set_layout(layout.0, layout.1, layout.2, c);
                if let Some(v) = ob {
                    s.set_orderbook_enabled(Some(v), c);
                }
                if let Some(v) = sz {
                    s.set_show_zone(Some(v), c);
                }
                if let Some(v) = ap {
                    s.set_auto_pin(Some(v), c);
                }
                if axis_pos.is_some() {
                    s.set_price_axis_pos(axis_pos, c);
                }
                if time_axis.is_some() {
                    s.set_time_axis_visible(time_axis, c);
                }
                if line_labels.is_some() {
                    s.set_line_labels(line_labels, c);
                }
                if cursor_labels.is_some() {
                    s.set_cursor_labels(cursor_labels, c);
                }
                for (core, market) in &coins {
                    s.add_coin(*core, market, coin_search::MANUAL_COIN_TTL_MS, c);
                }
                s.pin_all(c);
            });
            // Восстановить режим сравнения (якорь + метла) после заливки тикеров.
            if anchor.is_some() || broom {
                stack.update(cx, |s, c| s.restore_compare(anchor.clone(), broom, c));
            }
            self.watch_custom_stack(num, &bucket, &stack, cx);
            self.custom.push((num, bucket, stack));
            if let Some(label) = label {
                self.custom_labels.insert(num, label);
            }
            self.next_custom_num = self.next_custom_num.max(num + 1);
        }
        if !self.custom.is_empty() {
            self.refresh_orderbook_gates(cx);
        }
    }

    /// Обновить гейты стаканов кастомных вкладок по фокусу: активная → resume сразу; неактивные
    /// в стрипе → через 5с suspend (отписка), если так и не вернулись. Откреплённых нет в
    /// `self.custom` → они не suspend'ятся (окно само держит спрос).
    pub(super) fn refresh_orderbook_gates(&mut self, cx: &mut Context<Self>) {
        let active = self.active.clone();
        let customs: Vec<(u32, ChartBucket, Entity<AddChartStack>)> = self.custom.clone();
        for (n, b, stack) in customs {
            if Tab::Custom(n, b.clone()) == active {
                // Вернулись на вкладку → отменяем pending-таймер и сразу переподписываемся.
                *self.custom_gate_gen.entry(n).or_insert(0) += 1;
                stack.update(cx, |s, c| s.set_orderbook_suspended(false, c));
            } else {
                // Ушли с вкладки → ставим 5с-таймер на отписку (последний побеждает по поколению).
                let want_gen = {
                    let e = self.custom_gate_gen.entry(n).or_insert(0);
                    *e += 1;
                    *e
                };
                let stack = stack.clone();
                cx.spawn(async move |this, cx| {
                    let executor = cx.update(|cx| cx.background_executor().clone());
                    executor.timer(Duration::from_secs(5)).await;
                    let _ = cx.update(|cx| {
                        this.update(cx, |this, cx| {
                            // Таймер ещё актуален, вкладка всё ещё неактивна и в стрипе?
                            let still = this.custom_gate_gen.get(&n) == Some(&want_gen)
                                && !matches!(&this.active, Tab::Custom(nn, _) if *nn == n)
                                && this.custom.iter().any(|(num, _, _)| *num == n);
                            if still {
                                stack.update(cx, |s, c| s.set_orderbook_suspended(true, c));
                            }
                        })
                        .ok();
                    });
                })
                .detach();
            }
        }
    }

    /// Очистить поле монеты и закрыть список (после выбора / по клику вне).
    pub(super) fn clear_coin_search(&mut self, cx: &mut Context<Self>) {
        self.coin_query.clear();
        self.coin_popup_open = false;
        cx.notify();
    }
}
