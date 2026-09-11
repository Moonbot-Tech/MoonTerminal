//! Rendering of the expert core-settings window: title bar, the expert-mode switch, the core list
//! down the left, Moonbot's tab strip, the page body and the footer with its send plan and OK.
//!
//! The frame the pages hang in, and the two contracts it holds them to: nothing editable is drawn
//! unless the window is in [`PageState::Ready`], and the footer's two send buttons — Apply and OK
//! — are the only path to the wire. The pages themselves live in [`super::pages`], the left
//! column in [`super::sidebar`].

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonAlert, MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize,
    MoonPalette, MoonTabItem, MoonTabStrip, MoonWindowFrame, h_flex, v_flex,
};
use rust_i18n::t;

use crate::design::{self, moon, moon_alpha};

use super::{CoreExpertView, ExpertTab, PageState, TabSource, mixed, pages, widgets};

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
        let p = MoonPalette::active(cx);
        // What still needs the trader's attention — one list for the marks and for the badges,
        // so the two agree — set before the editors are built, since the boxes are emptied there.
        // Collected before the scope takes it: the iterator borrows the window the scope is a
        // field of.
        let attention: Vec<usize> = self.attention().collect();
        self.mixed.set_context(attention.into_iter(), p.amber);
        // Before anything reads them: a row draws the control its page declared, and the pages are
        // built below in this same frame.
        self.build_editors(window, cx);
        // Built before the tree: it needs `window` and `&mut cx`, which the builder chain below
        // cannot lend it while it is also reading `cx` for its own scaled metrics.
        let tab_strip = self.tab_strip(window, cx);
        // The page's widgets are free functions that cannot read this view back mid-render, so
        // the scope they ask is lent to the thread for exactly the body's construction.
        mixed::install(std::mem::take(&mut self.mixed));
        let body = self.body(p, window, cx).into_any_element();
        self.mixed = mixed::take();
        // How many selected cores OK reaches, and how many it skips for want of a page.
        let (pages, skipped) = self.roster.selected_pages(&self.selection);
        let sidebar = self.sidebar(p, cx).into_any_element();
        let footer = self.footer(pages, skipped, p, cx).into_any_element();
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
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .items_stretch()
                    .child(sidebar)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(tab_strip)
                            .child(body),
                    ),
            )
            .child(footer)
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
    /// Row above the list and the strip: the expert-mode switch that owns which face the gear
    /// opens, and the name of the core whose page is drawn.
    ///
    /// The switch sits here rather than only in the popup because unticking it is the ONLY way back
    /// to the compact face once this window is the one the gear opens.
    fn switch_row(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        // Read, never assumed: the preference is application-wide and can be cleared from the other
        // face, so a hardcoded tick would show this window disagreeing with the gear that opened it.
        let expert = self.backend.read(cx).core_settings_expert();
        // The roster already holds the name; a lookup per repaint is a scan of a few dozen rows
        // and a shared-string clone.
        let core_name = self
            .seeded()
            .and_then(|core| self.roster.name(core).cloned());
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
    ///
    /// Takes `window` because the strip is rendered through a lifted palette rather than the
    /// active one: MoonUI keys an inactive tab label off `text_muted`, which sits under the body
    /// contrast floor in both stock themes, and `render_with_theme` is the only way to hand it a
    /// different palette. `AnyElement` for the same reason the chart strip boxes its own — the
    /// returned element would otherwise hold the `&mut cx` borrow the caller still needs.
    ///
    /// Args:
    ///     window: Window that owns the strip's persistent overflow state.
    ///     cx: View context used to read the selected tab and render the themed strip.
    ///
    /// Returns:
    ///     The expert-tab strip in its fixed-height wrapper.
    fn tab_strip(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity();
        let selected = self.tab;
        // Badged with how many parameters on that page still need attention: where to look
        // before anything is typed.
        let attention = self.mixed.differing();
        let items: Vec<MoonTabItem> = ExpertTab::ALL
            .iter()
            .map(|tab| {
                let n = attention
                    .iter()
                    .filter(|&&field| {
                        ExpertTab::for_area(moon_core::feed::CORE_FIELDS[field].area) == Some(*tab)
                    })
                    .count();
                let item = MoonTabItem::new(tab.title()).selected(*tab == selected);
                if n > 0 {
                    item.badge(n.to_string())
                } else {
                    item
                }
            })
            .collect();
        let strip_h = design::tab_strip_h(cx);
        let p = MoonPalette::active(cx);
        let strip = MoonTabStrip::new("core-expert-tabs")
            .gap(4.0)
            .overflow_menu(true)
            .items(items)
            .on_click(move |ix, _event, _window, app| {
                let Some(next) = ExpertTab::at(ix) else {
                    return;
                };
                view.update(app, |this, cx| this.set_tab(next, cx));
            });
        let strip = design::chrome_tab_strip(strip, p, window, cx);
        div()
            .w_full()
            .flex_none()
            .h(strip_h)
            .child(strip)
            .into_any_element()
    }

    /// The Hotkeys page's own sub-tab strip, lifted out of [`Self::body`]'s builder chain.
    ///
    /// It lives in its own method for the same reason [`Self::tab_strip`] takes `window`: the
    /// strip is rendered through a lifted palette, which needs `window` and `&mut cx`, and the
    /// `.children(...)` closure it used to sit inside can capture neither.
    ///
    /// Args:
    ///     window: Window that owns the strip's persistent overflow state.
    ///     cx: View context used to read the selected sub-tab and render the themed strip.
    ///
    /// Returns:
    ///     The Hotkeys sub-tab strip in its fixed-height wrapper.
    fn hotkeys_sub_strip(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity();
        let selected = self.hotkeys_sub;
        let items: Vec<MoonTabItem> = pages::HotkeysSub::ALL
            .iter()
            .map(|sub| MoonTabItem::new(sub.title()).selected(*sub == selected))
            .collect();
        let strip_h = design::fit_h_px(cx, 26.0, 13.0, 7.5);
        let p = MoonPalette::active(cx);
        let strip = MoonTabStrip::new("core-expert-hotkeys-tabs")
            .gap(4.0)
            .overflow_menu(true)
            .items(items)
            .on_click(move |ix, _event, _window, app| {
                let Some(next) = pages::HotkeysSub::at(ix) else {
                    return;
                };
                view.update(app, |this, cx| this.set_hotkeys_sub(next, cx));
            });
        let strip = design::chrome_tab_strip(strip, p, window, cx);
        div()
            .w_full()
            .flex_none()
            .h(strip_h)
            .child(strip)
            .into_any_element()
    }

    /// Body of the selected page.
    ///
    /// Three things can be here, in this order of precedence. While the window is not
    /// [`PageState::Ready`] it says which hazard it is in — this window answers those by explaining
    /// rather than by closing. With a page staged, a PORTED tab draws its rows. A tab that is not
    /// ported yet says so, and says separately when the reason is that nothing can ever arrive for
    /// it.
    ///
    /// Args:
    ///     p: Active palette used by the page body and its notices.
    ///     window: Window forwarded to the Hotkeys sub-tab strip when that page is selected.
    ///     cx: View context used to read state and build the selected page.
    ///
    /// Returns:
    ///     The scrollable body for the selected expert page.
    fn body(
        &self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let view = cx.entity();
        // Read here, from the window's own `&self`: a page is built inside this render, where
        // reading the view back would panic.
        let profit = self
            .seeded()
            .and_then(|core| self.backend.read(cx).session.store().core(core))
            .and_then(|d| d.profit_state.as_ref())
            .map_or((None, None), |s| {
                (
                    Some((s.total_profit, s.total_trades)),
                    Some((s.hourly_profit, s.hourly_trades)),
                )
            });
        let rejected = self.seeded().and_then(|core| {
            let b = self.backend.read(cx);
            let row = b.session.store().core(core)?.core_config_edit.as_ref()?;
            // A named refusal first; a core that never answered at all — the queue gave up with
            // nothing to compare — is the other way a write is lost, and says so in its own words.
            crate::controls::core_config_rejection_caption(row.mismatches.as_ref()).or_else(|| {
                (row.phase == moon_core::feed::CoreConfigEditPhase::GaveUp)
                    .then(|| SharedString::from(t!("core_expert.write_gave_up").to_string()))
            })
        });
        let ctx = pages::PageCtx {
            backend: &self.backend,
            seeded: self.seeded(),
            profit,
            hotkeys_sub: self.hotkeys_sub,
            special_section: self.special_section,
            selected_channel: self.selected_channel,
        };
        let page = self
            .draft
            .as_ref()
            .filter(|_| self.state.can_send())
            .and_then(|draft| pages::page(self.tab, &view, &self.editors, draft, &ctx, p, cx));
        // A page whose rows are all dead still says WHY above itself: the note explains the page a
        // trader is looking at, rather than standing in for one that is missing. Only Login
        // qualifies now — nothing will ever arrive for it.
        let source_note_over_page = page.is_some() && self.tab.source() != TabSource::Projected;
        // Only when there is no page to draw: a note about a page the trader is already looking at
        // would be describing what is on screen beside it.
        let note = (page.is_none() || source_note_over_page).then(|| match self.state {
            PageState::NoCore => t!("core_expert.no_core"),
            PageState::Waiting => t!("core_expert.waiting"),
            PageState::Replaced => t!("core_expert.replaced"),
            PageState::Stale => t!("core_expert.stale"),
            PageState::Ready => match self.tab.source() {
                // Reached only if a page rated `Projected` builds no body at all, which no page
                // does today: the arm is the fallback that keeps such a page explained rather than
                // blank, not a state the strip can currently be in.
                TabSource::Projected => t!("core_expert.page_todo"),
                TabSource::Absent => t!("core_expert.page_absent"),
            },
        });
        // A warning, through the shared component: a page whose values cannot arrive at all is not
        // the same news as one merely awaiting its port.
        let warn = self.state.can_send() && self.tab.source() != TabSource::Projected;
        // Moonbot's Hotkeys page carries a strip of its own, above its body. Built HERE rather
        // than inside the `.children(...)` closure below: it renders through a lifted palette, so
        // it needs `window` and `&mut cx`, and a closure cannot capture either.
        let hotkeys_strip = (self.tab == ExpertTab::Hotkeys && page.is_some())
            .then(|| self.hotkeys_sub_strip(window, cx));
        v_flex()
            .id(self.tab.element_id())
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_y_scroll()
            .gap(design::ui_px(cx, 8.0))
            .px(design::ui_px(cx, design::HEADER_PAD_X))
            .py(design::ui_px(cx, 10.0))
            .children(self.write_refused.map(|(sent, total)| {
                MoonAlert::error(
                    "core-expert-refused",
                    t!(
                        "core_expert.write_refused",
                        sent = sent.to_string(),
                        total = total.to_string()
                    )
                    .to_string(),
                )
            }))
            // The anchor's last write, as the CORE answered it: a refused or clamped area is news
            // the send itself cannot give, and after Apply the page stays open over the values
            // the core may not hold. The same caption the toolbar draws for the same notice.
            .children(
                rejected
                    .map(|caption| MoonAlert::warning("core-expert-rejected", caption.to_string())),
            )
            .children(note.map(|note| {
                if warn {
                    MoonAlert::warning("core-expert-page-note", note.to_string()).into_any_element()
                } else {
                    widgets::text_block(note.to_string(), p.text_muted, false, cx)
                        .into_any_element()
                }
            }))
            .children(hotkeys_strip)
            .children(page)
    }

    /// The send plan, Apply, OK and Cancel: Apply sends and stays, OK sends and closes, Cancel
    /// discards.
    ///
    /// The plan says, before the click, what OK would do — how many parameters, to how many cores,
    /// and how many selected cores it would skip for want of a page. OK is dark unless the anchor's
    /// page is actually sendable, so pressing it can never read as a save of values that reached
    /// nothing.
    ///
    /// Args:
    ///     cores: Selected cores with a live page — what OK writes to.
    ///     skipped: Selected cores without one — what OK skips.
    fn footer(
        &self,
        cores: usize,
        skipped: usize,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let view = cx.entity();
        let cancel_view = view.clone();
        let apply_view = view.clone();
        let can_send = self.state.can_send();
        let params = self.changes.len();
        let has_changes = params > 0;
        let plan_text = if has_changes {
            t!(
                "core_expert.plan",
                params = params.to_string(),
                cores = cores.to_string()
            )
            .to_string()
        } else {
            t!("core_expert.plan_nothing").to_string()
        };
        let skipped = (skipped > 0)
            .then(|| t!("core_expert.plan_skipped", n = skipped.to_string()).to_string());
        h_flex()
            .w_full()
            .flex_none()
            .items_center()
            .justify_between()
            .gap(design::ui_px(cx, 8.0))
            .px(design::ui_px(cx, design::HEADER_PAD_X))
            .py(design::ui_px(cx, 8.0))
            .border_t(px(1.0))
            .border_color(moon_alpha(p.border, 1.0))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .child(widgets::text_line(
                        plan_text,
                        if has_changes { p.text } else { p.text_muted },
                        has_changes,
                        cx,
                    ))
                    .children(skipped.map(|text| widgets::text_line(text, p.amber, false, cx))),
            )
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
                MoonButton::new("core-expert-apply")
                    .label(t!("common.apply").to_string())
                    .size(MoonButtonSize::Action)
                    .variant(MoonButtonVariant::Soft)
                    // Dark while there is nothing to apply: unlike OK, it has no "close" to offer
                    // instead.
                    .disabled(!can_send || !has_changes)
                    .padding_x(14.0)
                    .on_click(move |_, _window, app| {
                        apply_view.update(app, |this, cx| this.apply(cx));
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
