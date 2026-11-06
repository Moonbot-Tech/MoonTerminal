//! Automation shares the existing full-settings writer; it never retries an uncertain full import.

use std::sync::mpsc::SyncSender;
use std::time::Instant;

use anyhow::{Result, ensure};
use serde_json::{Value, json};

use super::{ECHO_TIMEOUT, SharedConfigSequence};
use crate::automation::{Action, Receipt, Submission, settings};

/// Only the requested values and one receipt sender survive dispatch; no duplicate full config.
pub(super) struct PendingAutomation {
    expected: Value,
    reply: SyncSender<std::result::Result<Receipt, String>>,
    deadline: Instant,
    pub(super) updated: bool,
}

impl SharedConfigSequence {
    /// Admit a patch only while both settings channels are idle, then hold the full-config writer.
    /// A busy response requires a new read/preview; it never queues a stale full configuration.
    pub(in crate::feed::live) fn submit_automation(
        &mut self,
        client: &moonproto::MoonClient,
        request: &Submission,
        compact_idle: bool,
    ) -> Result<()> {
        ensure!(
            compact_idle
                && self.queue.is_empty()
                && !self.waiting_for_echo
                && self.pending_confirmation.is_none()
                && self.automation.is_none(),
            "settings writer is busy; wait for its result and preview again"
        );
        request.admitted()?;
        let Action::SettingsPatch { fields } = &request.action else {
            anyhow::bail!("not a settings patch");
        };
        let current = client.settings().build_shared_config()?;
        let preview = settings::preview(&current, fields)?;
        ensure!(
            preview == request.expected,
            "changed settings or catalog since preview; read and preview again"
        );
        if preview["before"] == preview["after"] {
            let _ = request.reply.try_send(Ok(json!({"status":"already_applied","execution_confirmed":true,"fields":preview["after"]}).into()));
            return Ok(());
        }
        let changed = current.patch_json(fields)?;
        request.admitted()?;
        self.transmit(client, &changed)?;
        let _ = request.reply.try_send(Ok(json!({"status":"submitted","pending":true,"execution_confirmed":false,"fields":preview["after"]}).into()));
        self.automation = Some(PendingAutomation {
            expected: preview["after"].clone(),
            reply: request.reply.clone(),
            deadline: Instant::now() + ECHO_TIMEOUT,
            updated: false,
        });
        Ok(())
    }

    /// Keep observing a sent import even when later compact edits are queued.
    pub(in crate::feed::live) fn automation_pending(&self) -> bool {
        self.automation.is_some()
    }

    /// Consume only a new full-settings echo; a compact overlay or the send receipt cannot confirm.
    pub(super) fn drive_automation(&mut self, client: &moonproto::MoonClient) {
        let snapshot = self
            .automation
            .as_ref()
            .filter(|pending| pending.updated)
            .and_then(|_| client.snapshot());
        self.observe_automation(
            snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.settings().shared_config.as_ref()),
            Instant::now(),
        );
    }

    /// Evaluate a raw full snapshot and deadline without compact overlays or resubmission.
    fn observe_automation(
        &mut self,
        full: Option<&moonproto::shared_config::SharedConfig>,
        now: Instant,
    ) -> bool {
        let Some(pending) = &mut self.automation else {
            return false;
        };
        let expired = now >= pending.deadline;
        let confirmed = if pending.updated {
            pending.updated = false;
            full.and_then(|config| settings::matches(config, &pending.expected).ok())
                .unwrap_or(false)
        } else {
            false
        };
        if !confirmed && !expired {
            return true;
        }
        let result = if confirmed {
            json!({"status":"confirmed","execution_confirmed":true,"fields":pending.expected,
                "confirmation":"All requested fields observed in a new full core snapshot; this is not a transaction acknowledgement."})
        } else {
            json!({"status":"unknown","execution_confirmed":false,"fields":pending.expected,
                "reason":"No matching full settings echo before timeout. Read actual settings; the import is not automatically repeated."})
        };
        let _ = pending.reply.try_send(Ok(result.into()));
        self.automation = None;
        false
    }

    /// A changed connection ends observation but cannot undo an already transmitted import.
    pub(in crate::feed::live) fn cancel_automation(&mut self, reason: &str) {
        if let Some(pending) = self.automation.take() {
            let _ = pending.reply.try_send(Ok(
                json!({"status":"unknown","execution_confirmed":false,"reason":reason}).into(),
            ));
        }
    }
}

#[cfg(test)]
mod tests;
