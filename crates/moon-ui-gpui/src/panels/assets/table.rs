//! Верх окна «Активы»: панель управления (счётчик/«показать всё»/итоги), полоса ядер
//! (баланс USDT), таблица позиций/балансов и нижний список ядер (свободно/итого).

use super::*;
use moon_ui::{MoonNotification, MoonWindowExt as _};
use rust_i18n::t;

/// Ширина карточки ядра в полосе (`core_strip`). Фиксирована → карточки при переносе
/// выстраиваются ровной сеткой (равная ширина = ровные колонки, без «лесенки»).
const CORE_CARD_W: f32 = 148.0;

/// Цвет знаковой величины (PnL): зелёный/красный/приглушённый.
fn tone(v: f64, p: MoonPalette) -> u32 {
    if v > 0.0 {
        p.green
    } else if v < 0.0 {
        p.red
    } else {
        p.text_muted
    }
}

impl AssetsView {
    /// Верхняя строка над таблицей позиций: счётчик строк, галка «показать всё», Σ стоимость.
    pub(super) fn controls(
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

    /// Сворачиваемая полоса ядер внизу: строка-шапка (кол-во ядер + Σ баланс + Σ PnL +
    /// стрелка ▾/▸), под ней — сетка карточек с вертикальным скроллом. Свёрнуто = только
    /// строка-итог (карточки скрыты, «не мешают»). Клик по шапке тогает.
    pub(super) fn core_strip(
        &self,
        aggs: &[CoreAgg],
        total_pnl: f64,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let total_balance: f64 = aggs.iter().map(|a| a.total).sum();
        let pnl_tone = tone(total_pnl, p);
        let collapsed = self.plates_collapsed;
        let arrow = if collapsed { "▸" } else { "▾" };

        // Этот PnL — read-only серверный Markets.FTotalPNL/total_pnl. Delphi ResetProfit
        // сбрасывает RepForm-счётчики отчёта, не это значение, поэтому кнопок сброса тут нет.
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
            )
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(pnl_tone))
                    .child(format!("PnL {}", money(total_pnl))),
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
            section = section.flex_1().min_h(px(0.0)).child(
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

    /// Одна карточка ядра фикс. ширины (`CORE_CARD_W`): имя сверху, «итого + PnL» снизу.
    /// Равная ширина → при переносе карточки ложатся ровными колонками.
    fn core_card(&self, a: &CoreAgg, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        v_flex()
            .w(design::ui_px(cx, CORE_CARD_W))
            .flex_none()
            .gap(px(1.0))
            .px(design::ui_px(cx, 8.0))
            .py(design::ui_px(cx, 4.0))
            .rounded(px(4.0))
            .bg(rgb(p.shell_high))
            .border_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .truncate()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text))
                    .child(a.name.clone()),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .text_size(design::t_body(cx))
                    .child(div().text_color(rgb(p.text_soft)).child(money(a.total)))
                    .child(div().text_color(rgb(tone(a.pnl, p))).child(money(a.pnl))),
            )
    }

    /// Нижняя секция: слева список ядер (имя + свободно/итого, выбор), справа —
    /// 3 контейнера кошельков выбранного ядра с переносом.
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

        let left = v_flex()
            .w(px(240.0))
            .h_full()
            .flex_none()
            .border_r_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .w_full()
                    .px(design::ui_px(cx, 8.0))
                    .py(design::ui_px(cx, 4.0))
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_muted))
                    .child(t!("assets.cores_free_total").to_string()),
            )
            .child(
                div()
                    .id("asset-core-list")
                    .flex_1()
                    .w_full()
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

        h_flex()
            .w_full()
            .h(px(380.0))
            .flex_none()
            .border_t_1()
            .border_color(rgb(p.border))
            .child(left)
            .child(div().flex_1().h_full().min_w_0().child(right))
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
    ]
}

pub(super) fn assets_table(
    id: &'static str,
    rows: Rc<Vec<AssetEntry>>,
    cx: &Context<AssetsView>,
) -> impl IntoElement {
    let empty = rows.is_empty();
    let row_count = rows.len();
    let table_rows = rows.clone();
    let p = MoonPalette::active(cx);

    crate::panels::common::data_table_host(
        SharedString::from(format!("{id}-host")),
        empty,
        t!("assets.empty").to_string(),
        p,
        cx,
        MoonDataTable::new(id, row_count, move |ix, _window, _app| {
            assets_row(&table_rows[ix], p)
        })
        .columns(assets_columns())
        .header_height(design::TABLE_HEAD_H)
        .row_height(design::TABLE_ROW_H),
    )
}

fn assets_row(e: &AssetEntry, _p: MoonPalette) -> MoonDataRow {
    let r = &e.row;
    let pnl = r.profit_b + r.profit_l + r.profit_s;
    let pnl_tone = if pnl > 0.0 {
        MoonTone::Positive
    } else if pnl < 0.0 {
        MoonTone::Danger
    } else {
        MoonTone::Muted
    };
    let pos = if r.pos_size != 0.0 {
        num(r.pos_size)
    } else {
        String::new()
    };
    let pos_price = if r.pos_size != 0.0 {
        num(r.pos_price)
    } else {
        String::new()
    };
    MoonDataRow::new([
        MoonDataCell::text(e.core_name.clone()).tone(MoonTone::Muted),
        MoonDataCell::text(r.coin.clone())
            .tone(MoonTone::Accent)
            .weight(500.0),
        MoonDataCell::text(num(r.qty)),
        MoonDataCell::text(num(r.price)),
        MoonDataCell::text(money(e.value)),
        MoonDataCell::text(pos),
        MoonDataCell::text(pos_price),
        MoonDataCell::text(money(pnl)).tone(pnl_tone),
        MoonDataCell::text(kind_label(r)).tone(MoonTone::Muted),
    ])
}
