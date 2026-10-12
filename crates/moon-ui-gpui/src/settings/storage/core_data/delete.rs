//! Deleting what the "Data by core" table selected: the confirmation and the writers' round trip.

use std::collections::BTreeSet;
use std::rc::Rc;

use gpui::*;
use rust_i18n::t;

use super::super::super::common::{Confirm, ConfirmBtn, ConfirmKind, confirm, count_label};
use super::super::super::{SettingsView, StatusMsg};
use super::{
    CoreCounts, CoreLine, Group, Kind, Kinds, Line, WRITER_WAIT, deletion_pairs, deletion_targets,
};

impl SettingsView {
    /// Ask before deleting. The shared confirm dialog closes itself, then the delete runs.
    ///
    /// Args:
    ///     window: Window the dialog opens over.
    ///     cx: Settings view context.
    pub(super) fn core_data_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let kinds = self.storage.core_data.kinds;
        let lines = self.core_data_lines(cx);
        let targets = deletion_targets(&lines, &self.storage.core_data.selected);
        if targets.is_empty() || !kinds.any() {
            return;
        }
        let wanted: BTreeSet<u64> = targets.iter().map(|target| target.uid).collect();
        let chosen: Vec<CoreLine> = lines
            .into_iter()
            .filter_map(|line| match line {
                Line::Core(core) if wanted.contains(&core.counts.uid) => Some(core),
                _ => None,
            })
            .collect();
        let summary = confirm_text(&chosen, kinds);
        let body = format!("{}\n\n{}", t!("storage.core_data.delete_body"), summary);
        let view = cx.entity().downgrade();
        let uids: Vec<u64> = targets.iter().map(|target| target.uid).collect();
        confirm(
            window,
            cx,
            Confirm {
                title: t!("storage.core_data.delete_title").to_string(),
                body,
                buttons: vec![
                    ConfirmBtn {
                        label: t!("dialogs.cancel").to_string(),
                        kind: ConfirmKind::Cancel,
                        on: Rc::new(|_, _| {}),
                    },
                    ConfirmBtn {
                        label: t!("storage.core_data_delete_yes").to_string(),
                        kind: ConfirmKind::DangerSolid,
                        on: Rc::new(move |_, app| {
                            let uids = uids.clone();
                            let _ = view.update(app, |this, cx| {
                                this.core_data_delete(&uids, kinds, cx);
                            });
                        }),
                    },
                ],
            },
        );
    }

    /// Delete the chosen stores for the chosen cores.
    ///
    /// Everything that needs the backend — the live report handles, the warnings store, which it
    /// owns on this thread — is taken here; the writers are then waited for off the UI thread.
    fn core_data_delete(&mut self, uids: &[u64], kinds: Kinds, cx: &mut Context<Self>) {
        // Checked before anything is touched: `storage_op` refuses while busy, and a delete that
        // ran half here and not at all there would report nothing.
        if self.storage.busy {
            self.status = Some((
                StatusMsg::Text(t!("storage.busy", op = t!("storage.op_core_delete")).to_string()),
                true,
            ));
            cx.notify();
            return;
        }
        // The request the test pins, re-read at "Yes": a catch-up that started while the dialog
        // was open drops that core.
        let lines = self.core_data_lines(cx);
        let wanted: BTreeSet<u64> = uids.iter().copied().collect();
        let pairs = deletion_pairs(&lines, &wanted, kinds);
        let archived: BTreeSet<u64> = deletion_targets(&lines, &wanted)
            .into_iter()
            .filter(|target| target.group == Group::Archive)
            .map(|target| target.uid)
            .collect();
        let of_kind = |kind: Kind| -> Vec<u64> {
            pairs
                .iter()
                .filter(|(_, k)| *k == kind)
                .map(|(uid, _)| *uid)
                .collect()
        };
        let mut uids: Vec<u64> = pairs.iter().map(|(uid, _)| *uid).collect();
        uids.dedup();
        let strategy_uids = of_kind(Kind::Strategies);
        let trace_uids = of_kind(Kind::Traces);
        if pairs.is_empty() {
            self.status = Some((
                StatusMsg::Text(t!("storage.core_data_nothing").to_string()),
                true,
            ));
            cx.notify();
            return;
        }
        // `(uid, live report handle, forget)`; the handle's type stays inferred — it is
        // moonproto's, which this crate reaches only through moon-core.
        let mut report_jobs = Vec::new();
        let mut warnings_deleted = 0usize;
        let mut warnings_failed: Option<String> = None;
        let report_sink = {
            let b = self.backend.read(cx);
            for uid in of_kind(Kind::Reports) {
                let server = b.config.servers.iter().find(|s| s.uid == uid);
                // Only a core that replicates reports downloads them again; a core with the
                // report feed off keeps an empty replica, as it asked for.
                let resync = server
                    .filter(|s| s.feed.reports)
                    .and_then(|_| b.session.live_reports(uid));
                report_jobs.push((uid, resync, archived.contains(&uid)));
            }
            for uid in of_kind(Kind::Warnings) {
                // On this thread because the backend owns the store's connection here, like every
                // episode it inserts: 16 ms for the core with the most episodes (480) on a real
                // store.
                match b.warn_store.as_ref().map(|w| w.forget_core(uid)) {
                    Some(Ok(n)) => warnings_deleted += n,
                    Some(Err(e)) => warnings_failed = Some(e.to_string()),
                    None => {}
                }
            }
            b.reports.as_ref().map(|h| h.tx.clone())
        };
        self.storage.core_data.selected.clear();
        self.storage.core_data.counts = None;
        self.storage_op(cx, "storage.op_core_delete", move || {
            let mut failed: Vec<String> = Vec::new();
            if let Some(e) = warnings_failed {
                failed.push(t!("storage.core_data_col_warnings").to_string() + ": " + &e);
            }
            if !report_jobs.is_empty() {
                match &report_sink {
                    None => failed.push(t!("storage.core_data_col_reports").to_string()),
                    Some(sink) => {
                        let mut waits = Vec::new();
                        for (core_uid, resync, forget) in report_jobs {
                            let (done, rx) = std::sync::mpsc::sync_channel(1);
                            sink.send(moon_core::db::DbMsg::ForgetCore {
                                core_uid,
                                resync,
                                forget,
                                done,
                            });
                            waits.push(rx);
                        }
                        if waits
                            .iter()
                            .any(|rx| rx.recv_timeout(WRITER_WAIT) != Ok(true))
                        {
                            failed.push(t!("storage.core_data_col_reports").to_string());
                        }
                    }
                }
            }
            if !strategy_uids.is_empty() {
                match moon_core::strat_db::sink() {
                    None => failed.push(t!("storage.core_data_col_strategies").to_string()),
                    Some(sink) => {
                        for uid in &strategy_uids {
                            let ok = sink.forget_core(*uid).is_some_and(|rx| {
                                matches!(rx.recv_timeout(WRITER_WAIT), Ok(Some(_)))
                            });
                            if !ok {
                                failed.push(t!("storage.core_data_col_strategies").to_string());
                                break;
                            }
                        }
                    }
                }
            }
            if !trace_uids.is_empty() {
                match moon_core::db::order_traces::sink() {
                    None => failed.push(t!("storage.core_data_col_traces").to_string()),
                    Some(sink) => {
                        let waits: Vec<_> = trace_uids
                            .iter()
                            .map(|uid| {
                                let (done, rx) = std::sync::mpsc::sync_channel(1);
                                sink.send(moon_core::db::order_traces::TraceDbMsg::ForgetCore {
                                    core_uid: *uid,
                                    done,
                                });
                                rx
                            })
                            .collect();
                        // A full queue drops the message and with it the sender, so a lost
                        // delete answers here as an error rather than as success.
                        if waits
                            .iter()
                            .any(|rx| rx.recv_timeout(WRITER_WAIT) != Ok(true))
                        {
                            failed.push(t!("storage.core_data_col_traces").to_string());
                        }
                    }
                }
            }
            if failed.is_empty() {
                Ok(Some(
                    t!(
                        "storage.core_data_deleted",
                        cores = uids.len(),
                        warnings = warnings_deleted
                    )
                    .to_string(),
                ))
            } else {
                Err(anyhow::anyhow!(
                    t!("storage.core_data_partly", what = failed.join(", ")).to_string()
                ))
            }
        });
    }
}

/// The confirmation text: what goes, for how many cores, and what comes back.
fn confirm_text(chosen: &[CoreLine], kinds: Kinds) -> String {
    let sum = |f: fn(&CoreCounts) -> u64| chosen.iter().map(|c| f(&c.counts)).sum::<u64>();
    let mut out = t!("storage.core_data_confirm_cores", n = chosen.len()).to_string();
    let names: Vec<String> = chosen
        .iter()
        .take(8)
        .map(|c| format!("{} #{}", c.name, c.counts.uid))
        .collect();
    out.push_str(&names.join(", "));
    if chosen.len() > 8 {
        out.push_str(&t!("storage.core_data_confirm_more", n = chosen.len() - 8));
    }
    out.push('\n');
    if kinds.reports {
        out.push('\n');
        out.push_str(&count_label(
            "storage.core_data_confirm_reports",
            sum(|c| c.reports),
        ));
        if chosen.iter().any(|c| c.group != Group::Archive) {
            out.push(' ');
            out.push_str(&t!("storage.core_data_confirm_redownload"));
        }
    }
    if kinds.strategies {
        out.push('\n');
        out.push_str(&t!(
            "storage.core_data_confirm_strategies",
            heads = sum(|c| c.strategies),
            versions = sum(|c| c.versions)
        ));
    }
    if kinds.traces {
        out.push('\n');
        out.push_str(&count_label(
            "storage.core_data_confirm_traces",
            sum(|c| c.traces),
        ));
    }
    if kinds.warnings {
        out.push('\n');
        out.push_str(&t!(
            "storage.core_data_confirm_warnings",
            n = sum(|c| c.warnings)
        ));
    }
    if kinds.strategies || kinds.traces || kinds.warnings {
        out.push_str("\n\n");
        out.push_str(&t!("storage.core_data_confirm_irreversible"));
    }
    out.push_str("\n\n");
    out.push_str(&t!("storage.core_data_confirm_untouched"));
    out
}
