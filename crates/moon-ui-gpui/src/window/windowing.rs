//! Platform-level window support: `WindowOptions` factories for every terminal window type
//! (trading, tool, independent Profit Monitor, detached panel, detached chart, and debug), display
//! selection from saved geometry or the owner window, theme-aware clear colors, Windows DWM frame
//! configuration, and HWND/geometry helpers. It also provides Windows AppUserModelIDs and group
//! icons embedded in the executable by `build.rs` through `embed_group_icons`.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
#[cfg(target_os = "windows")]
use std::time::Duration;

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
/// The normal independent relationship keeps the monitor usable while every Main window is
/// minimized and preserves its Alt+Tab entry. The caller completes the terminal's single-icon
/// taskbar policy with [`hide_window_from_taskbar_soon`].
///
/// Args:
///     title: Native window title.
///     window_bounds: Initial or restored geometry.
///     display_id: Display chosen from saved geometry or the launching window.
///     min_size: Smallest responsive monitor size.
///
/// Returns:
///     Independent normal-window options prepared for shared taskbar suppression.
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
    options.taskbar_visibility = WindowTaskbarVisibility::Hidden;
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

/// Build options for the startup login window.
///
/// Deliberately a standalone application window rather than a tool window: it can be the FIRST and
/// only window of the session, opened before any group window exists, so an owned floating window
/// would have nothing to be owned by and no taskbar entry to be found under while the user goes
/// looking for the password prompt.
///
/// # Arguments
///
/// * `title` - Native window title.
/// * `window_bounds` - Initial geometry.
/// * `min_size` - Smallest size that still fits the prompt.
///
/// # Returns
///
/// Standalone window options carrying the application icon and taskbar identity.
pub(crate) fn login_window_options(
    title: impl Into<SharedString>,
    window_bounds: WindowBounds,
    min_size: Option<Size<Pixels>>,
) -> WindowOptions {
    app_window_options(
        title,
        window_bounds,
        None,
        min_size,
        APP_ID.to_string(),
        app_icon(0),
        true,
    )
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

/// Read the display a window currently sits on, as the `DisplayId` its options factory wants.
///
/// Every route that opens a secondary window from inside another window's event handler needs this
/// exact value, because the handle-based [`owner_display_id`] cannot resolve while that window's
/// slot is borrowed. One spelling so the contract test can name it, and one place to fix when the
/// fork grows a direct `Window::display_id()` accessor — today's `Window::display` walks every
/// monitor to hand back an id the window already holds (see `docs-internal/FORK_BUGS.md`).
///
/// # Arguments
///
/// * `window` - Window whose display should be read.
/// * `cx` - Application context used to enumerate displays.
///
/// # Returns
///
/// The window's display, or `None` when the platform reports none.
pub(crate) fn window_display_id(window: &Window, cx: &App) -> Option<DisplayId> {
    window.display(cx).map(|display| display.id())
}

/// Place a first-time window's fallback origin on the display it is about to open on.
///
/// A cascade point like (200, 160) is expressed relative to a display, but Windows reads window
/// coordinates as global: handed a `display_id` whose area does not contain the point, its platform
/// layer silently discards the whole rectangle and substitutes the display's default bounds — the
/// window loses its intended SIZE as well as its position (fork: `moon-gpui-windows/src/window.rs`,
/// `retrieve_window_placement`). Offsetting by the target display's origin keeps the point inside
/// it. On macOS displays report a zero origin, which is correct there for the opposite reason: its
/// window coordinates are already display-relative.
///
/// # Arguments
///
/// * `offset` - Cascade point relative to the target display.
/// * `display` - Display the window will open on, when one was resolved.
/// * `cx` - Application context used to enumerate displays.
///
/// # Returns
///
/// The origin to open with, unchanged when no display resolved.
pub(crate) fn cascade_origin_on(
    offset: Point<Pixels>,
    display: Option<DisplayId>,
    cx: &App,
) -> Point<Pixels> {
    let Some(bounds) = display.and_then(|id| {
        cx.displays()
            .into_iter()
            .find(|d| d.id() == id)
            .map(|d| d.bounds())
    }) else {
        return offset;
    };
    bounds.origin + offset
}

/// Resolve the display currently containing an owner window.
///
/// This is the LAST resort of [`saved_or_owner_display_id`], not its normal route: a caller running
/// inside the owner window's own update cannot use it at all, because that window's slot in
/// `cx.windows` is taken and the update below returns `Err`. Such callers capture the display at the
/// call site instead. This resolves only for a caller running outside any owner-window update.
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
/// On macOS a window created on another display — owned or independent — may otherwise remain
/// hidden until the next application activation, making click N+1 reveal the window requested by
/// click N. Do not call this during bulk startup restoration, where activating each window would
/// steal focus, nor from a machine-driven route that no gesture asked for.
///
/// # Arguments
///
/// * `handle` - Handle of the newly created window.
/// * `cx` - Application context used to update and activate the window.
pub(crate) fn activate_new_window(handle: AnyWindowHandle, cx: &mut App) {
    // A closed window is the ordinary case here, not a fault: the user can close a just-opened
    // window before this runs. Logged rather than dropped so a silently unraised window — the very
    // defect this helper exists to remove — leaves a trace.
    if handle
        .update(cx, |_, window, _| window.activate_window())
        .is_err()
    {
        log::debug!("activate_new_window: window {handle:?} was gone before activation");
    }
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

/// Cancellation authority for one background taskbar-suppression burst.
///
/// Dropping or replacing the token prevents an old activation from continuing after a window is
/// released or a newer burst takes ownership of the same native window.
pub(crate) struct TaskbarHideTask {
    cancelled: Arc<AtomicBool>,
}

impl TaskbarHideTask {
    /// Cancel this burst without waiting for its worker thread to finish.
    ///
    /// Returns:
    ///     Nothing; the worker observes the flag before its next native call.
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Return whether cancellation has been requested.
    ///
    /// Returns:
    ///     Current cancellation state exposed only to lifecycle regressions.
    #[cfg(test)]
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

impl Drop for TaskbarHideTask {
    /// Cancel a still-running burst when its owning view is released or re-armed.
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Copy a Win32 HWND value while the GPUI window is leased on the application thread.
///
/// Args:
///     window: Live independent window whose taskbar item should be suppressed.
///
/// Returns:
///     Integer HWND safe to move to the dedicated native worker, or `None` off Win32.
#[cfg(target_os = "windows")]
fn taskbar_hwnd(window: &Window) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return None;
    };
    Some(h.hwnd.get())
}

/// Maximum deletion attempts in one bounded taskbar-suppression burst.
#[cfg(target_os = "windows")]
const TASKBAR_HIDE_ATTEMPTS: u32 = 20;

/// Delay between deletion attempts inside one taskbar-suppression burst.
#[cfg(target_os = "windows")]
const TASKBAR_HIDE_RETRY: Duration = Duration::from_millis(150);

/// Run one complete taskbar burst inside a dedicated COM apartment thread.
///
/// The COM interface is created, used, and dropped without an await or thread hop. The worker owns
/// no GPUI entity; a per-window cancellation token is its only lifetime input.
///
/// Args:
///     hwnd_value: Integer HWND copied while the GPUI window was live.
///     cancelled: Exact owning view's cancellation flag.
///
/// Returns:
///     Nothing; failures remain best-effort because taskbar visibility is cosmetic.
#[cfg(target_os = "windows")]
fn run_taskbar_hide_worker(hwnd_value: isize, cancelled: Arc<AtomicBool>) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    };
    use windows::Win32::UI::Shell::{ITaskbarList, TaskbarList};
    use windows::Win32::UI::WindowsAndMessaging::IsWindow;

    unsafe {
        if CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_err() {
            return;
        }
        struct ComApartment;
        impl Drop for ComApartment {
            fn drop(&mut self) {
                unsafe { CoUninitialize() };
            }
        }
        let _apartment = ComApartment;
        let taskbar: ITaskbarList = match CoCreateInstance(&TaskbarList, None, CLSCTX_ALL) {
            Ok(taskbar) => taskbar,
            Err(_) => return,
        };
        if taskbar.HrInit().is_err() {
            return;
        }
        let hwnd = HWND(hwnd_value as *mut _);
        for attempt in 0..TASKBAR_HIDE_ATTEMPTS {
            if cancelled.load(Ordering::Acquire) || !IsWindow(Some(hwnd)).as_bool() {
                break;
            }
            let _ = taskbar.DeleteTab(hwnd);
            if attempt + 1 < TASKBAR_HIDE_ATTEMPTS {
                std::thread::sleep(TASKBAR_HIDE_RETRY);
            }
        }
    }
}

/// Delete a window's taskbar item over the next few seconds.
///
/// Detached charts and the Profit Monitor remain independent so a minimized Main window does not
/// minimize them and PowerToys FancyZones can snap them, while the terminal still presents one
/// taskbar icon. `ITaskbarList::DeleteTab` removes an item that already exists; it is not durable
/// window state, and `WindowTaskbarVisibility::Hidden` only omits `WS_EX_APPWINDOW`, which an
/// unowned top-level window never needed to earn a taskbar button.
///
/// The shell publishes the item shortly after the native window is shown and republishes it when
/// an iconic window is restored. Callers therefore arm this burst at construction and after every
/// activation, replacing the token stored by the owning view. Replacement cancels an older burst,
/// so synchronous COM calls never accumulate on the application thread.
///
/// Args:
///     window: Live independent window used only to copy its integer HWND.
///
/// Returns:
///     Cancellation token that must remain owned by the exact window view.
#[cfg(target_os = "windows")]
pub(crate) fn hide_window_from_taskbar_soon(window: &Window) -> TaskbarHideTask {
    let task = TaskbarHideTask {
        cancelled: Arc::new(AtomicBool::new(false)),
    };
    if let Some(hwnd) = taskbar_hwnd(window) {
        let cancelled = task.cancelled.clone();
        let _ = std::thread::Builder::new()
            .name("moon-taskbar-hide".to_string())
            .spawn(move || run_taskbar_hide_worker(hwnd, cancelled));
    }
    task
}

/// Leave taskbar state unchanged outside Windows.
///
/// Args:
///
///     window: Window accepted for API parity.
///
/// Returns:
///     Inert cancellation token with the same ownership contract as Windows.
#[cfg(not(target_os = "windows"))]
pub(crate) fn hide_window_from_taskbar_soon(_: &Window) -> TaskbarHideTask {
    TaskbarHideTask {
        cancelled: Arc::new(AtomicBool::new(false)),
    }
}

#[cfg(test)]
mod tests;

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
