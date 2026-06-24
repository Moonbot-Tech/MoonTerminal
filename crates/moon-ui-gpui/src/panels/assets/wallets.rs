//! Нижняя часть окна «Активы»: 3 контейнера кошельков (Спот/Фьючерсы/Квартальные) с
//! монетами выбранного ядра, drag&drop переносом между ними и модальным диалогом
//! количества (дефолт — всё свободное).

use super::*;
use anyhow::Result;
use moon_ui::{MoonNotification, MoonWindowExt as _};
use rust_i18n::t;

/// Полезная нагрузка drag&drop переноса актива между кошельками.
#[derive(Clone)]
pub(super) struct AssetDrag {
    pub(super) core: CoreId,
    pub(super) asset: String,
    pub(super) from: WalletKind,
    /// Свободное количество монеты (дефолт для диалога — перенести всё).
    pub(super) free: f64,
}

/// Ожидающий подтверждения перенос (открыт диалог количества).
#[derive(Clone)]
pub(super) struct PendingTransfer {
    core: CoreId,
    asset: String,
    from: WalletKind,
    to: WalletKind,
    /// Свободное количество (максимум / дефолт).
    free: f64,
}

/// Превью под курсором при перетаскивании монеты.
struct AssetDragPreview {
    label: SharedString,
}

impl Render for AssetDragPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        div()
            .px_2()
            .py_1()
            .rounded(px(4.0))
            .bg(rgb(p.shell_high))
            .border_1()
            .border_color(rgb(p.blue))
            .text_color(rgb(p.text))
            .text_size(design::t_body(cx))
            .font_family(design::mono())
            .child(self.label.clone())
    }
}

impl AssetsView {
    /// Секция кошельков ядра: заголовок (+ ↻ refresh) и 3 контейнера в ряд.
    pub(super) fn wallets_section(
        &self,
        core: CoreId,
        wallets: &[WalletColumnSnapshot],
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        v_flex()
            .w_full()
            .h_full()
            .child(
                h_flex()
                    .w_full()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .px(design::ui_px(cx, 8.0))
                    .py(design::ui_px(cx, 4.0))
                    .child(
                        div()
                            .text_size(design::t_body(cx))
                            .text_color(rgb(p.text_muted))
                            .child(t!("assets.wallets_hint").to_string()),
                    )
                    .child(
                        MoonButton::new("assets-refresh-transfer")
                            .ghost()
                            .size(MoonButtonSize::Micro)
                            .label("↻")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Err(error) =
                                    this.backend.read(cx).session.refresh_transfer_assets(core)
                                {
                                    log::warn!("assets refresh failed for core {core}: {error}");
                                    window.push_notification(
                                        MoonNotification::error(error.to_string()),
                                        cx,
                                    );
                                }
                                let backend = this.backend.clone();
                                this.rebuild_cache(backend.read(cx));
                                cx.notify();
                            }))
                            .render(),
                    ),
            )
            .child(
                h_flex().w_full().flex_1().min_h(px(0.0)).children(
                    wallets
                        .iter()
                        .map(|snapshot| self.wallet_column(core, snapshot, cx)),
                ),
            )
    }

    /// Один контейнер кошелька (Спот/Фьючерсы/Квартальные): монеты (draggable) и
    /// drop-таргет. Бросок монеты из другого кошелька открывает диалог количества.
    fn wallet_column(
        &self,
        core: CoreId,
        snapshot: &WalletColumnSnapshot,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let kind = snapshot.kind;

        let mut list = v_flex().w_full().gap_0().p(px(4.0));
        if snapshot.rows.is_empty() {
            list = list.child(
                div()
                    .px(design::ui_px(cx, 6.0))
                    .py(px(2.0))
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_muted))
                    .child("—"),
            );
        }
        for a in &snapshot.rows {
            let drag = AssetDrag {
                core,
                asset: a.currency.clone(),
                from: kind,
                free: a.amount,
            };
            let preview_label: SharedString = format!("{} {}", a.currency, num(a.amount)).into();
            list = list.child(
                div()
                    .id(SharedString::from(format!(
                        "coin-{core}-{}-{}",
                        kind.to_u8(),
                        a.currency
                    )))
                    .w_full()
                    .h(design::fit_h_px(cx, 26.0, 12.0, 6.0))
                    .px(design::ui_px(cx, 6.0))
                    .rounded(px(3.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_grab()
                    .text_color(rgb(p.text))
                    // Заметная подсветка строки при наведении (видно, что потащишь).
                    .hover(|s| s.bg(rgba(0x3b82f626)).border_color(rgb(p.blue)))
                    .border_1()
                    .border_color(rgba(0x00000000))
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(p.blue))
                            .child(a.currency.clone()),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgb(p.text_muted))
                            .child(num(a.amount)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgb(p.text_soft))
                            .child(money(a.value_usdt)),
                    )
                    .on_drag(drag, move |_d, _pos, _w, cx| {
                        cx.new(|_| AssetDragPreview {
                            label: preview_label.clone(),
                        })
                    }),
            );
        }

        v_flex()
            .flex_1()
            .h_full()
            .min_w_0()
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
                    .child(format!("{} ({})", kind.label(), snapshot.total_count)),
            )
            .child(
                div()
                    .id(SharedString::from(format!(
                        "wallet-col-{core}-{}",
                        kind.to_u8()
                    )))
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .drag_over::<AssetDrag>(|s, _drag, _w, _cx| s.bg(rgba(0x3b82f622)))
                    .on_drop(cx.listener(move |this, drag: &AssetDrag, window, cx| {
                        if drag.core == core && drag.from != kind {
                            this.open_transfer_dialog(drag, kind, window, cx);
                        }
                    }))
                    .child(list),
            )
    }

    /// Открыть диалог количества для переноса монеты (дефолт — всё свободное).
    fn open_transfer_dialog(
        &mut self,
        drag: &AssetDrag,
        to: WalletKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let default_qty = num(drag.free);
        let input = cx.new(|cx| MoonInputState::new(window, cx).default_value(&default_qty));
        let pending = PendingTransfer {
            core: drag.core,
            asset: drag.asset.clone(),
            from: drag.from,
            to,
            free: drag.free,
        };
        self.pending_transfer = Some(pending.clone());
        self.transfer_input = Some(input);
        let view = cx.entity();
        window.open_unique_moon_dialog("assets-transfer-dialog", cx, move |dialog, _window, cx| {
            let p = MoonPalette::active(cx);
            let title = t!(
                "assets.transfer_title",
                coin = pending.asset,
                from = pending.from.label(),
                to = pending.to.label()
            )
            .to_string();
            let content_view = view.clone();
            let cancel_view = view.clone();
            let close_view = view.clone();
            let footer_cancel_view = view.clone();
            let footer_confirm_view = view.clone();

            dialog
                .w(px(320.0))
                .close_button(true)
                .overlay(true)
                .overlay_closable(true)
                .bg(rgb(p.shell_high))
                .border_color(rgb(p.border))
                .rounded(px(8.0))
                .text_color(rgb(p.text))
                .on_cancel(move |_, _, cx| {
                    cancel_view.update(cx, |this, cx| this.close_transfer_dialog(cx));
                    true
                })
                .on_close(move |_, _, cx| {
                    close_view.update(cx, |this, cx| this.close_transfer_dialog(cx));
                })
                .content(move |content, _window, cx| {
                    let input = content_view.read(cx).transfer_input.clone();
                    let mut body = v_flex()
                        .gap(design::ui_px(cx, 10.0))
                        .font_family(design::mono())
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(p.text))
                                .child(title.clone()),
                        )
                        .child(
                            div()
                                .text_size(design::t_body(cx))
                                .text_color(rgb(p.text_muted))
                                .child(t!("assets.free", n = num(pending.free)).to_string()),
                        );
                    if let Some(input) = input {
                        body = body.child(MoonInput::new("transfer-amount").state(&input).small());
                    }
                    content.child(body)
                })
                .footer(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .justify_end()
                        .child(
                            MoonButton::new("transfer-cancel")
                                .outline()
                                .size(MoonButtonSize::Action)
                                .label(t!("dialogs.cancel").to_string())
                                .on_click(move |_, window, cx| {
                                    footer_cancel_view
                                        .update(cx, |this, cx| this.close_transfer_dialog(cx));
                                    window.close_dialog(cx);
                                })
                                .render(),
                        )
                        .child(
                            MoonButton::new("transfer-confirm")
                                .primary()
                                .size(MoonButtonSize::Action)
                                .label(t!("assets.transfer_btn").to_string())
                                .on_click(move |_, window, cx| {
                                    match footer_confirm_view
                                        .update(cx, |this, cx| this.confirm_transfer(cx))
                                    {
                                        Ok(()) => window.close_dialog(cx),
                                        Err(error) => {
                                            log::warn!("asset transfer failed: {error}");
                                            window.push_notification(
                                                MoonNotification::error(error.to_string()),
                                                cx,
                                            );
                                        }
                                    }
                                })
                                .render(),
                        ),
                )
        });
        cx.notify();
    }

    /// Подтвердить перенос: прочитать количество из поля, выполнить и закрыть диалог.
    fn confirm_transfer(&mut self, cx: &mut Context<Self>) -> Result<()> {
        let Some(pt) = self.pending_transfer.clone() else {
            return Ok(());
        };
        let qty = self
            .transfer_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string())
            .and_then(|s| s.trim().replace(',', ".").parse::<f64>().ok())
            .unwrap_or(0.0);
        if qty > 0.0 {
            self.backend.read(cx).session.transfer_asset(
                pt.core,
                pt.asset.clone(),
                qty,
                pt.from,
                pt.to,
            )?;
        }
        self.close_transfer_dialog(cx);
        Ok(())
    }

    fn close_transfer_dialog(&mut self, cx: &mut Context<Self>) {
        self.pending_transfer = None;
        self.transfer_input = None;
        cx.notify();
    }
}
