//! One backend-owned command session, presented in one tool window per configured core.

mod chrome;
mod log_lines;
mod pages;
mod rail;
#[cfg(test)]
mod tests;
mod view;

use std::collections::HashMap;

use gpui::*;
use moon_remote::core_cmds::Cmd;
use moon_ui::{MoonBackgroundPolicy, Root};
use rust_i18n::t;

use crate::Backend;
use view::CoreCmdsView;

/// Initial command-window geometry in design pixels.
pub(crate) const DEFAULT_SIZE: (f32, f32) = (760.0, 520.0);
/// Smallest supported command-window geometry in design pixels.
pub(crate) const MIN_SIZE: (f32, f32) = (520.0, 400.0);

/// UI-thread registry; the backend retains results independently of these windows.
#[derive(Default)]
struct Windows(HashMap<u64, (AnyWindowHandle, WeakEntity<CoreCmdsView>)>);
impl Global for Windows {}

/// Open or activate the core's singleton, selecting the requested command before activation.
pub(crate) fn open_on(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    uid: u64,
    cmd: Cmd,
    start: bool,
    cx: &mut App,
) {
    if !cx.has_global::<Windows>() {
        cx.set_global(Windows::default());
    }
    if let Some((handle, view)) = cx.global::<Windows>().0.get(&uid).cloned()
        && handle
            .update(cx, |_, window, cx| {
                let _ = view.update(cx, |view, cx| view.select(cmd, start, window, cx));
                window.activate_window();
            })
            .is_ok()
    {
        return;
    }
    let zoom = moon_ui::MoonTheme::content_zoom(cx);
    let display = owner_display.or_else(|| crate::window::windowing::owner_display_id(owner, cx));
    let bounds = Bounds::centered(
        display,
        size(px(DEFAULT_SIZE.0 * zoom), px(DEFAULT_SIZE.1 * zoom)),
        cx,
    );
    let title_backend = backend.clone();
    let title = move |cx: &App| {
        let core = view::core_name(title_backend.read(cx), uid);
        t!("core_cmds.window.title", core = core).to_string()
    };
    // The title tracker needs its own backend reference after the opening closure takes one.
    let backend = backend.clone();
    let mut opts = crate::window::windowing::tool_window_options(
        title(cx),
        WindowBounds::Windowed(bounds),
        Some(size(px(MIN_SIZE.0 * zoom), px(MIN_SIZE.1 * zoom))),
        owner,
    );
    opts.display_id = display;
    let created = cx.open_window(opts, move |window, cx| {
        crate::window::windowing::configure_shell_clear_color(window, cx);
        let view = cx.new(|cx| CoreCmdsView::new(backend.clone(), uid, cmd, window, cx));
        let weak = view.downgrade();
        view.update(cx, |view, cx| {
            view.select(cmd, start, window, cx);
            cx.on_release(move |view, cx| {
                cx.global_mut::<Windows>().0.remove(&uid);
                view.forget(cx);
            })
            .detach();
        });
        cx.global_mut::<Windows>()
            .0
            .insert(uid, (window.window_handle(), weak));
        cx.new(|cx| {
            crate::window::windowing::frame(
                Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque),
            )
        })
    });
    match created {
        Ok(handle) => {
            crate::window::titles::track(handle.into(), owner.is_some(), title, cx);
            crate::window::windowing::activate_new_window(handle.into(), cx);
        }
        Err(error) => log::warn!("core commands window could not be opened: {error:#}"),
    }
}
