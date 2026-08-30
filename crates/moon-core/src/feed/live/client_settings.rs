//! Serializes complete ClientSettings mutations and manual orders for one core.
//!
//! MoonProto sends ClientSettings as a singleton full snapshot. A later packet can replace an
//! earlier pending packet, so every producer must share this queue. Manual orders form barriers:
//! the order is released only after the core echoes the exact visible group settings, and a later
//! settings generation cannot overtake it.

use std::collections::VecDeque;

use moonproto::MoonClient;

use super::convert::{apply_client_settings_edit, client_settings_from_proto};
use crate::config::{GroupExitSettings, TakeProfitMode};
use crate::feed::{ClientSettingsEdit, trade};

/// One manual order waiting behind its group-settings generation.
#[derive(Clone, Debug)]
pub(super) struct ManualOrder {
    /// Target market on the selected core.
    pub market: String,
    /// Position side.
    pub short: bool,
    /// Entry price.
    pub price: f64,
    /// Base-currency quantity already converted from the visible USD equivalent.
    pub size: f64,
    /// Optional core-owned strategy.
    pub strategy_id: Option<u64>,
    /// Visible settings that must be confirmed before placement.
    pub exit: GroupExitSettings,
}

/// A targeted mutation of the complete ClientSettings snapshot.
#[derive(Clone, Debug)]
enum SettingsMutation {
    /// Existing targeted toolbar/core-settings edit.
    Edit(ClientSettingsEdit),
    /// Blacklist flag and text, formerly sent through an independent full-snapshot path.
    Blacklist { on: bool, text: String },
    /// Complete visible group exit state.
    GroupExit(GroupExitSettings),
}

/// One queue operation; an order is a serialization barrier between settings generations.
#[derive(Clone, Debug)]
enum SequenceOp {
    /// Idempotent settings mutation.
    Mutation(SettingsMutation),
    /// Manual order released only after every preceding mutation is echoed.
    Order(ManualOrder),
}

/// Pure next action selected from a retained settings snapshot.
enum SequenceAction {
    /// No work is currently possible or required.
    Idle,
    /// Send this complete snapshot and confirm the listed prefix mutation count on echo.
    Send {
        settings: moonproto::ClientSettingsCommand,
        mutation_count: usize,
    },
    /// Place one order, then continue planning against the same confirmed snapshot.
    Place(ManualOrder),
}

/// Per-core serializer for every complete ClientSettings write and gated manual order.
pub(in crate::feed) struct ClientSettingsSequence {
    queue: VecDeque<SequenceOp>,
    waiting_for_echo: bool,
    pending_confirmation: Option<(crate::feed::ClientSettings, usize)>,
}

impl ClientSettingsSequence {
    /// Create an empty serializer for a newly connected client.
    pub(in crate::feed) fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            waiting_for_echo: false,
            pending_confirmation: None,
        }
    }

    /// Retain queued work but forget connection-local send state before a reconnect attempt.
    pub(in crate::feed) fn prepare_reconnect(&mut self) {
        self.waiting_for_echo = false;
        self.pending_confirmation = None;
    }

    /// Queue a targeted ClientSettings edit without dropping it when no snapshot exists yet.
    pub(super) fn enqueue_edit(&mut self, edit: ClientSettingsEdit) {
        self.queue
            .push_back(SequenceOp::Mutation(SettingsMutation::Edit(edit)));
    }

    /// Queue a blacklist edit through the same full-snapshot serializer.
    pub(super) fn enqueue_blacklist(&mut self, on: bool, text: String) {
        self.queue
            .push_back(SequenceOp::Mutation(SettingsMutation::Blacklist {
                on,
                text,
            }));
    }

    /// Queue a proactive group-settings synchronization.
    pub(super) fn enqueue_group_exit(&mut self, exit: GroupExitSettings) {
        self.queue
            .push_back(SequenceOp::Mutation(SettingsMutation::GroupExit(exit)));
    }

    /// Queue an order behind its complete group-exit generation.
    pub(super) fn enqueue_order(&mut self, order: ManualOrder) {
        self.queue
            .push_back(SequenceOp::Mutation(SettingsMutation::GroupExit(
                order.exit,
            )));
        self.queue.push_back(SequenceOp::Order(order));
    }

    /// Record a successful full-snapshot send so later commands cannot overtake its echo.
    fn observe_send_success(
        &mut self,
        settings: &moonproto::ClientSettingsCommand,
        mutation_count: usize,
    ) {
        self.waiting_for_echo = true;
        self.pending_confirmation = Some((client_settings_from_proto(settings), mutation_count));
    }

    /// Whether every queued compact write has been echoed by the core.
    ///
    /// The safe-share sequence beside this one asks before sending: `build_shared_config` overlays
    /// the RETAINED compact snapshot, so a full-config packet built while a compact edit is still
    /// in flight carries that edit's pre-change values and reverts it on arrival.
    ///
    /// KNOWN LIMIT: this queue has no attempt cap, so a compact mutation the core never reflects
    /// keeps the gate shut for the rest of the connection and safe-share writes queue up behind it.
    /// Reverting a trading control the user just set is the worse failure of the two, which is why
    /// the gate is strict; a cap belongs here rather than in the caller.
    ///
    /// The gate is deliberately ONE-WAY. A compact write issued while a safe-share packet is still
    /// unechoed can revert the fields the two channels share (`g_take_profit`, `trailing_drop`,
    /// `vol_drop_level`, the blacklist), and the symmetric gate would fix that — but this queue also
    /// carries MANUAL ORDERS, and holding those behind a settings echo is a worse trade than a
    /// window of a few hundred milliseconds in which the toolbar and the settings popup are driven
    /// at once. Closing it properly means merging the two queues, not gating this one.
    pub(in crate::feed) fn is_idle(&self) -> bool {
        self.queue.is_empty() && !self.waiting_for_echo
    }

    /// Allow the next plan after a ClientSettingsUpdated echo.
    pub(super) fn observe_update(&mut self) {
        self.waiting_for_echo = false;
    }

    /// Drive queued settings and orders against the client's retained snapshot.
    ///
    /// Returns whether at least one order was submitted so the caller may publish a best-effort
    /// order snapshot immediately.
    pub(super) fn drive(&mut self, client: &MoonClient, server_id: u64) -> bool {
        let Some(settings) = client
            .snapshot()
            .and_then(|snapshot| snapshot.settings().client_settings.clone())
        else {
            return false;
        };

        let mut order_submitted = false;
        loop {
            match self.next_action(&settings) {
                SequenceAction::Idle => return order_submitted,
                SequenceAction::Send {
                    settings: next,
                    mutation_count,
                } => {
                    match client.settings().send(next.clone()) {
                        Ok(()) => {
                            self.observe_send_success(&next, mutation_count);
                            log::info!(
                                "core {} serialized client settings sent",
                                crate::feed::core_label(server_id)
                            );
                        }
                        Err(error) => {
                            log::warn!(
                                "core {} serialized client settings failed: {error}",
                                crate::feed::core_label(server_id)
                            );
                        }
                    }
                    return order_submitted;
                }
                SequenceAction::Place(order) => {
                    trade::place_order(
                        client,
                        server_id,
                        order.market,
                        order.short,
                        order.price,
                        order.size,
                        order.strategy_id,
                        order.exit.use_stop_market,
                    );
                    order_submitted = true;
                }
            }
        }
    }

    /// Select the next deterministic action and discard mutations already reflected by the core.
    fn next_action(&mut self, settings: &moonproto::ClientSettingsCommand) -> SequenceAction {
        if self.waiting_for_echo {
            return SequenceAction::Idle;
        }
        if let Some((expected, mutation_count)) = self.pending_confirmation.take() {
            if client_settings_from_proto(settings) == expected {
                for _ in 0..mutation_count {
                    if !matches!(self.queue.front(), Some(SequenceOp::Mutation(_))) {
                        break;
                    }
                    self.queue.pop_front();
                }
            }
        }
        loop {
            match self.queue.front() {
                Some(SequenceOp::Mutation(mutation)) if mutation_satisfied(settings, mutation) => {
                    self.queue.pop_front();
                }
                Some(SequenceOp::Mutation(_)) => {
                    let mut next = settings.clone();
                    let mut mutation_count = 0;
                    for op in &self.queue {
                        let SequenceOp::Mutation(mutation) = op else {
                            break;
                        };
                        apply_mutation(&mut next, mutation);
                        mutation_count += 1;
                    }
                    return SequenceAction::Send {
                        settings: next,
                        mutation_count,
                    };
                }
                Some(SequenceOp::Order(order)) => {
                    if client_settings_from_proto(settings).group_exit_settings() != order.exit {
                        return SequenceAction::Idle;
                    }
                    let Some(SequenceOp::Order(order)) = self.queue.pop_front() else {
                        unreachable!("front was checked as an order");
                    };
                    return SequenceAction::Place(order);
                }
                None => return SequenceAction::Idle,
            }
        }
    }
}

/// Apply one idempotent mutation to a complete retained settings snapshot.
fn apply_mutation(settings: &mut moonproto::ClientSettingsCommand, mutation: &SettingsMutation) {
    match mutation {
        SettingsMutation::Edit(edit) => apply_client_settings_edit(settings, *edit),
        SettingsMutation::Blacklist { on, text } => {
            settings.use_coins_black_list = *on;
            settings.coins_black_list_text.clone_from(text);
        }
        SettingsMutation::GroupExit(exit) => apply_group_exit_settings(settings, *exit),
    }
}

/// Return whether applying a mutation would leave all projected settings unchanged.
fn mutation_satisfied(
    settings: &moonproto::ClientSettingsCommand,
    mutation: &SettingsMutation,
) -> bool {
    let before = client_settings_from_proto(settings);
    let mut after = settings.clone();
    apply_mutation(&mut after, mutation);
    before == client_settings_from_proto(&after)
}

/// Patch every visible group exit field while preserving invisible core-owned settings.
fn apply_group_exit_settings(
    settings: &mut moonproto::ClientSettingsCommand,
    exit: GroupExitSettings,
) {
    match exit.take_profit_mode {
        TakeProfitMode::Scalp => {
            apply_client_settings_edit(
                settings,
                ClientSettingsEdit::ScalpTakeProfit(exit.take_profit_pct),
            );
        }
        TakeProfitMode::Normal => {
            apply_client_settings_edit(
                settings,
                ClientSettingsEdit::TakeProfit {
                    pct: exit.take_profit_pct,
                    extended: false,
                },
            );
        }
        TakeProfitMode::Extended => {
            apply_client_settings_edit(
                settings,
                ClientSettingsEdit::TakeProfit {
                    pct: exit.take_profit_pct,
                    extended: true,
                },
            );
        }
    }
    for (index, pct) in exit.fixed_sell_pcts.into_iter().enumerate() {
        apply_client_settings_edit(
            settings,
            ClientSettingsEdit::SetFixedSellPct {
                slot: index + 1,
                pct,
            },
        );
    }
    if let Some(slot) = exit.fixed_sell_slot {
        apply_client_settings_edit(settings, ClientSettingsEdit::SelectFixedSellSlot(slot));
    } else {
        apply_client_settings_edit(settings, ClientSettingsEdit::EngageMainTakeProfit);
    }
    apply_client_settings_edit(
        settings,
        ClientSettingsEdit::StopLossPct(exit.stop_loss_pct),
    );
    apply_client_settings_edit(
        settings,
        ClientSettingsEdit::PanicIfPriceDrop(exit.stop_loss_enabled),
    );
    apply_client_settings_edit(
        settings,
        ClientSettingsEdit::UseStopMarket(exit.use_stop_market),
    );
}

#[cfg(test)]
mod tests;
