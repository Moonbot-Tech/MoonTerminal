//! Верх окна «Активы»: панель управления (счётчик/«показать всё»/итоги), полоса ядер
//! (баланс USDT), таблица позиций/балансов и нижний список ядер (свободно/итого).

use super::*;
use crate::controls::{CoinMenuCtx, CoinMenuOrigin};
use moon_ui::{MoonButtonVariant, MoonNotification, MoonText, MoonWindowExt as _};
use rust_i18n::t;

/// Открыть единое контекстное меню монеты по строке Активов (ПКМ на тикере). Стратегии/ордера
/// у балансов нет → меню = навигация + ЧС ядра/ядер. «Выбранные ядра» = ядра охвата панели
/// (группа → ядра группы, глобальное окно → все ядра).
fn open_asset_coin_menu(
    core: CoreId,
    market: String,
    coin: String,
    view: &Entity<AssetsView>,
    pos: Point<Pixels>,
    window: &mut Window,
    app: &mut App,
) {
    app.stop_propagation();
    let (backend, scope) = {
        let panel = view.read(app);
        (panel.backend.clone(), panel.scope.clone())
    };
    let (selected_cores, core_name) = {
        let b = backend.read(app);
        let sessions = b.session.sessions();
        let selected: Vec<CoreId> = sessions
            .iter()
            .filter(|s| match &scope {
                AssetsScope::Group(g) => &s.group == g,
                AssetsScope::All => true,
            })
            .map(|s| s.id)
            .collect();
        let name = sessions
            .iter()
            .find(|s| s.id == core)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        (selected, name)
    };
    let ctx = CoinMenuCtx {
        core,
        core_name,
        market,
        coin,
        selected_cores,
        strat_id: None,
        strat_name: None,
        order_uid: None,
        side: None,
        short: false,
        origin: CoinMenuOrigin::OrderTable,
    };
    crate::controls::open_coin_menu(ctx, backend, pos, window, app);
}


impl AssetsView {
    /// Верхняя полоса: поле-список выбора ядер (мультивыбор, как в «Ордерах») + справа слайдер
    /// порога «скрыть дешевле N $» с подписью `≥ N$`. Счётчик/Σ уехали в нижний футер [`Self::footer`].
    pub(super) fn core_bar(
        &self,
        cores: &[(CoreId, String)],
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        h_flex()
            .w_full()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(self.core_combo(cores, cx))
            // Слайдер порога пыли + подпись `≥ N$` (0 = показать всё). Колесо мыши над ним
            // меняет порог на ±1$ (тащить мышью по узкому слайдеру неудобно).
            .child(
                div()
                    .id("assets-min-value-wheel")
                    .w(px(120.0))
                    // Подсказка: что делает слайдер (0 = показать всё, колесо мыши меняет).
                    .tooltip(|_window, cx| {
                        cx.new(|_| {
                            moon_ui::MoonTooltipView::new(t!("assets.min_value_hint").to_string())
                        })
                        .into()
                    })
                    .on_scroll_wheel(cx.listener(
                        |this, ev: &ScrollWheelEvent, window, cx| {
                            let dy = match ev.delta {
                                ScrollDelta::Lines(pt) => pt.y,
                                ScrollDelta::Pixels(pt) => f32::from(pt.y),
                            };
                            if dy == 0.0 {
                                return;
                            }
                            let next = (this.min_value_usd + if dy > 0.0 { 1.0 } else { -1.0 })
                                .clamp(0.0, 100.0);
                            if next != this.min_value_usd {
                                this.min_value_usd = next;
                                this.min_value_slider.update(cx, |s, c| {
                                    s.set_value(next as f32, window, c);
                                });
                                let backend = this.backend.clone();
                                this.rebuild_cache(backend.read(cx));
                                this.persist_min_value(cx);
                                cx.notify();
                            }
                        },
                    ))
                    .child(
                        MoonSlider::new(&self.min_value_slider)
                            .id("assets-min-value")
                            .height(18.0),
                    ),
            )
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text_muted))
                    .child(format!("≥ {}$", self.min_value_usd.round() as i64)),
            )
    }

    /// Поле-список ядер — МУЛЬТИВЫБОР (общий виджет [`crate::controls::core_combo`], как в
    /// «Ордерах»/«Отчёте»). Подпись: «Все ядра» (пусто/все) / имя единственного / «Ядер: N».
    pub(super) fn core_combo(
        &self,
        cores: &[(CoreId, String)],
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let view = cx.entity();
        crate::controls::core_combo(
            "assets-core",
            cores,
            &self.sel_cores,
            t!("assets.all_cores").to_string(),
            |n| t!("assets.cores_n", n = n).to_string(),
            170.0,
            move |id, app| {
                view.update(app, |t, c| t.toggle_core(id, c));
            },
        )
    }

    /// Нижний футер: счётчик позиций «Позиции N» и Σ-стоимость справа. Единый визуальный
    /// образец футера (как в «Отчёте»). Порог видимости («показать всё») переехал слайдером
    /// в верхнюю полосу [`Self::core_bar`].
    pub(super) fn footer(
        &self,
        count: usize,
        total_value: f64,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        h_flex()
            .w_full()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_soft))
                    .child(t!("assets.section_positions").to_string()),
            )
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_muted))
                    .child(format!("{count}")),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_soft))
                    .child(format!("Σ {}", money(total_value))),
            )
    }

    /// Секция «Кошельки»: шапка (стрелка + подпись + ↻ refresh), контент — 4 одинаковых
    /// вертикальных контейнера: список ядер (выбор) + Спот/Фьючерсы/Квартальные с переносом.
    /// Как и остальные секции — сворачиваемая, развёрнутая делит высоту (никаких фикс. 380px:
    /// раньше низ не двигался и прятал под собой развёрнутые плашки ядер).
    pub(super) fn bottom(
        &self,
        cores: &[(CoreId, String)],
        aggs: &[CoreAgg],
        wallets: &[WalletColumnSnapshot],
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        // Эффективный выбор: сохранённый, если он есть в охвате, иначе первое ядро.
        let selected = self
            .selected_core
            .filter(|c| cores.iter().any(|(id, _)| id == c))
            .or_else(|| cores.first().map(|(id, _)| *id));

        // ── Шапка секции (в стиле шапок «Позиции»/«Ядра») ──
        let collapsed = self.wallets_collapsed;
        let arrow = if collapsed { "▸" } else { "▾" };
        let mut header = h_flex()
            .id("assets-wallets-bar")
            .w_full()
            .flex_none()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .border_t_1()
            .border_color(rgb(p.border))
            .child(
                h_flex()
                    .id("assets-wallets-toggle")
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|s| s.text_color(rgb(p.text)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.wallets_collapsed = !this.wallets_collapsed;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .text_size(design::t_body(cx))
                            .text_color(rgb(p.text_muted))
                            .child(arrow),
                    )
                    .child(
                        div()
                            .text_size(design::t_body(cx))
                            .text_color(rgb(p.text_soft))
                            .child(t!("assets.wallets_hint").to_string()),
                    ),
            )
            .child(div().flex_1());
        if let Some(core) = selected {
            header = header.child(
                MoonButton::new("assets-refresh-transfer")
                    .ghost()
                    .size(MoonButtonSize::Micro)
                    .label("↻")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Err(error) =
                            this.backend.read(cx).session.refresh_transfer_assets(core)
                        {
                            log::warn!("assets refresh failed for core {core}: {error}");
                            window
                                .push_notification(MoonNotification::error(error.to_string()), cx);
                        }
                        let backend = this.backend.clone();
                        this.rebuild_cache(backend.read(cx));
                        cx.notify();
                    }))
                    .render(),
            );
        }
        if collapsed {
            return v_flex().w_full().flex_none().child(header);
        }

        // ── Левая колонка: список ядер (имя + свободно/итого USDT) ──
        let mut list = v_flex().w_full().gap_0();
        for (id, name) in cores {
            let cid = *id;
            let active = selected == Some(cid);
            let (free, total) = aggs
                .iter()
                .find(|a| a.id == cid)
                .map(|a| (a.free, a.total))
                .unwrap_or((0.0, 0.0));
            let mut item = h_flex()
                .id(SharedString::from(format!("asset-core-{cid}")))
                .w_full()
                .h(design::fit_h_px(cx, 24.0, 13.0, 5.0))
                .px(design::ui_px(cx, 8.0))
                .items_center()
                .justify_between()
                .gap_2()
                .cursor_pointer()
                .text_color(rgb(p.text))
                .child(div().flex_1().min_w_0().truncate().child(name.clone()))
                .child(
                    div()
                        .text_size(design::t_body(cx))
                        .text_color(rgb(p.text_soft))
                        .child(format!("{} / {}", money(free), money(total))),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.selected_core != Some(cid) {
                        this.selected_core = Some(cid);
                        if let Err(error) =
                            this.backend.read(cx).session.refresh_transfer_assets(cid)
                        {
                            log::warn!("assets refresh failed for core {cid}: {error}");
                            window
                                .push_notification(MoonNotification::error(error.to_string()), cx);
                        }
                        let backend = this.backend.clone();
                        this.rebuild_cache(backend.read(cx));
                        cx.notify();
                    }
                }));
            if active {
                item = item.bg(rgb(p.panel)).text_color(rgb(p.blue));
            } else {
                item = item.hover(|s| s.bg(rgb(p.shell_high)));
            }
            list = list.child(item);
        }

        // Левый контейнер оформлен КАК колонки кошельков (та же плашка-заголовок shell_high)
        // — «4 одинаковых вертикальных контейнера».
        let left = v_flex()
            .w(px(240.0))
            .h_full()
            .flex_none()
            .border_r_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .w_full()
                    .flex_none()
                    .px(design::ui_px(cx, 6.0))
                    .py(design::ui_px(cx, 3.0))
                    .bg(rgb(p.shell_high))
                    .text_size(design::t_body(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(p.text_soft))
                    .child(t!("assets.cores_free_total").to_string()),
            )
            .child(
                div()
                    .id("asset-core-list")
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .child(list),
            );

        // ── Правая часть: 3 контейнера кошельков (Спот/Фьючерсы/Квартальные) ──
        let right = match selected {
            Some(core) => self.wallets_section(core, wallets, cx).into_any_element(),
            None => div()
                .p_4()
                .text_color(rgb(p.text_muted))
                .child(t!("assets.no_cores").to_string())
                .into_any_element(),
        };

        // Развёрнутая секция: flex-доля с минимумом видимости (заголовки колонок + пара строк).
        v_flex()
            .w_full()
            .flex_1()
            .min_h(px(160.0))
            .child(header)
            .child(
                h_flex()
                    .w_full()
                    .flex_1()
                    .min_h(px(0.0))
                    .border_t_1()
                    .border_color(rgb(p.border))
                    .child(left)
                    .child(div().flex_1().h_full().min_w_0().child(right)),
            )
    }
}

/// Колонки — 1:1 с окном Assets Moonbot: монета, количество, сумма в $, две кнопки.
/// Плюс «Ядро» слева: у Moonbot окно per-core, наше — общее на все ядра охвата.
fn assets_columns() -> Vec<MoonDataTableColumn> {
    let numeric =
        |key: &'static str, title: String, w: f32| MoonDataTableColumn::new(key, title, w).right();
    vec![
        MoonDataTableColumn::new("core", t!("assets.col.core").to_string(), 90.0),
        MoonDataTableColumn::new("coin", t!("assets.col.coin").to_string(), 80.0),
        numeric("qty", t!("assets.col.qty").to_string(), 130.0),
        numeric("value", t!("assets.col.value").to_string(), 110.0),
        // Кнопки Market sell / Order.
        MoonDataTableColumn::new("actions", String::new(), 170.0),
    ]
}

pub(super) fn assets_table(
    id: &'static str,
    rows: Rc<Vec<AssetEntry>>,
    sell_marked: Rc<std::collections::HashSet<(CoreId, String)>>,
    state: &Entity<MoonDataTableState>,
    cx: &Context<AssetsView>,
) -> impl IntoElement {
    let empty = rows.is_empty();
    let row_count = rows.len();
    let view = cx.entity();
    let table_rows = rows.clone();
    let p = MoonPalette::active(cx);

    crate::panels::common::data_table_host(
        SharedString::from(format!("{id}-host")),
        empty,
        t!("assets.empty").to_string(),
        p,
        cx,
        MoonDataTable::new(id, row_count, move |ix, _window, _app| {
            let e = &table_rows[ix];
            // Матч «в продаже» — по МОНЕТЕ, не по имени рынка: у кошельковых строк
            // HL-спота рынок не резолвится (индексные имена «@151»), а монета совпадает.
            let on_sale = sell_marked.contains(&(e.core, e.row.coin.to_ascii_uppercase()));
            assets_row(e, &view, p, on_sale)
        })
        .columns(assets_columns())
        .state(state)
        .header_height(design::TABLE_HEAD_H)
        .row_height(design::TABLE_ROW_H),
    )
}

fn assets_row(e: &AssetEntry, view: &Entity<AssetsView>, p: MoonPalette, on_sale: bool) -> MoonDataRow {
    let r = &e.row;
    let is_position = r.pos_size != 0.0;
    // Кол-во/Сумма: спот — ПОЛНЫЙ удерживаемый остаток (free + заблокированное в открытых
    // sell-ордерах), как Moonbot — открытую позу с TP-ордерами показываем целиком, а не
    // «свободно≈0»; фьюч-позиция — остаток позиции и её стоимость (размер × цена рынка;
    // котируемая у фьючей — USD-стейбл). Кол-во — ограниченная точность по величине
    // (`fmt::qty`: макс. тысячные, мин. десятые), не adaptive.
    let (qty, sum) = if is_position {
        (
            moon_core::util::fmt::qty(r.pos_size),
            money(r.pos_size.abs() * r.price),
        )
    } else {
        let held = if r.qty_full.abs() > r.qty.abs() {
            r.qty_full
        } else {
            r.qty
        };
        (moon_core::util::fmt::qty(held), money(e.value))
    };
    MoonDataRow::new([
        // Ядро кликабельно → фильтр панели ровно на это ядро (повторный клик — сброс на «Все»).
        MoonDataCell::element(core_cell(e, view, p)),
        // Тикер кликабелен → открыть чарт монеты на Main НА ЯДРЕ строки (как в Ордерах/Отчёте).
        MoonDataCell::element(coin_cell(e, view, p, on_sale)),
        MoonDataCell::text(qty),
        MoonDataCell::text(sum),
        actions_cell(e, view, p, is_position),
    ])
    // Подсветка строки: монета/позиция сейчас СТОИТ НА ПРОДАЖУ (активный sell-ордер,
    // фаза SellSet/SellAlmostDone на этом ядре). Плюс синяя метка «SELL» у тикера.
    .selected(on_sale)
}

/// Ячейка «Ядро» (имя ядра): левый клик выставляет фильтр панели РОВНО на это ядро (повторный
/// клик по уже единственному выбранному — сброс на «Все»). ПКМ-меню монеты живёт на тикере —
/// сюда не добавляем. Текст — прежним тусклым (Muted) стилем.
fn core_cell(
    e: &AssetEntry,
    view: &Entity<AssetsView>,
    p: MoonPalette,
) -> impl IntoElement + 'static {
    let core = e.core;
    let view = view.clone();
    div()
        .id(SharedString::from(format!(
            "asset-core-cell-{core}-{}",
            e.row.market
        )))
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .cursor_pointer()
        // Кегль/шрифт наследуются от стиля ячейки (каскад moonui, фикс `9a33dbf`).
        .text_color(rgb(MoonTone::Muted.color(p)))
        .child(e.core_name.clone())
        .on_click(move |_, _window, app| {
            view.update(app, |this, cx| this.filter_to_core(core, cx));
        })
}

/// Ячейка тикера (базовая монета): клик открывает чарт монеты на Main на ядре строки.
/// Порт клика по тикеру из панели «Ордера». `on_sale` — рядом с тикером синяя метка
/// «SELL» (у монеты сейчас активный sell-ордер, тон Info как в таблице «Ордера»).
fn coin_cell(
    e: &AssetEntry,
    view: &Entity<AssetsView>,
    p: MoonPalette,
    on_sale: bool,
) -> impl IntoElement + 'static {
    let coin = e.row.coin.clone();
    let core = e.core;
    let market = e.row.market.clone();
    let view = view.clone();
    let view_menu = view.clone();
    let coin_menu = coin.clone();
    let market_menu = market.clone();
    // Значок монеты (assets/coins). Нет значка → тикер без иконки (без пустого места:
    // тут колонка узкая, выравнивание держит сама таблица).
    let icon = crate::coin_icons::coin_icon(&coin);
    div()
        .id(SharedString::from(format!("asset-coin-{core}-{}", e.row.market)))
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .gap_1()
        .cursor_pointer()
        .when_some(icon, |el, tex| {
            el.child(img(tex).w(px(14.0)).h(px(14.0)).flex_none())
        })
        // Кегль/шрифт тикера — от стиля ячейки (каскад moonui, фикс `9a33dbf`).
        .text_color(rgb(MoonTone::Accent.color(p)))
        .font_weight(FontWeight::MEDIUM)
        .child(coin)
        .when(on_sale, |el| {
            // Метка «SELL» меньше тикера — дефолтный кегль MoonText (9).
            el.child(
                MoonText::new("SELL")
                    .color(MoonTone::Info.color(p))
                    .line_height(14.0)
                    .weight(600.0)
                    .mono(true)
                    .uppercase(false)
                    .render(),
            )
        })
        .on_click(move |_, _window, app| {
            if market.is_empty() {
                return; // строка без рынка (чистый баланс) — открывать нечего
            }
            view.update(app, |this, cx| {
                this.backend.update(cx, |b, bcx| {
                    b.open_request = Some((core, market.clone()));
                    b.open_request_rev = b.open_request_rev.wrapping_add(1);
                    // Открыть монету на Main, но окно НЕ поднимать (как в Ордерах).
                    b.open_request_activate = false;
                    bcx.notify();
                });
            });
        })
        // ПКМ — единое контекстное меню монеты (ЧС ядра/ядер охвата; стратегии/ордера нет).
        .on_mouse_down(
            MouseButton::Right,
            move |e: &MouseDownEvent, window, app| {
                open_asset_coin_menu(
                    core,
                    market_menu.clone(),
                    coin_menu.clone(),
                    &view_menu,
                    e.position,
                    window,
                    app,
                );
            },
        )
}

/// Ячейка действий позиции: «Market sell» (panic-sell по рынку) + «Order» (заглушка — окно
/// настроек ордера будет позже). Показывается только у строк с открытой позицией.
fn actions_cell(
    e: &AssetEntry,
    view: &Entity<AssetsView>,
    _p: MoonPalette,
    is_position: bool,
) -> MoonDataCell {
    // Продаваемо: открытая позиция ЛИБО спот-баланс монеты (есть что продать по рынку).
    // Кнопку показываем ТОЛЬКО если рынок `<coin><quote>` реально существует у ядра
    // (иначе продавать негде — напр. USDT на USDC-аккаунте: рынка USDTUSDC нет).
    let size = if e.row.qty.abs() > 0.0 {
        e.row.qty.abs()
    } else {
        e.row.qty_full.abs()
    };
    let sellable = is_position || size > 0.0;
    if !sellable || e.row.market.is_empty() || !e.market_exists {
        return MoonDataCell::text(String::new());
    }
    let core = e.core;
    let market = e.row.market.clone();
    let msell_id = SharedString::from(format!("asset-msell-{core}-{market}"));
    let order_id = SharedString::from(format!("asset-order-{core}-{market}"));
    let view_ms = view.clone();
    let market_ms = market.clone();
    let coin_ms = e.row.coin.clone();
    let el = h_flex()
        .w_full()
        .h_full()
        .items_center()
        .justify_end()
        .gap(px(4.0))
        .child(
            MoonButton::new(msell_id)
                .label(t!("assets.market_sell").to_string())
                .size(MoonButtonSize::Micro)
                .variant(MoonButtonVariant::Danger)
                .on_click(move |_, window, app| {
                    // Спрашиваем подтверждение (да/нет) ДО закрытия по маркету — необратимое
                    // действие, легко нажать случайно. Сама продажа — в кнопке «Да» диалога.
                    open_market_sell_confirm(
                        view_ms.clone(),
                        core,
                        market_ms.clone(),
                        is_position,
                        size,
                        coin_ms.clone(),
                        window,
                        app,
                    );
                })
                .render(),
        )
        .child(
            MoonButton::new(order_id)
                .label(t!("assets.order").to_string())
                .size(MoonButtonSize::Micro)
                .variant(MoonButtonVariant::Soft)
                .on_click(move |_, _w, _app| {
                    // Заглушка: окно настроек ордера будет позже.
                    log::info!("assets: order button (stub) core={core} market={market}");
                })
                .render(),
        );
    MoonDataCell::element(el)
}

/// Модалка подтверждения «Market sell» (да/нет). Само закрытие по маркету исполняется только
/// по кнопке «Да» — позиция закрывается `market_sell_position`, спот-остаток `market_sell_token`.
/// Открывается прямо из клика кнопки (у `MoonButton::on_click` под рукой `&mut App`).
#[allow(clippy::too_many_arguments)]
fn open_market_sell_confirm(
    view: Entity<AssetsView>,
    core: CoreId,
    market: String,
    is_position: bool,
    size: f64,
    coin: String,
    window: &mut Window,
    app: &mut App,
) {
    window.open_unique_moon_dialog("assets-market-sell-confirm", app, move |dialog, _window, cx| {
        let p = MoonPalette::active(cx);
        let confirm_view = view.clone();
        let market_c = market.clone();
        let question = t!("assets.market_sell_q", coin = coin.clone()).to_string();
        dialog
            .w(px(320.0))
            .close_button(true)
            .overlay(true)
            .overlay_closable(true)
            .bg(rgb(p.shell_high))
            .border_color(rgb(p.border))
            .rounded(design::r_container(cx))
            .text_color(rgb(p.text))
            .header(
                div()
                    .w_full()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!("assets.market_sell_confirm").to_string()),
            )
            .content(move |content, _window, cx| {
                let p = MoonPalette::active(cx);
                content.child(
                    div()
                        .font_family(design::mono())
                        .text_size(design::t_body(cx))
                        .text_color(rgb(p.text))
                        .child(question.clone()),
                )
            })
            .footer(
                h_flex()
                    .w_full()
                    .gap_2()
                    .justify_end()
                    .child(
                        MoonButton::new("assets-msell-no")
                            .outline()
                            .size(MoonButtonSize::Action)
                            .label(format!("  {}  ", t!("dialogs.no")))
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                            })
                            .render(),
                    )
                    .child(
                        MoonButton::new("assets-msell-yes")
                            .size(MoonButtonSize::Action)
                            .variant(MoonButtonVariant::Danger)
                            .label(format!("  {}  ", t!("dialogs.yes")))
                            .on_click(move |_, window, cx| {
                                confirm_view.update(cx, |this, cx| {
                                    let b = this.backend.read(cx);
                                    // Позиция → закрыть по маркету; спот-токен → продать остаток.
                                    let res = if is_position {
                                        b.session.market_sell_position(core, market_c.clone())
                                    } else {
                                        b.session.market_sell_token(core, market_c.clone(), size)
                                    };
                                    if let Err(err) = res {
                                        log::warn!("assets market sell {market_c} failed: {err:#}");
                                    }
                                    cx.notify();
                                });
                                window.close_dialog(cx);
                            })
                            .render(),
                    ),
            )
    });
}
