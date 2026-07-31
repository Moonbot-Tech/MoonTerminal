//! Stages `locale_switch` / `locale_switch_verify`: the language change reaches rust-i18n live and
//! redraws the open windows instead of recreating them.
//!
//! Recreating a window on a language change would look right on screen and quietly break window
//! deduplication and layout continuity, so this stage checks the tool-window identities as well as
//! the global locale. The original locale is restored afterwards so no later stage or log line runs
//! in the switched language.

use gpui::Context;
use moon_core::config::Language;

use crate::Backend;

use crate::firetest::Runtime;
use crate::firetest::logging::firetest_info;

impl Runtime {
    /// Switch the interface locale through the same live path as `Settings::apply_settings`:
    /// update rust-i18n's global locale and call `refresh_windows()`. English switches to Russian;
    /// any other locale switches to English. The original and target are recorded for verification
    /// and restoration.
    pub(in crate::firetest) fn request_locale_switch(
        &mut self,
        backend: &mut Backend,
        cx: &mut Context<Backend>,
    ) {
        let original = backend.config.language;
        let target = if original == Language::En {
            Language::Ru
        } else {
            Language::En
        };
        self.locale_switch = Some((original, target));
        backend.config.language = target;
        rust_i18n::set_locale(target.code());
        cx.refresh_windows();
        cx.notify();
        firetest_info(&format!(
            "[firetest] locale_switch from={} to={}",
            original.code(),
            target.code()
        ));
    }

    /// Verify that the global locale reached the target and that the tool-window identities did not
    /// change.
    pub(in crate::firetest) fn verify_locale_switch(
        &self,
        backend: &Backend,
    ) -> Result<(), String> {
        let (_, target) = self
            .locale_switch
            .ok_or_else(|| "locale switch contract has no recorded target".to_string())?;
        let active = rust_i18n::locale();
        if &*active != target.code() {
            return Err(format!(
                "locale switch did not reach rust-i18n: expected {}, got {}",
                target.code(),
                &*active
            ));
        }
        let before = self
            .tool_window_ids
            .as_ref()
            .ok_or_else(|| "locale switch has no tool window baseline ids".to_string())?;
        let after = Self::tool_window_ids(backend)?;
        if *before != after {
            return Err("locale switch recreated a tool window instead of redrawing it".into());
        }
        firetest_info(&format!(
            "[firetest] locale_switch_verify locale={} windows_stable=true",
            target.code()
        ));
        Ok(())
    }

    /// Restore the recorded original locale so this stage does not affect later stages or logs.
    pub(in crate::firetest) fn restore_locale(
        &mut self,
        backend: &mut Backend,
        cx: &mut Context<Backend>,
    ) {
        if let Some((original, _)) = self.locale_switch.take() {
            backend.config.language = original;
            rust_i18n::set_locale(original.code());
            cx.refresh_windows();
            cx.notify();
        }
    }
}
