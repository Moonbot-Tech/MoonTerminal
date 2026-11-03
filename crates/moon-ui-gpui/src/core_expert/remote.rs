//! The Remote block's outgoing Telegram bot (#1174): the window's side of it — the own-bot switch
//! and its token, the channel PIN, the channel reset, and waiting on the core's verdict for each.
//!
//! All of it is moonproto's `client.telegram()` service API (MoonBot 7.72 L6 with service
//! settings), never the safe-share page: the token and the channel binding are this core's own
//! identity, and the page OK sends was built without them. So the block is not part of OK — the bot
//! is saved with its own button, the PIN and the reset are actions — and it always addresses the
//! ANCHOR core alone, for the reason the hang watchdog does (`watchdog`): a token and a channel
//! belong to one core, and copying them across a selection is not a decision this window makes.
//!
//! The controls are disabled while a request waits, as the observer block's are: an applied save
//! resets the switch and empties the token box, which would wipe a choice made meanwhile, and a
//! second request would take the track's one pending slot and orphan the first one's verdict.
//!
//! The token is write-only. The core never returns it (`token_set` says whether one is saved), so the
//! box holds only what the trader types to replace it, is masked, and is emptied once the core took
//! it. An empty box keeps the saved token.

use gpui::*;
use moon_ui::MoonInputState;

use moon_core::feed::{ConnStatus, CoreTelegramState, ServiceAction, TelegramCmd};
use moon_core::session::CoreId;

use crate::Backend;

use super::pages::SpecialSection;
use super::service::{Awaiting, verdict_applied};
use super::{CoreExpertView, ExpertTab};

/// Why the window refused to send the bot settings, named above the block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RemoteInvalid {
    /// The own-bot switch is on, the core holds no token, and none was typed.
    OwnBotNeedsToken,
}

/// The window's state for the Remote block of its anchor core.
#[derive(Default)]
pub(crate) struct RemoteBlock {
    /// Core this state belongs to; everything below is dropped when the anchor moves.
    core: Option<CoreId>,
    /// The own-bot switch as the trader set it, or `None` to show the core's.
    pub(super) own_bot: Option<bool>,
    /// What the last save was refused for locally.
    pub(super) invalid: Option<RemoteInvalid>,
    /// A request this window queued and has not seen answered.
    pub(super) awaiting: Option<Awaiting>,
    /// Whether this window sent anything to this core since the anchor last moved.
    pub(super) sent_here: bool,
    /// Why the last action could not even be queued (the core's channel is gone).
    pub(super) queue_failed: bool,
    /// The core a refresh was last asked of; `None` asks on the next sync.
    asked: Option<CoreId>,
    /// The store's `(telegram_rev, telegram_service_rev)` last drawn — the block's repaint gate:
    /// the snapshot it shows, and the track its banner reads.
    seen_rev: (u64, u64),
    /// The core took a token: empty the box on the next build, which has the context to.
    clear_token: bool,
    /// The masked token box, built while the block is on screen.
    pub(super) token: Option<Entity<MoonInputState>>,
}

impl RemoteBlock {
    /// Forget everything about the previous anchor, keeping the box but emptying it.
    fn rebind(&mut self, core: Option<CoreId>) {
        let token = self.token.take();
        *self = Self {
            core,
            token,
            clear_token: true,
            ..Self::default()
        };
    }

    /// The own-bot switch as drawn: the trader's choice, or the core's.
    pub(super) fn own_bot_shown(&self, snapshot: Option<&CoreTelegramState>) -> bool {
        self.own_bot
            .unwrap_or_else(|| snapshot.is_some_and(|s| s.own_bot))
    }
}

/// What `set_outgoing_bot` should carry, or why it cannot be sent.
///
/// A token box left empty keeps the saved token (`None`); a typed one replaces it. Switching the own
/// bot on with no token saved and none typed would route messages through a bot the core cannot
/// log in as, so it is refused here rather than by the core.
///
/// Args:
///     own_bot: The switch as drawn.
///     typed: The token box's text.
///     token_set: Whether the core holds a token already.
pub(super) fn bot_request(
    own_bot: bool,
    typed: &str,
    token_set: bool,
) -> Result<TelegramCmd, RemoteInvalid> {
    let typed = typed.trim();
    let token = (!typed.is_empty()).then(|| typed.to_string());
    if own_bot && token.is_none() && !token_set {
        return Err(RemoteInvalid::OwnBotNeedsToken);
    }
    Ok(TelegramCmd::SetOutgoingBot { own_bot, token })
}

impl CoreExpertView {
    /// Keep the Remote block in step with the store: rebind on a new anchor, settle what this
    /// window waited on, and ask for a snapshot when the block is on screen.
    ///
    /// Returns:
    ///     Whether anything the block draws changed.
    pub(super) fn sync_remote(&mut self, b: &Backend) -> bool {
        let anchor = self.anchor;
        let mut changed = false;
        if self.remote.core != anchor {
            self.remote.rebind(anchor);
            changed = true;
        }
        let Some(core) = anchor else {
            return changed;
        };
        let Some(entry) = b.session.store().core(core) else {
            return changed;
        };
        let rev = (entry.telegram_rev, entry.telegram_service_rev);
        if rev != self.remote.seen_rev {
            self.remote.seen_rev = rev;
            changed = true;
        }
        // A link that is down answers nothing: ask again once it is back.
        if entry.status != ConnStatus::Ready {
            self.remote.asked = None;
        }
        if let Some(awaiting) = self.remote.awaiting.clone()
            && entry.service.telegram.settled_since(awaiting.sent_before)
        {
            self.remote.awaiting = None;
            if let Some((ServiceAction::TelegramOutgoingBot, verdict)) =
                &entry.service.telegram.outcome
                && verdict_applied(verdict)
            {
                // The core holds the switch and the token now: show its state again, and do not
                // leave a secret it already took sitting in the box.
                self.remote.own_bot = None;
                self.remote.clear_token = true;
            }
            changed = true;
        }
        self.ask_remote(b, core);
        changed
    }

    /// Ask the anchor for its Telegram snapshot when the Remote block is on screen: once per open,
    /// per anchor and per stretch of a Ready link.
    fn ask_remote(&mut self, b: &Backend, core: CoreId) {
        let open = self.tab == ExpertTab::Special
            && self.special_section == SpecialSection::Remote
            && self.seeded() == Some(core);
        if !open || self.remote.asked == Some(core) {
            return;
        }
        let ready = b
            .session
            .store()
            .core(core)
            .is_some_and(|d| d.status == ConnStatus::Ready);
        if !ready {
            return;
        }
        self.remote.asked = Some(core);
        if let Err(error) = b.session.telegram_cmd(core, TelegramCmd::Refresh) {
            log::warn!("expert window could not ask core {core} for its Telegram state: {error}");
        }
    }

    /// The Remote block was opened (its section, or its tab): ask afresh.
    pub(super) fn remote_opened(&mut self, cx: &App) {
        self.remote.asked = None;
        let b = self.backend.read(cx);
        if let Some(core) = self.anchor {
            self.ask_remote(b, core);
        }
    }

    /// The anchor's Telegram snapshot, when the outgoing-bot actions may be used right now: a
    /// current snapshot over a Ready link, from a core that supports them.
    fn remote_live(&self, cx: &App) -> Option<(CoreId, std::sync::Arc<CoreTelegramState>)> {
        let core = self.seeded()?;
        let entry = self.backend.read(cx).session.store().core(core)?;
        let live = entry.status == ConnStatus::Ready && entry.telegram_fresh;
        let snapshot = entry.telegram.clone().filter(|s| s.settings_supported)?;
        live.then_some((core, snapshot))
    }

    /// Set the own-bot switch. Nothing is sent until the bot is saved.
    pub(super) fn remote_set_own_bot(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.remote.awaiting.is_some() || self.remote_live(cx).is_none() {
            return;
        }
        self.remote.own_bot = Some(on);
        self.remote.invalid = None;
        cx.notify();
    }

    /// Send the own-bot switch and, when one was typed, the new token.
    pub(super) fn remote_save_bot(&mut self, cx: &mut Context<Self>) {
        if self.remote.awaiting.is_some() {
            return;
        }
        let Some((core, snapshot)) = self.remote_live(cx) else {
            return;
        };
        let typed = self
            .remote
            .token
            .as_ref()
            .map(|t| t.read(cx).value().to_string())
            .unwrap_or_default();
        let own_bot = self.remote.own_bot_shown(Some(&snapshot));
        match bot_request(own_bot, &typed, snapshot.token_set) {
            Ok(cmd) => {
                self.remote.invalid = None;
                self.send_remote(core, cmd, cx);
            }
            Err(invalid) => self.remote.invalid = Some(invalid),
        }
        cx.notify();
    }

    /// Ask the core for the PIN that binds a channel.
    pub(super) fn remote_generate_pin(&mut self, cx: &mut Context<Self>) {
        self.remote_action(TelegramCmd::GenerateChannelPin, cx);
    }

    /// Unbind the channel the outgoing bot posts to.
    pub(super) fn remote_reset_channel(&mut self, cx: &mut Context<Self>) {
        self.remote_action(TelegramCmd::ResetChannel, cx);
    }

    /// Queue one of the block's one-shot actions, if the block is live and idle.
    fn remote_action(&mut self, cmd: TelegramCmd, cx: &mut Context<Self>) {
        if self.remote.awaiting.is_some() {
            return;
        }
        let Some((core, _)) = self.remote_live(cx) else {
            return;
        };
        self.send_remote(core, cmd, cx);
        cx.notify();
    }

    /// Queue one tracked action and start waiting on its verdict.
    fn send_remote(&mut self, core: CoreId, cmd: TelegramCmd, cx: &App) {
        let Some(action) = cmd.tracked_action() else {
            return;
        };
        let b = self.backend.read(cx);
        let sent_before = b
            .session
            .store()
            .core(core)
            .map_or(0, |d| d.service.telegram.sent);
        self.remote.sent_here = true;
        match b.session.telegram_cmd(core, cmd) {
            Ok(()) => {
                self.remote.queue_failed = false;
                self.remote.awaiting = Some(Awaiting {
                    action,
                    sent_before,
                    removed: None,
                });
            }
            Err(error) => {
                log::warn!(
                    "expert window could not queue a Telegram action for core {core}: {error}"
                );
                self.remote.queue_failed = true;
            }
        }
    }

    /// Build the masked token box while the Remote block is on screen, and empty it when the core
    /// took what it held.
    pub(super) fn build_remote_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.tab == ExpertTab::Special
            && self.special_section == SpecialSection::Remote
            && self.draft.is_some();
        if !open {
            return;
        }
        let token = self
            .remote
            .token
            .get_or_insert_with(|| cx.new(|c| MoonInputState::new(window, c).masked(true)))
            .clone();
        if std::mem::take(&mut self.remote.clear_token) {
            token.update(cx, |s, c| s.sync_value("", c));
        }
    }

    /// Drop the token box, e.g. when the block stops being drawn: a typed secret does not outlive
    /// the block it was typed into.
    pub(super) fn release_remote_inputs(&mut self) {
        self.needs_blur |= self.remote.token.is_some();
        self.remote.token = None;
    }
}

#[cfg(test)]
mod tests;
