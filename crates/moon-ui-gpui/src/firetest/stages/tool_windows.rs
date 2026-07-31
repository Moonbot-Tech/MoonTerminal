//! Stages `tool_windows_open` … `tool_windows_verify_dedup`: Settings, Strategies and Assets open
//! as real GPUI windows, and opening them again focuses the existing window.
//!
//! Identity is compared by `WindowId`, not by "a window exists": a second `open` that silently
//! created a duplicate would still leave a window under each field, and only the changed id
//! exposes it.

use gpui::Context;

use crate::Backend;

use crate::firetest::Runtime;
use crate::firetest::logging::firetest_info;

impl Runtime {
    /// Ask for all three tool windows. Deferred because opening a window from inside a `Backend`
    /// update would re-enter the context that is currently borrowed.
    pub(in crate::firetest) fn request_tool_windows_open(&self, cx: &mut Context<Backend>) {
        let backend_entity = cx.entity();
        cx.defer(move |cx| {
            crate::settings::open(backend_entity.clone(), None, None, cx);
            crate::strategies::open(backend_entity.clone(), None, None, cx);
            crate::panels::open_assets_window(backend_entity, None, None, cx);
        });
        firetest_info("[firetest] tool_windows_open deferred=settings,strategies,assets");
    }

    /// The current `WindowId` of each tool window, or an error naming the one that did not open.
    pub(in crate::firetest) fn tool_window_ids(
        backend: &Backend,
    ) -> Result<(String, String, String), String> {
        let Some(settings) = backend
            .settings_window
            .map(|h| format!("{:?}", h.window_id()))
        else {
            return Err("settings tool window did not open".into());
        };
        let Some(strategies) = backend
            .strategies_window
            .map(|h| format!("{:?}", h.window_id()))
        else {
            return Err("strategies tool window did not open".into());
        };
        let Some(assets) = backend
            .assets_window
            .map(|h| format!("{:?}", h.window_id()))
        else {
            return Err("assets tool window did not open".into());
        };
        Ok((settings, strategies, assets))
    }

    /// Record the ids of the freshly opened windows as the dedup baseline.
    pub(in crate::firetest) fn verify_tool_windows_open(
        &mut self,
        backend: &Backend,
    ) -> Result<(), String> {
        let ids = Self::tool_window_ids(backend)?;
        firetest_info(&format!(
            "[firetest] tool_windows_open settings={} strategies={} assets={}",
            ids.0, ids.1, ids.2
        ));
        self.tool_window_ids = Some(ids);
        Ok(())
    }

    /// Require that the second open reused every window rather than creating a new one.
    pub(in crate::firetest) fn verify_tool_windows_dedup(
        &self,
        backend: &Backend,
    ) -> Result<(), String> {
        let before = self
            .tool_window_ids
            .as_ref()
            .ok_or_else(|| "tool window dedup has no baseline ids".to_string())?;
        let after = Self::tool_window_ids(backend)?;

        if before.0 != after.0 {
            return Err("settings tool window dedup created a new window".into());
        }
        if before.1 != after.1 {
            return Err("strategies tool window dedup created a new window".into());
        }
        if before.2 != after.2 {
            return Err("assets tool window dedup created a new window".into());
        }

        firetest_info(&format!(
            "[firetest] tool_windows_dedup settings={} strategies={} assets={}",
            after.0, after.1, after.2
        ));
        Ok(())
    }
}
