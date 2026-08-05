//! Platform-level window support: `WindowOptions` factories for every terminal window type
//! (trading, tool, independent Profit Monitor, detached panel, detached chart, and debug), display
//! selection from saved geometry or the owner window, theme-aware clear colors, Windows DWM frame
//! configuration, and HWND/geometry helpers. It also provides Windows AppUserModelIDs and group
//! icons embedded in the executable by `build.rs` through `embed_group_icons`.

use std::sync::Arc;

use gpui::*;

use crate::design;

pub(crate) const APP_ID: &str = "MoonTerminal";

/// Close every window in `handles`, ignoring the ones already gone.
///
/// `WindowHandle::update` returns `Err` for a window that has been removed, which is the normal
/// outcome for an OS-owned child whose owner closed first — a teardown enumerates what it means to
/// close and does not care which of them the platform got to first.
pub(crate) fn close_all(handles: Vec<WindowHandle<moon_ui::Root>>, cx: &mut App) {
    for h in handles {
        let _ = h.update(cx, |_, window, _| window.remove_window());
    }
}

/// Group icons from `assets/icons/<id>.png`, embedded in the executable by the build script's
/// `embed_group_icons` step. Each index is a `GroupConfig.icon` ID, so icons require no runtime
/// filesystem path in either development or deployed builds.
mod group_icons {
    include!(concat!(env!("OUT_DIR"), "/group_icons.rs"));
}

/// Return the embedded PNG bytes for a group icon ID.
///
/// # Arguments
///
/// * `id` - `GroupConfig.icon` ID used as the embedded icon-table index.
///
/// # Returns
///
/// The static PNG bytes, or `None` when the ID is absent or has no embedded icon.
pub(crate) fn group_icon_png(id: u32) -> Option<&'static [u8]> {
    group_icons::GROUP_ICONS.get(id as usize).copied().flatten()
}

/// Build the platform app ID for a group window.
///
/// On Windows the AppUserModelID is the taskbar grouping key, so each non-empty group receives
/// `MoonTerminal.<group>` and separate groups do not collapse into one taskbar button. The live
/// window icon is set separately with `WM_SETICON`. Outside Windows this always returns the base
/// `MoonTerminal` ID: Wayland uses it to match the `.desktop` entry, X11 receives the icon through
/// `_NET_WM_ICON`, and macOS does not use this app ID.
///
/// # Arguments
///
/// * `group` - Group name to append on Windows; an empty name keeps the base ID.
///
/// # Returns
///
/// The group-specific Windows AppUserModelID or the platform-neutral base ID.
pub(crate) fn group_app_id(group: &str) -> String {
    if cfg!(target_os = "windows") && !group.is_empty() {
        format!("{APP_ID}.{group}")
    } else {
        APP_ID.to_string()
    }
}

/// Decode an embedded group icon for `WindowOptions.icon`.
///
/// GPUI applies this field on X11 through `_NET_WM_ICON`. Windows sets the live taskbar and
/// Alt-Tab icon separately through [`set_group_window_icon`]; macOS uses the `.app` bundle icon,
/// and Wayland resolves the `.desktop` icon instead.
///
/// # Arguments
///
/// * `icon_id` - Embedded group-icon ID to decode.
///
/// # Returns
///
/// The decoded RGBA image, or `None` when the ID is absent or decoding fails.
pub(crate) fn app_icon(icon_id: u32) -> Option<Arc<image::RgbaImage>> {
    let png = group_icon_png(icon_id)?;
    image::load_from_memory(png)
        .ok()
        .map(|img| Arc::new(img.to_rgba8()))
}

#[cfg(target_os = "windows")]
pub(crate) fn window_hwnd(window: &Window) -> Option<isize> {
    use raw_window_handle::RawWindowHandle;
    let Ok(handle) = raw_window_handle::HasWindowHandle::window_handle(window) else {
        return None;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return None;
    };
    Some(handle.hwnd.get() as isize)
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn window_hwnd(_window: &Window) -> Option<isize> {
    None
}

fn app_window_options(
    title: impl Into<SharedString>,
    window_bounds: WindowBounds,
    display_id: Option<DisplayId>,
    min_size: Option<Size<Pixels>>,
    app_id: String,
    icon: Option<Arc<image::RgbaImage>>,
    transparent_titlebar: bool,
) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(window_bounds),
        display_id,
        titlebar: Some(TitlebarOptions {
            title: Some(title.into()),
            appears_transparent: transparent_titlebar,
            ..Default::default()
        }),
        app_id: Some(app_id),
        window_min_size: min_size,
        window_decorations: design::platform_window_decorations(),
        icon,
        ..Default::default()
    }
}

fn rgb_to_rgba(rgb_hex: u32) -> Rgba {
    rgba((rgb_hex << 8) | 0xFF)
}

pub(crate) fn configure_shell_clear_color(window: &Window, cx: &App) {
    window.set_clear_color(Some(rgb_to_rgba(moon_ui::MoonPalette::active(cx).shell)));
}

pub(crate) fn configure_chart_clear_color(window: &Window, cx: &App) {
    window.set_clear_color(Some(rgb_to_rgba(moon_ui::MoonPalette::active(cx).chart_bg)));
}

pub(crate) fn trading_window_options(
    title: impl Into<SharedString>,
    group: &str,
    icon_id: u32,
    window_bounds: WindowBounds,
    display_id: Option<DisplayId>,
    min_size: Option<Size<Pixels>>,
) -> WindowOptions {
    app_window_options(
        title,
        window_bounds,
        display_id,
        min_size,
        group_app_id(group),
        app_icon(icon_id),
        true,
    )
}

pub(crate) fn tool_window_options(
    title: impl Into<SharedString>,
    window_bounds: WindowBounds,
    min_size: Option<Size<Pixels>>,
    owner: Option<AnyWindowHandle>,
) -> WindowOptions {
    owned_window_options(title, window_bounds, None, min_size, owner, true)
}

/// Build options for the independent desktop Profit Monitor.
///
/// Unlike owned tool windows, the monitor remains independently minimizable and restorable after
/// every Main window is minimized. Its explicit taskbar entry is the user's route back to it.
///
/// Args:
///     title: Native window title.
///     window_bounds: Initial or restored geometry.
///     display_id: Display chosen from saved geometry or the launching window.
///     min_size: Smallest responsive monitor size.
///
/// Returns:
///     Independent normal-window options with a visible taskbar entry.
pub(crate) fn profit_monitor_window_options(
    title: impl Into<SharedString>,
    window_bounds: WindowBounds,
    display_id: Option<DisplayId>,
    min_size: Option<Size<Pixels>>,
) -> WindowOptions {
    let mut options = app_window_options(
        title,
        window_bounds,
        display_id,
        min_size,
        APP_ID.to_string(),
        app_icon(0),
        true,
    );
    options.relationship = WindowRelationship::default();
    options.taskbar_visibility = WindowTaskbarVisibility::Visible;
    options
}

/// Build options for a detached non-chart panel such as Orders, Assets, Log, or Report.
///
/// When an owner is present, the panel is an owned floating window that follows the group window
/// and has no separate taskbar entry. During restoration the owner may be unavailable; in that
/// case the relationship falls back to independent so the restored panel is not lost.
///
/// # Arguments
///
/// * `title` - Window title.
/// * `window_bounds` - Initial or restored window state and geometry.
/// * `display_id` - Display selected for the window, if known.
/// * `owner` - Optional group-window owner.
///
/// # Returns
///
/// Floating window options with an owner relationship when one is available.
pub(crate) fn detached_panel_window_options(
    title: impl Into<SharedString>,
    window_bounds: WindowBounds,
    display_id: Option<DisplayId>,
    owner: Option<AnyWindowHandle>,
) -> WindowOptions {
    owned_window_options(title, window_bounds, display_id, None, owner, true)
}

/// Build options for an independent detached chart window.
///
/// The project keeps chart windows neither owned nor tool windows so PowerToys FancyZones can
/// discover and snap them. `WindowTaskbarVisibility::Hidden` avoids an app-window taskbar style,
/// while [`hide_window_from_taskbar`] removes the taskbar item after the native window appears.
/// Keeping the window independent also avoids raising the group window when the chart is clicked.
///
/// # Arguments
///
/// * `title` - Window title.
/// * `window_bounds` - Initial or restored window state and geometry.
/// * `display_id` - Display selected for the window, if known.
///
/// # Returns
///
/// Independent chart-window options with hidden taskbar visibility.
pub(crate) fn detached_chart_window_options(
    title: impl Into<SharedString>,
    window_bounds: WindowBounds,
    display_id: Option<DisplayId>,
) -> WindowOptions {
    let mut options = app_window_options(
        title,
        window_bounds,
        display_id,
        None,
        APP_ID.to_string(),
        None,
        true,
    );
    options.taskbar_visibility = WindowTaskbarVisibility::Hidden;
    options
}

pub(crate) fn debug_window_options(
    title: impl Into<SharedString>,
    window_bounds: WindowBounds,
    min_size: Option<Size<Pixels>>,
    owner: Option<AnyWindowHandle>,
    transparent_titlebar: bool,
) -> WindowOptions {
    owned_window_options(
        title,
        window_bounds,
        None,
        min_size,
        owner,
        transparent_titlebar,
    )
}

fn owned_window_options(
    title: impl Into<SharedString>,
    window_bounds: WindowBounds,
    display_id: Option<DisplayId>,
    min_size: Option<Size<Pixels>>,
    owner: Option<AnyWindowHandle>,
    transparent_titlebar: bool,
) -> WindowOptions {
    // Owned tool, detached, and debug windows are absent from the taskbar, so they need no icon.
    let mut options = app_window_options(
        title,
        window_bounds,
        display_id,
        min_size,
        APP_ID.to_string(),
        None,
        transparent_titlebar,
    );
    options.kind = WindowKind::Floating;
    options.relationship = owner.map(WindowRelationship::owned).unwrap_or_default();
    options
}

/// Resolve the display currently containing an owner window.
///
/// Tool and detached windows use this to open on the same display as the group window that
/// launched them.
///
/// # Arguments
///
/// * `owner` - Optional owner-window handle.
/// * `cx` - Application context used to update and inspect the owner window.
///
/// # Returns
///
/// The owner's current display ID, or `None` if the owner or display cannot be resolved.
pub(crate) fn owner_display_id(owner: Option<AnyWindowHandle>, cx: &mut App) -> Option<DisplayId> {
    owner?
        .update(cx, |_, window, cx| window.display(cx).map(|d| d.id()))
        .ok()
        .flatten()
}

/// Resolve the display for a secondary window with saved geometry.
///
/// Display containment is reliable on backends with global window coordinates, such as Windows
/// and X11. macOS reports display-relative coordinates, so this skips containment there and uses
/// the owner display. Wayland placement is compositor-controlled and coordinates may be
/// surface-local, so callers may need to rely on the owner-display fallback instead of a saved
/// origin.
///
/// `owner_display` is a display ID captured at the call site with `window.display(cx)`. Pass it
/// when this function runs inside an owner-window event handler, because that window's slot in
/// `cx.windows` is already borrowed and the `owner.update()` fallback will fail.
///
/// # Arguments
///
/// * `saved_origin` - Saved window origin used for display containment outside macOS.
/// * `owner` - Optional owner handle used as the final fallback.
/// * `owner_display` - Owner display captured by the caller when direct owner access is unavailable.
/// * `cx` - Application context used to enumerate displays and inspect the owner.
///
/// # Returns
///
/// The display selected from saved geometry or owner state, or `None` when neither resolves.
pub(crate) fn saved_or_owner_display_id(
    saved_origin: Option<Point<Pixels>>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) -> Option<DisplayId> {
    if cfg!(not(target_os = "macos")) {
        if let Some(origin) = saved_origin {
            if let Some(d) = cx
                .displays()
                .into_iter()
                .find(|d| d.bounds().contains(&origin))
            {
                return Some(d.id());
            }
        }
    }
    owner_display.or_else(|| owner_display_id(owner, cx))
}

/// Activate a newly created window in response to an explicit user action.
///
/// On macOS an owned window created on another display may otherwise remain hidden until the next
/// application activation, making click N+1 reveal the window requested by click N. Do not call
/// this during bulk startup restoration, where activating each window would steal focus.
///
/// # Arguments
///
/// * `handle` - Handle of the newly created window.
/// * `cx` - Application context used to update and activate the window.
pub(crate) fn activate_new_window(handle: AnyWindowHandle, cx: &mut App) {
    let _ = handle.update(cx, |_, window, _| window.activate_window());
}

/// Read windowed geometry as logical pixels `(x, y, width, height)`.
///
/// This centralizes the float-to-integer conversion used to persist detached panel and chart
/// geometry.
///
/// # Arguments
///
/// * `window` - Window whose current bounds should be inspected.
///
/// # Returns
///
/// Integer geometry for a `Windowed` window, or `None` for fullscreen or maximized bounds.
pub(crate) fn window_geom(window: &Window) -> Option<(i32, i32, u32, u32)> {
    let WindowBounds::Windowed(b) = window.window_bounds() else {
        return None;
    };
    Some((
        f32::from(b.origin.x) as i32,
        f32::from(b.origin.y) as i32,
        f32::from(b.size.width) as u32,
        f32::from(b.size.height) as u32,
    ))
}

/// Configure an opaque Windows DWM frame with square corners and fixed dark border/caption colors.
///
/// Each native call is best effort. Other platforms use a no-op counterpart.
///
/// # Arguments
///
/// * `window` - Window whose native DWM attributes should be configured.
#[cfg(target_os = "windows")]
pub(crate) fn configure_dwm_window(window: &Window) {
    use raw_window_handle::RawWindowHandle;
    use windows::Win32::{
        Foundation::HWND,
        Graphics::Dwm::{
            DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE,
            DWMWCP_DONOTROUND, DwmSetWindowAttribute,
        },
    };

    window.set_background_appearance(WindowBackgroundAppearance::Opaque);

    let Ok(handle) = raw_window_handle::HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };

    let hwnd = HWND(handle.hwnd.get() as *mut _);
    let corner = DWMWCP_DONOTROUND;
    let colorref_header = 0x001F1C1A_u32;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const _ as *const _,
            std::mem::size_of_val(&corner) as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &colorref_header as *const _ as *const _,
            std::mem::size_of_val(&colorref_header) as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR,
            &colorref_header as *const _ as *const _,
            std::mem::size_of_val(&colorref_header) as u32,
        );
    }
}

#[cfg(not(target_os = "windows"))]
/// Leave DWM-specific configuration unchanged on non-Windows platforms.
///
/// # Arguments
///
/// * `_` - Window accepted for API parity with the Windows implementation.
pub(crate) fn configure_dwm_window(_: &Window) {}

/// Set a Windows group window's large and small native icons from embedded icon bytes.
///
/// The function asks `CreateIconFromResourceEx` for 32-pixel and 16-pixel `HICON` handles using
/// resource version `0x00030000`, then sends them with `WM_SETICON` for the live taskbar and
/// Alt-Tab representations. X11 receives the same group image through `WindowOptions.icon` instead.
///
/// # Arguments
///
/// * `window` - Group window whose native icons should be replaced.
/// * `icon_id` - Embedded group-icon ID.
#[cfg(target_os = "windows")]
pub(crate) fn set_group_window_icon(window: &Window, icon_id: u32) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateIconFromResourceEx, ICON_BIG, ICON_SMALL, LR_DEFAULTCOLOR, SendMessageW, WM_SETICON,
    };

    let Some(png) = group_icon_png(icon_id) else {
        return;
    };
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(h.hwnd.get() as *mut _);
    unsafe {
        for (size, which) in [(32_i32, ICON_BIG), (16_i32, ICON_SMALL)] {
            if let Ok(hicon) =
                CreateIconFromResourceEx(png, true, 0x0003_0000, size, size, LR_DEFAULTCOLOR)
            {
                let _ = SendMessageW(
                    hwnd,
                    WM_SETICON,
                    Some(WPARAM(which as usize)),
                    Some(LPARAM(hicon.0 as isize)),
                );
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
/// Leave the native window icon unchanged outside Windows.
///
/// # Arguments
///
/// * `_` - Window accepted for API parity with the Windows implementation.
/// * `_` - Embedded icon ID accepted for API parity.
pub(crate) fn set_group_window_icon(_: &Window, _: u32) {}

/// Remove a Windows taskbar item without changing the window's native style.
///
/// Detached charts stay independent windows so PowerToys FancyZones can discover them; this
/// calls `ITaskbarList::DeleteTab` only after the native window has been shown and its taskbar item
/// can exist. Handle lookup, COM creation, initialization, and deletion are all best effort.
///
/// # Arguments
///
/// * `window` - Displayed window whose taskbar item should be removed.
#[cfg(target_os = "windows")]
pub(crate) fn hide_window_from_taskbar(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
    use windows::Win32::UI::Shell::{ITaskbarList, TaskbarList};

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(h.hwnd.get() as *mut _);
    unsafe {
        let taskbar: ITaskbarList = match CoCreateInstance(&TaskbarList, None, CLSCTX_ALL) {
            Ok(t) => t,
            Err(_) => return,
        };
        if taskbar.HrInit().is_err() {
            return;
        }
        let _ = taskbar.DeleteTab(hwnd);
    }
}

#[cfg(not(target_os = "windows"))]
/// Leave taskbar state unchanged outside Windows.
///
/// # Arguments
///
/// * `_` - Window accepted for API parity with the Windows implementation.
pub(crate) fn hide_window_from_taskbar(_: &Window) {}

/// Restore a Windows window and move it into an on-screen cascade near the primary origin.
///
/// `SW_RESTORE` unmaximizes or restores the window, and `SetWindowPos` moves it without changing
/// its size. This recovers detached windows left off-screen, on a disconnected display, or
/// minimized. The GPUI fork has no position setter, so the move uses Win32 directly.
///
/// # Arguments
///
/// * `window` - Window to restore and reposition.
/// * `index` - Cascade index used to offset this window from its peers.
#[cfg(target_os = "windows")]
pub(crate) fn reset_window_onscreen(window: &Window, index: usize) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_TOP, SW_RESTORE, SWP_NOSIZE, SWP_SHOWWINDOW, SetWindowPos, ShowWindow,
    };

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(h.hwnd.get() as *mut _);
    // Offset by 40 pixels and wrap every eight windows so the first windows do not fully overlap.
    let off = 60 + (index as i32 % 8) * 40;
    unsafe {
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOP),
            off,
            off,
            0,
            0,
            SWP_NOSIZE | SWP_SHOWWINDOW,
        );
    }
}

#[cfg(not(target_os = "windows"))]
/// Leave window geometry unchanged outside Windows.
///
/// # Arguments
///
/// * `_` - Window accepted for API parity with the Windows implementation.
/// * `_` - Cascade index accepted for API parity.
pub(crate) fn reset_window_onscreen(_: &Window, _: usize) {}

/// Reset every application window into an on-screen cascade for the Ctrl+Shift+F10 recovery action.
///
/// Enumerating `App::windows()` avoids dependence on the backend's detached-window registry, so
/// the action covers group windows and detached panel windows alike. The per-window operation is
/// effective only on Windows.
///
/// # Arguments
///
/// * `cx` - Application context used to enumerate and update every window.
pub(crate) fn reset_all_windows_onscreen(cx: &mut App) {
    for (i, handle) in cx.windows().into_iter().enumerate() {
        let _ = handle.update(cx, |_, window, _| reset_window_onscreen(window, i));
    }
}
