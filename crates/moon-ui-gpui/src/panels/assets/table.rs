//! Верх окна «Активы»: панель управления (счётчик/«показать всё»/итоги), полоса ядер
//! (баланс USDT), таблица позиций/балансов и нижний список ядер (свободно/итого).

use super::*;
use moon_ui::{MoonButtonVariant, MoonNotification, MoonText, MoonWindowExt as _};
use rust_i18n::t;

/// Ширина карточки ядра в полосе (`core_strip`). Фиксирована → карточки при переносе
/// выстраиваются ровной сеткой (равная ширина = ровные колонки, без «лесенки»).
const CORE_CARD_W: f32 = 148.0;

impl AssetsView {
    /// Шапка секции «Позиции» (над таблицей): стрелка сворачивания + подпись, счётчик строк,
    /// галка «показать всё», Σ стоимость. Один визуальный образец для всех трёх секций окна.
    pub(super) fn controls(
        &self,
        count: usize,
        total_value: f64,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let arrow = if self.positions_collapsed { "▸" } else { "▾" };
        h_flex()
            .w_full()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(
                h_flex()
                    .id("assets-positions-toggle")
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|s| s.text_color(rgb(p.text)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.positions_collapsed = !this.positions_collapsed;
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
                            .child(t!("assets.section_positions").to_string()),
                    ),
            )
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_muted))
                    .child(format!("{count}")),
            )
            .child(
                MoonCheckbox::new("assets-show-all")
                    .label(t!("assets.show_all").to_string())
                    .checked(self.show_all)
                    .size(MoonCheckboxSize::Compact)
                    .on_change(cx.listener(|this, ch: &bool, _, cx| {
                        if this.show_all != *ch {
                            this.show_all = *ch;
                            let backend = this.backend.clone();
                            this.rebuild_cache(backend.read(cx));
                            cx.notify();
                        }
                    })),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_soft))
                    .child(format!("Σ {}", money(total_value))),
            )
    }

    /// Сворачиваемая полоса ядер внизу: строка-шапка (кол-во ядер + Σ баланс + стрелка ▾/▸),
    /// под ней — сетка карточек с вертикальным скроллом. Свёрнуто = только строка-итог
    /// (карточки скрыты, «не мешают»). Клик по шапке тогает. PnL здесь не показываем
    /// (серверный Markets.FTotalPNL решили не выносить в Активы — только балансы).
    pub(super) fn core_strip(&self, aggs: &[CoreAgg], cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let total_balance: f64 = aggs.iter().map(|a| a.total).sum();
        let collapsed = self.plates_collapsed;
        let arrow = if collapsed { "▸" } else { "▾" };

        let header = h_flex()
            .id("assets-plates-bar")
            .w_full()
            .flex_none()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            // Кликабельная только зона сворачивания (стрелка + подпись).
            .child(
                h_flex()
                    .id("assets-plates-toggle")
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|s| s.text_color(rgb(p.text)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.plates_collapsed = !this.plates_collapsed;
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
                            .child(t!("assets.cores_count", n = aggs.len()).to_string()),
                    ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_soft))
                    .child(format!("Σ {}", money(total_balance))),
            );

        // Свёрнуто → секция = только строка-шапка (flex_none, таблица держит натуральную
        // высоту, ниже пусто). Развёрнуто → секция забирает ОСТАТОК места под таблицей
        // (flex_1), а сетка карточек скроллится внутри — плашки НЕ давят таблицу в 0.
        let mut section = v_flex().w_full().child(header);
        if collapsed {
            section = section.flex_none();
        } else {
            let mut grid = h_flex().w_full().flex_wrap().gap_2().px_2().py_1();
            for a in aggs {
                grid = grid.child(self.core_card(a, cx));
            }
            // min_h: развёрнутая секция ГАРАНТИРОВАННО показывает хотя бы ~2 ряда карточек,
            // отжимая соседей (таблица сверху сжимается, кошельки ниже делят flex-остаток) —
            // раньше flex_1 без минимума схлопывался в 0 под фикс. низом.
            section = section.flex_1().min_h(px(88.0)).child(
                div()
                    .id("assets-plates-scroll")
                    .w_full()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .child(grid),
            );
        }
        section
    }

    /// Одна карточка ядра фикс. ширины (`CORE_CARD_W`), в одну строку: имя слева, итоговый
    /// баланс справа (PnL убран). Равная ширина → при переносе карточки ложатся ровными колонками.
    fn core_card(&self, a: &CoreAgg, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        h_flex()
            .w(design::ui_px(cx, CORE_CARD_W))
            .flex_none()
            .items_center()
            .justify_between()
            .gap_2()
            .px(design::ui_px(cx, 8.0))
            .py(design::ui_px(cx, 4.0))
            .rounded(px(4.0))
            .bg(rgb(p.shell_high))
            .border_1()
            .border_color(rgb(p.border))
            .text_size(design::t_body(cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(rgb(p.text))
                    .child(a.name.clone()),
            )
            .child(div().text_color(rgb(p.text_soft)).child(money(a.total)))
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

fn assets_columns() -> Vec<MoonDataTableColumn> {
    let numeric =
        |key: &'static str, title: String, w: f32| MoonDataTableColumn::new(key, title, w).right();
    vec![
        MoonDataTableColumn::new("core", t!("assets.col.core").to_string(), 90.0),
        MoonDataTableColumn::new("coin", t!("assets.col.coin").to_string(), 70.0),
        numeric("qty", t!("assets.col.qty").to_string(), 90.0),
        numeric("price", t!("assets.col.price").to_string(), 84.0),
        numeric("value", t!("assets.col.value").to_string(), 92.0),
        numeric("pos", t!("assets.col.pos").to_string(), 80.0),
        numeric("pos_price", t!("assets.col.pos_price").to_string(), 84.0),
        numeric("profit", t!("assets.col.profit").to_string(), 86.0),
        MoonDataTableColumn::new("kind", t!("assets.col.kind").to_string(), 80.0),
        // Кнопки Market sell / Order — только у строк с открытой позицией.
        MoonDataTableColumn::new("actions", String::new(), 170.0),
    ]
}

pub(super) fn assets_table(
    id: &'static str,
    rows: Rc<Vec<AssetEntry>>,
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
            assets_row(&table_rows[ix], &view, p)
        })
        .columns(assets_columns())
        .header_height(design::TABLE_HEAD_H)
        .row_height(design::TABLE_ROW_H),
    )
}

fn assets_row(e: &AssetEntry, view: &Entity<AssetsView>, p: MoonPalette) -> MoonDataRow {
    let r = &e.row;
    let pnl = r.profit_b + r.profit_l + r.profit_s;
    let pnl_tone = if pnl > 0.0 {
        MoonTone::Positive
    } else if pnl < 0.0 {
        MoonTone::Danger
    } else {
        MoonTone::Muted
    };
    let is_position = r.pos_size != 0.0;
    let pos = if is_position {
        num(r.pos_size)
    } else {
        String::new()
    };
    let pos_price = if is_position {
        num(r.pos_price)
    } else {
        String::new()
    };
    MoonDataRow::new([
        MoonDataCell::text(e.core_name.clone()).tone(MoonTone::Muted),
        // Тикер кликабелен → открыть чарт монеты на Main НА ЯДРЕ строки (как в Ордерах/Отчёте).
        MoonDataCell::element(coin_cell(e, view, p)),
        MoonDataCell::text(num(r.qty)),
        MoonDataCell::text(num(r.price)),
        MoonDataCell::text(money(e.value)),
        MoonDataCell::text(pos),
        MoonDataCell::text(pos_price),
        MoonDataCell::text(money(pnl)).tone(pnl_tone),
        MoonDataCell::text(kind_label(r)).tone(MoonTone::Muted),
        actions_cell(e, view, p, is_position),
    ])
}

/// Ячейка тикера (базовая монета): клик открывает чарт монеты на Main на ядре строки.
/// Порт клика по тикеру из панели «Ордера».
fn coin_cell(e: &AssetEntry, view: &Entity<AssetsView>, p: MoonPalette) -> impl IntoElement + 'static {
    let coin = e.row.coin.clone();
    let core = e.core;
    let market = e.row.market.clone();
    let view = view.clone();
    div()
        .id(SharedString::from(format!("asset-coin-{core}-{}", e.row.market)))
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .cursor_pointer()
        .child(
            MoonText::new(coin)
                .color(MoonTone::Accent.color(p))
                .font_size(10.5)
                .line_height(14.0)
                .weight(500.0)
                .mono(true)
                .uppercase(false)
                .render(),
        )
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
}

/// Ячейка действий позиции: «Market sell» (panic-sell по рынку) + «Order» (заглушка — окно
/// настроек ордера будет позже). Показывается только у строк с открытой позицией.
fn actions_cell(
    e: &AssetEntry,
    view: &Entity<AssetsView>,
    _p: MoonPalette,
    is_position: bool,
) -> MoonDataCell {
    if !is_position || e.row.market.is_empty() {
        return MoonDataCell::text(String::new());
    }
    let core = e.core;
    let market = e.row.market.clone();
    let msell_id = SharedString::from(format!("asset-msell-{core}-{market}"));
    let order_id = SharedString::from(format!("asset-order-{core}-{market}"));
    let view_ms = view.clone();
    let market_ms = market.clone();
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
                .on_click(move |_, _w, app| {
                    view_ms.update(app, |this, cx| {
                        let b = this.backend.read(cx);
                        if let Err(err) =
                            b.session.panic_sell_market(core, market_ms.clone(), true)
                        {
                            log::warn!("assets market sell {market_ms} failed: {err:#}");
                        }
                        cx.notify();
                    });
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
