//! Дренаж запросов backend'а (инлайн-правки размеров/sell, тосты Engine-действий) и
//! обработка хоткеев корня окна. Вынесено из `shell/mod.rs` точь-в-точь.

use gpui::*;

use moon_core::feed::ClientSettingsEdit;

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
                            .title(rust_i18n::t!("shell.engine_toast.fail", core = core).to_string())
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

    /// Обработка хоткея корня окна: F1-F6 = выбрать пресет размера активного ядра; S1-S6 =
    /// задействовать fixed-sell слот (гасит TP); cancel_buy — отмена покупок Main. Гасит событие,
    /// если хоткей совпал.
    pub(super) fn on_hotkey(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let group = self.group.clone();
        let handled = self.backend.update(cx, |b, bcx| {
            // Фаза 1 (только чтение cfg): какой хоткей совпал. Сравниваем нажатую
            // клавишу с каждым настроенным сочетанием (gpui Keystroke).
            let (size_ix, sell_ix, is_cancel, fig_tool, is_fig_delete, is_escape, is_fig_alert) = {
                let cfg = b.preview.as_ref().unwrap_or(&b.config);
                // Сравниваем ТОЛЬКО modifiers+key: event на Windows несёт ещё и
                // key_char (напечатанный символ), которого у Keystroke::parse нет —
                // полное `==` из-за этого никогда не совпадает для буквенных клавиш
                // (F-клавиши работали случайно: у них key_char=None с обеих сторон).
                let pressed = |raw: &str| {
                    let raw = raw.trim();
                    !raw.is_empty()
                        && matches!(
                            Keystroke::parse(raw),
                            Ok(k) if k.modifiers == ev.keystroke.modifiers
                                && k.key == ev.keystroke.key
                        )
                };
                let size_ix = cfg.hotkeys.order_size.iter().position(|r| pressed(r));
                let sell_ix = cfg.hotkeys.sell_preset.iter().position(|r| pressed(r));
                use moon_core::figures::FigureTool;
                let fig_tool = if pressed(&cfg.hotkeys.draw_hline) {
                    Some(FigureTool::HLine)
                } else if pressed(&cfg.hotkeys.draw_segment) {
                    Some(FigureTool::Segment)
                } else if pressed(&cfg.hotkeys.draw_triangle) {
                    Some(FigureTool::Triangle)
                } else if pressed(&cfg.hotkeys.draw_channel) {
                    Some(FigureTool::Channel)
                } else {
                    None
                };
                (
                    size_ix,
                    sell_ix,
                    pressed(&cfg.hotkeys.cancel_buy),
                    fig_tool,
                    pressed(&cfg.hotkeys.fig_delete),
                    ev.keystroke.key == "escape" && ev.keystroke.modifiers == Modifiers::default(),
                    pressed(&cfg.hotkeys.fig_alert),
                )
            };
            // Слой рисования: хоткей инструмента выбирает его И включает режим-карандаш
            // (повтор того же инструмента в активном режиме — выключает). Esc выключает
            // режим. fig_delete/fig_alert работают по выделенной фигуре. До торговых веток.
            if let Some(tool) = fig_tool {
                if b.fig_draw_mode && b.fig_tool == tool {
                    b.fig_draw_mode = false;
                } else {
                    b.fig_tool = tool;
                    b.fig_draw_mode = true;
                }
                bcx.notify();
                return true;
            }
            if is_escape && b.fig_draw_mode {
                b.fig_draw_mode = false;
                bcx.notify();
                return true;
            }
            if is_fig_alert && b.toggle_selected_figure_alert() {
                bcx.notify();
                return true;
            }
            if is_fig_delete {
                if let Some((core, market, id)) = b.fig_selected.clone() {
                    b.remove_figure(core, &market, id);
                    bcx.notify();
                    return true;
                }
            }
            // Фаза 2 (мутация): F1-F6 = выбрать пресет размера активного ядра; S1-S6 =
            // задействовать fixed-sell слот (гасит TP); cancel_buy — отмена покупок Main.
            if let Some(i) = size_ix {
                match b.active_trade_core(&group) {
                    Some(core) => {
                        b.order_size_sel.insert(core, i);
                        b.order_size_rev = b.order_size_rev.wrapping_add(1);
                        bcx.notify();
                        true
                    }
                    None => false,
                }
            } else if let Some(i) = sell_ix {
                match b.active_trade_core(&group) {
                    Some(core) => {
                        if let Err(error) = b.session.edit_client_settings(
                            core,
                            ClientSettingsEdit::SelectFixedSellSlot(i + 1),
                        ) {
                            log::warn!("hotkey select fixed-sell slot failed: {error}");
                        }
                        true
                    }
                    None => false,
                }
            } else if is_cancel {
                b.cancel_buy_for_main_chart(&group);
                true
            } else {
                false
            }
        });
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
        K::SetLeverage { market, leverage } => {
            t!("shell.engine_action.set_leverage", market = market, lev = leverage).to_string()
        }
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
