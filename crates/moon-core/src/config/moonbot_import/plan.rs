//! Builds a [`MoonBotImportPlan`] by mapping a parsed [`MoonBotConfig`] to the current
//! Terminal settings. This is a PURE function that writes and sends nothing; separate code
//! applies selected items by their stable item `id` values.
//!
//! Rules (spec sections 2/8/9):
//! - import only UNAMBIGUOUSLY mapped fields; record every other field in `unsupported`
//!   with a reason rather than silently dropping it;
//! - create items for both changed and matching values, marking matches with `same` so the
//!   preview shows the complete import picture;
//! - do not import empty MoonBot shortcuts (import assigns rather than clears);
//! - map `ColorsLight` to the light set and `ColorsDark` to the dark set, never across sets;
//! - decode the `Charts`/`ArbColors` sections (Ini block) for preview, but do NOT apply them
//!   until an explicit mapping table exists (spec section 9);
//! - do not silently discard meaningful color alpha; send the item to `unsupported`.

use super::schema_v7::{MoonBotConfig, ShortcutAction, SHORTCUT_ACTIONS};
use super::shortcut::{self, DecodedShortcut};
use crate::config::hotkeys::HotkeysConfig;
use crate::config::orders::OrdersStyleSet;
use crate::config::theme::ChartThemeSet;

/// Typed value of an importable item, so application code does not parse strings.
#[derive(Debug, Clone, PartialEq)]
pub enum PlannedValue {
    /// Hotkey in `gpui::Keystroke` format (`ctrl-shift-f7`).
    Keystroke(String),
    /// Whether the light UI theme is enabled.
    UiThemeLight(bool),
    /// sRGB color.
    Rgb([u8; 3]),
    /// Six manual-order sizes.
    OrderSizes([f64; 6]),
    /// Selected size-preset index.
    OrderSizeSel(usize),
    /// Six fixed-sell percentages reserved for preview; no application path consumes them.
    FixedSellPrices([f32; 6]),
    /// Selected fixed-sell slot (core-owned).
    FixedSellSel(u8),
}

/// One mapped preview item: a stable `id` used to locate its setter, a label, and
/// "before → after" display values. ALL items are shown, including values that already match
/// (`same`), so the user sees the complete import picture.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingChange {
    /// Stable identifier (`hotkey.cancel_buy`, `theme.bg.dark`,
    /// `orders.buy.color.light`, `core.order_sizes`, …).
    pub id: String,
    /// Human-readable item label (Moonbot terms remain untranslated).
    pub label: String,
    /// Current Terminal value for preview.
    pub current: String,
    /// New value from MoonBot for preview.
    pub new: String,
    /// Typed value used for application.
    pub value: PlannedValue,
    /// Whether the value ALREADY matches MoonBot. It is shown with a marker and unselected
    /// by default; applying it is a no-op but is not prohibited.
    pub same: bool,
}

/// MoonBot field that is NOT imported, with a reason (spec section 2, group 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported {
    pub name: String,
    pub reason: String,
}

/// Import plan containing preview groups and warnings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MoonBotImportPlan {
    /// "Terminal" group: UI theme and other non-hotkey settings.
    pub terminal: Vec<SettingChange>,
    /// "Hotkeys" group: all mapped shortcuts, including matches.
    pub hotkeys: Vec<SettingChange>,
    /// MoonBot hotkeys that are NOT imported (missing action/unknown VK), with reasons.
    /// These are displayed inside the "Hotkeys" group.
    pub unsupported_hotkeys: Vec<Unsupported>,
    /// "Chart and lines" group: colors for both themes.
    pub chart: Vec<SettingChange>,
    /// "Core" group, local config: per-core order-size presets.
    pub per_core: Vec<SettingChange>,
    /// "Core" preview/reserved group: fixed-sell values are not applied or sent to cores.
    pub core_commands: Vec<SettingChange>,
    /// "Not imported" group, excluding hotkeys stored in `unsupported_hotkeys`.
    pub unsupported: Vec<Unsupported>,
    /// Warnings such as out-of-range values.
    pub warnings: Vec<String>,
    /// Hotkeys NOT assigned in MoonBot (empty slots are not imported, preserving local values).
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

    /// Returns all applicable local items for selection construction and application iteration.
    pub fn local_items(&self) -> impl Iterator<Item = &SettingChange> {
        self.terminal
            .iter()
            .chain(self.hotkeys.iter())
            .chain(self.chart.iter())
            .chain(self.per_core.iter())
    }
}

/// Current Terminal settings against which the diff is built.
///
/// This narrow view avoids carrying the entire `AppConfig` and improves testability.
pub struct PlanContext<'a> {
    pub hotkeys: &'a HotkeysConfig,
    pub theme: &'a ChartThemeSet,
    pub orders: &'a OrdersStyleSet,
    /// Active UI theme: true means light.
    pub ui_theme_light: bool,
    /// Current size presets of the active core, used as the preview's "before" values.
    pub order_sizes: [f64; 6],
    pub order_size_sel: Option<usize>,
}

/// Builds the plan without mutating anything.
pub fn build_plan(mb: &MoonBotConfig, cur: &PlanContext) -> MoonBotImportPlan {
    let mut plan = MoonBotImportPlan::default();
    map_ui_theme(mb, cur, &mut plan);
    map_hotkeys(mb, cur, &mut plan);
    map_colors(mb, cur, &mut plan);
    map_core(mb, cur, &mut plan);
    collect_static_unsupported(mb, &mut plan);
    plan
}

// ── UI theme ─────────────────────────────────────────────────────────────────

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

// ── Hotkeys ──────────────────────────────────────────────────────────────────

/// Returns the local destination field for a MoonBot shortcut slot.
///
/// `None` means Terminal has no such action. These actions were removed on 2026-07-10 because
/// moonproto provides no send commands for them.
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

/// Returns the Moonbot slot name used in preview/unsupported lists.
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

/// Returns the current named destination-field value for the preview's "before" column.
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

/// Adds one hotkey item to the "Hotkeys" group by converting MoonBot `TShortCut` to a GPUI string.
///
/// Shows ALL values, including matches (`same`). Empty values are skipped because import assigns
/// rather than clears, but they are counted in the summary. Unsupported values go to
/// `unsupported_hotkeys` with a reason.
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
            // Empty values are not imported but are counted for the summary.
            _ => plan.hotkeys_empty += 1,
        },
    }
}

fn map_hotkeys(mb: &MoonBotConfig, cur: &PlanContext, plan: &mut MoonBotImportPlan) {
    let h = &mb.ui.hotkeys;
    // Order-size slots (OKeys → order_size) and fixed-sell slots (SKeys → sell_preset).
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
    // 27 shortcut slots: 17 importable and 10 without a Terminal command.
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
                // Terminal has no such action, so do not create a dead hotkey. Show only
                // ASSIGNED shortcuts because an empty slot has nothing to import.
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

// ── Colors (Theme block: ColorsLight → light, ColorsDark → dark) ─────────────

/// Parses a TColor from an INI string using the `0xAARRGGBB` bit layout (spec section 8).
///
/// Real MoonBot exports write an eight-character hex string (`FF008000`, `FFFFFFFF`, as verified
/// with a live clipboard), while decimal notation is also accepted. An eight-character value is
/// recognized as hexadecimal only when it contains an A-F digit (in either case) or starts with
/// `0`; an ambiguous all-digit value is parsed as decimal. Returns RGB and the alpha byte.
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
            // Our color fields have no alpha; do not silently discard meaningful alpha.
            if alpha != 0 && alpha != 0xFF {
                plan.unsupported.push(Unsupported {
                    name: format!("{key} ({theme_side})"),
                    reason: format!("цвет несёт alpha 0x{alpha:02X}, у целевого поля её нет"),
                });
                continue;
            }
            // Explicit MoonBot key → Terminal field mapping (spec section 8). Keys outside
            // this table go to unsupported: they are decoded but not applied.
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
            // The theme side is NOT in the label: preview groups colors into "Light"/"Dark"
            // columns, with the side encoded in the item id. Matching values are also shown
            // (`same`) to provide the complete import picture.
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
    // Do not apply Charts/ArbColors sections from the Ini block without an explicit mapping
    // table (spec section 9). Expand keys BY NAME with their values; this list determines which
    // mappings/fields should be added to Terminal next.
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

// ── Core ─────────────────────────────────────────────────────────────────────

/// Formats a compact preview list such as `70, 80, 300` instead of `[70.0, 80.0, 300.0]`.
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
    // Six manual-order sizes → ServerConfig.order_sizes for selected cores.
    plan.per_core.push(SettingChange {
        id: "core.order_sizes".into(),
        label: "Пресеты размера ордера (F1-F6)".into(),
        current: fmt_nums(&cur.order_sizes),
        new: fmt_nums(&h.order_sizes),
        value: PlannedValue::OrderSizes(h.order_sizes),
        same: h.order_sizes == cur.order_sizes,
    });
    // Selected preset: bNum must be in 0..=5; otherwise warn and skip it (spec section 9).
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
    // Keep fixed-sell percentages and the selected slot in a reserved preview group. Terminal
    // does not currently apply these values or send them to cores.
    plan.core_commands.push(SettingChange {
        id: "core.fixed_sell_prices".into(),
        label: "Fixed sell проценты (S1-S6)".into(),
        current: "текущие значения ядра".into(),
        new: fmt_nums(&h.fixed_sell_prices),
        value: PlannedValue::FixedSellPrices(h.fixed_sell_prices),
        same: false, // Current core values are not known locally.
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

// ── Static "not imported" items ──────────────────────────────────────────────

/// Adds UI-block fields without a working Terminal equivalent (spec section 2, group 4).
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
    // SplitParts from HotkeysPublic: Terminal has no part-count setting for Split Order.
    if mb.ui.hotkeys.split_parts > 0 {
        plan.unsupported.push(Unsupported {
            name: "SplitParts".into(),
            reason: "число частей Split Order в Terminal не настраивается".into(),
        });
    }
}

#[cfg(test)]
mod tests;
