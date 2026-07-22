//! Copies and pastes portable per-tab settings between users. The text uses the corresponding
//! file format: Interface = `theme.toml`, Lines = `orders.toml`, Badges = `badges.json`, and
//! Hotkeys = `hotkeys.toml`. Copied text can be saved as a file, and received file contents can be
//! pasted back. Paste updates the live-preview draft; Save persists it. `moon-core` owns format
//! serialization and validation through each file structure's `to_share_string`/`parse_share`.

use super::{SettingsView, Tab, badges, interface, lines};
use gpui::*;
use moon_core::config::{AppConfig, BadgesConfig, ChartThemeSet, HotkeysConfig, OrdersStyleSet};

impl SettingsView {
    /// Returns whether the active tab has a portable file and should show Copy/Paste actions.
    pub(super) fn shareable(&self) -> bool {
        matches!(
            self.active,
            Tab::Interface | Tab::Lines | Tab::Badges | Tab::Hotkeys
        )
    }

    /// Serializes the active tab's draft slice in its file format and copies it to the clipboard.
    pub(super) fn copy_tab(&mut self, cx: &mut Context<Self>) {
        let text = {
            let b = self.backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            match self.active {
                Tab::Interface => d.theme.to_share_string(),
                Tab::Lines => d.orders.to_share_string(),
                Tab::Hotkeys => d.hotkeys.to_share_string(),
                Tab::Badges => d.badges.to_share_string(),
                _ => None,
            }
        };
        let Some(text) = text else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.status = Some((super::StatusMsg::Key("settings.copied"), false));
        cx.notify();
    }

    /// Parses clipboard text as the active tab's file and applies it to the live-preview draft.
    ///
    /// Editors that cache draft values during `build` are rebuilt after a successful paste.
    /// Empty, foreign, or malformed text sets an error status without changing the draft.
    pub(super) fn paste_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .filter(|t| !t.trim().is_empty())
        else {
            self.status = Some((super::StatusMsg::Key("settings.paste_empty"), true));
            cx.notify();
            return;
        };

        let applied = match self.active {
            Tab::Interface => {
                let cur = self.draft_snapshot(cx, |d| d.theme.clone());
                ChartThemeSet::parse_share(&text, &cur)
                    .map(|set| self.apply_draft(cx, move |d| d.theme = set))
                    .is_some()
            }
            Tab::Lines => {
                let cur = self.draft_snapshot(cx, |d| d.orders.clone());
                OrdersStyleSet::parse_share(&text, &cur)
                    .map(|set| self.apply_draft(cx, move |d| d.orders = set))
                    .is_some()
            }
            Tab::Hotkeys => HotkeysConfig::parse_share(&text)
                .map(|h| self.apply_draft(cx, move |d| d.hotkeys = h))
                .is_some(),
            Tab::Badges => BadgesConfig::parse_share(&text)
                .map(|b| self.apply_draft(cx, move |d| d.badges = b))
                .is_some(),
            _ => false,
        };

        if applied {
            // Rebuild editor states initialized from the draft during `build`, matching the
            // add/remove pattern used by badge and server editors.
            match self.active {
                Tab::Interface => self.iface = interface::build(&self.backend, window, cx),
                Tab::Lines => self.lines = lines::build(&self.backend, window, cx),
                Tab::Badges => self.badges = badges::build(&self.backend, window, cx),
                _ => {}
            }
            self.status = Some((super::StatusMsg::Key("settings.pasted"), false));
        } else {
            self.status = Some((super::StatusMsg::Key("settings.paste_wrong"), true));
        }
        cx.notify();
    }

    /// Returns a snapshot of the selected draft slice for parsers that need the current set.
    fn draft_snapshot<T>(&self, cx: &Context<Self>, get: impl Fn(&AppConfig) -> T) -> T {
        let b = self.backend.read(cx);
        get(b.preview.as_ref().unwrap_or(&b.config))
    }

    /// Updates the draft and notifies the backend for the same live preview as ordinary tab edits.
    fn apply_draft(&self, cx: &mut Context<Self>, apply: impl FnOnce(&mut AppConfig)) {
        self.backend.update(cx, |b, bcx| {
            if let Some(p) = b.preview.as_mut() {
                apply(p);
                bcx.notify();
            }
        });
    }
}
