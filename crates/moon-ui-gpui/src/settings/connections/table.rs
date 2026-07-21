//! Таблица ядер вкладки «Подключения»: строки серверов (Акт·Окно·Имя·Ключ·Группа·
//! [Данные n/8]·Цвет·Удалить·↻реконнект·●статус), колонки/шапка, поповер фид-флагов
//! и add/delete сервера.

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckboxSize, MoonColorPicker, MoonDropdown,
    MoonInput, MoonInputState, MoonMenuItem, MoonMenuSize, MoonPalette, MoonText, MoonTone,
    MoonTooltipView, StyledExt, h_flex,
};
use rust_i18n::t;

use super::{ConnRow, SettingsView, build_conn, sync_groups_from_servers};
use crate::design;
use moon_core::config::{FeedFlags, Secret, ServerConfig};
use moon_core::feed::ConnStatus;
use moon_core::session::CoreId;

/// 8 фид-флагов приёма данных ядра (i18n-ключ подписи, геттер, сеттер) — для
/// поповера «Данные». Ключи `conn.tip.*` (подпись локализуется на use-сайте, см.
/// `feed_popover`); к каждой в поповере добавляется суффикс «(фильтр на клиенте)»
/// (`conn.filter_note`). Const хранит `&'static str` ключ, а не готовую строку.
const FEED_FLAGS: [(&str, fn(&FeedFlags) -> bool, fn(&mut FeedFlags, bool)); 8] = [
    ("conn.tip.orders", |f| f.orders, |f, v| f.orders = v),
    ("conn.tip.detects", |f| f.detects, |f, v| f.detects = v),
    ("conn.tip.reports", |f| f.reports, |f, v| f.reports = v),
    ("conn.tip.balance", |f| f.balance, |f, v| f.balance = v),
    ("conn.tip.strat", |f| f.strategies, |f, v| f.strategies = v),
    ("conn.tip.log", |f| f.log, |f, v| f.log = v),
    ("conn.tip.alerts", |f| f.alerts, |f, v| f.alerts = v),
    ("conn.tip.arb", |f| f.arb, |f, v| f.arb = v),
];

/// Render a connection-status dot with a localized tooltip, including failure details.
/// Inactive rows are always gray; live states use ready, connecting, and failure colors.
pub(super) fn status_dot(
    i: usize,
    active: bool,
    status: Option<&ConnStatus>,
    p: MoonPalette,
) -> impl IntoElement {
    let (color, tip) = match status {
        _ if !active => (p.text_soft, t!("conn.status.inactive").to_string()),
        Some(ConnStatus::Ready) => (p.green, t!("conn.status.ready").to_string()),
        Some(ConnStatus::Connecting) => (p.amber, t!("conn.status.connecting").to_string()),
        Some(ConnStatus::Stage(s)) => (p.amber, t!("conn.status.stage", stage = s).to_string()),
        Some(ConnStatus::Failed(e)) => (p.red, t!("conn.status.failed", err = e).to_string()),
        Some(ConnStatus::Disconnected) => (p.text_soft, t!("conn.status.disconnected").to_string()),
        None => (p.text_soft, t!("conn.status.none").to_string()),
    };
    div()
        .id(SharedString::from(format!("st-{i}")))
        .w(px(10.0))
        .h(px(10.0))
        .rounded_full()
        .bg(rgb(color))
        .tooltip(move |_window, cx| {
            cx.new(|_| MoonTooltipView::new(tip.clone()).max_width(320.0))
                .into()
        })
}

impl SettingsView {
    /// Checkbox булева поля сервера `servers[i]` (пишет в draft).
    fn srv_check(
        &self,
        cx: &Context<Self>,
        i: usize,
        suffix: &str,
        label: &'static str,
        get: fn(&ServerConfig) -> bool,
        set: fn(&mut ServerConfig, bool),
    ) -> impl IntoElement {
        let cur = {
            let b = self.backend.read(cx);
            b.preview
                .as_ref()
                .unwrap_or(&b.config)
                .servers
                .get(i)
                .map(get)
                .unwrap_or(false)
        };
        let mut checkbox = self
            .draft_checkbox(cx, format!("{suffix}-{i}"), cur, move |p, v| {
                if let Some(s) = p.servers.get_mut(i) {
                    if get(s) != v {
                        set(s, v);
                        return true;
                    }
                }
                false
            })
            .size(MoonCheckboxSize::Compact);
        if !label.is_empty() {
            checkbox = checkbox.label(label);
        }
        checkbox
    }

    /// Аффикс «Вставить» внутри поля ключа (стиль как встроенные глаз/×): глиф-кнопка,
    /// по клику берёт ключ из буфера обмена и кладёт в `servers[i].key` (и в стейт инпута —
    /// `set_value` не эмитит Change, поэтому пишем в draft напрямую).
    fn paste_key_affix(
        &self,
        i: usize,
        key_state: Entity<MoonInputState>,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        div()
            .id(SharedString::from(format!("paste-key-{i}")))
            .flex()
            .items_center()
            .justify_center()
            .px(px(2.0))
            .cursor_pointer()
            .tooltip(|_window, cx| {
                cx.new(|_| {
                    MoonTooltipView::new(t!("conn.paste_key_tip").to_string()).max_width(320.0)
                })
                .into()
            })
            .child(
                MoonText::new("⧉")
                    .color(p.text_muted)
                    .font_size(11.0)
                    .mono(true)
                    .uppercase(false)
                    .render(),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                let Some(text) = cx
                    .read_from_clipboard()
                    .and_then(|it| it.text())
                    .filter(|t| !t.trim().is_empty())
                else {
                    return;
                };
                let text = text.trim().to_string();
                key_state.update(cx, |st, c| st.set_value(text.clone(), window, c));
                this.backend.update(cx, |b, bcx| {
                    if let Some(pv) = b.preview.as_mut() {
                        if let Some(s) = pv.servers.get_mut(i) {
                            s.key = Secret::new(text.clone());
                            bcx.notify();
                        }
                    }
                });
            }))
    }

    /// Добавить сервер в draft (id = max+1) в указанную группу и пересобрать editor-стейты.
    pub(super) fn add_server(
        &mut self,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let default_color = design::u32_to_rgb(MoonPalette::active(cx).accent);
        self.backend.update(cx, |b, bcx| {
            if let Some(p) = b.preview.as_mut() {
                let next = p.servers.iter().map(|s| s.id).max().unwrap_or(0) + 1;
                p.servers.push(ServerConfig {
                    id: next,
                    uid: 0,
                    name: format!("server {next}"),
                    active: true,
                    show_window: true,
                    feed: FeedFlags::default(),
                    key: Secret::new(""),
                    group,
                    market: "BTCUSDT".into(),
                    color: default_color,
                    synthetic: false,
                    chart_bundle: String::new(),
                    order_sizes: None,
                    order_size_sel: None,
                    default_alert_strategy: 0,
                });
                sync_groups_from_servers(p);
                bcx.notify();
            }
        });
        let rows = build_conn(&self.backend, window, cx);
        self.conn = rows;
        cx.notify();
    }

    /// Удалить сервер `i` из draft и пересобрать editor-стейты.
    fn delete_server(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.backend.update(cx, |b, bcx| {
            if let Some(p) = b.preview.as_mut() {
                if i < p.servers.len() {
                    p.servers.remove(i);
                    sync_groups_from_servers(p);
                    bcx.notify();
                }
            }
        });
        let rows = build_conn(&self.backend, window, cx);
        self.conn = rows;
        cx.notify();
    }

    /// Поповер «Данные n/8» (порт egui `feed_button`): кнопка с числом включённых
    /// фид-флагов ядра; клик раскрывает 8 чекбоксов приёма данных (пишут в draft).
    fn feed_popover(&self, cx: &Context<Self>, i: usize) -> impl IntoElement {
        let feed = {
            let b = self.backend.read(cx);
            let s = b.preview.as_ref().unwrap_or(&b.config).servers.get(i);
            s.map(|s| s.feed.clone()).unwrap_or_default()
        };
        let on = FEED_FLAGS.iter().filter(|(_, g, _)| g(&feed)).count();
        let tinted = on < FEED_FLAGS.len();

        let mut items = Vec::new();
        for (ix, (key, get, set)) in FEED_FLAGS.iter().copied().enumerate() {
            let cur = get(&feed);
            let backend = self.backend.clone();
            items.push(
                MoonMenuItem::with_key(
                    format!("feed-{i}-{ix}"),
                    format!("{} ({})", t!(key), t!("conn.filter_note")),
                )
                .checked(cur)
                // Явное различие вкл/выкл: включённые — зелёные с галочкой, выключенные —
                // приглушённые без галочки (чтобы сразу видеть, какой пункт отключён, а не
                // угадывать по счётчику «7/8»).
                .tone(if cur {
                    MoonTone::Positive
                } else {
                    MoonTone::Muted
                })
                .on_click(move |_, _, cx| {
                    backend.update(cx, |b, bcx| {
                        if let Some(p) = b.preview.as_mut() {
                            if let Some(s) = p.servers.get_mut(i) {
                                set(&mut s.feed, !cur);
                                bcx.notify();
                            }
                        }
                    });
                }),
            );
        }

        MoonDropdown::new(SharedString::from(format!("feed-{i}")))
            .label(format!("{on}/8"))
            .trigger_variant(if tinted {
                MoonButtonVariant::Amber
            } else {
                MoonButtonVariant::Neutral
            })
            .trigger_size(MoonButtonSize::Micro)
            .trigger_width(design::font_w(cx, 52.0))
            .menu_width(design::font_w(cx, 272.0))
            .menu_size(MoonMenuSize::Compact)
            .close_on_select(false)
            .items(items)
    }

    /// Строка сервера в таблице (порт egui `servers_panel` row): Акт·Окно·Имя·Ключ·
    /// Группа·[Данные]·Цвет·Удалить·↻реконнект·●статус.
    pub(super) fn server_row(
        &self,
        cx: &Context<Self>,
        i: usize,
        row: &ConnRow,
        core_id: CoreId,
        active: bool,
        status: Option<ConnStatus>,
    ) -> impl IntoElement {
        // Реконнект — только для активных ядер (у неактивных нет сессии).
        let recon: AnyElement = if active {
            div()
                .id(SharedString::from(format!("rec-tip-{i}")))
                .tooltip(|_window, cx| {
                    cx.new(|_| MoonTooltipView::new(t!("conn.reconnect").to_string()))
                        .into()
                })
                .child(
                    MoonButton::new(SharedString::from(format!("rec-{i}")))
                        .ghost()
                        .size(MoonButtonSize::Micro)
                        .width(24.0)
                        .label("↻")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.backend.update(cx, |b, bcx| {
                                b.reconnect_request.push(core_id);
                                bcx.notify();
                            });
                        }))
                        .render(),
                )
                .into_any_element()
        } else {
            div().w(px(24.0)).into_any_element()
        };
        h_flex()
            .w_full()
            .gap_1()
            .items_center()
            .py_0p5()
            .child(Self::cell(28.0, false).child(self.srv_check(
                cx,
                i,
                "act",
                "",
                |s| s.active,
                |s, v| s.active = v,
            )))
            .child(Self::cell(34.0, false).child(self.srv_check(
                cx,
                i,
                "win",
                "",
                |s| s.show_window,
                |s, v| s.show_window = v,
            )))
            .child(
                Self::cell(150.0, true).child(
                    MoonInput::new(SharedString::from(format!("name-{i}")))
                        .state(&row.name)
                        .small(),
                ),
            )
            .child(
                Self::cell(200.0, true).child(
                    // Поле ключа + ⧉-кнопка «Вставить» РЯДОМ (не в компоненте — порядок встроенных
                    // аффиксов задаёт форк, между глаз/× не встроить). set_value не эмитит Change →
                    // affix пишет и в draft.
                    h_flex()
                        .w_full()
                        .gap_1()
                        .items_center()
                        .child(
                            div().flex_grow_1().min_w_0().child(
                                MoonInput::new(SharedString::from(format!("key-{i}")))
                                    .state(&row.key)
                                    .small()
                                    // Placeholder намекает, что сюда ждём ключ ядра.
                                    .placeholder(t!("conn.key_ph").to_string())
                                    .mask_toggle()
                                    // Кнопка очистки (×) — быстро удалить/заменить ключ.
                                    .cleanable(true),
                            ),
                        )
                        .child(self.paste_key_affix(i, row.key.clone(), cx)),
                ),
            )
            .child(
                Self::cell(110.0, false).child(
                    MoonInput::new(SharedString::from(format!("group-{i}")))
                        .state(&row.group)
                        .small(),
                ),
            )
            .child(
                Self::cell(96.0, false).child(
                    MoonInput::new(SharedString::from(format!("bundle-{i}")))
                        .state(&row.bundle)
                        .small(),
                ),
            )
            .child(Self::cell(52.0, false).child(self.feed_popover(cx, i)))
            .child(Self::cell(110.0, false).child(MoonColorPicker::new(&row.color)))
            .child(
                Self::cell(24.0, false).child(
                    MoonButton::new(SharedString::from(format!("del-{i}")))
                        .danger()
                        .size(MoonButtonSize::Micro)
                        .width(24.0)
                        .label("x")
                        .on_click(cx.listener(move |this, _, w, cx| this.delete_server(i, w, cx)))
                        .render(),
                ),
            )
            .child(Self::cell(24.0, false).child(recon))
            .child(Self::cell(16.0, false).child(status_dot(
                i,
                active,
                status.as_ref(),
                MoonPalette::active(cx),
            )))
    }

    /// Ячейка-колонка таблицы ядер: ОДИН flex-спек, общий для шапки и строк (ключ к тому,
    /// чтобы колонки не съезжали — обе раскладки тянутся/жмутся одинаково). `grow=true` —
    /// растягивается под ширину (flex-grow, shrink по умолчанию); `grow=false` — фикс. ширина
    /// (`flex-grow:0`+`flex-shrink:0`). `basis` = базовая ширина колонки.
    fn cell(basis: f32, grow: bool) -> Div {
        let d = div().flex_basis(px(basis));
        if grow {
            d.flex_grow_1()
        } else {
            d.flex_grow_0().flex_shrink_0()
        }
    }

    /// Заголовок колонки с тултипом (порт egui `head_tip`). Подпись помечена подчёркиванием
    /// + чуть ярче цветом — сигнал «наведи, есть подсказка». `pad`/`grow` — как в `col_head`.
    /// Тултип — штатный `MoonTooltipView` движка; длинный текст переносится внутри max width.
    fn col_head_tip(
        id: &'static str,
        label: &str,
        basis: f32,
        grow: bool,
        pad: f32,
        tip: SharedString,
        p: MoonPalette,
        cx: &App,
    ) -> impl IntoElement {
        Self::cell(basis, grow)
            .id(id)
            .child(
                div()
                    .ml(px(pad))
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text))
                    .underline()
                    .text_decoration_color(rgb(p.text_soft))
                    .child(label.to_string()),
            )
            .tooltip(move |_window, cx| {
                cx.new(|_| MoonTooltipView::new(tip.clone()).max_width(320.0))
                    .into()
            })
    }

    /// Подпись-«есть подсказка» произвольной ширины (не колонка): подчёркивание + переносящий
    /// тултип. Для заголовков секций/групп, где надо пояснить смысл при наведении.
    pub(super) fn hint_label(
        id: &'static str,
        label: impl Into<SharedString>,
        tip: SharedString,
        p: MoonPalette,
    ) -> impl IntoElement {
        div()
            .id(id)
            .font_bold()
            .text_color(rgb(p.text))
            .underline()
            .text_decoration_color(rgb(p.text_soft))
            .child(label.into())
            .tooltip(move |_window, cx| {
                cx.new(|_| MoonTooltipView::new(tip.clone()).max_width(360.0))
                    .into()
            })
    }

    /// Шапка колонок таблицы ядер: тот же левый отступ (`pl 20`), что у заголовка группы,
    /// чтобы колонки строк вставали ровно под подписями. Хвостовые плейсхолдеры
    /// (цвет/удалить/реконнект/статус) ОБЯЗАТЕЛЬНЫ — иначе растяжимые колонки шапки съедут.
    pub(super) fn conn_col_head_row(p: MoonPalette, cx: &App) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap_1()
            .items_center()
            .pl(px(20.0))
            .child(Self::col_head_tip(
                "h-act",
                &t!("conn.col.act"),
                28.0,
                false,
                0.0,
                t!("conn.tip.act").to_string().into(),
                p,
                cx,
            ))
            .child(Self::col_head_tip(
                "h-win",
                &t!("conn.col.win"),
                34.0,
                false,
                0.0,
                t!("conn.tip.win").to_string().into(),
                p,
                cx,
            ))
            .child(Self::col_head_tip(
                "h-name",
                &t!("conn.col.name"),
                150.0,
                true,
                8.0,
                t!("conn.tip.name").to_string().into(),
                p,
                cx,
            ))
            .child(Self::col_head_tip(
                "h-key",
                &t!("conn.col.key"),
                200.0,
                true,
                8.0,
                t!("conn.tip.key").to_string().into(),
                p,
                cx,
            ))
            .child(Self::col_head_tip(
                "h-group",
                &t!("conn.col.group"),
                110.0,
                false,
                8.0,
                t!("conn.tip.group").to_string().into(),
                p,
                cx,
            ))
            .child(Self::col_head_tip(
                "h-bundle",
                &t!("conn.col.bundle"),
                96.0,
                false,
                8.0,
                t!("conn.tip.bundle").to_string().into(),
                p,
                cx,
            ))
            .child(Self::col_head_tip(
                "h-data",
                &t!("conn.col.data"),
                52.0,
                false,
                0.0,
                t!("conn.tip.flags").to_string().into(),
                p,
                cx,
            ))
            // Хвостовые плейсхолдеры под колонки строки (цвет/удалить/реконнект/статус).
            .child(Self::cell(110.0, false))
            .child(Self::cell(24.0, false))
            .child(Self::cell(24.0, false))
            .child(Self::cell(16.0, false))
    }
}
