//! Native window titles that follow the interface language.
//!
//! A title reaches the platform once, in the `WindowOptions` a window opens with, while the
//! interface language can change later with the window still open. The header each window draws
//! itself goes through `t!` on every render and follows on its own; the native title — what the
//! macOS Dock and Window menus, the Windows Task Manager and a screen reader show — would stay in
//! the language the window was opened in. A window whose title is localized registers here how to
//! build it, and a locale switch writes each one again: once per switch, nothing per frame.

use std::rc::Rc;

use gpui::{AnyWindowHandle, App, Global};

/// How one window's title is built; run again on every locale switch.
type TitleFn = Rc<dyn Fn(&App) -> String>;

/// One open window with a localized title.
#[derive(Clone)]
struct Entry {
    window: AnyWindowHandle,
    /// Opened owned by another window (`owned_window_options` with an owner).
    owned: bool,
    title: TitleFn,
}

/// The open windows with a localized title.
#[derive(Default)]
struct Titles(Vec<Entry>);

impl Global for Titles {}

/// Remember how `window`'s title is built, so a later locale switch writes it again.
///
/// Entries of windows that have closed since are dropped here, so opening and closing a tool
/// window all session long cannot grow the list between two switches. `App::windows` keeps a
/// window that is mid-update, so registering from another window's handler drops nothing alive.
///
/// Args:
///     window: The window just opened.
///     owned: Whether it was opened with an owner window; see [`follow_locale`] for why it matters.
///     title: Builds the title in the current locale; the same text the window opened with.
///     cx: Application context holding the registry.
pub(crate) fn track(
    window: AnyWindowHandle,
    owned: bool,
    title: impl Fn(&App) -> String + 'static,
    cx: &mut App,
) {
    let live = cx.windows();
    let titles = cx.default_global::<Titles>();
    titles.0.retain(|entry| live.contains(&entry.window));
    titles.0.push(Entry {
        window,
        owned,
        title: Rc::new(title),
    });
}

/// Replace the localized title builder after a singleton window changes its subject.
pub(crate) fn retitle(
    window: AnyWindowHandle,
    title: impl Fn(&App) -> String + 'static,
    cx: &mut App,
) {
    if let Some(entry) = cx
        .default_global::<Titles>()
        .0
        .iter_mut()
        .find(|entry| entry.window == window)
    {
        entry.title = Rc::new(title);
    }
}

/// Write every tracked window's native title again, in the locale that is current now.
///
/// Deferred to the end of the effect cycle: a switch is made from inside a window's own handler —
/// Save in Settings — and a window that is mid-update cannot be updated again, so writing at once
/// would skip exactly the window the switch was made from.
///
/// On macOS an owned window is left alone. The fork keeps it out of the app's window lists
/// (`setExcludedFromWindowsMenu`, set once at creation), and its `set_title` also calls
/// `changeWindowsItem`, which AppKit documents as ADDING a window missing from the Window menu — so
/// a rename could put it back in a list it was kept out of, while its title shows in none of them.
///
/// Args:
///     cx: Application context; call it right after `rust_i18n::set_locale`.
pub(crate) fn follow_locale(cx: &mut App) {
    cx.defer(|cx| {
        if !cx.has_global::<Titles>() {
            return;
        }
        let live = cx.windows();
        let titles = cx.global_mut::<Titles>();
        titles.0.retain(|entry| live.contains(&entry.window));
        let entries = titles.0.clone();
        for entry in entries {
            if cfg!(target_os = "macos") && entry.owned {
                continue;
            }
            let text = (entry.title)(cx);
            // Every entry is live after the prune above, and no window is mid-update at the end
            // of the cycle, so a failure here has nothing to report.
            let _ = entry
                .window
                .update(cx, |_, window, _| window.set_window_title(&text));
        }
    });
}

#[cfg(test)]
mod tests;
