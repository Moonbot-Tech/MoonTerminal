//! The Log tab's day pull: the selected core's daily trading logs on its server, listed and fetched.
//!
//! The live copy the terminal writes holds only the hours it was connected, so "yesterday" is
//! usually a few hours of it. The whole day is on the core's server; this menu lists the days that
//! are there and pulls the picked one into the terminal's log folder, where it joins the «Файл»
//! list as a file of its own (`moon_core::applog::server_day`). It rides the login the core's
//! server commands already use and adds no way in of its own: a server the terminal cannot enter
//! says so and offers the same «add a login» step the commands window does.
//!
//! The request, the SSH session and the file write all live in the backend's per-core command
//! worker (`crate::backend::core_cmds`); this module only asks, and looks again when the backend's
//! day revision moves.

use super::*;
use crate::backend::core_cmds::{
    CmdArgs, CmdError, CmdPhase, CmdRequest, DayLog, LogPullOutcome, core_host_port,
};
use moon_core::applog::server_day::server_file_name;
use moon_remote::core_cmds::Cmd;
use moon_ui::MoonTone;

/// The core whose server the day pull talks to.
struct ServerCore {
    /// Backend key of the core's command worker.
    uid: u64,
    /// Address the server login is stored under.
    host: String,
    /// Label the core's log files are named with.
    label: String,
}

/// One day in the menu.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct DayRow {
    pub(super) date: String,
    pub(super) bytes: u64,
    /// Whether this day is already in the terminal's log folder.
    pub(super) stored: bool,
    /// Whether this day is being pulled right now.
    pub(super) pulling: bool,
}

/// Why the server gave no answer about its days.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum DayFailure {
    /// The server could not be entered or did not answer in time.
    Server,
    /// The server answered, and no core listens on this port there.
    NoCore(u16),
    /// The pull was refused, with the worker's reason.
    Refused(String),
}

/// What the day menu shows for the current knowledge about a core's server.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum DayMenu {
    NoLogin,
    /// The first answer has not arrived yet.
    Asking,
    Failed(DayFailure),
    Days {
        rows: Vec<DayRow>,
        /// A day that was asked for and turned out not to be on the server.
        absent: Option<String>,
    },
}

/// This panel's side of the day pull: what it last looked at and what it is waiting for.
#[derive(Default)]
pub(super) struct DaySync {
    /// The day this panel asked for and has not seen the end of: core uid and date.
    awaiting: Option<(u64, String)>,
    /// Backend day revision this panel last looked at; `None` before the first look.
    rev: Option<u64>,
    /// Core on screen at the last look and how many days had been stored for it by then.
    saves: Option<(u64, u64)>,
}

/// Decide what the day menu shows.
///
/// Days already known stay listed while a new listing or a pull runs: emptying the menu under the
/// pointer for the length of an SSH round trip would only make the same rows jump back.
///
/// Args:
///     has_login: Whether the terminal holds a login for the core's server.
///     phase: Lifecycle of the day command for this core.
///     day_log: Days and the latest pull the backend knows of.
///     files: Names in the core's «Файл» list, to mark the days already stored.
///     label: Label the core's log files are named with.
///
/// Returns:
///     The menu's content.
pub(super) fn day_menu(
    has_login: bool,
    phase: Option<&CmdPhase>,
    day_log: &DayLog,
    files: &[String],
    label: &str,
) -> DayMenu {
    if !has_login {
        return DayMenu::NoLogin;
    }
    if let Some(CmdPhase::Failed(error)) = phase {
        return DayMenu::Failed(match error {
            CmdError::Ssh(_) | CmdError::Timeout => DayFailure::Server,
            CmdError::NoCore { port } => DayFailure::NoCore(*port),
            CmdError::Refused(reason) => DayFailure::Refused(reason.clone()),
            // The day pull is open to every located core, so this refusal cannot be raised for it.
            CmdError::NotOurs => DayFailure::Refused(String::new()),
        });
    }
    let Some(days) = day_log.days.as_ref() else {
        return DayMenu::Asking;
    };
    // The pull's own outcome, not the command's phase: a listing queued ahead of the pull finishes
    // first and reports the command done while the day is still on its way.
    let pull = day_log.pull.as_ref();
    let pulling = pull
        .filter(|pull| pull.outcome == LogPullOutcome::Running)
        .map(|pull| pull.date.as_str());
    DayMenu::Days {
        rows: days
            .iter()
            .map(|day| DayRow {
                stored: files.contains(&server_file_name(&day.date, label)),
                pulling: pulling == Some(day.date.as_str()),
                date: day.date.clone(),
                bytes: day.bytes,
            })
            .collect(),
        absent: pull
            .filter(|pull| pull.outcome == LogPullOutcome::Absent)
            .map(|pull| pull.date.clone()),
    }
}

/// A file size in megabytes, with one decimal below ten so a small day does not read as zero.
fn megabytes(bytes: u64) -> String {
    let megabytes = bytes as f64 / (1024.0 * 1024.0);
    if megabytes < 10.0 {
        format!("{megabytes:.1}")
    } else {
        format!("{megabytes:.0}")
    }
}

impl LogPanel {
    /// The configured core behind the effective source, when its address can carry server commands.
    ///
    /// `None` under an Auto workspace: Auto pins the panel to Live, so a pulled day could be
    /// fetched there and never opened.
    fn server_core(&self, b: &Backend) -> Option<ServerCore> {
        let (source, _, workspace_owned) = self.effective_selection(b);
        if workspace_owned {
            return None;
        }
        let LogSource::Core(id) = source else {
            return None;
        };
        let server = b.config.servers.iter().find(|server| server.id == id)?;
        let (host, _) = core_host_port(server)?;
        crate::settings::connections::server_commands_eligible(&host).then(|| ServerCore {
            uid: server.uid,
            host,
            label: applog::sanitize_label(&server.name),
        })
    }

    /// Queue one day command for the effective core, unless the same is already queued or under
    /// way: a listing behind any day command, a pull behind another pull. A pull may follow a
    /// listing still in flight — the menu shows the days it knew before — and waits its turn.
    ///
    /// Returns the core's uid when the request was queued.
    fn send_day_command(&mut self, args: CmdArgs, cx: &mut Context<Self>) -> Option<u64> {
        let backend = self.backend.clone();
        let core = self.server_core(backend.read(cx))?;
        let pull = matches!(args, CmdArgs::LogDay { .. });
        backend.update(cx, |b, bcx| {
            b.core_cmds_login_for(&core.host)?;
            if b.core_cmds_day_busy(core.uid, pull) {
                return None;
            }
            b.core_cmds_send(
                core.uid,
                CmdRequest::Run {
                    cmd: Cmd::LogDay,
                    args,
                },
            );
            bcx.notify();
            Some(core.uid)
        })
    }

    /// Ask the server which days it has; called when the menu opens.
    pub(super) fn ask_server_days(&mut self, cx: &mut Context<Self>) {
        self.send_day_command(CmdArgs::None, cx);
    }

    /// Pull one day of the selected core's log. The panel opens the file when it lands.
    pub(super) fn pull_server_day(&mut self, date: String, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let Some(label) = self.server_core(backend.read(cx)).map(|core| core.label) else {
            return;
        };
        let args = CmdArgs::LogDay {
            date: date.clone(),
            label,
        };
        if let Some(uid) = self.send_day_command(args, cx) {
            self.day_sync.awaiting = Some((uid, date));
        }
    }

    /// Pick up what changed about the day pull.
    ///
    /// Runs on every backend notification, and on all but a few of them does one thing: compares
    /// the backend's day revision with the one it last looked at. Only when that moved — a day
    /// command was queued, answered or lost, or a login was added, for any core — does it resolve
    /// its own core. Then a day this panel asked for is opened, a day another panel pulled over
    /// the file on screen is read again, and the «Файл» list is re-read — the last only when a day
    /// was stored for the core on screen, and nothing at all for a panel that shows no such core.
    ///
    /// Returns:
    ///     Whether the panel has to repaint.
    pub(super) fn sync_server_day(&mut self, cx: &mut Context<Self>) -> bool {
        let backend = self.backend.clone();
        let (landed, replaced, stored_anew, label) = {
            let b = backend.read(cx);
            let rev = b.core_cmds_day_rev();
            if self.day_sync.rev == Some(rev) {
                return false;
            }
            self.day_sync.rev = Some(rev);
            let core = self.server_core(b);
            let landed = self.settle_awaited(b, core.as_ref().map(|core| core.uid));
            // A day stored for the core on screen since the last look, by whichever panel.
            let day_log = core
                .as_ref()
                .and_then(|core| b.core_cmds_facts(core.uid))
                .map(|facts| &facts.day_log);
            let saves = core
                .as_ref()
                .map(|core| (core.uid, day_log.map_or(0, |day_log| day_log.saves)));
            let stored_anew = matches!(
                (self.day_sync.saves, saves),
                (Some((was_uid, was)), Some((uid, now))) if was_uid == uid && was != now
            );
            self.day_sync.saves = saves;
            let replaced = day_log
                .filter(|_| stored_anew)
                .and_then(|day_log| day_log.pull.as_ref())
                .and_then(|pull| match &pull.outcome {
                    LogPullOutcome::Saved { file } => Some(file.clone()),
                    _ => None,
                });
            (landed, replaced, stored_anew, core.map(|core| core.label))
        };
        let Some(label) = label else {
            // No server core on screen — Aggregate, Local, or an Auto workspace holding the panel —
            // so nothing here shows the day pull and nothing is repainted. A day may still land for
            // the core this panel returns to, so its list is read again when it does; dropping the
            // cache key costs no disk.
            self.available_files_label = None;
            return false;
        };
        if landed.is_some() || stored_anew {
            // The list was read before this day existed.
            self.available_files_label = None;
            self.refresh_available_files(&label);
        }
        if let Some(file) = landed {
            self.open_pulled_file(file, cx);
        } else if let Some(file) = replaced.filter(|file| self.file == LogFile::Named(file.clone()))
        {
            // Pulled by another panel over the file this one shows: its index points into the
            // file that was replaced.
            self.open_pulled_file(file, cx);
        }
        true
    }

    /// Settle the day this panel asked for, on whichever core it asked.
    ///
    /// Returns the stored file when the day landed for the core still on screen. A day that landed
    /// for a core the panel has since left is not opened — that file belongs to another source —
    /// and a pull that failed, or was lost with a replaced worker, is simply no longer awaited.
    fn settle_awaited(&mut self, b: &Backend, shown: Option<u64>) -> Option<String> {
        let (uid, date) = self.day_sync.awaiting.clone()?;
        // Still queued or running: an outcome read now would be the previous pull's.
        if b.core_cmds_day_busy(uid, true) {
            return None;
        }
        self.day_sync.awaiting = None;
        let pull = b.core_cmds_facts(uid)?.day_log.pull.as_ref()?;
        match &pull.outcome {
            LogPullOutcome::Saved { file } if pull.date == date && shown == Some(uid) => {
                Some(file.clone())
            }
            _ => None,
        }
    }

    /// Show a day that just landed, re-reading it when it replaced the file already on screen.
    fn open_pulled_file(&mut self, file: String, cx: &mut Context<Self>) {
        let target = LogFile::Named(file);
        if self.file != target {
            self.set_file(target, cx);
            return;
        }
        // Pulled again: the name is the same and the content is not.
        self.named_page.invalidate();
        self.reset_to_live();
        let backend = self.backend.clone();
        self.reload_rows(backend.read(cx), cx);
    }

    /// The «from the server» menu for a core whose address can carry server commands, or nothing.
    pub(super) fn server_day_combo(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let backend = self.backend.clone();
        let b = backend.read(cx);
        let core = self.server_core(b)?;
        let facts = b.core_cmds_facts(core.uid);
        let empty = DayLog::default();
        let menu = day_menu(
            b.core_cmds_login_for(&core.host).is_some(),
            facts.and_then(|facts| facts.phases.get(&Cmd::LogDay)),
            facts.map_or(&empty, |facts| &facts.day_log),
            &self.available_files,
            &core.label,
        );
        let pulling =
            matches!(&menu, DayMenu::Days { rows, .. } if rows.iter().any(|row| row.pulling));
        // Weak: these closures are kept by the popup, which the panel owns.
        let panel = cx.entity().downgrade();
        let items = day_menu_items(menu, core.host, backend.clone(), panel.clone());
        let label = if pulling {
            t!("log.server.pulling")
        } else {
            t!("log.server.button")
        };
        Some(
            MoonDropdown::new("log-server-day")
                .label(label.to_string())
                .trigger_caret(true)
                .trigger_variant(MoonButtonVariant::Soft)
                .trigger_size(MoonButtonSize::density(cx))
                .trigger_tooltip(t!("log.server.tip").to_string())
                .fit_menu_width(220.0, 460.0)
                .menu_max_height_ui(360.0)
                // The menu stays open through a pull, so its progress and result are read in place.
                .close_on_select(false)
                .on_open_change(move |open, _, app| {
                    if open {
                        let _ = panel.update(app, |this, cx| this.ask_server_days(cx));
                    }
                })
                .items(items)
                .into_any_element(),
        )
    }
}

/// Build the menu rows for one state of the day pull.
fn day_menu_items(
    menu: DayMenu,
    host: String,
    backend: Entity<Backend>,
    panel: WeakEntity<LogPanel>,
) -> Vec<MoonMenuItem> {
    let ask_again = |key: &'static str| {
        let panel = panel.clone();
        MoonMenuItem::with_key(key, t!("core_cmds.common.retry_again").to_string())
            .closes_menu(false)
            .on_click(move |_, _, app| {
                let _ = panel.update(app, |this, cx| this.ask_server_days(cx));
            })
    };
    match menu {
        DayMenu::NoLogin => vec![
            MoonMenuItem::label(t!("core_cmds.error.no_login").to_string()),
            MoonMenuItem::with_key("lsd-login", t!("core_cmds.error.add_login").to_string())
                .tone(MoonTone::Accent)
                .closes_menu(true)
                .on_click(move |_, window, app| {
                    crate::connect_wizard::open_login(
                        backend.clone(),
                        Some(window.window_handle()),
                        window.display(app).map(|display| display.id()),
                        host.clone(),
                        app,
                    );
                }),
        ],
        DayMenu::Asking => vec![MoonMenuItem::label(t!("log.server.asking").to_string())],
        DayMenu::Failed(failure) => vec![
            MoonMenuItem::label(match failure {
                DayFailure::Server => t!("core_cmds.error.ssh").to_string(),
                DayFailure::NoCore(port) => t!("core_cmds.error.no_core", port = port).to_string(),
                DayFailure::Refused(reason) => {
                    t!("core_cmds.error.failed", reason = reason).to_string()
                }
            }),
            ask_again("lsd-retry"),
        ],
        DayMenu::Days { rows, absent } => {
            let busy = rows.iter().any(|row| row.pulling);
            let mut items = Vec::with_capacity(rows.len() + 2);
            if let Some(date) = absent {
                items.push(MoonMenuItem::label(
                    t!("log.server.absent", date = date).to_string(),
                ));
            }
            if rows.is_empty() {
                items.push(MoonMenuItem::label(t!("log.server.none").to_string()));
            }
            for row in rows {
                let size = t!("core_cmds.status.memory_mb", mb = megabytes(row.bytes)).to_string();
                let note = if row.pulling {
                    t!("log.server.pulling").to_string()
                } else if row.stored {
                    format!("{} · {size}", t!("log.server.stored"))
                } else {
                    size
                };
                let panel = panel.clone();
                let date = row.date.clone();
                items.push(
                    MoonMenuItem::with_key(format!("lsd-{}", row.date), row.date)
                        .right_label(note)
                        .checked(row.stored)
                        .disabled(busy)
                        .closes_menu(false)
                        .on_click(move |_, _, app| {
                            let date = date.clone();
                            let _ = panel.update(app, |this, cx| this.pull_server_day(date, cx));
                        }),
                );
            }
            items
        }
    }
}

#[cfg(test)]
mod tests;
