//! Lower Assets-window section with Spot, Futures, and Quarterly wallet containers for the
//! selected core, drag-and-drop transfers between them, and an amount dialog whose initial text
//! adaptively formats the available quantity.

use super::*;
use anyhow::Result;
use moon_ui::{MoonNotification, MoonWindowExt as _};
use rust_i18n::t;

/// Drag-and-drop payload for moving an asset between wallets.
#[derive(Clone)]
pub(super) struct AssetDrag {
    pub(super) core: CoreId,
    pub(super) asset: String,
    pub(super) from: WalletKind,
    /// Raw available quantity passed through `num` to seed the dialog input.
    pub(super) free: f64,
}

/// Transfer awaiting confirmation while its amount dialog is open.
#[derive(Clone)]
pub(super) struct PendingTransfer {
    core: CoreId,
    asset: String,
    from: WalletKind,
    to: WalletKind,
    /// Raw available quantity displayed through `num` and used to derive the input's initial text.
    free: f64,
}

/// Coin-transfer preview displayed under the drag cursor.
struct AssetDragPreview {
    label: SharedString,
}

impl Render for AssetDragPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        div()
            .px_2()
            .py_1()
            .rounded(design::r_button(cx))
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
    /// Renders the selected core's three wallet containers in one row.
    ///
    /// The section header, label, and refresh button are owned by `bottom()`, following the
    /// window's shared collapsible-section pattern.
    pub(super) fn wallets_section(
        &self,
        core: CoreId,
        wallets: &[WalletColumnSnapshot],
        cx: &Context<Self>,
    ) -> impl IntoElement {
        h_flex().w_full().h_full().min_h(px(0.0)).children(
            wallets
                .iter()
                .map(|snapshot| self.wallet_column(core, snapshot, cx)),
        )
    }

    /// Renders one Spot, Futures, or Quarterly wallet as draggable coin rows and a drop target.
    ///
    /// Dropping a coin from another wallet of the same core opens the transfer-amount dialog;
    /// same-wallet and cross-core drops are ignored.
    fn wallet_column(
        &self,
        core: CoreId,
        snapshot: &WalletColumnSnapshot,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let kind = snapshot.kind;
        let drop_bg = design::moon_alpha(p.blue, 0.13);

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
            let fmt_qty = moon_core::util::fmt::qty;
            let preview_label: SharedString =
                format!("{} {}", a.currency, fmt_qty(a.amount)).into();
            // Match the Moonbot Transfer row: `COIN available / total (value$)`, all left-aligned.
            // Value is based on `total`; an unknown conversion rate displays `(?$)`. Keep output
            // bounded: `fmt::qty` uses up to three decimal places and `fmt::usd` up to two,
            // rather than emitting long adaptive decimals.
            let qty_txt = format!("{} / {}", fmt_qty(a.amount), fmt_qty(a.total));
            let value_txt = if a.value_usdt > 0.0 {
                format!("({}$)", moon_core::util::fmt::usd(a.value_usdt))
            } else {
                "(?$)".to_string()
            };
            // Load the coin image from assets/coins or its runtime override and render it at the
            // theme-scaled icon size, without assuming source dimensions. A missing icon gets an
            // equal-width empty cell to align tickers.
            let icon_side = design::ui_px(cx, 16.0);
            let icon: AnyElement = match crate::media::coin_icons::coin_icon(&a.currency) {
                Some(tex) => img(tex)
                    .w(icon_side)
                    .h(icon_side)
                    .flex_none()
                    .into_any_element(),
                None => div()
                    .w(icon_side)
                    .h(icon_side)
                    .flex_none()
                    .into_any_element(),
            };
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
                    .rounded(design::ui_px(cx, 3.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_grab()
                    .text_color(rgb(p.text))
                    // Give draggable rows a conspicuous hover highlight.
                    .hover(|s| {
                        s.bg(design::moon_alpha(p.blue, 0.15))
                            .border_color(rgb(p.blue))
                    })
                    .border_1()
                    .border_color(rgba(0x00000000))
                    .child(icon)
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(a.currency.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgb(p.text_muted))
                            .child(qty_txt),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgb(p.text_soft))
                            .child(value_txt),
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
                    // Center the heading like Moonbot's Spot/Futures/Quarterly groups.
                    .text_center()
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
                    .drag_over::<AssetDrag>(move |s, _drag, _w, _cx| s.bg(drop_bg))
                    .on_drop(cx.listener(move |this, drag: &AssetDrag, window, cx| {
                        if drag.core == core && drag.from != kind {
                            this.open_transfer_dialog(drag, kind, window, cx);
                        }
                    }))
                    .child(list),
            )
    }

    /// Opens an amount dialog for a dropped coin.
    ///
    /// Seeds the input with `num(drag.free)`, an adaptive representation that may round and need
    /// not equal the exact raw available quantity.
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
                .rounded(design::r_container(cx))
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
                                .label(format!("  {}  ", t!("dialogs.cancel")))
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
                                .label(format!("  {}  ", t!("assets.transfer_btn")))
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

    /// Parses and confirms the pending transfer amount.
    ///
    /// A positive amount sends the transfer command; invalid, zero, or negative input sends
    /// nothing. Success clears dialog state, while a command error is returned so the caller can
    /// keep the dialog open and show a notification.
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
