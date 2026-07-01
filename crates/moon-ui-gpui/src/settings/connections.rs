//! Вкладка «Подключения» — порт egui `settings/connections.rs`: слева таблица ядер
//! (Акт·Окно·Имя·Ключ·Группа·[Данные n/8]·Цвет·Удалить·↻реконнект·●статус), справа
//! панель групп (галка·иконка·имя·👁показать·выбор иконки + пикер). Над ними — источник
//! рыночных данных (выпадающий). Правки идут в draft; статус/реконнект — через `Backend`.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonColorPicker,
    MoonColorPickerState, MoonDropdown, MoonInput, MoonInputEvent, MoonInputState, MoonMenuItem,
    MoonMenuSize, MoonPalette, MoonSelect, MoonTooltipView, StyledExt, h_flex, v_flex,
};
use rust_i18n::t;

use super::SettingsView;
use crate::{Backend, design};
use moon_core::config::{AppConfig, FeedFlags, GroupConfig, Secret, ServerConfig};
use moon_core::feed::ConnStatus;
use moon_core::session::CoreId;

/// Редактор одной строки сервера: текст-поля + цвет (entity-стейты компонентов).
pub(super) struct ConnRow {
    name: Entity<MoonInputState>,
    key: Entity<MoonInputState>,
    group: Entity<MoonInputState>,
    /// Имя чарт-связки AddToChart (пусто = по глоб. настройке). См. `ServerConfig::chart_bundle`.
    bundle: Entity<MoonInputState>,
    color: Entity<MoonColorPickerState>,
}

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

pub(super) fn sync_groups_from_servers(cfg: &mut AppConfig) -> bool {
    let mut names: Vec<String> = cfg.servers.iter().map(|s| s.group.clone()).collect();
    names.sort();
    names.dedup();

    let mut changed = false;
    cfg.groups.retain(|g| {
        let keep = names.contains(&g.name);
        changed |= !keep;
        keep
    });
    for name in names {
        if !cfg.groups.iter().any(|g| g.name == name) {
            cfg.groups.push(GroupConfig::new(name));
            changed = true;
        }
    }
    changed
}

/// TextInput, привязанный к полю сервера `servers[i]` (пишет в draft).
fn conn_input(
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    i: usize,
    init: String,
    get: fn(&ServerConfig) -> String,
    set: fn(&mut ServerConfig, String),
    sync_groups: bool,
) -> Entity<MoonInputState> {
    let st = cx.new(|cx| MoonInputState::new(window, cx).default_value(init));
    cx.subscribe(&st, move |this, emitter, ev: &MoonInputEvent, cx| {
        if matches!(ev, MoonInputEvent::Change) {
            let val = emitter.read(cx).value().to_string();
            this.backend.update(cx, |b, bcx| {
                if let Some(p) = b.preview.as_mut() {
                    if let Some(s) = p.servers.get_mut(i) {
                        if get(s) != val {
                            set(s, val);
                            if sync_groups {
                                sync_groups_from_servers(p);
                            }
                            bcx.notify();
                        }
                    }
                }
            });
        }
    })
    .detach();
    st
}

/// Color-picker, привязанный к `servers[i].color` (пишет в draft).
fn conn_color(
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    i: usize,
    init: [u8; 3],
) -> Entity<MoonColorPickerState> {
    super::draft_color(window, cx, init, move |p, c| {
        if let Some(s) = p.servers.get_mut(i) {
            if s.color != c {
                s.color = c;
                return true;
            }
        }
        false
    })
}

/// Построить per-server editor-стейты из draft-серверов. Зовётся в `SettingsView::new`
/// и после add/remove сервера (индексы в подписках свежие).
pub(super) fn build_conn(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> Vec<ConnRow> {
    let servers = {
        let b = backend.read(cx);
        b.preview.as_ref().unwrap_or(&b.config).servers.clone()
    };
    servers
        .iter()
        .enumerate()
        .map(|(i, s)| ConnRow {
            name: conn_input(
                window,
                cx,
                i,
                s.name.clone(),
                |s| s.name.clone(),
                |s, v| s.name = v,
                false,
            ),
            // Ключ — поле пароля (порт egui `.password(true)`): символы скрыты, рядом
            // переключатель видимости (mask_toggle), чтобы при необходимости показать.
            key: {
                let st = conn_input(
                    window,
                    cx,
                    i,
                    s.key.expose().to_string(),
                    |s| s.key.expose().to_string(),
                    |s, v| s.key = Secret::new(v),
                    false,
                );
                st.update(cx, |st, c| st.set_masked(true, window, c));
                st
            },
            group: conn_input(
                window,
                cx,
                i,
                s.group.clone(),
                |s| s.group.clone(),
                |s, v| s.group = v,
                true,
            ),
            bundle: conn_input(
                window,
                cx,
                i,
                s.chart_bundle.clone(),
                |s| s.chart_bundle.clone(),
                |s, v| s.chart_bundle = v,
                false,
            ),
            color: conn_color(window, cx, i, s.color),
        })
        .collect()
}

/// Кружок статуса подключения ядра (порт egui `status_dot`): зелёный=Ready, акцент=
/// подключается, красный=ошибка, серый=неактивно/нет. `active=false` → всегда серый.
/// Тултип поясняет состояние (для Failed — текст ошибки), как egui `on_hover_text`.
fn status_dot(
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

    /// Добавить сервер в draft (id = max+1) в указанную группу и пересобрать editor-стейты.
    fn add_server(&mut self, group: String, window: &mut Window, cx: &mut Context<Self>) {
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
            .trigger_width(52.0)
            .menu_width(272.0)
            .menu_size(MoonMenuSize::Compact)
            .close_on_select(false)
            .items(items)
    }

    /// Строка сервера в таблице (порт egui `servers_panel` row): Акт·Окно·Имя·Ключ·
    /// Группа·[Данные]·Цвет·Удалить·↻реконнект·●статус.
    fn server_row(
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
                    MoonInput::new(SharedString::from(format!("key-{i}")))
                        .state(&row.key)
                        .small()
                        .mask_toggle()
                        // Кнопка очистки (×) — быстро удалить/заменить ключ. Авто-выделение всей
                        // строки при фокусе недоступно из moon_ui (select_all приватный в форке).
                        .cleanable(true),
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

    /// Заголовок колонки (тусклая подпись). `pad` — левый отступ ТЕКСТА (через внутренний
    /// margin) под внутренний отступ инпута (`px_2`≈8px у MoonInput), чтобы подпись стояла
    /// над текстом поля; margin внутреннего блока НЕ меняет ширину колонки. `grow` — как в `cell`.
    fn col_head(
        label: &str,
        basis: f32,
        grow: bool,
        pad: f32,
        p: MoonPalette,
        cx: &App,
    ) -> impl IntoElement {
        Self::cell(basis, grow).child(
            div()
                .ml(px(pad))
                .text_size(design::t_body(cx))
                .text_color(rgb(p.text_soft))
                .child(label.to_string()),
        )
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
    fn hint_label(
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
    fn conn_col_head_row(p: MoonPalette, cx: &App) -> impl IntoElement {
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
            .child(Self::col_head(
                &t!("conn.col.name"),
                150.0,
                true,
                8.0,
                p,
                cx,
            ))
            .child(Self::col_head(&t!("conn.col.key"), 200.0, true, 8.0, p, cx))
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

    /// Заголовок-ветка группы: галка active · иконка · имя · кол-во · win · Иконка · +ядро.
    /// `ico_el` готовится снаружи (берёт `&mut self.icons` для текстуры).
    fn group_header_row(
        &mut self,
        name: &str,
        active: bool,
        ico_el: AnyElement,
        member_count: usize,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let nm_act = name.to_string();
        let nm_eye = name.to_string();
        let nm_pick = name.to_string();
        let nm_add = name.to_string();
        h_flex()
            .w_full()
            .gap_1()
            .items_center()
            .px_1()
            .py_0p5()
            .rounded(px(4.0))
            .bg(rgb(p.panel_high))
            .child(
                MoonCheckbox::new(SharedString::from(format!("grp-{name}")))
                    .checked(active)
                    .size(MoonCheckboxSize::Compact)
                    .on_change(cx.listener(move |this, ch: &bool, _w, cx| {
                        let v = *ch;
                        let n = nm_act.clone();
                        this.backend.update(cx, |b, bcx| {
                            if let Some(p) = b.preview.as_mut() {
                                if let Some(gc) = p.groups.iter_mut().find(|g| g.name == n) {
                                    gc.active = v;
                                    bcx.notify();
                                }
                            }
                        });
                        cx.notify();
                    })),
            )
            .child(ico_el)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_bold()
                    .child(name.to_string()),
            )
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_soft))
                    .child(t!("conn.member_count", n = member_count).to_string()),
            )
            .child(
                div()
                    .id(SharedString::from(format!("eye-tip-{name}")))
                    .tooltip(|_window, cx| {
                        cx.new(|_| MoonTooltipView::new(t!("conn.show_group").to_string()))
                            .into()
                    })
                    .child(
                        MoonButton::new(SharedString::from(format!("eye-{name}")))
                            .ghost()
                            .size(MoonButtonSize::Micro)
                            .width(34.0)
                            .label("win")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let n = nm_eye.clone();
                                this.backend.update(cx, |b, bcx| {
                                    b.show_group_request.push(n);
                                    bcx.notify();
                                });
                            }))
                            .render(),
                    ),
            )
            .child(
                MoonButton::new(SharedString::from(format!("pick-{name}")))
                    .outline()
                    .size(MoonButtonSize::Micro)
                    .width(54.0)
                    .label(t!("conn.icon_btn").to_string())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.picking = Some(nm_pick.clone());
                        cx.notify();
                    }))
                    .render(),
            )
            .child(
                MoonButton::new(SharedString::from(format!("addgrp-{name}")))
                    .outline()
                    .size(MoonButtonSize::Micro)
                    .width(56.0)
                    .label(format!("+ {}", t!("conn.add_core_short")))
                    .on_click(
                        cx.listener(move |this, _, w, cx| this.add_server(nm_add.clone(), w, cx)),
                    )
                    .render(),
            )
    }

    /// Пикер иконок под выбранной группой: строка-заголовок с «×» + скроллируемая сетка
    /// иконок (клик = назначить иконку группе и закрыть). Возвращает обе строки списком,
    /// чтобы вставить их в дерево без обёртки-контейнера.
    fn icon_picker_rows(
        &mut self,
        name: &str,
        pick_ids: &[u32],
        icon_tex: &HashMap<u32, Option<Arc<RenderImage>>>,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut grid = h_flex().w_full().flex_wrap().gap_1();
        for id in pick_ids.iter().copied() {
            let cell: AnyElement = match icon_tex.get(&id).and_then(|t| t.clone()) {
                Some(arc) => img(arc)
                    .w(design::ui_px(cx, 22.0))
                    .h(design::ui_px(cx, 22.0))
                    .into_any_element(),
                None => continue,
            };
            let nm = name.to_string();
            grid = grid.child(
                div()
                    .id(SharedString::from(format!("ico-{id}")))
                    .p_0p5()
                    .cursor_pointer()
                    .rounded(design::ui_px(cx, 4.0))
                    .hover(move |s| s.bg(rgb(p.panel_high)))
                    .child(cell)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let n = nm.clone();
                        this.backend.update(cx, |b, bcx| {
                            if let Some(p) = b.preview.as_mut() {
                                if let Some(g) = p.groups.iter_mut().find(|g| g.name == n) {
                                    g.icon = id;
                                    bcx.notify();
                                }
                            }
                        });
                        this.picking = None;
                        cx.notify();
                    })),
            );
        }
        vec![
            h_flex()
                .w_full()
                .items_center()
                .gap_1()
                .pl(px(20.0))
                .child(
                    div()
                        .flex_1()
                        .text_size(design::t_body(cx))
                        .text_color(rgb(p.text_soft))
                        .child(t!("conn.icon_for", name = name).to_string()),
                )
                .child(
                    MoonButton::new("pick-close")
                        .ghost()
                        .size(MoonButtonSize::Micro)
                        .width(24.0)
                        .label("x")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.picking = None;
                            cx.notify();
                        }))
                        .render(),
                )
                .into_any_element(),
            div()
                .id("icon-picker")
                .pl(px(20.0))
                .max_h(px(220.0))
                .overflow_y_scroll()
                .child(grid)
                .into_any_element(),
        ]
    }

    /// Источник рыночных данных — выпадающий список (порт egui ComboBox).
    fn market_src_selector(&self) -> impl IntoElement {
        h_flex()
            .gap_2()
            .items_center()
            .child(
                div()
                    .id("market-src-lbl")
                    .font_bold()
                    .child(t!("conn.market_src").to_string())
                    .tooltip(|_window, cx| {
                        cx.new(|_| {
                            MoonTooltipView::new(t!("conn.market_src_tip").to_string())
                                .max_width(420.0)
                        })
                        .into()
                    }),
            )
            .child(
                div().w(px(260.0)).child(
                    MoonSelect::new(&self.mode)
                        .trigger_size(MoonButtonSize::Action)
                        .menu_width(260.0)
                        .menu_size(MoonMenuSize::Compact),
                ),
            )
    }

    /// Вкладка «Подключения» — порт egui `settings/connections.rs`: источник данных
    /// (выпадающий), таблица ядер слева, панель групп (с иконками/👁/пикером) справа.
    pub(super) fn connections_tab(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        // Живой статус ядер для точек.
        let status = self.backend.read(cx).session.status_map();
        // Снимки серверов (id, active, группа) и групп (name, active, icon).
        let (servers, mut groups) = {
            let b = self.backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            (
                d.servers
                    .iter()
                    .map(|s| (s.id, s.active, s.group.clone()))
                    .collect::<Vec<_>>(),
                d.groups
                    .iter()
                    .map(|g| (g.name.clone(), g.active, g.icon))
                    .collect::<Vec<_>>(),
            )
        };
        // Стабильный порядок групп — по имени (заголовки-ветки в списке ядер).
        groups.sort_by(|a, b| a.0.cmp(&b.0));
        // Предзагрузить иконки (групп + весь набор, если открыт пикер) — texture() берёт
        // &mut self.icons, поэтому грузим ДО построения UI, потом читаем из карты.
        let picking = self.picking.clone();
        let mut icon_tex: HashMap<u32, Option<Arc<RenderImage>>> = HashMap::new();
        for (_, _, icon) in &groups {
            icon_tex
                .entry(*icon)
                .or_insert_with(|| self.icons.texture(*icon));
        }
        let pick_ids: Vec<u32> = if picking.is_some() {
            self.icons.ids.clone()
        } else {
            Vec::new()
        };
        for id in &pick_ids {
            icon_tex
                .entry(*id)
                .or_insert_with(|| self.icons.texture(*id));
        }

        // ── Единый список-«дерево»: заголовок-ветка группы + ядра-листья под ней ──
        // (Не настоящий tree: ветки/листья задаём отступом, без раскрытия.) Колонки
        // ядер выровнены под заголовком группы: шапка колонок с тем же левым отступом.
        let mut list_col = v_flex()
            .w_full()
            .min_w_0()
            .gap_1()
            .child(Self::hint_label(
                "h-section",
                t!("conn.groups_panel_heading").to_string(),
                t!("conn.groups_panel_tip").to_string().into(),
                p,
            ))
            .child(Self::conn_col_head_row(p, cx));

        // Нет групп (ни у одного ядра не задана) → поясняющий хинт.
        if groups.is_empty() {
            list_col = list_col.child(
                div()
                    .text_color(rgb(p.text_soft))
                    .child(t!("conn.no_groups").to_string()),
            );
        }

        for (name, active, icon) in &groups {
            let member_count = servers.iter().filter(|(_, _, g)| g == name).count();
            let ico_el: AnyElement = match icon_tex.get(icon).and_then(|t| t.clone()) {
                Some(arc) => img(arc)
                    .w(design::ui_px(cx, 20.0))
                    .h(design::ui_px(cx, 20.0))
                    .into_any_element(),
                None => div()
                    .w(design::ui_px(cx, 20.0))
                    .h(design::ui_px(cx, 20.0))
                    .into_any_element(),
            };
            list_col =
                list_col.child(self.group_header_row(name, *active, ico_el, member_count, p, cx));
            // ── Ядра-листья этой группы (с отступом + вертикальная линия ветки) ──
            // Неактивные сервера — вниз (стабильная сортировка: порядок внутри групп сохраняется).
            // `i` — исходный индекс в config.servers (нужен для мутаций draft), его сохраняем.
            let mut members: Vec<(usize, &(u64, bool, String))> = servers
                .iter()
                .enumerate()
                .filter(|(_, (_, _, g))| g == name)
                .collect();
            members.sort_by_key(|(_, (_, active, _))| !*active);
            for (i, (id, srv_active, _g)) in members {
                if let Some(row) = self.conn.get(i) {
                    let st = status.get(id).cloned();
                    list_col = list_col.child(
                        div()
                            .ml(px(8.0))
                            .pl(px(11.0))
                            .border_l_1()
                            .border_color(rgb(p.border))
                            .child(self.server_row(cx, i, row, *id, *srv_active, st)),
                    );
                }
            }
            // ── Пикер иконок под выбранной группой ──
            if picking.as_deref() == Some(name.as_str()) {
                list_col =
                    list_col.children(self.icon_picker_rows(name, &pick_ids, &icon_tex, p, cx));
            }
        }

        // Глобальная кнопка: новое ядро в группу «default» (дальше можно переписать «Группу»).
        list_col = list_col.child(
            MoonButton::new("add-srv")
                .outline()
                .small()
                .width(220.0)
                .label(format!("+ {}", t!("conn.add_core")))
                .on_click(cx.listener(|this, _, w, cx| this.add_server("default".into(), w, cx)))
                .render(),
        );

        v_flex()
            .w_full()
            .gap_2()
            // Источник рыночных данных — выпадающий список (порт egui ComboBox).
            .child(self.market_src_selector())
            .child(list_col)
    }
}
