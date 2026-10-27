//! Synthetic Core Status rows shared by the layout tests.
//!
//! One offline core, one high ping, one lagging build, and one expiring key, so every health
//! chip has a count. Addresses are `10.0.0.x` and names are `CORE-A*` / `Server *`. Nothing
//! here reads the network, config, or a backend.

use std::net::IpAddr;

use gpui::{Bounds, Pixels, px};
use moon_core::feed::{ConnStatus, CoreEndpoint, CoreTimeOffsetStatus};
use moon_core::session::{CoreStartupStatus, CoreSysStatus};
use moon_ui::MoonTheme;

use super::model::{ApiKeyState, CoreStatusRow, GroupVersion, ServerKey, ServerStatusGroup};
use crate::backend::core_warn::LatencySeverity;

/// Dock widths the layout tests walk, wide then narrow.
pub(super) const WIDTHS: [f32; 3] = [1640.0, 1100.0, 720.0];

/// Locales the layout tests walk. The order matches [`moon_core::config::Language::ALL`].
pub(super) const LOCALES: [&str; 7] = ["ru", "en", "es", "uk", "tr", "pt", "vi"];

/// Install the dark presentation theme at zoom 1. Layout does not depend on the light palette.
pub(super) fn install_dark(cx: &mut gpui::TestAppContext) {
    // Alarm and update copy translates; the first lookup must not run on the 2 MiB test stack.
    crate::test_locale::warm();
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(
                moon_core::config::UiThemeMode::Dark,
                1.0,
            ),
            cx,
        );
    });
}

/// Leak a formatted debug id. `debug_bounds` only accepts a `'static` selector.
pub(super) fn leak_selector(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

/// Draw until GPUI has settled the frame the assertions read.
pub(super) fn draw(visual: &mut gpui::VisualTestContext) {
    for _ in 0..3 {
        visual.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
}

/// Whether `inner` sits inside `outer`, allowing half a pixel of rounding.
pub(super) fn inside(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    const SLOP: f32 = 0.5;
    f32::from(inner.origin.x) + SLOP >= f32::from(outer.origin.x)
        && f32::from(inner.right()) <= f32::from(outer.right()) + SLOP
        && f32::from(inner.origin.y) + SLOP >= f32::from(outer.origin.y)
        && f32::from(inner.bottom()) <= f32::from(outer.bottom()) + SLOP
}

/// Whether two boxes overlap by more than the half-pixel gap that touching edges are allowed.
pub(super) fn overlaps(a: Bounds<Pixels>, b: Bounds<Pixels>) -> bool {
    const GAP: f32 = 0.5;
    f32::from(a.right()) > f32::from(b.origin.x) + GAP
        && f32::from(b.right()) > f32::from(a.origin.x) + GAP
        && f32::from(a.bottom()) > f32::from(b.origin.y) + GAP
        && f32::from(b.bottom()) > f32::from(a.origin.y) + GAP
}

/// Width of a laid-out box, in pixels.
pub(super) fn width_of(bounds: Bounds<Pixels>) -> f32 {
    f32::from(bounds.size.width)
}

/// Height of a laid-out box, in pixels.
pub(super) fn height_of(bounds: Bounds<Pixels>) -> f32 {
    f32::from(bounds.size.height)
}

/// Open a window of `width` by `height` and return its visual context.
pub(super) fn open<V>(
    cx: &mut gpui::TestAppContext,
    width: f32,
    height: f32,
    build: impl FnOnce(&mut gpui::Window, &mut gpui::Context<V>) -> V + 'static,
) -> gpui::VisualTestContext
where
    V: 'static + gpui::Render,
{
    let window = cx.open_window(gpui::size(px(width), px(height)), build);
    gpui::VisualTestContext::from_window(window.into(), cx)
}

/// Four cores that light every health chip, plus one quiet core so the verdict has a total.
///
/// Returns:
///     Rows in display order. The quiet core is last. API columns stay visible: one row carries
///     a day count and one carries a quota.
pub(super) fn cores() -> Vec<CoreStatusRow> {
    vec![
        row(
            1,
            "CORE-A1",
            [10, 0, 0, 1],
            ConnStatus::Failed("down".into()),
            false,
            None,
            ApiKeyState::Unknown,
            false,
            false,
            None,
        ),
        row(
            2,
            "CORE-A2",
            [10, 0, 0, 2],
            ConnStatus::Ready,
            true,
            None,
            ApiKeyState::Perpetual,
            false,
            false,
            Some(50),
        ),
        row(
            3,
            "CORE-A3",
            [10, 0, 0, 3],
            ConnStatus::Ready,
            false,
            Some(771),
            ApiKeyState::Days(40),
            false,
            true,
            Some(80),
        ),
        row(
            4,
            "CORE-A4",
            [10, 0, 0, 4],
            ConnStatus::Ready,
            false,
            None,
            ApiKeyState::Days(3),
            true,
            false,
            Some(12),
        ),
        row(
            5,
            "CORE-A5",
            [10, 0, 0, 2],
            ConnStatus::Ready,
            false,
            None,
            ApiKeyState::Perpetual,
            false,
            false,
            Some(50),
        ),
    ]
}

/// Two server cards. Server 1 is warned and carries the offline core; Server 2 is calm.
///
/// Returns:
///     Groups in construction order (not card order) and a two-point CPU series per address.
pub(super) fn servers() -> (
    Vec<ServerStatusGroup>,
    std::collections::HashMap<IpAddr, Vec<(f32, f32)>>,
) {
    let cores = cores();
    let warned = server(
        "Server 1",
        IpAddr::from([10, 0, 0, 1]),
        true,
        vec![cores[0].clone()],
        82,
        Some(771),
    );
    let calm = server(
        "Server 2",
        IpAddr::from([10, 0, 0, 2]),
        false,
        vec![cores[1].clone(), cores[4].clone()],
        18,
        Some(771),
    );
    let mut history = std::collections::HashMap::new();
    history.insert(IpAddr::from([10, 0, 0, 1]), vec![(0.0, 20.0), (1.0, 80.0)]);
    history.insert(IpAddr::from([10, 0, 0, 2]), vec![(0.0, 10.0), (1.0, 18.0)]);
    (vec![calm, warned], history)
}

#[allow(clippy::too_many_arguments)]
fn row(
    id: u64,
    name: &str,
    address: [u8; 4],
    status: ConnStatus,
    ping_warn: bool,
    version_behind: Option<u32>,
    api_key: ApiKeyState,
    api_warn: bool,
    api_notice: bool,
    api_quota: Option<u64>,
) -> CoreStatusRow {
    CoreStatusRow {
        fault: None,
        mode_suggestion: None,
        id,
        name: name.to_string(),
        status,
        sys: CoreSysStatus {
            process_cpu_percent: Some(7),
            system_cpu_percent: Some(11),
            used_memory_mb: Some(40),
            free_physical_memory_mb: Some(800),
            round_trip_ms: Some(12),
            order_api_latency_ms: Some(20),
            logical_cpu_count: Some(4),
            ..CoreSysStatus::default()
        },
        endpoint: Some(CoreEndpoint {
            address: IpAddr::from(address),
            port: 4242,
        }),
        ping_warn,
        ping_sev: if ping_warn {
            LatencySeverity::Warning
        } else {
            LatencySeverity::Normal
        },
        exch_sev: LatencySeverity::Normal,
        api_key,
        api_warn,
        api_notice,
        api_quota,
        api_quota_warn: false,
        startup: CoreStartupStatus::default(),
        time_offset: CoreTimeOffsetStatus {
            offset_secs: Some(7_200),
            ..CoreTimeOffsetStatus::default()
        },
        server_version: Some(770),
        server_version_suffix: None,
        version_behind,
        update: None,
    }
}

fn server(
    name: &str,
    address: IpAddr,
    warned: bool,
    members: Vec<CoreStatusRow>,
    cpu: u8,
    build: Option<u32>,
) -> ServerStatusGroup {
    ServerStatusGroup {
        key: ServerKey::Address(address),
        display_name: name.to_string(),
        cpu_warn: warned,
        mem_warn: false,
        conn_warn: false,
        ping_warn: warned,
        exch_warn: false,
        api_warn: false,
        api_notice: false,
        api_key: ApiKeyState::Days(3),
        version: GroupVersion::Uniform {
            number: build.unwrap_or(770),
            suffix: None,
        },
        version_behind: None,
        address: Some(address),
        cores: members,
        system_cpu_percent: Some(cpu),
        process_memory_mb: Some(400),
        free_physical_memory_mb: Some(800),
        logical_cpu_count: Some(4),
    }
}
