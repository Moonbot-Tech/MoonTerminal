//! Deleting what the "Data by core" table selected: the confirmation and the writers' round trip.

use gpui::*;
use moon_ui::{MoonButton, MoonButtonVariant, MoonPalette, MoonWindowExt as _, h_flex};
use rust_i18n::t;

use super::super::super::{SettingsView, StatusMsg};
use super::{CoreCounts, CoreLine, DIALOG_ID, Group, Kinds, Line, WRITER_WAIT};
use crate::design;

impl SettingsView {
    /// Ask before deleting, naming the cores and what goes, with the counts.
    pub(super) fn core_data_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let kinds = self.storage.core_data.kinds;
        let lines = self.core_data_lines(cx);
        let chosen: Vec<CoreLine> = lines
            .into_iter()
            .filter_map(|l| match l {
                Line::Core(c)
                    if c.selectable()
                        && self.storage.core_data.selected.contains(&c.counts.uid) =>
                {
                    Some(c)
                }
                _ => None,
            })
            .collect();
        if chosen.is_empty() || !kinds.any() {
            return;
        }
        let summary = confirm_text(&chosen, kinds);
        let view = cx.entity().downgrade();
        let targets: Vec<u64> = chosen.iter().map(|c| c.counts.uid).collect();
        window.open_unique_moon_dialog(DIALOG_ID, cx, move |dialog, _window, cx| {
            let p = MoonPalette::active(cx);
            let view = view.clone();
            let targets = targets.clone();
            let summary = summary.clone();
            dialog
                .w(px(460.0))
                .close_button(true)
                .overlay(true)
                .overlay_closable(true)
                .bg(rgb(p.shell_high))
                .border_color(rgb(p.border))
                .rounded(design::r_container(cx))
                .text_color(rgb(p.text))
                .header(
                    div()
                        .w_full()
                        .py_2()
                        .border_b_1()
                        .border_color(rgb(p.border))
                        .font_family(design::ui_font())
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t!("storage.core_data_confirm_title").to_string()),
                )
                .content(move |content, _window, cx| {
                    let p = MoonPalette::active(cx);
                    content.child(
                        div()
                            .w_full()
                            .font_family(design::ui_font())
                            .text_size(design::t_body(cx))
                            .text_color(rgb(p.text))
                            .whitespace_normal()
                            .child(summary.clone()),
                    )
                })
                .footer(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .justify_end()
                        .child(
                            MoonButton::new("core-data-delete-no")
                                .outline()
                                .mono(false)
                                .label(format!("  {}  ", t!("dialogs.no")))
                                .on_click(|_, window, cx| window.close_dialog(cx))
                                .render(),
                        )
                        .child(
                            MoonButton::new("core-data-delete-yes")
                                .variant(MoonButtonVariant::Danger)
                                .mono(false)
                                .label(format!("  {}  ", t!("storage.core_data_delete_yes")))
                                .on_click(move |_, window, cx| {
                                    let targets = targets.clone();
                                    let _ = view.update(cx, |this, cx| {
                                        this.core_data_delete(&targets, kinds, cx);
                                    });
                                    window.close_dialog(cx);
                                })
                                .render(),
                        ),
                )
        });
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
        // Re-read at the moment of "Yes", not when the dialog opened: a catch-up may have started
        // or the core may have joined or left Connections meanwhile.
        let targets: Vec<(u64, Group)> = self
            .core_data_lines(cx)
            .into_iter()
            .filter_map(|l| match l {
                Line::Core(c) if c.selectable() && uids.contains(&c.counts.uid) => {
                    Some((c.counts.uid, c.group))
                }
                _ => None,
            })
            .collect();
        if targets.is_empty() {
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
            for &(uid, group) in &targets {
                if kinds.reports {
                    let server = b.config.servers.iter().find(|s| s.uid == uid);
                    // Only a core that replicates reports downloads them again; a core with the
                    // report feed off keeps an empty replica, as it asked for.
                    let resync = server
                        .filter(|s| s.feed.reports)
                        .and_then(|_| b.session.live_reports(uid));
                    report_jobs.push((uid, resync, group == Group::Archive));
                }
                // On this thread because the backend owns the store's connection here, like every
                // episode it inserts: 16 ms for the core with the most episodes (480) on a real
                // store.
                if kinds.warnings {
                    match b.warn_store.as_ref().map(|w| w.forget_core(uid)) {
                        Some(Ok(n)) => warnings_deleted += n,
                        Some(Err(e)) => warnings_failed = Some(e.to_string()),
                        None => {}
                    }
                }
            }
            b.reports.as_ref().map(|h| h.tx.clone())
        };
        let uids: Vec<u64> = targets.iter().map(|(uid, _)| *uid).collect();
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
            if kinds.strategies {
                match moon_core::strat_db::sink() {
                    None => failed.push(t!("storage.core_data_col_strategies").to_string()),
                    Some(sink) => {
                        for uid in &uids {
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
            if kinds.traces {
                match moon_core::db::order_traces::sink() {
                    None => failed.push(t!("storage.core_data_col_traces").to_string()),
                    Some(sink) => {
                        let waits: Vec<_> = uids
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
        out.push_str(&t!(
            "storage.core_data_confirm_reports",
            n = sum(|c| c.reports)
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
        out.push_str(&t!(
            "storage.core_data_confirm_traces",
            n = sum(|c| c.traces)
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
