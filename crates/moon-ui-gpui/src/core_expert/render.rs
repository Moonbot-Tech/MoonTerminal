//! Rendering of the expert core-settings window: title bar, the expert-mode switch, Moonbot's tab
//! strip, the page body and the OK/Cancel footer.
//!
//! The frame the pages hang in, and the two contracts it holds them to: nothing editable is drawn
//! unless the window is in [`PageState::Ready`], and OK is the only path to the wire. The pages
//! themselves live in [`super::pages`].

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonAlert, MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize,
    MoonPalette, MoonTabItem, MoonTabStrip, MoonWindowFrame, h_flex, v_flex,
};
use rust_i18n::t;

use crate::design::{self, moon, moon_alpha};

use super::{CoreExpertView, ExpertTab, PageState, TabSource, pages};

/// Title-bar height, matching the Screener window this one is built after.
const HEADER_H: f32 = 32.0;

impl Render for CoreExpertView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A control that held focus was dropped since the last frame; take the keyboard off it before
        // drawing, exactly as `Shell::render` does when the gear's popover goes away.
        if std::mem::take(&mut self.needs_blur) {
            window.blur();
        }
        // And put it back on the window itself, as every other window root here does: a window
        // holding no focus at all answers no hotkey.
        crate::hotkeys::restore_root_focus(&self.focus, window, cx);
        // Before anything reads them: a row draws the control its page declared, and the pages are
        // built below in this same frame.
        self.build_editors(window, cx);
        let p = MoonPalette::active(cx);
        let chrome_width = crate::window::windowing::responsive_width(window);
        v_flex()
            .size_full()
            .relative()
            .bg(moon(p.shell))
            .text_color(moon(p.text))
            // The UI face, not the monospaced one. Moonbot's dialog is drawn in a proportional
            // font, and every `MoonText` on these pages already renders in it — leaving the root on
            // `mono` made the two disagree line by line, which is exactly what a mirrored dialog
            // must not do.
            .font_family(design::ui_font())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0))
            .track_focus(&self.focus)
            .child(title_bar(p, cx))
            .child(self.switch_row(p, cx))
            .child(self.tab_strip(cx))
            .child(self.body(p, cx))
            .child(self.footer(p, cx))
            .child(
                MoonWindowFrame::tool("core-expert-frame-hit", chrome_width)
                    .header_height(HEADER_H)
                    .leading_inset(design::titlebar_leading_inset())
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            )
    }
}

/// Window title bar, drawn by the shared tool-window frame like every other tool window here.
fn title_bar(p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .id("core-expert-header")
        .relative()
        .flex_none()
        .w_full()
        .h(design::fit_h_px(cx, HEADER_H, 14.0, 9.0))
        .justify_between()
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .bg(moon(p.shell_high))
        .border_b(px(1.0))
        .border_color(moon_alpha(p.border, 1.0))
        .child(
            MoonWindowFrame::tool("core-expert-titlebar-title", 0.0)
                .title_cluster(t!("core_expert.window_title").to_string(), cx)
                .h_full()
                .flex_1()
                .min_w_0(),
        )
        .when(design::show_custom_window_controls(), |this| {
            this.child(
                MoonWindowFrame::tool("core-expert-frame-visual", 0.0)
                    .header_height(HEADER_H)
                    .show_controls(true)
                    .visual_controls(cx),
            )
        })
}

impl CoreExpertView {
    /// Row above the strip: the expert-mode switch that owns which face the gear opens, and the
    /// name of the core this window is bound to.
    ///
    /// The switch sits here rather than only in the popup because unticking it is the ONLY way back
    /// to the compact face once this window is the one the gear opens.
    fn switch_row(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        // Read, never assumed: the preference is application-wide and can be cleared from the other
        // face, so a hardcoded tick would show this window disagreeing with the gear that opened it.
        let expert = self.backend.read(cx).core_settings_expert();
        // Resolved when the binding changed, not here: this runs on every repaint, including one
        // per hover over the tab strip.
        let core_name = self.core_name.clone();
        h_flex()
            .w_full()
            .flex_none()
            .items_center()
            .gap(design::ui_px(cx, 10.0))
            .px(design::ui_px(cx, design::HEADER_PAD_X))
            .py(design::ui_px(cx, 6.0))
            .child(
                MoonCheckbox::new("core-expert-mode")
                    .label(t!("core_settings.expert").to_string())
                    .checked(expert)
                    .size(MoonCheckboxSize::Compact)
                    .on_change(move |value, window, cx| {
                        if *value {
                            return;
                        }
                        view.update(cx, |this, cx| this.leave_expert(window, cx));
                    }),
            )
            .child(div().flex_1())
            .children(core_name.map(|name| {
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text_muted))
                    .child(name)
            }))
    }

    /// Moonbot's tab strip, in Moonbot's order.
    fn tab_strip(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let selected = self.tab;
        let items: Vec<MoonTabItem> = ExpertTab::ALL
            .iter()
            .map(|tab| MoonTabItem::new(tab.title()).selected(*tab == selected))
            .collect();
        div()
            .w_full()
            .flex_none()
            .h(design::fit_h_px(cx, 28.0, 13.0, 7.5))
            .child(
                MoonTabStrip::new("core-expert-tabs")
                    .gap(4.0)
                    .overflow_menu(true)
                    .items(items)
                    .on_click(move |ix, _event, _window, app| {
                        let Some(next) = ExpertTab::at(ix) else {
                            return;
                        };
                        view.update(app, |this, cx| this.set_tab(next, cx));
                    })
                    .render(),
            )
    }

    /// Body of the selected page.
    ///
    /// Three things can be here, in this order of precedence. While the window is not
    /// [`PageState::Ready`] it says which hazard it is in — this window answers those by explaining
    /// rather than by closing. With a page staged, a PORTED tab draws its rows. A tab that is not
    /// ported yet says so, and says separately when the reason is that nothing can ever arrive for
    /// it.
    fn body(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        // Read here, from the window's own `&self`: a page is built inside this render, where
        // reading the view back would panic.
        let profit = self
            .seeded
            .and_then(|core| self.backend.read(cx).session.store().core(core))
            .and_then(|d| d.profit_state.as_ref())
            .map_or((None, None), |s| {
                (
                    Some((s.total_profit, s.total_trades)),
                    Some((s.hourly_profit, s.hourly_trades)),
                )
            });
        let ctx = pages::PageCtx {
            backend: &self.backend,
            group: &self.group,
            seeded: self.seeded,
            profit,
            hotkeys_sub: self.hotkeys_sub,
            special_section: self.special_section,
        };
        let page = self
            .draft
            .as_ref()
            .filter(|_| self.state.can_send())
            .and_then(|draft| pages::page(self.tab, &view, &self.editors, draft, &ctx, p, cx));
        // A page whose rows are all dead still says WHY above itself: the note explains the page a
        // trader is looking at, rather than standing in for one that is missing. Both dead kinds
        // qualify — nothing will ever arrive for an Absent page, and a Wire page waits on the
        // projection and the field mask.
        let source_note_over_page = page.is_some() && self.tab.source() != TabSource::Projected;
        // Only when there is no page to draw: a note about a page the trader is already looking at
        // would be describing what is on screen beside it.
        let note = (page.is_none() || source_note_over_page).then(|| match self.state {
            PageState::NoCore => t!("core_expert.no_core"),
            PageState::Overview => t!("core_expert.overview"),
            PageState::CoreMoved => t!("core_expert.core_moved"),
            PageState::Waiting => t!("core_expert.waiting"),
            PageState::Replaced => t!("core_expert.replaced"),
            PageState::Stale => t!("core_expert.stale"),
            PageState::Ready => match self.tab.source() {
                TabSource::Projected => t!("core_expert.page_todo"),
                TabSource::Wire => t!("core_expert.page_unprojected"),
                TabSource::Absent => t!("core_expert.page_absent"),
            },
        });
        // A warning, through the shared component: a page whose values cannot arrive at all is not
        // the same news as one merely awaiting its port.
        let warn = self.state.can_send() && self.tab.source() != TabSource::Projected;
        v_flex()
            .id(self.tab.element_id())
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_y_scroll()
            .gap(design::ui_px(cx, 8.0))
            .px(design::ui_px(cx, design::HEADER_PAD_X))
            .py(design::ui_px(cx, 10.0))
            .children(self.write_refused.then(|| {
                MoonAlert::error(
                    "core-expert-refused",
                    t!("core_expert.write_refused").to_string(),
                )
            }))
            .children(note.map(|note| {
                if warn {
                    MoonAlert::warning("core-expert-page-note", note.to_string()).into_any_element()
                } else {
                    crate::core_expert::widgets::text_block(
                        note.to_string(),
                        p.text_muted,
                        false,
                        cx,
                    )
                    .into_any_element()
                }
            }))
            // Moonbot's Hotkeys page carries a strip of its own, above its body.
            .children((self.tab == ExpertTab::Hotkeys && page.is_some()).then(|| {
                let view = cx.entity();
                let selected = self.hotkeys_sub;
                let items: Vec<MoonTabItem> = pages::HotkeysSub::ALL
                    .iter()
                    .map(|sub| MoonTabItem::new(sub.title()).selected(*sub == selected))
                    .collect();
                div()
                    .w_full()
                    .flex_none()
                    .h(design::fit_h_px(cx, 26.0, 13.0, 7.5))
                    .child(
                        MoonTabStrip::new("core-expert-hotkeys-tabs")
                            .gap(4.0)
                            .overflow_menu(true)
                            .items(items)
                            .on_click(move |ix, _event, _window, app| {
                                let Some(next) = pages::HotkeysSub::at(ix) else {
                                    return;
                                };
                                view.update(app, |this, cx| this.set_hotkeys_sub(next, cx));
                            })
                            .render(),
                    )
            }))
            .children(page)
    }

    /// OK and Cancel, with Moonbot's meaning: OK sends the whole page, Cancel discards it.
    ///
    /// OK is dark unless a page is actually sendable, so pressing it can never read as a save of
    /// values that reached nothing.
    fn footer(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let cancel_view = view.clone();
        let can_send = self.state.can_send();
        h_flex()
            .w_full()
            .flex_none()
            .items_center()
            .justify_end()
            .gap(design::ui_px(cx, 8.0))
            .px(design::ui_px(cx, design::HEADER_PAD_X))
            .py(design::ui_px(cx, 8.0))
            .border_t(px(1.0))
            .border_color(moon_alpha(p.border, 1.0))
            .child(
                MoonButton::new("core-expert-cancel")
                    .label(t!("core_settings.cancel").to_string())
                    .size(MoonButtonSize::Action)
                    .variant(MoonButtonVariant::Soft)
                    .padding_x(14.0)
                    .on_click(move |_, window, app| {
                        cancel_view.update(app, |this, _cx| this.cancel(window));
                    })
                    .render(),
            )
            .child(
                MoonButton::new("core-expert-ok")
                    .label(t!("core_settings.ok").to_string())
                    .size(MoonButtonSize::Action)
                    .variant(MoonButtonVariant::Blue)
                    .disabled(!can_send)
                    .padding_x(18.0)
                    .on_click(move |_, window, app| {
                        view.update(app, |this, cx| this.commit(window, cx));
                    })
                    .render(),
            )
    }
}
