//! Панель «Алерты» — откпрепляемая док-панель (как Ордера/Лог/Отчёт). Список всех
//! chart-алертов (нарисованных фигур с галкой Alert — локальных и созданных в ядре)
//! ядер ГРУППЫ: Ядро/Монета/Фигура/Цена/Время + удаление (Clear); клик по монете
//! открывает её график. Внизу — настройки звука (длительность/повтор, UI).
//!
//! Звук алерта/детекта берётся из стратегии (см. `detect_sound`), поэтому «Выбор
//! звука» здесь — глобальный дефолт для алертов БЕЗ стратегии (пока не задействован).

use gpui::prelude::FluentBuilder;
use gpui::*;
use std::collections::HashMap;

use moon_ui::{
    DockArea, MoonButton, MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonInput,
    MoonInputEvent, MoonInputState, MoonMenuSize, MoonPalette, Panel, PanelEvent, PanelState,
    h_flex, v_flex,
};
use rust_i18n::t;

use moon_core::figures::FigureKind;
use moon_core::session::CoreId;

use crate::design::moon;
use crate::panels::{RadioMark, radio_items};
use crate::{Backend, design};

/// Ordinal вида стратегии «Alerts» (только их назначаем алертам). Из moonproto
/// `StrategyKindId::ALERTS = 22`.
const ALERTS_KIND: u8 = 22;

/// Единая высота контролов нижней панели (px) — чтобы степперы/выпадашки/поле стояли ровно.
const CTRL_H: f32 = 26.0;

#[derive(Clone)]
struct Row {
    core: CoreId,
    core_name: String,
    market: String,
    figure: String,
    price: f64,
    time_ms: i64,
    from_server: bool,
    /// id алерта (для назначения стратегии/удаления).
    fig_id: u64,
    /// Привязанная стратегия (0 = без).
    strategy_id: u64,
}

pub struct AlertsPanel {
    backend: Entity<Backend>,
    group: String,
    rows: Vec<Row>,
    coin_input: Entity<MoonInputState>,
    duration_s: u32,
    repeat: u32,
    /// Стратегии вида «Alerts» по ядру (id, имя) — для выпадашки назначения.
    alert_strategies: HashMap<CoreId, Vec<(u64, String)>>,
    /// Док-область (для кнопки открепления в своё окно). Ставится в `on_added_to`.
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl AlertsPanel {
    pub fn new(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let coin_input = cx.new(|cx| MoonInputState::new(window, cx).placeholder("BTC, ETH"));
        cx.subscribe(
            &coin_input,
            |this: &mut Self, _, ev: &MoonInputEvent, cx| {
                if matches!(ev, MoonInputEvent::Change) {
                    this.rebuild(cx);
                    cx.notify();
                }
            },
        )
        .detach();
        cx.observe(&backend, |this, _, cx| {
            this.rebuild(cx);
            cx.notify();
        })
        .detach();
        let mut this = Self {
            backend,
            group,
            rows: Vec::new(),
            coin_input,
            duration_s: 20,
            repeat: 2,
            alert_strategies: HashMap::new(),
            dock: None,
            focus: cx.focus_handle(),
        };
        this.rebuild(cx);
        this
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let filter = self.coin_input.read(cx).value().trim().to_uppercase();
        let b = self.backend.read(cx);
        // Ядра ЭТОЙ группы (алерты панели — только своей группы).
        let names: HashMap<CoreId, String> = b
            .session
            .sessions()
            .iter()
            .filter(|s| s.group == self.group)
            .map(|s| (s.id, s.name.clone()))
            .collect();
        // Стратегии вида «Alerts» по каждому ядру группы — для выпадашки назначения.
        let mut strategies: HashMap<CoreId, Vec<(u64, String)>> = HashMap::new();
        for id in names.keys() {
            if let Some(core) = b.session.store().core(*id) {
                let list: Vec<(u64, String)> = core
                    .strategies
                    .iter()
                    .filter(|s| s.kind_ordinal == ALERTS_KIND)
                    .map(|s| (s.id, s.name.clone()))
                    .collect();
                if !list.is_empty() {
                    strategies.insert(*id, list);
                }
            }
        }
        self.alert_strategies = strategies;
        let mut rows: Vec<Row> = b
            .figures
            .borrow()
            .all_alerts()
            .filter(|(core, _, _)| names.contains_key(core))
            .filter(|(_, market, _)| filter.is_empty() || market.to_uppercase().contains(&filter))
            .map(|(core, market, f)| Row {
                core,
                core_name: names.get(&core).cloned().unwrap_or_default(),
                market: market.to_string(),
                figure: figure_label(&f.kind),
                price: f.kind.anchor_price(),
                time_ms: f.created_ms,
                from_server: f.from_server,
                fig_id: f.id,
                strategy_id: f.strategy_id,
            })
            .collect();
        rows.sort_by(|a, b| b.time_ms.cmp(&a.time_ms));
        self.rows = rows;
    }

    /// Имя стратегии по id для ядра (или «—»).
    fn strategy_name(&self, core: CoreId, strategy_id: u64) -> String {
        if strategy_id == 0 {
            return "—".to_string();
        }
        self.alert_strategies
            .get(&core)
            .and_then(|v| v.iter().find(|(id, _)| *id == strategy_id))
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| "—".to_string())
    }

    fn table(&self, p: MoonPalette, cx: &App) -> impl IntoElement {
        let header = h_flex()
            .w_full()
            .px(px(8.0))
            .py(px(4.0))
            .gap(px(6.0))
            .bg(moon(p.shell_high))
            .text_color(moon(p.text_muted))
            .child(cell(t!("alerts.col.core").to_string(), 90.0))
            .child(cell(t!("alerts.col.coin").to_string(), 70.0))
            .child(cell(t!("alerts.col.figure").to_string(), 110.0))
            .child(cell(t!("alerts.col.price").to_string(), 100.0))
            .child(cell(t!("alerts.col.time").to_string(), 100.0))
            .child(cell(t!("alerts.col.strategy").to_string(), 160.0))
            .child(div().flex_1())
            .child(cell(String::new(), 60.0));

        let mut list = v_flex().w_full().gap(px(1.0));
        for r in &self.rows {
            let core = r.core;
            let fid = r.fig_id;
            let backend_open = self.backend.clone();
            let open_market = r.market.clone();
            let coin_cell = div()
                .id(SharedString::from(format!(
                    "alert-open-{core}-{}",
                    r.market
                )))
                .w(px(70.0))
                .cursor_pointer()
                .text_color(moon(p.accent))
                .child(r.market.clone())
                .on_click(move |_, _w, app| {
                    backend_open.update(app, |b, bcx| {
                        b.open_request = Some((core, open_market.clone()));
                        b.open_request_rev = b.open_request_rev.wrapping_add(1);
                        b.open_request_activate = true;
                        bcx.notify();
                    });
                });
            // Выпадашка стратегии (виды «Alerts» этого ядра + «—»); назначение → blob @32.
            let mut options: Vec<(u64, SharedString, SharedString)> =
                vec![(0u64, "strat-none".into(), "—".into())];
            if let Some(list) = self.alert_strategies.get(&core) {
                for (id, name) in list {
                    options.push((
                        *id,
                        SharedString::from(format!("strat-{id}")),
                        SharedString::from(name.clone()),
                    ));
                }
            }
            let backend_strat = self.backend.clone();
            let smarket = r.market.clone();
            let items = radio_items(options, r.strategy_id, RadioMark::Check, move |app, sid| {
                let m = smarket.clone();
                backend_strat.update(app, |b, bcx| {
                    b.set_figure_strategy(core, &m, fid, sid);
                    bcx.notify();
                });
            });
            let strat_dd =
                MoonDropdown::new(SharedString::from(format!("alert-strat-{core}-{fid}")))
                    .label(format!("{} ▾", self.strategy_name(core, r.strategy_id)))
                    .trigger_variant(MoonButtonVariant::Soft)
                    .trigger_size(MoonButtonSize::Action)
                    .trigger_width(design::font_w(cx, 150.0))
                    .menu_width(design::font_w(cx, 200.0))
                    .menu_size(MoonMenuSize::Compact)
                    .items(items);

            let backend_del = self.backend.clone();
            let dmarket = r.market.clone();
            let row = h_flex()
                .w_full()
                .px(px(8.0))
                .py(px(3.0))
                .gap(px(6.0))
                .bg(moon(p.surface))
                .text_color(moon(p.text))
                .child(cell(r.core_name.clone(), 90.0))
                .child(coin_cell)
                .child(
                    cell(r.figure.clone(), 110.0)
                        .when(r.from_server, |e| e.text_color(moon(p.accent))),
                )
                .child(cell(fmt_price(r.price), 100.0))
                .child(cell(fmt_time(r.time_ms), 100.0))
                .child(div().w(px(160.0)).child(strat_dd))
                .child(div().flex_1())
                .child(
                    MoonButton::new(("alert-clear", fid as usize))
                        .label(t!("alerts.clear").to_string())
                        .size(MoonButtonSize::Micro)
                        .variant(MoonButtonVariant::Ghost)
                        .on_click(move |_, _w, app| {
                            backend_del.update(app, |b, bcx| {
                                b.remove_figure(core, &dmarket, fid);
                                bcx.notify();
                            });
                        })
                        .render(),
                );
            list = list.child(row);
        }

        v_flex().flex_1().w_full().child(header).child(
            div()
                .id("alerts-scroll")
                .flex_1()
                .w_full()
                .overflow_y_scroll()
                .child(list),
        )
    }

    fn bottom_bar(&self, p: MoonPalette, cx: &mut Context<Self>) -> impl IntoElement {
        // Единый блок «подпись сверху + контрол фикс.высоты снизу» — чтобы все контролы
        // стояли на одной линии (по низу) и одинаковой высоты. `cap` посчитан заранее,
        // чтобы замыкание не держало `cx` (иначе конфликт с cx.listener степпера).
        let cap = design::t_caption(cx);
        let field = move |title: String, control: AnyElement| {
            v_flex()
                .gap(px(3.0))
                .child(
                    div()
                        .h(px(13.0))
                        .text_size(cap)
                        .text_color(moon(p.text_muted))
                        .child(title),
                )
                .child(h_flex().h(px(CTRL_H)).items_center().child(control))
        };
        let stepper = |id: &'static str,
                       val: String,
                       dn: fn(&mut AlertsPanel),
                       up: fn(&mut AlertsPanel),
                       cx: &mut Context<Self>|
         -> AnyElement {
            h_flex()
                .h(px(CTRL_H))
                .items_center()
                .gap(px(2.0))
                .child(
                    MoonButton::new((id, 0u64))
                        .label("‹")
                        .size(MoonButtonSize::Action)
                        .variant(MoonButtonVariant::Soft)
                        .on_click(cx.listener(move |this, _, _w, cx| {
                            dn(this);
                            cx.notify();
                        }))
                        .render(),
                )
                .child(
                    div()
                        .w(px(34.0))
                        .text_center()
                        .text_color(moon(p.text))
                        .child(val),
                )
                .child(
                    MoonButton::new((id, 1u64))
                        .label("›")
                        .size(MoonButtonSize::Action)
                        .variant(MoonButtonVariant::Soft)
                        .on_click(cx.listener(move |this, _, _w, cx| {
                            up(this);
                            cx.notify();
                        }))
                        .render(),
                )
                .into_any_element()
        };
        h_flex()
            .flex_none()
            .w_full()
            .px(px(10.0))
            .py(px(8.0))
            .gap(px(16.0))
            .bg(moon(p.shell_high))
            .border_t(px(1.0))
            .border_color(moon(p.border))
            .items_end()
            .child(field(
                t!("alerts.duration").to_string(),
                stepper(
                    "alert-dur",
                    format!("{}", self.duration_s),
                    |v| v.duration_s = v.duration_s.saturating_sub(5).max(5),
                    |v| v.duration_s = (v.duration_s + 5).min(600),
                    cx,
                ),
            ))
            .child(field(
                t!("alerts.repeat").to_string(),
                stepper(
                    "alert-rep",
                    format!("{}", self.repeat),
                    |v| v.repeat = v.repeat.saturating_sub(1),
                    |v| v.repeat = (v.repeat + 1).min(20),
                    cx,
                ),
            ))
            .child(field(
                t!("alerts.sound").to_string(),
                self.sound_dropdown(cx).into_any_element(),
            ))
            .child(div().flex_1())
            .child(field(
                t!("alerts.col.coin").to_string(),
                div()
                    .w(px(150.0))
                    .child(
                        MoonInput::new("alerts-coin-filter")
                            .state(&self.coin_input)
                            .small(),
                    )
                    .into_any_element(),
            ))
    }

    /// Выпадашка «Выбор звука» — дефолт для алертов без стратегии (Backend). Клик по
    /// пункту сразу проигрывает звук для прослушивания.
    fn sound_dropdown(&self, cx: &Context<Self>) -> impl IntoElement {
        let cur = self.backend.read(cx).default_alert_sound.clone();
        let backend = self.backend.clone();
        let options: Vec<(&'static str, SharedString, SharedString)> = crate::sound::names()
            .map(|n| {
                (
                    n,
                    SharedString::from(format!("snd-{n}")),
                    SharedString::from(n),
                )
            })
            .collect();
        let items = radio_items(
            options,
            leak_str(&cur),
            RadioMark::Check,
            move |app, name: &'static str| {
                backend.update(app, |b, bcx| {
                    b.default_alert_sound = name.to_string();
                    bcx.notify();
                });
                crate::sound::play(name);
            },
        );
        MoonDropdown::new("alerts-sound")
            .label(format!("{cur} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(design::font_w(cx, 120.0))
            .menu_width(design::font_w(cx, 150.0))
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }
}

/// `&'static str` из набора звуков для сравнения с текущим (radio_items требует Copy).
fn leak_str(cur: &str) -> &'static str {
    crate::sound::names().find(|n| *n == cur).unwrap_or("ding1")
}

impl EventEmitter<PanelEvent> for AlertsPanel {}
impl Focusable for AlertsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for AlertsPanel {
    fn closable(&self, _cx: &App) -> bool {
        true
    }
    fn show_dock_header(&self, _cx: &App) -> bool {
        true
    }
    fn panel_name(&self) -> &'static str {
        "Alerts"
    }
    /// Visible tab caption. `panel_name` is the stable persistence key and stays untouched.
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        crate::panel_meta::tab_label(self.panel_name())
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::panel_meta::panel_title(self.panel_name())
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::dock_persist::panel_state_with_group("Alerts", &self.group)
    }
    fn on_added_to(
        &mut self,
        dock_area: WeakEntity<DockArea>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.dock = Some(dock_area);
    }
    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Vec<AnyElement>> {
        Some(vec![crate::panels::detach_button(
            "Alerts",
            self.group.clone(),
            self.backend.clone(),
            self.dock.clone(),
        )])
    }
}

impl Render for AlertsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        v_flex()
            .id("alerts-panel")
            .size_full()
            .bg(moon(p.shell))
            .text_color(moon(p.text))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .track_focus(&self.focus)
            .child(self.table(p, cx))
            .child(self.bottom_bar(p, cx))
    }
}

fn cell(text: String, w: f32) -> Div {
    div().w(px(w)).child(text)
}

fn figure_label(kind: &FigureKind) -> String {
    match kind {
        FigureKind::HLine { .. } => t!("alerts.fig.hline").to_string(),
        FigureKind::Segment { .. } => t!("alerts.fig.line").to_string(),
        FigureKind::Triangle { .. } => t!("alerts.fig.triangle").to_string(),
        FigureKind::Channel { .. } => t!("alerts.fig.channel").to_string(),
    }
}

fn fmt_price(price: f64) -> String {
    if price == 0.0 {
        String::new()
    } else {
        format!("{price:.6}")
    }
}

fn fmt_time(ms: i64) -> String {
    let (date, hms) = moon_core::applog::split_unix_ms(ms);
    let md = date.get(5..).unwrap_or(&date);
    let hm = hms.get(..5).unwrap_or(&hms);
    format!("{md} {hm}")
}
