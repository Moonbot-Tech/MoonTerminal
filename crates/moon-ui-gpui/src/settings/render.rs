//! Renders the Settings window: page navigation (sidebar, or a picker in a narrow window), the
//! active page with its header and save bar (`frame`), and the window header (`settings_header`).

use gpui::prelude::FluentBuilder;
use gpui::*;
use std::rc::Rc;

use moon_ui::{
    MoonPalette, MoonScrollAxis, MoonScrollbarVisibility, MoonWindowFrame, h_flex,
    moon_scrollbar_overlay_with_palette, rgba_from, v_flex,
};
use rust_i18n::t;

use super::frame;
use super::nav;
use super::{SETTINGS_HEADER_H, SettingsView, Tab};
use crate::design;

/// Inset of every page body, header and save bar, in unscaled pixels.
pub(super) const BODY_PAD: f32 = 18.0;

/// A callback that runs `f` on the Settings view, handed to the navigation and the save bar.
fn handler<A: 'static>(
    cx: &Context<SettingsView>,
    f: impl Fn(&mut SettingsView, A, &mut Window, &mut Context<SettingsView>) + 'static,
) -> Rc<dyn Fn(A, &mut Window, &mut App)> {
    let view = cx.entity();
    Rc::new(move |arg, window, app| view.update(app, |this, cx| f(this, arg, window, cx)))
}

/// [`handler`] for a plain click.
fn click(
    cx: &Context<SettingsView>,
    f: impl Fn(&mut SettingsView, &mut Window, &mut Context<SettingsView>) + 'static,
) -> frame::OnClick {
    let run = handler(cx, move |this, (), window, cx| f(this, window, cx));
    Rc::new(move |window, app| run((), window, app))
}

impl Render for SettingsView {
    /// Render the active Settings tab, its bounded or scrolling body, and the shared footer.
    ///
    /// Args:
    ///     window: Settings window providing viewport dimensions.
    ///     cx: Settings context used to render the active tab and install callbacks.
    ///
    /// Returns:
    ///     The complete Settings window element tree.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::bump(&crate::diag::SETTINGS_RENDER);
        // A finished station job clears the secrets typed for it: its inputs need the window.
        self.server_bot_sync(window, cx);
        // "Save and close" waiting on the station closes or stays on its answer.
        self.close_after_station_sync(window, cx);
        // The bot menu trees take a new button order before they render.
        self.bot_menu_sync(cx);
        // The picked chats' notification editors take their stored rows.
        self.chat_notify_sync(window, cx);
        // The station tape's long-position field follows the station's reads.
        self.server_tape_input_sync(window, cx);
        let p = MoonPalette::active(cx);
        let chrome_width = f32::from(window.viewport_size().width);

        // ── Active tab body ─────────────────────────────────────────────────
        // The body column is narrower than the window by the sidebar; pages that size their own
        // column from the width get the width they actually have.
        let narrow = nav::nav_hidden(chrome_width, cx);
        let body_width = if narrow {
            chrome_width
        } else {
            (chrome_width - nav::nav_w(cx)).max(0.0)
        };
        let parts = match self.active {
            Tab::Cores => self.cores_page(narrow, body_width, cx),
            Tab::TgBot => self.tg_bot_page(body_width, cx),
            Tab::TgCore => self.tg_core_page(body_width, cx),
            Tab::StOverview => self.st_overview_page(body_width, cx),
            Tab::Appearance => self.appearance_page(cx),
            Tab::Lines => self.lines_page(cx),
            Tab::Badges => self.badges_page(body_width, cx),
            Tab::Hotkeys => self.hotkeys_page(window, cx),
            Tab::General => self.general_page(narrow, cx),
            Tab::Security => self.security_page(cx),
            Tab::Storage => self.storage_page(cx),
            Tab::Sounds => self.sounds_page(body_width, cx),
            Tab::TgNotify => self.tg_notify_page(body_width, cx),
            Tab::TgMessages => self.tg_messages_page(body_width, cx),
            Tab::StCores => self.st_cores_page(body_width, cx),
            Tab::StTape => self.st_tape_page(body_width, cx),
            Tab::StMaint => self.st_maint_page(body_width, cx),
        };
        let frame::PageParts {
            header,
            body: content,
            bar,
        } = parts;
        // Keep the viewport bounded but measure its child at intrinsic height. The convenience
        // scrollbar wrapper forces its child to flex_1, so expanded descendants can overflow
        // that child's bounds without increasing the scroll extent.
        //
        // Connections is the exception and gets a BOUNDED, non-scrolling parent of its own: it
        // owns a virtualized core list, and nesting it inside this scrollbar would give it an
        // effectively infinite height to lay out against, since `MoonVirtualList` fills whatever
        // its parent hands it. It takes `flex_1 + min_h_0` down to its list, matching every other
        // `MoonVirtualList` call site in this app. Every other tab scrolls here.
        let bounded = matches!(self.active, Tab::Cores);
        let body_inner = if bounded {
            div()
                .id("settings-body")
                .size_full()
                .bg(rgba_from(p.shell, 1.0))
                .flex()
                .flex_col()
                .min_h(px(0.0))
                .p(design::ui_px(cx, BODY_PAD))
                .gap(design::ui_px(cx, 10.0))
                .overflow_hidden()
                .child(content)
                .into_any_element()
        } else {
            let scroll = window
                .use_keyed_state("settings-body-scroll", cx, |_, _| ScrollHandle::new())
                .read(cx)
                .clone();
            div()
                .relative()
                .size_full()
                .bg(rgba_from(p.shell, 1.0))
                .child(scrollable_tab_content(content, &scroll, cx))
                .children(moon_scrollbar_overlay_with_palette(
                    "settings-body-scrollbar",
                    &scroll,
                    MoonScrollAxis::Vertical,
                    MoonScrollbarVisibility::Always,
                    p,
                    window,
                    cx,
                ))
                .into_any_element()
        };
        // ── Save bar ────────────────────────────────────────────────────────
        // Resolve status keys against the current locale here so a language change cannot leave
        // text from the previous locale behind. A SUCCESS status is dropped once the draft is
        // dirty again: "Сохранено" over a bar naming unsaved pages is a contradiction, and the
        // stale half is the one that has to go. An ERROR status always stays.
        let dirty_pages = self.nav_dirty_pages(cx);
        let dirty = self.config_unsaved(cx);
        let status = match &self.status {
            Some((msg, err)) if *err || !dirty => Some((
                match msg {
                    super::StatusMsg::Key(k) => t!(*k).to_string(),
                    super::StatusMsg::Text(s) => s.clone(),
                },
                *err,
            )),
            _ => None,
        };
        let on_save = click(cx, |this, window, cx| {
            this.save(window, cx);
        });
        let on_revert = click(cx, |this, window, cx| this.revert_draft(window, cx));
        let save_bar = frame::save_bar(bar, status, on_save, on_revert, cx);

        let body = v_flex()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .h_full()
            .child(frame::page_header(header, narrow, cx))
            .child(div().flex_1().min_h(px(0.0)).w_full().child(body_inner))
            .children(save_bar);

        // ── Navigation ──────────────────────────────────────────────────────
        let main = if narrow {
            let model = nav::nav_model("", dirty_pages, &self.nav_counts);
            self.sync_nav_picker(&model, cx);
            v_flex()
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .child(nav::nav_picker(&self.nav_pick, cx))
                .child(body)
                .into_any_element()
        } else {
            let query = self.nav_query.read(cx).value().to_string();
            let model = nav::nav_model(&query, dirty_pages, &self.nav_counts);
            let on_pick: nav::OnPick = handler(cx, |this, tab, _, cx| this.show_page(tab, cx));
            let on_import = click(cx, |this, _, cx| this.start_moonbot_import(cx));
            h_flex()
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .items_start()
                .child(
                    div()
                        .h_full()
                        .flex_none()
                        .border_r_1()
                        .border_color(rgba_from(p.border, 1.0))
                        .child(nav::nav_sidebar(
                            &model,
                            self.active,
                            &self.nav_query,
                            on_pick,
                            on_import,
                            cx,
                        )),
                )
                .child(body)
                .into_any_element()
        };

        v_flex()
            .size_full()
            .relative()
            .bg(rgba_from(p.shell, 1.0))
            .font_family(design::ui_font())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0))
            .text_color(rgba_from(p.text, 1.0))
            .child(settings_header(p, cx))
            .child(main)
            // Render the MoonBot import preview over the entire window body.
            .children(self.import_overlay(cx))
            .child(
                MoonWindowFrame::tool("settings-window-frame-hit", chrome_width)
                    .controls(moon_ui::MoonWindowFrameControls::MinimizeMaximizeClose)
                    .header_height(SETTINGS_HEADER_H)
                    .leading_inset(design::titlebar_leading_inset())
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            )
    }
}

impl SettingsView {
    /// Show `page`.
    ///
    /// Opening a Telegram page refreshes the picked core, and a station page asks the station for
    /// its state.
    ///
    /// Args:
    ///     page: The page to show.
    ///     cx: Settings context notified after the page changes.
    pub(super) fn show_page(&mut self, page: Tab, cx: &mut Context<Self>) {
        self.active = page;
        if matches!(page, Tab::TgBot | Tab::TgCore) {
            self.telegram_tab_activated(cx);
        }
        if matches!(
            page,
            Tab::StOverview | Tab::StCores | Tab::StTape | Tab::StMaint
        ) {
            self.backend.update(cx, |b, bcx| b.station_refresh_bot(bcx));
            // A catch-up already running starts its minute re-read now, not at the next answer.
            self.server_tape_sync(cx);
        }
        self.nav_pick
            .update(cx, |state, _| state.set_selected_value(&page));
        cx.notify();
    }

    /// Refill the narrow-window picker when a caption changed since the last frame.
    ///
    /// Refilling notifies the picker, so doing it every frame would repaint forever.
    fn sync_nav_picker(&mut self, model: &nav::NavModel, cx: &mut Context<Self>) {
        let labels: Vec<String> = model.items.iter().map(nav::picker_label).collect();
        if labels == self.nav_pick_labels {
            return;
        }
        self.nav_pick_labels = labels;
        let items = nav::picker_items(model);
        let active = self.active;
        self.nav_pick.update(cx, |state, cx| {
            state.set_items(items, cx);
            state.set_selected_value(&active);
        });
    }
}

/// Build the window header with native minimize, maximize, and close controls.
fn settings_header(p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .id("settings-window-header")
        .relative()
        .flex_none()
        .w_full()
        .h(design::fit_h_px(cx, SETTINGS_HEADER_H, 14.0, 8.0))
        .justify_between()
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .bg(rgba_from(p.shell_high, 1.0))
        .border_b(px(1.0))
        .border_color(rgba_from(p.border, 1.0))
        .child(
            MoonWindowFrame::tool("settings-titlebar-title", 0.0)
                .title_cluster(t!("settings.title").to_string(), cx)
                .h_full()
                .flex_1()
                .min_w_0(),
        )
        .when(design::show_custom_window_controls(), |this| {
            this.child(
                MoonWindowFrame::tool("settings-window-frame-visual", 0.0)
                    .controls(moon_ui::MoonWindowFrameControls::MinimizeMaximizeClose)
                    .header_height(SETTINGS_HEADER_H)
                    .show_controls(true)
                    .visual_controls(cx),
            )
        })
}

/// Measure the whole padded tab at intrinsic height inside a fixed scroll viewport.
///
/// The direct non-flex child makes expanded descendants contribute to the scroll extent. A tab
/// that narrows its column gives it an explicit width, never `w_full().max_w(..)`: the column's
/// height is then measured with its wrapped text laid out at the full viewport width — fewer
/// lines than it renders at the narrower width — and its end falls outside the scroll extent.
pub(super) fn scrollable_tab_content(
    content: impl IntoElement,
    scroll: &ScrollHandle,
    cx: &App,
) -> impl IntoElement {
    let measured_scroll = scroll.clone();
    let previous = (scroll.bounds(), scroll.max_offset(), scroll.offset());
    div()
        .on_children_prepainted(move |_, window, _| {
            // MoonUI builds its overlay from the previous layout. Repaint once when the new
            // measurement changes, including first layout, expansion, resizing, and scrolling.
            let current = (
                measured_scroll.bounds(),
                measured_scroll.max_offset(),
                measured_scroll.offset(),
            );
            if current != previous {
                window.request_animation_frame();
            }
        })
        .id("settings-body")
        .size_full()
        .overflow_y_scroll()
        .track_scroll(scroll)
        .child(
            v_flex()
                .w_full()
                .flex_none()
                .p(design::ui_px(cx, BODY_PAD))
                .gap(design::ui_px(cx, 10.0))
                .child(content),
        )
}

#[cfg(test)]
mod tests;
