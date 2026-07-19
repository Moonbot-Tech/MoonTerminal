//! Дренаж запросов backend'а (инлайн-правки размеров/sell, тосты Engine-действий) и
//! обработка хоткеев корня окна. Вынесено из `shell/mod.rs` точь-в-точь.

use gpui::*;

use super::Shell;
use crate::controls;

impl Shell {
    pub(super) fn drain_order_size_edit_request(&mut self, cx: &mut Context<Self>) {
        let edit_req = self.backend.update(cx, |b, _| b.order_size_edit_req.take());
        let Some((core, ix)) = edit_req.filter(|(_, i)| *i < 6) else {
            return;
        };
        let cur = {
            let b = self.backend.read(cx);
            let base = b.session.core_base(core).unwrap_or("");
            b.config
                .servers
                .iter()
                .find(|s| s.id == core)
                .map(|s| s.order_sizes_or_default(base)[ix])
                .unwrap_or_else(|| moon_core::config::servers::default_order_sizes(base)[ix])
        };
        self.size_edit = Some((core, ix));
        let input = self.size_input.clone();
        let value = controls::fmt_adaptive(cur);
        let handle = self.window_handle;
        cx.defer(move |app| {
            let _ = handle.update(app, move |_, window, app| {
                input.update(app, |st, cx| {
                    st.set_value(value, window, cx);
                    st.focus(window, cx);
                });
            });
        });
    }

    /// Тосты результатов Engine-действий (плечо/hedge/cancel-all/перенос/…).
    /// Очередь общая в CoreStore; забирает её только Shell АКТИВНОГО окна — иначе
    /// тот же тост показали бы все окна-группы (наблюдают один Backend). Пока ни
    /// одно окно не активно, результаты копятся в очереди (кап в CoreData) и
    /// всплывут на ближайшем notify после активации.
    pub(super) fn drain_engine_action_toasts(&mut self, cx: &mut Context<Self>) {
        if !self.window_active {
            return;
        }
        let toasts = self
            .backend
            .update(cx, |b, _| b.session.take_engine_action_toasts());
        if toasts.is_empty() {
            return;
        }
        let handle = self.window_handle;
        cx.defer(move |app| {
            let _ = handle.update(app, move |_, window, app| {
                use moon_ui::MoonWindowExt as _;
                for (core, r) in toasts {
                    let action = engine_action_label(&r.kind);
                    let note = if r.success {
                        moon_ui::MoonNotification::success(action)
                            .title(rust_i18n::t!("shell.engine_toast.ok", core = core).to_string())
                    } else {
                        let mut err = r.error_msg.clone();
                        if err.is_empty() {
                            err = format!("code {}", r.error_code);
                        }
                        moon_ui::MoonNotification::error(format!("{action}: {err}"))
                            .title(
                                rust_i18n::t!("shell.engine_toast.fail", core = core).to_string(),
                            )
                            .autohide(false)
                    };
                    window.push_notification(note, app);
                }
            });
        });
    }

    /// Дабл-клик по S-кнопке: открыть инпут поверх неё с текущим процентом пресета и
    /// сфокусировать (аналог `drain_order_size_edit_request`, но значение — из ядра).
    pub(super) fn drain_sell_edit_request(&mut self, cx: &mut Context<Self>) {
        let edit_req = self.backend.update(cx, |b, _| b.sell_edit_req.take());
        let Some((core, ix)) = edit_req.filter(|(_, i)| *i < 6) else {
            return;
        };
        let cur = self.backend.read(cx).fixed_sell_pct(core, ix);
        self.sell_edit = Some((core, ix));
        let input = self.sell_input.clone();
        let value = controls::fmt_adaptive(cur);
        let handle = self.window_handle;
        cx.defer(move |app| {
            let _ = handle.update(app, move |_, window, app| {
                input.update(app, |st, cx| {
                    st.set_value(value, window, cx);
                    st.focus(window, cx);
                });
            });
        });
    }

    /// Обработка хоткея корня окна группы через ЕДИНЫЙ распознаватель [`crate::hotkeys`].
    /// Таргет торговых действий — рынок фуллскрин-чарта группы (`main_chart_target`),
    /// активное ядро — `active_trade_core`. Масштаб идёт через rev-механизм группы (свой,
    /// не через `apply`). Гасит событие, если хоткей совпал.
    pub(super) fn on_hotkey(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        use crate::hotkeys::HotkeyAction;
        let action = {
            let b = self.backend.read(cx);
            let hk = &b.preview.as_ref().unwrap_or(&b.config).hotkeys;
            crate::hotkeys::resolve(ev, hk)
        };
        let Some(action) = action else {
            return;
        };
        let group = self.group.clone();
        let handled = match action {
            // Масштаб Y активной вкладки группы: листаем ступень и командуем ИМЕННО этой
            // группе (price_scale_group=group) — её ChartTabs применит к активной панели
            // (sig.rs сворачивает rev только для своей группы).
            HotkeyAction::ScalePlus | HotkeyAction::ScaleMinus => {
                let zoom_in = matches!(action, HotkeyAction::ScalePlus);
                self.backend.update(cx, |b, _| {
                    let next = crate::controls::step_scale(b.price_scale, zoom_in);
                    b.price_scale = next;
                    b.price_scale_group = Some(group.clone());
                    b.price_scale_rev = b.price_scale_rev.wrapping_add(1);
                });
                true
            }
            // Переключить активный (fullscreen) чарт Main-стека группы на следующий — через
            // rev-механизм этой группы (её ChartTabs слушает backend и листает свой стек), как
            // масштаб. Отдельный от scale rev, чтобы зум и переключение не мешали друг другу.
            HotkeyAction::SwitchCharts => {
                self.backend.update(cx, |b, _| {
                    b.switch_charts_group = Some(group.clone());
                    b.switch_charts_rev = b.switch_charts_rev.wrapping_add(1);
                });
                true
            }
            // Ручной ордер по цене под курсором: ставит чарт под мышью (`hovered_chart`) —
            // цену знает только он. Работает независимо от того, какое окно в фокусе.
            HotkeyAction::NewLong | HotkeyAction::NewShort => {
                let short = matches!(action, HotkeyAction::NewShort);
                let chart = self
                    .backend
                    .read(cx)
                    .hovered_chart
                    .clone()
                    .and_then(|w| w.upgrade());
                match chart {
                    Some(chart) => chart.update(cx, |p, pcx| p.place_order_at_cursor(short, pcx)),
                    None => false,
                }
            }
            // Встроенный Ctrl+Shift+F10: сброс позиций всех окон (спасение уехавших).
            HotkeyAction::ResetWindows => {
                crate::windowing::reset_all_windows_onscreen(cx);
                true
            }
            // Встроенный Tab/Del: отмена ордера под курсором (чарт под мышью знает наведённый).
            HotkeyAction::CancelHoveredOrder => {
                crate::hotkeys::cancel_hovered_order(&self.backend, cx)
            }
            // Del: приоритет — удалить выделенную фигуру; выделения нет → фолбэк на встроенную
            // отмену ордера под курсором (дефолтный fig_delete=Del резолвится раньше встроенной
            // ветки Tab/Del и без фолбэка затенял бы её навсегда).
            HotkeyAction::FigDelete => {
                self.backend.update(cx, |b, bcx| {
                    // Таргет/ядро FigDelete не использует.
                    crate::hotkeys::apply(action, b, bcx, None, None)
                }) || crate::hotkeys::cancel_hovered_order(&self.backend, cx)
            }
            // Встроенный Shift+Esc: закрыть все графики Main всех групп (глобальный бамп rev).
            HotkeyAction::CloseAllCharts => {
                self.backend.update(cx, |b, _| {
                    b.close_all_charts_rev = b.close_all_charts_rev.wrapping_add(1);
                });
                true
            }
            // Встроенный Esc: ВСЕГДА закрывает ЧАРТ в фулскрине Main ЭТОЙ группы (и только его),
            // как в Moonbot — режим рисования при этом НЕ выключается (он тумблерится карандашом).
            // Окно/откреп не трогаем.
            HotkeyAction::CloseActiveChart => {
                self.backend.update(cx, |b, _| {
                    b.close_active_chart_group = Some(group.clone());
                    b.close_active_chart_rev = b.close_active_chart_rev.wrapping_add(1);
                });
                true
            }
            other => self.backend.update(cx, |b, bcx| {
                let target = b.main_chart_target(&group);
                let active_core = b.active_trade_core(&group);
                crate::hotkeys::apply(other, b, bcx, target, active_core)
            }),
        };
        if handled {
            cx.stop_propagation();
        }
    }
}

/// Человекочитаемая подпись Engine-действия для тоста (что именно подтвердило/
/// отклонило ядро). Кошельки — через `WalletKind::label()`, как в дереве Активов.
fn engine_action_label(kind: &moon_core::feed::EngineActionKind) -> String {
    use moon_core::feed::EngineActionKind as K;
    use rust_i18n::t;
    match kind {
        K::CancelAllOrders => t!("shell.engine_action.cancel_all").to_string(),
        K::SetLeverage { market, leverage } => t!(
            "shell.engine_action.set_leverage",
            market = market,
            lev = leverage
        )
        .to_string(),
        K::SetHedgeMode { on: true } => t!("shell.engine_action.hedge_on").to_string(),
        K::SetHedgeMode { on: false } => t!("shell.engine_action.hedge_off").to_string(),
        K::ChangePositionType { market } => {
            t!("shell.engine_action.change_position_type", market = market).to_string()
        }
        K::ConvertDust => t!("shell.engine_action.convert_dust").to_string(),
        K::ConfirmRiskLimit { market } => {
            t!("shell.engine_action.confirm_risk_limit", market = market).to_string()
        }
        K::SetMaMode { on: true } => t!("shell.engine_action.ma_on").to_string(),
        K::SetMaMode { on: false } => t!("shell.engine_action.ma_off").to_string(),
        K::TransferAsset {
            asset,
            qty,
            from,
            to,
        } => t!(
            "shell.engine_action.transfer",
            qty = controls::fmt_adaptive(*qty),
            asset = asset,
            from = from.label(),
            to = to.label()
        )
        .to_string(),
        K::ReloadOrderBook => t!("shell.engine_action.reload_order_book").to_string(),
    }
}
