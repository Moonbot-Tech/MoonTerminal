//! The terminal as the host of the Telegram bot and Mini App (`moon_tg`).
//!
//! Everything the bot says and does lives in `moon-tg`; this file only lends it the Backend — the
//! saved configuration and its persistence, the sessions, the header-clock zone, the Panic Sell
//! override the chart button shares, the reconnect queue — and runs its reads on GPUI's
//! background executor.

use crate::Backend;
use gpui::Context;
use moon_core::config::{AppConfig, TelegramConfig};
use moon_core::session::{CoreId, SessionManager};
use moon_tg::{HostKind, Job, TelegramState, TgHost};

/// The Backend lent to `moon_tg` for one call on the UI thread.
struct GuiTgHost<'a, 'b> {
    backend: &'a mut Backend,
    cx: &'a mut Context<'b, Backend>,
}

impl TgHost for GuiTgHost<'_, '_> {
    fn kind(&self) -> HostKind {
        HostKind::Terminal
    }

    fn config(&self) -> &AppConfig {
        &self.backend.config
    }

    fn session(&self) -> &SessionManager {
        &self.backend.session
    }

    fn session_mut(&mut self) -> &mut SessionManager {
        &mut self.backend.session
    }

    fn state(&self) -> &TelegramState {
        &self.backend.telegram
    }

    fn state_mut(&mut self) -> &mut TelegramState {
        &mut self.backend.telegram
    }

    fn report_zone(&self) -> chrono_tz::Tz {
        moon_core::util::display_time::zone_or_utc(self.backend.header_clock_zone())
    }

    /// Return the terminal's notification file path without creating it.
    fn notifications_path(&self) -> std::path::PathBuf {
        moon_core::config::paths::telegram_notifications()
    }

    fn report_revision(&self) -> Option<moon_tg::ReportRevision> {
        let reports = self.backend.reports.as_ref()?;
        moon_tg::ReportRevision::current(
            &reports.generation,
            self.backend
                .valuation
                .as_ref()
                .map(|valuation| &*valuation.generation),
        )
    }

    fn save_paired_chat(&mut self, chat_id: i64) -> bool {
        let backend = &mut *self.backend;
        let mut candidate = backend.config.clone();
        let newly_paired = candidate.telegram.pair_chat(chat_id);
        if candidate.save_telegram().is_err() {
            return false;
        }
        backend.config = candidate;
        // An open Settings draft learns the new chat too, so its next Save does not drop it.
        if newly_paired && let Some(preview) = backend.preview.as_mut() {
            preview.telegram.pair_chat(chat_id);
        }
        true
    }

    fn save_cleared_pairing(&mut self) -> bool {
        let backend = &mut *self.backend;
        let mut candidate = backend.config.clone();
        candidate.telegram.clear_pairing();
        if candidate.save_telegram().is_err() {
            return false;
        }
        backend.config = candidate;
        if let Some(preview) = backend.preview.as_mut() {
            preview.telegram.clear_pairing();
        }
        true
    }

    fn is_panic_armed(&self, core: CoreId, market: &str) -> bool {
        self.backend.is_panic_armed(core, market)
    }

    fn toggle_panic_sell(&mut self, core: CoreId, market: String) -> bool {
        self.backend.toggle_panic_sell(core, market)
    }

    fn request_reconnect(&mut self, core: CoreId) {
        if !self.backend.reconnect_request.contains(&core) {
            self.backend.reconnect_request.push(core);
        }
        // The coordination tick drains the queue; the notify repaints what shows it.
        self.cx.notify();
    }

    fn spawn(&mut self, job: Job) {
        self.cx
            .spawn(async move |this, cx| {
                let executor = cx.update(|cx| cx.background_executor().clone());
                let finish = executor.spawn(async move { job() }).await;
                cx.update(|cx| {
                    let _ = this.update(cx, |backend, cx| finish(&mut GuiTgHost { backend, cx }));
                });
            })
            .detach();
    }

    fn repaint(&mut self) {
        self.cx.notify();
    }
}

impl Backend {
    /// Revoke local transport when station presence or durable ownership forbids it.
    pub(crate) fn gate_terminal_telegram(&mut self) {
        if !self.station.allows_terminal_bot() && !self.telegram.suspended() {
            self.telegram.suspend();
        }
    }

    /// Keep saved tokens gated through Save while a station owns the terminal's only bot.
    pub(crate) fn reconcile_telegram(&mut self, before: &TelegramConfig) {
        self.gate_terminal_telegram();
        moon_tg::reconcile(&mut self.telegram, &self.config.telegram, before);
    }

    /// Issue a fresh ten-minute pairing code from the live transport ledger.
    pub(crate) fn issue_telegram_pairing(&mut self) {
        moon_tg::issue_pairing(&mut self.telegram);
    }

    /// Persist revocation before displaying an empty paired set or restarting service.
    pub(crate) fn reset_telegram_pairing(&mut self, cx: &mut Context<Self>) {
        moon_tg::reset_pairing(&mut GuiTgHost { backend: self, cx });
    }

    /// Drain bounded work after preserving station ownership through any service retirement.
    pub(crate) fn tick_telegram(&mut self, cx: &mut Context<Self>) {
        self.gate_terminal_telegram();
        moon_tg::tick(&mut GuiTgHost { backend: self, cx });
    }

    /// The service's own state, without the per-core roster.
    ///
    /// A settings surface must show only whether the service itself is up.
    pub(crate) fn telegram_service_status_text(&self) -> String {
        moon_tg::status_text(&self.telegram.status)
    }
}
