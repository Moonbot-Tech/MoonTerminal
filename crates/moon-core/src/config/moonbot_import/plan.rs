//! Построение [`MoonBotImportPlan`]: сопоставление прочитанного [`MoonBotConfig`] с
//! текущими настройками Terminal. ЧИСТАЯ функция — ничего не пишет и не отправляет;
//! применение выбранных пунктов делает отдельный код по стабильным `id` пунктов.
//!
//! Правила (ТЗ §2/§8/§9):
//! - переносим только ОДНОЗНАЧНО сопоставленные поля; для прочих — запись в
//!   `unsupported` с причиной, молча не забываем;
//! - пункт создаётся только если новое значение ОТЛИЧАЕТСЯ от текущего;
//! - пустые сочетания MoonBot не переносим (перенос назначает, а не стирает);
//! - `ColorsLight` → светлый набор, `ColorsDark` → тёмный, никогда крест-накрест;
//! - секции `Charts`/`ArbColors` (Ini-блок) декодированы для preview, но НЕ применяются
//!   до явной таблицы соответствия (ТЗ §9);
//! - значимая alpha цвета не отбрасывается молча — пункт уходит в unsupported.

use super::schema_v7::{MoonBotConfig, ShortcutAction, SHORTCUT_ACTIONS};
use super::shortcut::{self, DecodedShortcut};
use crate::config::hotkeys::HotkeysConfig;
use crate::config::orders::OrdersStyleSet;
use crate::config::theme::ChartThemeSet;

/// Значение переносимого пункта — типизировано, чтобы применение не парсило строки.
#[derive(Debug, Clone, PartialEq)]
pub enum PlannedValue {
    /// Хоткей в формате `gpui::Keystroke` (`ctrl-shift-f7`).
    Keystroke(String),
    /// Светлая тема UI вкл/выкл.
    UiThemeLight(bool),
    /// Цвет sRGB.
    Rgb([u8; 3]),
    /// Шесть размеров ручного ордера.
    OrderSizes([f64; 6]),
    /// Индекс выбранного пресета размера.
    OrderSizeSel(usize),
    /// Шесть fixed-sell процентов (core-owned, применяется через ClientSettings).
    FixedSellPrices([f32; 6]),
    /// Выбранный fixed-sell слот (core-owned).
    FixedSellSel(u8),
}

/// Один сопоставленный пункт preview: стабильный `id` (по нему применение находит
/// сеттер), подпись и «было → станет» для отображения. Показываются ВСЕ пункты,
/// включая уже совпадающие (`same`) — пользователь видит полную картину переноса.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingChange {
    /// Стабильный идентификатор (`hotkey.cancel_buy`, `theme.dark.bg`,
    /// `orders.light.buy.color`, `core.order_sizes`, …).
    pub id: String,
    /// Человекочитаемая подпись пункта (термины Moonbot не переводим).
    pub label: String,
    /// Текущее значение Terminal (для preview).
    pub current: String,
    /// Новое значение из MoonBot (для preview).
    pub new: String,
    /// Типизированное значение для применения.
    pub value: PlannedValue,
    /// Значение УЖЕ совпадает с MoonBot: показываем с пометкой, по умолчанию не
    /// выбран (применение — no-op, но не запрещено).
    pub same: bool,
}

/// Поле MoonBot, которое НЕ переносится — с причиной (ТЗ §2 группа 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported {
    pub name: String,
    pub reason: String,
}

/// План импорта: группы preview + предупреждения.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MoonBotImportPlan {
    /// Группа «Терминал»: тема UI (и прочие не-хоткейные настройки).
    pub terminal: Vec<SettingChange>,
    /// Группа «Хоткеи»: все сопоставленные сочетания (включая совпадающие).
    pub hotkeys: Vec<SettingChange>,
    /// Хоткеи MoonBot, которые НЕ переносятся (нет действия/неизвестный VK) — с
    /// причинами; показываются внутри группы «Хоткеи».
    pub unsupported_hotkeys: Vec<Unsupported>,
    /// Группа «График и линии»: цвета обеих тем.
    pub chart: Vec<SettingChange>,
    /// Группа «Ядро», локальный конфиг: пресеты размера ордера (per-core).
    pub per_core: Vec<SettingChange>,
    /// Группа «Ядро», команды: fixed-sell (ClientSettings, шлётся выбранным ядрам).
    pub core_commands: Vec<SettingChange>,
    /// Группа «Не перенесено» (без хоткеев — те в `unsupported_hotkeys`).
    pub unsupported: Vec<Unsupported>,
    /// Предупреждения (значения вне диапазона и т.п.).
    pub warnings: Vec<String>,
    /// Хоткеи, НЕ назначенные в MoonBot (пустые не переносим — не стираем свои).
    pub hotkeys_empty: usize,
}

impl MoonBotImportPlan {
    pub fn is_empty(&self) -> bool {
        self.terminal.is_empty()
            && self.hotkeys.is_empty()
            && self.chart.is_empty()
            && self.per_core.is_empty()
            && self.core_commands.is_empty()
    }

    /// Все применимые локальные пункты (для сборки выбора/итерации применения).
    pub fn local_items(&self) -> impl Iterator<Item = &SettingChange> {
        self.terminal
            .iter()
            .chain(self.hotkeys.iter())
            .chain(self.chart.iter())
            .chain(self.per_core.iter())
    }
}

/// Текущие настройки Terminal, против которых строится дифф. Узкий срез —
/// не тащим весь `AppConfig` (тестируемость).
pub struct PlanContext<'a> {
    pub hotkeys: &'a HotkeysConfig,
    pub theme: &'a ChartThemeSet,
    pub orders: &'a OrdersStyleSet,
    /// Активная тема UI: true = светлая.
    pub ui_theme_light: bool,
    /// Текущие пресеты размера активного ядра (для «было» в preview).
    pub order_sizes: [f64; 6],
    pub order_size_sel: Option<usize>,
}

/// Построить план. Ничего не мутирует.
pub fn build_plan(mb: &MoonBotConfig, cur: &PlanContext) -> MoonBotImportPlan {
    let mut plan = MoonBotImportPlan::default();
    map_ui_theme(mb, cur, &mut plan);
    map_hotkeys(mb, cur, &mut plan);
    map_colors(mb, cur, &mut plan);
    map_core(mb, cur, &mut plan);
    collect_static_unsupported(mb, &mut plan);
    plan
}

// ── Тема UI ──────────────────────────────────────────────────────────────────

fn map_ui_theme(mb: &MoonBotConfig, cur: &PlanContext, plan: &mut MoonBotImportPlan) {
    let mb_light = !mb.theme.is_dark();
    let name = |l: bool| if l { "светлая" } else { "тёмная" };
    plan.terminal.push(SettingChange {
        id: "ui.theme_mode".into(),
        label: "Тема UI".into(),
        current: name(cur.ui_theme_light).into(),
        new: name(mb_light).into(),
        value: PlannedValue::UiThemeLight(mb_light),
        same: mb_light == cur.ui_theme_light,
    });
}

// ── Хоткеи ───────────────────────────────────────────────────────────────────

/// Наше поле-приёмник для shortcut-слота MoonBot. `None` = в Terminal нет действия
/// (перечень удалённых 2026-07-10 — под них нет send-команд в moonproto).
fn action_target(action: ShortcutAction) -> Option<&'static str> {
    use ShortcutAction::*;
    Some(match action {
        CancelBuy => "cancel_buy",
        PanicSell => "panic_sell",
        JoinSells => "join_sells",
        SwitchCharts => "switch_charts",
        NewLong => "new_long",
        NewShort => "new_short",
        SplitOrder => "split_order",
        SplitOrderX => "split_order_x",
        ShiftBuyUp => "shift_buy_up",
        ShiftBuyDown => "shift_buy_down",
        ShiftSellUp => "shift_sell_up",
        ShiftSellDown => "shift_sell_down",
        ScalePlus => "scale_plus",
        ScaleMinus => "scale_minus",
        SwitchFigure => "switch_figure",
        PanicSellOne => "panic_sell_one",
        CancelAllBuys => "cancel_all_buys",
        ReloadBook | MakeShot | MakeShotBot | ReloadChart | SellPlus | SellMinus | SpyMode
        | ShowCharts | FitSells | Broadcast => return None,
    })
}

/// Имя слота для preview/unsupported — как в Moonbot.
fn action_name(action: ShortcutAction) -> &'static str {
    use ShortcutAction::*;
    match action {
        CancelBuy => "Cancel Buy",
        PanicSell => "Panic Sell",
        JoinSells => "Join Sells",
        SwitchCharts => "Switch Charts",
        ReloadBook => "Reload Book",
        NewLong => "New Long",
        NewShort => "New Short",
        SplitOrder => "Split Order",
        ShiftBuyUp => "Shift Buy Up",
        ShiftBuyDown => "Shift Buy Down",
        ShiftSellUp => "Shift Sell Up",
        ShiftSellDown => "Shift Sell Down",
        MakeShot => "Make Shot",
        MakeShotBot => "Make Shot Bot",
        ReloadChart => "Reload Chart",
        ScalePlus => "Scale +",
        ScaleMinus => "Scale −",
        SellPlus => "Sell +",
        SellMinus => "Sell −",
        SpyMode => "Spy Mode",
        ShowCharts => "Show Charts",
        SplitOrderX => "Split Order X",
        SwitchFigure => "Switch Figure",
        FitSells => "Fit Sells",
        PanicSellOne => "Panic Sell One",
        CancelAllBuys => "Cancel All Buys",
        Broadcast => "Broadcast",
    }
}

/// Текущее значение нашего поля-приёмника по имени (для «было» в preview).
fn hotkey_field(cfg: &HotkeysConfig, field: &str) -> String {
    match field {
        "cancel_buy" => cfg.cancel_buy.clone(),
        "panic_sell" => cfg.panic_sell.clone(),
        "panic_sell_one" => cfg.panic_sell_one.clone(),
        "cancel_all_buys" => cfg.cancel_all_buys.clone(),
        "join_sells" => cfg.join_sells.clone(),
        "switch_charts" => cfg.switch_charts.clone(),
        "new_long" => cfg.new_long.clone(),
        "new_short" => cfg.new_short.clone(),
        "split_order" => cfg.split_order.clone(),
        "split_order_x" => cfg.split_order_x.clone(),
        "shift_buy_up" => cfg.shift_buy_up.clone(),
        "shift_buy_down" => cfg.shift_buy_down.clone(),
        "shift_sell_up" => cfg.shift_sell_up.clone(),
        "shift_sell_down" => cfg.shift_sell_down.clone(),
        "scale_plus" => cfg.scale_plus.clone(),
        "scale_minus" => cfg.scale_minus.clone(),
        "switch_figure" => cfg.switch_figure.clone(),
        _ => String::new(),
    }
}

/// Один хоткей-пункт (группа «Хоткеи»): MoonBot `TShortCut` → gpui-строка; показываем
/// ВСЕ, включая совпадающие (`same`). Empty пропускаем (перенос назначает, а не
/// стирает; считаем в сводку), Unsupported — в `unsupported_hotkeys` с причиной.
fn push_hotkey(plan: &mut MoonBotImportPlan, id: String, label: String, raw: u16, current: &str) {
    let decoded = shortcut::decode(raw);
    match shortcut::to_gpui_keystroke(decoded) {
        Some(ks) => {
            let same = ks == current;
            plan.hotkeys.push(SettingChange {
                id,
                label,
                current: if current.is_empty() {
                    "—".into()
                } else {
                    current.into()
                },
                new: shortcut::display(decoded),
                value: PlannedValue::Keystroke(ks),
                same,
            });
        }
        None => match decoded {
            DecodedShortcut::Unsupported { raw } => {
                plan.unsupported_hotkeys.push(Unsupported {
                    name: label,
                    reason: format!("неизвестная клавиша (VK 0x{:02X})", raw & 0xFF),
                });
            }
            // Empty — не переносим (и считаем для сводки).
            _ => plan.hotkeys_empty += 1,
        },
    }
}

fn map_hotkeys(mb: &MoonBotConfig, cur: &PlanContext, plan: &mut MoonBotImportPlan) {
    let h = &mb.ui.hotkeys;
    // Слоты размера ордера (OKeys → order_size) и fixed-sell (SKeys → sell_preset).
    for i in 0..6 {
        push_hotkey(
            plan,
            format!("hotkey.order_size.{i}"),
            format!("Размер ордера {}", i + 1),
            h.order_size_keys[i],
            &cur.hotkeys.order_size[i],
        );
        push_hotkey(
            plan,
            format!("hotkey.sell_preset.{i}"),
            format!("Fixed sell {}", i + 1),
            h.fixed_sell_keys[i],
            &cur.hotkeys.sell_preset[i],
        );
    }
    // 27 shortcut-слотов: 17 переносимых + 10 без команды в Terminal.
    for action in SHORTCUT_ACTIONS {
        let raw = h.shortcuts.get(action);
        match action_target(action) {
            Some(field) => push_hotkey(
                plan,
                format!("hotkey.{field}"),
                action_name(action).to_string(),
                raw,
                &hotkey_field(cur.hotkeys, field),
            ),
            None => {
                // Действия в Terminal нет: мёртвый хоткей не создаём. Показываем
                // только НАЗНАЧЕННЫЕ (пустой слот нечего переносить).
                if raw != 0 {
                    plan.unsupported_hotkeys.push(Unsupported {
                        name: action_name(action).to_string(),
                        reason: "в Terminal нет такого действия (нет команды ядра)".into(),
                    });
                }
            }
        }
    }
}

// ── Цвета (Theme-блок: ColorsLight → light, ColorsDark → dark) ───────────────

/// TColor из INI-строки: битовая раскладка `0xAARRGGBB` (ТЗ §8). Реальный экспорт
/// MoonBot пишет hex-строку из 8 символов (`FF008000`, `FFFFFFFF` — проверено живым
/// буфером); допускаем и десятичную запись. Неоднозначность «8 цифр без букв»
/// решаем в пользу десятичной (десятичные не пишутся с ведущим нулём, hex из одних
/// цифр начинался бы с '0' либо содержал букву). Возвращает RGB и alpha-байт.
fn parse_tcolor(s: &str) -> Option<([u8; 3], u8)> {
    let s = s.trim();
    let hex8 = s.len() == 8
        && s.bytes().all(|b| b.is_ascii_hexdigit())
        && (s.bytes().any(|b| b.is_ascii_alphabetic()) || s.starts_with('0'));
    let v = if hex8 {
        u32::from_str_radix(s, 16).ok()?
    } else {
        let v = s.parse::<i64>().ok()?;
        u32::try_from(v & 0xFFFF_FFFF).ok()?
    };
    let a = (v >> 24) as u8;
    let r = ((v >> 16) & 0xFF) as u8;
    let g = ((v >> 8) & 0xFF) as u8;
    let b = (v & 0xFF) as u8;
    Some(([r, g, b], a))
}

fn rgb_hex(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

fn map_colors(mb: &MoonBotConfig, cur: &PlanContext, plan: &mut MoonBotImportPlan) {
    for light in [true, false] {
        let section = if light {
            mb.theme.colors_light()
        } else {
            mb.theme.colors_dark()
        };
        let Some(section) = section else { continue };
        let theme_side = if light { "light" } else { "dark" };
        let theme = cur.theme.get(light);
        let orders = cur.orders.get(light);
        for (key, value) in &section.entries {
            let Some((rgb, alpha)) = parse_tcolor(value) else {
                plan.unsupported.push(Unsupported {
                    name: format!("{key} ({theme_side})"),
                    reason: format!("значение «{value}» не разобрано как цвет"),
                });
                continue;
            };
            // Наши цветовые поля без alpha: значимую alpha не отбрасываем молча.
            if alpha != 0 && alpha != 0xFF {
                plan.unsupported.push(Unsupported {
                    name: format!("{key} ({theme_side})"),
                    reason: format!("цвет несёт alpha 0x{alpha:02X}, у целевого поля её нет"),
                });
                continue;
            }
            // Явная таблица MoonBot key → поле Terminal (ТЗ §8). Ключи вне таблицы —
            // в unsupported (декодированы, не применяются).
            let (target_id, label, current_rgb): (&str, &str, [u8; 3]) = match key.as_str() {
                "graphBK" => ("theme.bg", "Фон графика", theme.bg),
                "graphNet" => ("theme.grid", "Сетка", theme.grid),
                "graphCursor" => ("theme.cross", "Перекрестие", theme.cross),
                "graphFont" => (
                    "theme.labels",
                    "Нейтральные подписи (оси/курсор)",
                    theme.axis_label,
                ),
                "CandleGreen" => ("theme.candle_up", "Растущая свеча", theme.candle_up),
                "CandleRed" => ("theme.candle_down", "Падающая свеча", theme.candle_down),
                "CandleNeutral" => (
                    "theme.candle_neutral",
                    "Нейтральная свеча",
                    theme.candle_neutral,
                ),
                "OrderBookGreen" => ("theme.book_bid", "Стакан bid", theme.book_bid),
                "OrderBookRed" => ("theme.book_ask", "Стакан ask", theme.book_ask),
                "BuyOrder" => ("orders.buy.color", "Линия Buy", orders.buy.color),
                "BuyPendingOrder" => (
                    "orders.buy.pending_color",
                    "Линия Buy (pending)",
                    orders.buy.pending_color.unwrap_or(orders.buy.color),
                ),
                "SellOrder" => ("orders.sell.color", "Линия Sell", orders.sell.color),
                "BuyShort" => (
                    "orders.buy_short.color",
                    "Линия Buy (short)",
                    orders.buy_short.color,
                ),
                "SellShort" => (
                    "orders.sell_short.color",
                    "Линия Sell (short)",
                    orders.sell_short.color,
                ),
                "Trailing" => (
                    "orders.trailing.color",
                    "Линия Trailing",
                    orders.trailing.color,
                ),
                "LiqPrice" => ("orders.liq.color", "Линия Liquidation", orders.liq.color),
                "BookLevelGreen" | "BookLevelRed" => {
                    plan.unsupported.push(Unsupported {
                        name: format!("{key} ({theme_side})"),
                        reason: "в Terminal нет отдельного цвета линий уровней стакана".into(),
                    });
                    continue;
                }
                "BuyOrderDone" => {
                    plan.unsupported.push(Unsupported {
                        name: format!("{key} ({theme_side})"),
                        reason: "закрытые ордера в Terminal — прозрачностью, не цветом".into(),
                    });
                    continue;
                }
                "MarkPrice" | "LiqOrdersLong" | "LiqOrdersShort" => {
                    plan.unsupported.push(Unsupported {
                        name: format!("{key} ({theme_side})"),
                        reason: "в Terminal нет эквивалентного стиля".into(),
                    });
                    continue;
                }
                _ => {
                    plan.unsupported.push(Unsupported {
                        name: format!("{key} ({theme_side})"),
                        reason: "нет в таблице соответствия (не применяем)".into(),
                    });
                    continue;
                }
            };
            // Сторона темы НЕ в label: preview группирует цвета колонками
            // «Светлая»/«Тёмная» (side закодирован в id пункта). Совпавшие тоже
            // показываем (same) — полная картина переноса.
            plan.chart.push(SettingChange {
                id: format!("{target_id}.{theme_side}"),
                label: label.to_string(),
                current: rgb_hex(current_rgb),
                new: rgb_hex(rgb),
                value: PlannedValue::Rgb(rgb),
                same: current_rgb == rgb,
            });
        }
    }
    // Секции Charts/ArbColors Ini-блока: без явной таблицы не применяем (ТЗ §9).
    // Ключи разворачиваем ПОИМЁННО (со значением) — по этому списку решаем, какие
    // маппинги/поля добавлять в Terminal следующим шагом.
    for name in ["Charts", "ArbColors"] {
        if let Some(s) = mb.ini.section(name) {
            for (key, value) in &s.entries {
                plan.unsupported.push(Unsupported {
                    name: format!("[{name}] {key} = {value}"),
                    reason: "таблица соответствия для этой секции ещё не определена".into(),
                });
            }
        }
    }
}

// ── Ядро ─────────────────────────────────────────────────────────────────────

/// Компактный список чисел для preview: `70, 80, 300` вместо `[70.0, 80.0, 300.0]`.
fn fmt_nums<T: Into<f64> + Copy>(vals: &[T]) -> String {
    vals.iter()
        .map(|v| {
            let v: f64 = (*v).into();
            if v.fract() == 0.0 && v.abs() < 1e12 {
                format!("{}", v as i64)
            } else {
                format!("{v}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn map_core(mb: &MoonBotConfig, cur: &PlanContext, plan: &mut MoonBotImportPlan) {
    let h = &mb.ui.hotkeys;
    // Шесть размеров ручного ордера → ServerConfig.order_sizes выбранных ядер.
    plan.per_core.push(SettingChange {
        id: "core.order_sizes".into(),
        label: "Пресеты размера ордера (F1-F6)".into(),
        current: fmt_nums(&cur.order_sizes),
        new: fmt_nums(&h.order_sizes),
        value: PlannedValue::OrderSizes(h.order_sizes),
        same: h.order_sizes == cur.order_sizes,
    });
    // Выбранный пресет: bNum строго 0..=5, иначе warning и пропуск (ТЗ §9).
    match usize::try_from(h.order_size_sel).ok().filter(|v| *v <= 5) {
        Some(sel) => {
            plan.per_core.push(SettingChange {
                id: "core.order_size_sel".into(),
                label: "Выбранный пресет размера".into(),
                current: cur
                    .order_size_sel
                    .map_or("—".into(), |v| format!("F{}", v + 1)),
                new: format!("F{}", sel + 1),
                value: PlannedValue::OrderSizeSel(sel),
                same: Some(sel) == cur.order_size_sel,
            });
        }
        None => plan.warnings.push(format!(
            "bNum = {} вне диапазона 0..=5 — выбранный пресет не переносим",
            h.order_size_sel
        )),
    }
    // Fixed-sell: проценты и слот принадлежат ядру (ClientSettings) — отдельная
    // группа, применяется отправкой выбранным ядрам (шаг применения, ТЗ §10).
    plan.core_commands.push(SettingChange {
        id: "core.fixed_sell_prices".into(),
        label: "Fixed sell проценты (S1-S6)".into(),
        current: "текущие значения ядра".into(),
        new: fmt_nums(&h.fixed_sell_prices),
        value: PlannedValue::FixedSellPrices(h.fixed_sell_prices),
        same: false, // текущих значений ядра локально не знаем
    });
    if h.fixed_sell_sel <= 5 {
        plan.core_commands.push(SettingChange {
            id: "core.fixed_sell_sel".into(),
            label: "Выбранный fixed sell слот".into(),
            current: "текущий слот ядра".into(),
            new: format!("S{}", h.fixed_sell_sel + 1),
            value: PlannedValue::FixedSellSel(h.fixed_sell_sel),
            same: false,
        });
    } else {
        plan.warnings.push(format!(
            "sbNum = {} вне диапазона 0..=5 — выбранный fixed sell слот не переносим",
            h.fixed_sell_sel
        ));
    }
}

// ── Статичные «не перенесено» ────────────────────────────────────────────────

/// Поля UI-блока, для которых в Terminal нет работающего эквивалента (ТЗ §2 гр. 4).
fn collect_static_unsupported(mb: &MoonBotConfig, plan: &mut MoonBotImportPlan) {
    let none = "в Terminal нет эквивалентной настройки";
    for name in [
        "HideDemoButton",
        "ConfirmClose",
        "NewMarketsOnTop",
        "CoinsSortOrder",
        "StratEditorChapters",
        "MainButtonsIndex",
        "StratExpandedState",
    ] {
        plan.unsupported.push(Unsupported {
            name: name.into(),
            reason: none.into(),
        });
    }
    plan.unsupported.push(Unsupported {
        name: "MarketsTable (колонки)".into(),
        reason: "смысл столбцов таблиц не сопоставлен — переносить рано".into(),
    });
    plan.unsupported.push(Unsupported {
        name: "Мышиные жесты".into(),
        reason: "недоступны в этой версии экспорта (появятся с блоком Interop)".into(),
    });
    // SplitParts из HotkeysPublic: у Split Order в Terminal нет настройки частей.
    if mb.ui.hotkeys.split_parts > 0 {
        plan.unsupported.push(Unsupported {
            name: "SplitParts".into(),
            reason: "число частей Split Order в Terminal не настраивается".into(),
        });
    }
}

#[cfg(test)]
mod tests;
