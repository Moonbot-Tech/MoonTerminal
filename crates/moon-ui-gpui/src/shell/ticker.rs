//! Header ticker-source popup hosted under the header's rate ticker.
//!
//! The popup reuses [`crate::chart_tabs::coin_search`] and persists the selected market and stable
//! core ID through `Backend::set_header_ticker`. Its overlay and dismiss layers follow the other
//! header popups.

use gpui::*;

use moon_ui::{MoonInput, MoonPalette, v_flex};
use rust_i18n::t;

use crate::chart_tabs::coin_search;
use crate::design;

use super::Shell;

impl Shell {
    /// Toggle the ticker-source popup and clear its search query when opening it.
    pub(crate) fn toggle_ticker_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ticker_popup_open {
            self.ticker_popup_open = false;
        } else {
            self.ticker_popup_open = true;
            self.ticker_popup_hovered = false;
            // Defaults on open: the popup is closed from several places that never pass
            // `close_ticker_popup`, so opening is the only reliable reset point.
            self.ticker_expanded.clear();
            self.ticker_input
                .update(cx, |st, c| st.set_value(String::new(), window, c));
        }
        cx.notify();
    }

    pub(super) fn close_ticker_popup(&mut self, cx: &mut Context<Self>) {
        if self.ticker_popup_open {
            self.ticker_popup_open = false;
            // The next open starts from the defaults, like the query does.
            self.ticker_expanded.clear();
            cx.notify();
        }
    }

    /// Records an explicit expansion override so the shared size-based defaults need no seeding.
    fn toggle_ticker_expanded(
        &mut self,
        key: crate::controls::coin_search::CoinGroupKey,
        cx: &mut Context<Self>,
    ) {
        if !self.ticker_expanded.remove(&key) {
            self.ticker_expanded.insert(key);
        }
        cx.notify();
    }

    /// Build the right-anchored ticker popup and its full-window dismiss layer.
    ///
    /// Both elements are `None` when the popup is closed or the header `level` hides the ticker
    /// trigger. `widths` are the header's measured widths for this frame, the same the header was
    /// drawn from.
    pub(super) fn ticker_popup_layers(
        &mut self,
        level: design::HeaderLevel,
        widths: &design::HeaderWidths,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> (Option<AnyElement>, Option<AnyElement>) {
        // Collapsing past the ticker's level takes the trigger away, so the open state has to go with it:
        // returning the layers as `None` while leaving the flag set means widening the window later
        // resurrects the popup — full-window dismiss layer and all — with no user action.
        // No `notify` needed: this frame renders nothing either way, and the flag only decides
        // subsequent frames.
        if !design::ticker_visible(level) {
            self.ticker_popup_open = false;
            return (None, None);
        }
        if !self.ticker_popup_open {
            return (None, None);
        }
        let query = self.ticker_input.read(cx).value().to_string();
        let results = {
            let b = self.backend.read(cx);
            coin_search::search(b, &self.group, None, &query)
        };
        let backend = self.backend.clone();
        let view = cx.entity();
        let ticker_field = self.ticker_input.clone();
        let view_expand = cx.entity();
        // Always a query list: this field picks a RATE to display in the header, so "recently
        // opened chart" and "biggest mover" would be the wrong universe to offer.
        let list = coin_search::render_popup(
            "header-ticker-search",
            crate::controls::coin_search::CoinResults::Query(results),
            &Default::default(),
            &self.ticker_expanded,
            false,
            // The header ticker is not scoped to a core: its rows open on the first core that
            // carries the instrument, exactly the row it showed first before grouping.
            None,
            None,
            // No tabs: this field picks a rate for the header, not a chart to open.
            None,
            p,
            cx,
            move |core, market, window, app| {
                backend.update(app, |b, bcx| {
                    b.set_header_ticker(core, market);
                    bcx.notify();
                });
                view.update(app, |this, cx| this.close_ticker_popup(cx));
                // The pick closed the list; holding the keyboard afterwards would eat the editing
                // shortcuts the hotkeys are bound to. See `release_focus`.
                crate::controls::coin_search::release_focus(&ticker_field, window, app);
            },
            |_, _, _| {},
            move |key, app| {
                view_expand.update(app, |this, cx| this.toggle_ticker_expanded(key, cx));
            },
            |_, _| {},
        );

        // Anchored to the window's RIGHT edge and offset inward past everything that stands to the
        // ticker's right in the header: the sleep cluster, the clock, the rule, the launchers, the
        // window controls, every gap between them and the row's own padding. `right_of_ticker`
        // is that sum, from the same measured widths the header was laid out with this frame, so
        // the popup cannot detach from its trigger by a header tweak that forgets this file.
        //
        // A MoonUI Root-owned anchored layer that follows the trigger would delete this arithmetic
        // outright; `MoonPopover` alone does not fit, because it cannot preserve this popup's
        // double-click-only opening.
        let right = widths.right_of_ticker(level);
        // Start below the drawn header, whatever height the font delta gives it, so the popup
        // never overlaps chrome.
        let top = design::header_height(cx);
        let overlay = div()
            .id("header-ticker-popup-box")
            .absolute()
            .right(px(right))
            .top(px(top))
            .on_mouse_down(MouseButton::Left, |_, _w, app| app.stop_propagation())
            // The results list inside occludes on its own, but this box extends above it — over
            // the caption and the query input — and that band would otherwise pass the wheel
            // straight through to whatever the header sits on.
            .occlude()
            .on_hover(cx.listener(|this, hovered: &bool, window, cx| {
                if *hovered {
                    this.ticker_popup_hovered = true;
                } else if this.ticker_popup_hovered && !cx.has_active_drag() {
                    this.close_ticker_popup(cx);
                    crate::controls::coin_search::release_focus(&this.ticker_input, window, cx);
                }
            }))
            .child(
                // `render_popup` font-scales the shared nominal width, while this outer box keeps
                // it as a raw pixel base and adds scaled horizontal padding plus two raw border
                // pixels. The dimensions align at the default font scale but do not scale together.
                v_flex()
                    .w(px(crate::controls::coin_search::COIN_POPUP_W
                        + 2.0 * f32::from(design::ui_px(cx, 6.0))
                        + 2.0))
                    .gap(design::ui_px(cx, 4.0))
                    .p(design::ui_px(cx, 6.0))
                    // `panel_high`, NOT the `shell_high` a `MoonPopover` paints: this popup opens
                    // directly over the header and toolbar, and `chrome`/`tabbar` are literally the
                    // same value as `shell_high`, so that token would leave only a 1px border to
                    // separate the popup from the strip behind it. The anchored popovers survive it
                    // because they drop a shadow, which this hand-positioned box has no way to draw.
                    .bg(rgb(p.panel_high))
                    .border_1()
                    .border_color(rgb(p.border))
                    .rounded(design::r_button(cx))
                    .child(
                        div()
                            .text_size(design::t_caption(cx))
                            .text_color(rgb(p.text_muted))
                            .child(t!("header.ticker_pick").to_string()),
                    )
                    .child(
                        MoonInput::new("header-ticker-query")
                            .state(&self.ticker_input)
                            .size(design::INPUT_SIZE),
                    )
                    .child(list),
            )
            .into_any_element();
        let dismiss = div()
            .id("header-ticker-popup-dismiss")
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _ev, window, cx| {
                    this.close_ticker_popup(cx);
                    crate::controls::coin_search::release_focus(&this.ticker_input, window, cx);
                    cx.stop_propagation();
                }),
            )
            .into_any_element();
        (Some(overlay), Some(dismiss))
    }
}
