//! Capture the window's full deleted-strategy batch independently of tree search and expansion.

use std::collections::HashMap;

use gpui::{Context, Window};
use moon_core::{session::CoreId, strat_db::stats::HeadRow};
use rust_i18n::t;

use super::{StrategiesView, tree::ui::TreeOp};

/// Exact per-core history purges named by one confirmation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ForgetPlan {
    pub(super) targets: Vec<(CoreId, Vec<u64>)>,
}

impl ForgetPlan {
    /// Capture deleted ids for the tree's core roster, including collapsed and search-hidden roots.
    /// Empty cores are omitted so the confirmation counts only cores with history to forget.
    pub(super) fn capture(cores: &[CoreId], deleted: &HashMap<CoreId, Vec<HeadRow>>) -> Self {
        Self {
            targets: cores
                .iter()
                .filter_map(|core| {
                    let rows = deleted.get(core)?;
                    (!rows.is_empty()).then(|| {
                        (
                            *core,
                            rows.iter().map(|row| row.strategy_id as u64).collect(),
                        )
                    })
                })
                .collect(),
        }
    }

    /// Return the full deleted count shown by the menu and confirmation.
    pub(super) fn count(&self) -> usize {
        self.targets.iter().map(|(_, ids)| ids.len()).sum()
    }

    /// Offer a second action only when it reaches more than the existing per-core action.
    pub(super) fn available(&self) -> bool {
        self.targets.len() > 1
    }
}

impl StrategiesView {
    /// Confirm the captured local-history batch once, reusing the per-core dialog and erase path.
    pub(super) fn request_forget_all(
        &mut self,
        plan: ForgetPlan,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !plan.available() {
            return;
        }
        let label = t!(
            "strat.count_strategies_all_cores",
            n = plan.count(),
            cores = plan.targets.len()
        )
        .to_string();
        self.op = Some(TreeOp::ConfirmForget {
            targets: plan.targets,
            label,
        });
        self.open_op_dialog(window, cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests;
