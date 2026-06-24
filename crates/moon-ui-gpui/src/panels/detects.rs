//! Лента детектов — откпрепляемая панель (порт egui `DetectRibbon`). Втягивает детекты
//! ядер группы с `SoundAlert=Yes` и `AddToChart==0` (AddToChart-детекты — в чарт-вкладки),
//! держит `KeepAlert` секунд, новые сверху. Клик → открыть монету на Main (через
//! `Backend.open_request`, который читает Shell).

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use gpui::*;
use moon_ui::{MoonPalette, Panel, PanelEvent, PanelState, h_flex, v_flex};
use rust_i18n::t;

use crate::{Backend, design};
use moon_chart::paint::now_unix_ms;
use moon_core::session::CoreId;

/// Кнопка ленты детектов (порт `src/dock/detects.rs::RibbonItem`).
struct DetectItem {
    core: CoreId,
    core_name: String,
    /// Полный символ рынка (для подписки при клике).
    market: String,
    /// Подпись кнопки — монета без quote подключения (`ADAUSDT` → `ADA`).
    base: String,
    color: [u8; 3],
    born_ms: f64,
    ttl_ms: f64,
}

/// Линейная смесь двух sRGB-цветов (порт egui `theme::lerp_color`).
fn lerp_u8(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2])]
}

/// Яркость цвета (для инверсии цвета таймера на светлом глоу), порт egui-логики.
fn luminance(c: [u8; 3]) -> f32 {
    0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32
}

pub struct DetectsPanel {
    backend: Entity<Backend>,
    group: String,
    items: VecDeque<DetectItem>,
    last_seq: HashMap<CoreId, u64>,
    last_sig: u64,
    prune_timer_armed: bool,
    focus: FocusHandle,
}

const MAX_DETECT_BTNS: usize = 48;
const DEFAULT_SERVER_COLOR: [u8; 3] = [0xff, 0xb3, 0x47];

impl DetectsPanel {
    pub fn new(backend: Entity<Backend>, group: String, cx: &mut Context<Self>) -> Self {
        let initial_sig = detects_sig(backend.read(cx), &group);
        cx.observe(&backend, |this, backend, cx| {
            let now = now_unix_ms();
            let sig = detects_sig(backend.read(cx), &this.group);
            let mut changed = false;
            if sig != this.last_sig {
                this.last_sig = sig;
                changed |= this.ingest(backend.read(cx));
            }
            changed |= this.prune(now);
            this.arm_prune_timer(cx);
            if changed {
                cx.notify();
            }
        })
        .detach();
        let initial_backend = backend.clone();
        let mut this = Self {
            backend,
            group,
            items: VecDeque::new(),
            last_seq: HashMap::new(),
            last_sig: initial_sig,
            prune_timer_armed: false,
            focus: cx.focus_handle(),
        };
        this.ingest(initial_backend.read(cx));
        this.prune(now_unix_ms());
        this.arm_prune_timer(cx);
        this
    }

    /// Втянуть свежие детекты ядер группы (seq > курсора, sound_alert, не AddToChart).
    fn ingest(&mut self, b: &Backend) -> bool {
        let mut changed = false;
        // Цвет + quote ядра берём из его сервера в конфиге: quote выводим из рынка по
        // умолчанию (`server.market`), чтобы резать суффикс монеты (`ADAUSDT` → `ADA`),
        // как egui `CoreInfo.quote`.
        let cores: Vec<(CoreId, String, [u8; 3], String)> = b
            .session
            .sessions()
            .iter()
            .filter(|s| s.group == self.group)
            .map(|s| {
                let (color, quote) = b
                    .config
                    .servers
                    .iter()
                    .find(|sv| sv.id == s.id)
                    .map(|sv| (sv.color, moon_core::symbol::resolve_quote(&sv.market)))
                    .unwrap_or((DEFAULT_SERVER_COLOR, String::new()));
                (s.id, s.name.clone(), color, quote)
            })
            .collect();
        for (id, name, color, quote) in cores {
            let Some(d) = b.session.store().core(id) else {
                continue;
            };
            let last = self.last_seq.get(&id).copied().unwrap_or(0);
            let mut fresh: Vec<&moon_core::feed::DetectRow> = Vec::new();
            for det in d.detects.iter().rev() {
                if det.seq <= last {
                    break;
                }
                fresh.push(det);
            }
            if fresh.is_empty() {
                continue;
            }
            self.last_seq.insert(id, fresh[0].seq);
            for det in fresh.iter().rev() {
                if !det.sound_alert || det.add_to_chart > 0 {
                    continue;
                }
                let ttl = (det.keep_alert_secs.max(1) as f64) * 1000.0;
                if let Some(it) = self
                    .items
                    .iter_mut()
                    .find(|it| it.core == id && it.market == det.market)
                {
                    it.born_ms = det.time_ms;
                    it.ttl_ms = ttl;
                    it.color = color;
                    changed = true;
                } else {
                    self.items.push_back(DetectItem {
                        core: id,
                        core_name: name.clone(),
                        market: det.market.clone(),
                        base: moon_core::symbol::base_symbol(&det.market, &quote).to_string(),
                        color,
                        born_ms: det.time_ms,
                        ttl_ms: ttl,
                    });
                    changed = true;
                }
            }
        }
        while self.items.len() > MAX_DETECT_BTNS {
            self.items.pop_front();
            changed = true;
        }
        changed
    }

    fn prune(&mut self, now_ms: f64) -> bool {
        let before = self.items.len();
        self.items.retain(|it| now_ms - it.born_ms < it.ttl_ms);
        self.items.len() != before
    }

    /// 1-Гц тик ПОКА есть детекты: обновляет обратный отсчёт («Ns») по СВОИМ часам и
    /// убирает истёкшие. Раньше отсчёт перерисовывался только по приходу данных (backend-
    /// пульс) — это «время сцеплено с данными»: на тихой/отключённой группе цифры замирали,
    /// а кнопка просто исчезала в конце. Время — по часам, не по приходу тиков. Тик сам
    /// гаснет, когда детектов нет (не плодим таймер вхолостую — executor.timer недёшев).
    fn arm_prune_timer(&mut self, cx: &mut Context<Self>) {
        if self.prune_timer_armed || self.items.is_empty() {
            return;
        }
        self.prune_timer_armed = true;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(Duration::from_millis(1000)).await;
            let alive = cx.update(|cx| {
                this.update(cx, |this, cx| {
                    this.prune_timer_armed = false;
                    this.prune(now_unix_ms());
                    // Перерисовать отсчёт / отразить пропажу истёкших, пока есть что показывать.
                    if !this.items.is_empty() {
                        cx.notify();
                    }
                    this.arm_prune_timer(cx);
                })
                .is_ok()
            });
            if !alive {
                return;
            }
        })
        .detach();
    }

    /// Открыть монету на Main: запрос в Backend (Shell откроет чарт) + убрать кнопку.
    fn open(&mut self, core: CoreId, market: String, cx: &mut Context<Self>) {
        self.items
            .retain(|it| !(it.core == core && it.market == market));
        self.backend.update(cx, |b, bcx| {
            b.open_request = Some((core, market.clone()));
            b.open_request_rev = b.open_request_rev.wrapping_add(1);
            // Клик по детекту открывает монету на Main, но окно НЕ поднимает.
            b.open_request_activate = false;
            bcx.notify();
        });
        self.arm_prune_timer(cx);
        cx.notify();
    }
}

fn detects_sig(b: &Backend, group: &str) -> u64 {
    let store = b.session.store();
    b.session
        .sessions()
        .iter()
        .filter(|s| s.group == group)
        .filter_map(|s| store.core(s.id))
        .fold(0u64, |a, c| a.wrapping_mul(31).wrapping_add(c.detects_rev))
}

impl EventEmitter<PanelEvent> for DetectsPanel {}
impl Focusable for DetectsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for DetectsPanel {
    fn panel_name(&self) -> &'static str {
        "Detects"
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from(t!("dock.tab.detects").to_string())
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::dock_persist::panel_state_with_group("Detects", &self.group)
    }
}
impl Render for DetectsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let now = now_unix_ms();
        let mut col = v_flex()
            .id("detects")
            .size_full()
            .gap_1p5()
            .p_2()
            .track_focus(&self.focus);
        // Новые сверху.
        for (i, it) in self.items.iter().enumerate().rev() {
            let secs = ((it.ttl_ms - (now - it.born_ms)) / 1000.0).ceil().max(0.0) as u32;
            // Меш-градиент кнопки (порт egui `detect_button`): верх = LIFT, низ = смесь
            // LIFT + цвет ядра. В покое доля 0.55, на ховере 0.80 (ярче). Цвет таймера
            // инвертируем по яркости низа (тёмный на светлом глоу), чтобы не сливался.
            let top = design::u32_to_rgb(p.panel);
            let top_hover = design::u32_to_rgb(p.panel_high);
            let bottom = lerp_u8(top, it.color, 0.55);
            let bottom_hover = lerp_u8(top_hover, it.color, 0.80);
            let grad = |top: [u8; 3], bot: [u8; 3]| {
                linear_gradient(
                    180.0,
                    linear_color_stop(rgb(design::rgb_to_u32(top)), 0.0),
                    linear_color_stop(rgb(design::rgb_to_u32(bot)), 1.0),
                )
            };
            let secs_color = if luminance(bottom) > 140.0 {
                rgb(0x141416)
            } else {
                rgb(p.text)
            };
            let (core, market) = (it.core, it.market.clone());
            col = col.child(
                div()
                    .id(SharedString::from(format!("det-{i}")))
                    .w_full()
                    .h(design::fit_h_px(cx, 34.0, 14.0, 10.0))
                    .px_2()
                    .py_1()
                    .cursor_pointer()
                    .rounded(design::ui_px(cx, 4.0))
                    .border_1()
                    .border_color(rgb(p.border))
                    .bg(grad(top, bottom))
                    .hover(|s| {
                        s.border_color(rgb(p.amber))
                            .bg(grad(top_hover, bottom_hover))
                    })
                    // Токен крупно сверху-слева; нижняя строка — таймер слева, ядро справа.
                    .child(
                        v_flex()
                            .size_full()
                            .justify_between()
                            .child(div().text_color(rgb(p.text)).child(it.base.clone()))
                            .child(
                                h_flex()
                                    .w_full()
                                    .justify_between()
                                    .items_end()
                                    .text_size(design::t_body(cx))
                                    .text_color(secs_color)
                                    .child(div().child(format!("{secs}s")))
                                    .child(div().child(it.core_name.clone())),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open(core, market.clone(), cx);
                    })),
            );
        }
        col
    }
}
