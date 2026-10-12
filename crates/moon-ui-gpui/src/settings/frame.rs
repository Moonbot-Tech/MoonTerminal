//! Page chrome of the Settings window: the header every page opens with (title, save-mode chip,
//! one-line description, page actions) and the save bar under the page body.
//!
//! Page files build only their body and actions and hand them over as [`PageParts`]; `render.rs`
//! is the one place that composes the navigation, the header, the body and the bar.

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonButton, MoonButtonVariant, MoonPalette, MoonTag, h_flex, rgba_from, v_flex};
use rust_i18n::t;

use super::{SaveMode, SettingsView, Tab};
use crate::design;

/// Click handler of a save-bar or header button.
pub(super) type OnClick = Rc<dyn Fn(&mut Window, &mut App)>;

/// What the header of one page shows.
pub(super) struct PageHeader {
    pub(super) tab: Tab,
    pub(super) mode: SaveMode,
    /// Replaces the page's stock description when the page knows something more specific.
    pub(super) desc_override: Option<String>,
    /// Buttons drawn at the right of the title.
    pub(super) actions: Vec<AnyElement>,
}

/// A station-applied page's bar: what is pending there and how to send or drop it.
pub(super) struct RemoteBar {
    pub(super) msg: String,
    pub(super) dirty: bool,
    pub(super) apply: OnClick,
    pub(super) revert: Option<OnClick>,
    /// The apply button's caption, already localized; a counted send names its count.
    pub(super) label: String,
}

/// The bar under a page body.
pub(super) enum SaveBar {
    /// Draft page: names the dirty pages, offers revert and Save. `dirty` is the window's one
    /// unsaved-work verdict; it can hold with no page to name.
    Draft { dirty_pages: Vec<Tab>, dirty: bool },
    /// Everything on the page applies at once.
    Live,
    /// The page applies its edits on the station.
    Remote(RemoteBar),
    /// The page draws no bar.
    None,
}

/// Everything one page hands to `render.rs`.
pub(super) struct PageParts {
    pub(super) header: PageHeader,
    pub(super) body: AnyElement,
    pub(super) bar: SaveBar,
}

/// The chip's caption for a save mode.
fn mode_label(mode: SaveMode) -> String {
    match mode {
        SaveMode::Draft => t!("settings.mode.draft"),
        SaveMode::Live => t!("settings.mode.live"),
        SaveMode::Remote => t!("settings.mode.remote"),
    }
    .to_string()
}

/// Draw a page header.
///
/// Args:
///     h: Title source, chip, description and actions.
///     narrow: Whether the window is below the navigation breakpoint; the actions then wrap under
///         the title instead of sharing its row.
///     cx: Application context for theme and scale.
///
/// Returns:
///     The header row.
pub(super) fn page_header(h: PageHeader, narrow: bool, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    let chip = match h.mode {
        SaveMode::Draft => MoonTag::info(),
        SaveMode::Live => MoonTag::positive(),
        SaveMode::Remote => MoonTag::warning(),
    }
    .mono(false)
    // A child, not `label()`: the mockup's chip is sentence case, `label()` upper-cases.
    .child(mode_label(h.mode));
    let desc = h.desc_override.unwrap_or_else(|| h.tab.desc());
    let title = v_flex()
        .min_w_0()
        .flex_1()
        .gap(design::ui_px(cx, 4.0))
        .child(
            h_flex()
                .min_w_0()
                .gap(design::ui_px(cx, 10.0))
                .items_center()
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(design::ui_font())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(design::ui_px(cx, 17.0))
                        .child(h.tab.title()),
                )
                .child(div().flex_none().child(chip)),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(rgba_from(p.text_muted, 1.0))
                .child(desc),
        );
    let actions = (!h.actions.is_empty()).then(|| {
        h_flex()
            .flex_none()
            .flex_wrap()
            .gap(design::ui_px(cx, 8.0))
            .children(h.actions)
    });
    let row = if narrow {
        v_flex()
            .gap(design::ui_px(cx, 8.0))
            .child(title)
            .children(actions)
            .into_any_element()
    } else {
        h_flex()
            .gap(design::ui_px(cx, 12.0))
            .items_start()
            .child(title)
            .children(actions)
            .into_any_element()
    };
    div()
        .id(SharedString::from(format!(
            "settings-page-head-{}",
            h.tab.id()
        )))
        .w_full()
        .flex_none()
        .px(design::ui_px(cx, super::render::BODY_PAD))
        .pt(design::ui_px(cx, 14.0))
        .pb(design::ui_px(cx, 10.0))
        .border_b_1()
        .border_color(rgba_from(p.border, 1.0))
        .child(row)
        .into_any_element()
}

/// The dirty bar's sentence naming how many pages hold unsaved edits, and which.
///
/// Args:
///     pages: Dirty pages in navigation order.
///
/// Returns:
///     The localized sentence, in the plural form `pages.len()` takes.
pub(super) fn dirty_text(pages: &[Tab]) -> String {
    let names = pages
        .iter()
        .map(|t| t.title())
        .collect::<Vec<_>>()
        .join(", ");
    let n = pages.len();
    let key = super::telegram::plural_key("settings.savebar.dirty", n as u64);
    t!(&key, n = n, pages = names).to_string()
}

/// Draw the bar under a page body.
///
/// Args:
///     bar: Which bar the page asked for.
///     status: The last Save, paste or import outcome as `(text, is_error)`; it replaces the
///         bar's own sentence while it is shown.
///     on_save: Save handler of a draft bar.
///     on_revert: Revert handler of a draft bar.
///     cx: Application context for theme and scale.
///
/// Returns:
///     The bar, or `None` for [`SaveBar::None`].
pub(super) fn save_bar(
    bar: SaveBar,
    status: Option<(String, bool)>,
    on_save: OnClick,
    on_revert: OnClick,
    cx: &App,
) -> Option<AnyElement> {
    let p = MoonPalette::active(cx);
    let (msg, dirty, buttons): (String, bool, Vec<AnyElement>) = match bar {
        SaveBar::None => return None,
        SaveBar::Live => (t!("settings.savebar.live").to_string(), false, Vec::new()),
        SaveBar::Draft {
            dirty_pages,
            dirty: true,
        } => (
            if dirty_pages.is_empty() {
                t!("settings.savebar.dirty_other").to_string()
            } else {
                dirty_text(&dirty_pages)
            },
            true,
            vec![
                MoonButton::new("settings-revert")
                    .ghost()
                    .label(t!("settings.savebar.revert").to_string())
                    .on_click(move |_, window, cx| on_revert(window, cx))
                    .render()
                    .into_any_element(),
                MoonButton::new("save")
                    .variant(MoonButtonVariant::Blue)
                    .label(t!("settings.savebar.save").to_string())
                    .on_click(move |_, window, cx| on_save(window, cx))
                    .render()
                    .into_any_element(),
            ],
        ),
        SaveBar::Draft { .. } => (
            t!("settings.savebar.clean").to_string(),
            false,
            vec![
                // Disabled while nothing differs from disk: the approved design shows a clean
                // window by a Save that cannot be pressed, beside a line saying so.
                MoonButton::new("save")
                    .variant(MoonButtonVariant::Neutral)
                    .disabled(true)
                    .label(t!("settings.savebar.save").to_string())
                    .render()
                    .into_any_element(),
            ],
        ),
        SaveBar::Remote(remote) => {
            let mut buttons = Vec::new();
            if let Some(revert) = remote.revert {
                buttons.push(
                    MoonButton::new("settings-remote-revert")
                        .ghost()
                        .label(t!("settings.savebar.revert").to_string())
                        .on_click(move |_, window, cx| revert(window, cx))
                        .render()
                        .into_any_element(),
                );
            }
            let apply = remote.apply;
            buttons.push(
                MoonButton::new("settings-remote-apply")
                    .variant(MoonButtonVariant::Blue)
                    .disabled(!remote.dirty)
                    .label(remote.label.clone())
                    .on_click(move |_, window, cx| apply(window, cx))
                    .render()
                    .into_any_element(),
            );
            (remote.msg, remote.dirty, buttons)
        }
    };
    let (text, color) = match status {
        Some((text, true)) => (text, p.red),
        Some((text, false)) => (text, p.green),
        None => (msg, if dirty { p.text } else { p.text_muted }),
    };
    Some(
        h_flex()
            .id("settings-save-bar")
            .w_full()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap(design::ui_px(cx, 10.0))
            .px(design::ui_px(cx, super::render::BODY_PAD))
            .py(design::ui_px(cx, 8.0))
            .border_t_1()
            .when(dirty, |el| {
                el.bg(rgba_from(p.blue, 0.12))
                    .border_color(rgba_from(p.blue, 1.0))
            })
            .when(!dirty, |el| {
                el.bg(rgba_from(p.shell_high, 1.0))
                    .border_color(rgba_from(p.border, 1.0))
            })
            .child(
                div()
                    .debug_selector(|| "settings-save-bar-text".into())
                    .flex_1()
                    .min_w(design::ui_px(cx, 200.0))
                    .text_color(rgba_from(color, 1.0))
                    .child(text),
            )
            .children(buttons)
            .into_any_element(),
    )
}

/// Whether a window holds unsaved work of any kind.
///
/// Args:
///     draft_dirty: The draft differs from the saved config, in any field.
///     pages_dirty: Some page names edits (config fields or a password change).
///     station_pending: The station's draft was edited and not applied.
///
/// Returns:
///     `true` when Save has work or closing must ask.
pub(super) fn has_unsaved(draft_dirty: bool, pages_dirty: bool, station_pending: bool) -> bool {
    draft_dirty || pages_dirty || station_pending
}

/// Whether the draft save bar is dirty: only what Save writes. A station draft is applied by its
/// own pages' buttons, so `station_pending` never makes the bar dirty (it still makes closing ask,
/// see [`has_unsaved`]).
///
/// Args:
///     draft_dirty: The draft differs from the saved config, in any field.
///     pages_dirty: Some page names edits (config fields or a password change).
///     station_pending: The station's draft was edited and not applied.
///
/// Returns:
///     `true` when Save has work.
pub(super) fn bar_dirty(draft_dirty: bool, pages_dirty: bool, station_pending: bool) -> bool {
    let _ = station_pending;
    draft_dirty || pages_dirty
}

impl SettingsView {
    /// Assemble a page from its body and header actions, with the save mode and bar its tab
    /// takes.
    ///
    /// Args:
    ///     tab: The page.
    ///     body: The page body.
    ///     actions: Header buttons.
    ///     cx: Settings context used to read the station and security state.
    ///
    /// Returns:
    ///     The parts `render.rs` composes.
    pub(super) fn page_parts(
        &self,
        tab: Tab,
        body: AnyElement,
        actions: Vec<AnyElement>,
        cx: &App,
    ) -> PageParts {
        let mode = tab.save_mode(self.station_known());
        let bar = match mode {
            SaveMode::Draft => SaveBar::Draft {
                dirty_pages: self.dirty_page_set(cx).iter().collect(),
                dirty: self.config_unsaved(cx),
            },
            SaveMode::Live => SaveBar::Live,
            // Station pages wire their own apply bar in phase 4.
            SaveMode::Remote => SaveBar::None,
        };
        PageParts {
            header: PageHeader {
                tab,
                mode,
                desc_override: None,
                actions,
            },
            body,
            bar,
        }
    }

    /// Pages with unsaved edits: the config memo plus Security while a password change is
    /// typed (Save applies it before writing the config) or accepted but not yet written.
    pub(super) fn dirty_page_set(&self, cx: &App) -> super::TabSet {
        let mut pages = self.dirty_pages;
        if self.security.has_unsaved(cx) {
            pages.insert(Tab::Security);
        }
        pages
    }

    /// What Save writes: a draft that differs from disk, a page with edits, or a password change.
    /// The save bar and Save read this one; a station draft is applied by its own pages' buttons,
    /// so it never makes the bar dirty.
    pub(super) fn config_unsaved(&self, cx: &App) -> bool {
        bar_dirty(
            self.draft_dirty,
            self.dirty_page_set(cx).len() > 0,
            self.station_draft_pending(),
        )
    }

    /// Every kind of unsaved work, read by the close request and the navigation dots: what Save
    /// writes plus a station draft not yet applied.
    pub(super) fn has_unsaved(&self, cx: &App) -> bool {
        has_unsaved(
            self.draft_dirty,
            self.dirty_page_set(cx).len() > 0,
            self.station_draft_pending(),
        )
    }

    /// Pages the navigation marks: the dirty pages plus the station's bot pages while its draft
    /// waits to be applied.
    pub(super) fn nav_dirty_pages(&self, cx: &App) -> super::TabSet {
        let mut pages = self.dirty_page_set(cx);
        if self.station_draft_pending() {
            pages.insert(Tab::TgBot);
            pages.insert(Tab::TgMessages);
        }
        pages
    }

    /// Copy and Paste header buttons of a page with a portable file.
    pub(super) fn share_actions(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        vec![
            MoonButton::new("share-copy")
                .variant(MoonButtonVariant::Neutral)
                .label(t!("settings.copy").to_string())
                .on_click(cx.listener(|this, _, _, cx| this.copy_tab(cx)))
                .render()
                .into_any_element(),
            MoonButton::new("share-paste")
                .variant(MoonButtonVariant::Neutral)
                .label(t!("settings.paste").to_string())
                .on_click(cx.listener(|this, _, window, cx| this.paste_tab(window, cx)))
                .render()
                .into_any_element(),
        ]
    }
}

#[cfg(test)]
mod tests;
