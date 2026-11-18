//! Group-window status bar with glanceable connection, market/render, resource, and action groups.

use gpui::*;
use rust_i18n::t;

use moon_ui::{MoonPalette, MoonStatusBar, MoonStatusIndicator, MoonStatusItem, MoonTooltipView};

use moon_core::config::{AppConfig, TransportVersion};
use moon_core::feed::{ConnFault, ConnStatus, CoreStartupStatus, ReportSyncProgress};
use moon_core::metrics::MetricsSnapshot;
use moon_core::session::{ConnSummary, CoreId, CoreStore, LicenseSummary, store::CoreData};
use moon_core::update::BuildIdentity;
use std::borrow::Cow;
use std::collections::HashMap;
use std::rc::Rc;

use crate::design;

use super::Shell;
use super::fleet_model::FleetModelKey;

/// How long a report catch-up must run before the status bar mentions it, ms.
const REPORT_SYNC_SHOW_AFTER_MS: i64 = 2_000;

/// Borrowed inputs to one group window's connection and license presentation.
pub(super) struct StatusInputs<'a> {
    pub(super) config: &'a AppConfig,
    pub(super) store: &'a CoreStore,
    pub(super) group: &'a str,
    pub(super) locale: &'a str,
    pub(super) now_ms: i64,
}

/// Visible connection facts and the catch-up threshold for one session.
#[derive(PartialEq, Eq)]
struct StatusSession<'a> {
    id: CoreId,
    name: Cow<'a, str>,
    status: Option<Cow<'a, ConnStatus>>,
    fault: Option<Cow<'a, ConnFault>>,
    startup: Option<CoreStartupStatus>,
    sync: Option<(ReportSyncProgress, bool)>,
    paid_license: Option<bool>,
}

impl<'a> StatusSession<'a> {
    /// Only ready cores contribute catch-up progress, and only down cores need diagnosis.
    fn read(id: CoreId, name: &'a str, core: Option<&'a CoreData>, now_ms: i64) -> Self {
        let ready = core.is_some_and(|core| core.status == ConnStatus::Ready);
        let down = core.filter(|_| !ready);
        Self {
            id,
            name: Cow::Borrowed(name),
            status: core.map(|core| Cow::Borrowed(&core.status)),
            fault: down.and_then(|core| core.fault.as_ref().map(Cow::Borrowed)),
            startup: down.map(|core| core.startup),
            sync: core
                .filter(|_| ready)
                .and_then(|core| core.report_sync)
                .map(|sync| (sync, sync_visible(sync, now_ms))),
            paid_license: core
                .and_then(|core| core.license)
                .map(|license| license.paid_version),
        }
    }

    /// Retain an owned copy only on a miss.
    fn into_owned(self) -> StatusSession<'static> {
        StatusSession {
            id: self.id,
            name: Cow::Owned(self.name.into_owned()),
            status: self.status.map(|status| Cow::Owned(status.into_owned())),
            fault: self.fault.map(|fault| Cow::Owned(fault.into_owned())),
            startup: self.startup,
            sync: self.sync,
            paid_license: self.paid_license,
        }
    }
}

/// The precise wall-clock transition at which a persistent catch-up first becomes visible.
fn sync_visible(progress: ReportSyncProgress, now_ms: i64) -> bool {
    now_ms.saturating_sub(progress.started_ms) >= REPORT_SYNC_SHOW_AFTER_MS
}

/// Stable status fields shared by compact and regular layouts.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct StatusModel {
    pub(super) ready: usize,
    pub(super) total: usize,
    pub(super) any_failed: bool,
    pub(super) down_text: SharedString,
    pub(super) sync_value: Option<SharedString>,
    pub(super) license_total: usize,
    pub(super) license_known: usize,
    pub(super) license_paid: usize,
    pub(super) license_free: usize,
}

impl StatusModel {
    /// Rank and diagnose the snapshot, indexing transport evidence once for the whole fleet.
    pub(super) fn build(
        input: &StatusInputs<'_>,
        mut conn: ConnSummary,
        license: LicenseSummary,
    ) -> Self {
        // The tooltip follows the same canonical ordering as the header core list.
        super::workspace::sort_status_lists(
            &mut conn.down,
            &mut conn.report_sync,
            |row| row.id,
            |row| row.id,
            || moon_core::session::core_order::CoreOrder::new(input.config),
        );
        let mode_suggestions = if conn.down.is_empty() {
            HashMap::new()
        } else {
            let index = crate::conn_diag::FleetModeIndex::new(&input.config.servers, |id| {
                input
                    .store
                    .core(id)
                    .is_some_and(|core| core.status == ConnStatus::Ready)
            });
            conn.down
                .iter()
                .map(|row| (row.id, index.suggestion(row.id)))
                .collect()
        };
        let (down_text, sync_value) =
            connection_status_text(&conn, &mode_suggestions, input.now_ms);
        Self {
            total: conn.total,
            ready: conn.ready,
            any_failed: conn
                .down
                .iter()
                .any(|row| matches!(row.status, ConnStatus::Failed(_) | ConnStatus::Disconnected)),
            down_text: down_text.into(),
            sync_value: sync_value.map(Into::into),
            license_total: license.total,
            license_known: license.known,
            license_paid: license.paid,
            license_free: license.free,
        }
    }
}

/// Per-window memo. Validate borrowed source facts even when refresh bypasses Backend observers.
#[derive(Default)]
pub(super) struct StatusMemo(Option<StatusCache>);

/// The snapshot that produced one memoized status model.
struct StatusCache {
    group: String,
    locale: String,
    fleet: FleetModelKey,
    sessions: Vec<StatusSession<'static>>,
    model: Rc<StatusModel>,
}

impl StatusMemo {
    /// Reuse formatted diagnostics for equal inputs, including elapsed catch-up visibility.
    pub(super) fn resolve<'a>(
        &mut self,
        input: &StatusInputs<'a>,
        sessions: impl Iterator<Item = (CoreId, &'a str)> + Clone,
        build: impl FnOnce() -> StatusModel,
    ) -> Rc<StatusModel> {
        let facts = sessions
            .map(|(id, name)| StatusSession::read(id, name, input.store.core(id), input.now_ms));
        let mut current = facts.clone();
        if let Some(cache) = &self.0
            && cache.group == input.group
            && cache.locale == input.locale
            && cache.fleet.matches(input.config, input.store)
            && cache
                .sessions
                .iter()
                .all(|old| current.next().is_some_and(|new| old == &new))
            && current.next().is_none()
        {
            return cache.model.clone();
        }
        let model = Rc::new(build());
        self.0 = Some(StatusCache {
            group: input.group.to_owned(),
            locale: input.locale.to_owned(),
            fleet: FleetModelKey::capture(input.config, input.store),
            sessions: facts.map(StatusSession::into_owned).collect(),
            model: model.clone(),
        });
        model
    }
}

/// Format the connection diagnostics and report catch-up indicator at the supplied time.
fn connection_status_text(
    conn: &ConnSummary,
    mode_suggestions: &HashMap<CoreId, Option<TransportVersion>>,
    now_ms: i64,
) -> (String, Option<String>) {
    // Include only non-ready cores in the tooltip, each as name, reason and NEXT STEP.
    //
    // This is the surface a user who has opened no panel at all reads first, so it carries the
    // action and not just the complaint. It used to print the raw `ConnStatus` payload: an
    // English stage name such as "connected, init..." and a MoonProto error string, both built
    // in a crate that cannot translate them. The verdict replaces both, and the two remaining
    // arms are the states that genuinely carry no further evidence.
    let down_text: String = conn
        .down
        .iter()
        .map(|row| {
            let reason =
                match moon_core::feed::diagnose(&row.status, row.fault.as_ref(), &row.startup) {
                    Some(d) => crate::conn_diag::fault_line(
                        &d,
                        mode_suggestions.get(&row.id).copied().flatten(),
                    ),
                    None => match row.status {
                        ConnStatus::Disconnected => t!("status.disconnected").to_string(),
                        _ => t!("status.connecting").to_string(),
                    },
                };
            format!("{}: {reason}", row.name)
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Report history still arriving (#665) — without this nothing tells a user that the Report
    // is short because it is still downloading. A catch-up that ends within a couple of seconds
    // is the steady state on every connect and would only blink the bar.
    let syncing: Vec<_> = conn
        .report_sync
        .iter()
        .filter(|row| sync_visible(row.progress, now_ms))
        .collect();
    let percent_text = |done: i64, span: i64| {
        if span > 0 {
            format!("{}%", i128::from(done) * 100 / i128::from(span))
        } else {
            "…".to_string()
        }
    };
    let sync_value = (!syncing.is_empty()).then(|| {
        let (done, span) = syncing
            .iter()
            .filter_map(|row| row.progress.done_and_span())
            .fold((0i64, 0i64), |(d, s), (rd, rs)| {
                (d.saturating_add(rd), s.saturating_add(rs))
            });
        let percent = percent_text(done, span);
        if syncing.len() > 1 {
            format!("{} · {percent}", syncing.len())
        } else {
            percent
        }
    });
    let sync_text = (!syncing.is_empty()).then(|| {
        let mut text = t!("status.reports_sync_tip").to_string();
        for row in &syncing {
            let (done, span) = row.progress.done_and_span().unwrap_or((0, 0));
            text.push('\n');
            text.push_str(&t!(
                "status.reports_sync_row",
                name = row.name.as_str(),
                percent = percent_text(done, span),
                rows = row.progress.rows
            ));
        }
        text
    });
    let down_text = [Some(down_text).filter(|t| !t.is_empty()), sync_text]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n\n");
    (down_text, sync_value)
}

impl Shell {
    /// Build the lower status bar as three semantic groups plus right-aligned actions.
    ///
    /// The left connection indicator is green when every core is ready, red when any core failed
    /// or disconnected, and amber otherwise. Its tooltip lists non-ready cores. The row preserves
    /// license, book/FPS, CPU/GPU, current RAM, and five-second RAM-delta telemetry while stronger
    /// dividers separate those concerns.
    ///
    /// Args:
    ///     status: Memoized connection and license facts for this group window.
    ///     snap: Current process and system resource telemetry.
    ///     book_levels: Order-book level count for the active Main chart.
    ///     fps: Smoothed shell render rate.
    ///     chrome_width: Current window width used to select the compact narrow layout.
    ///     cx: Application context used for theme tokens, text measurement, and actions.
    ///
    /// Returns:
    ///     The complete scaled status row with connection tooltip and clickable right actions.
    pub(super) fn status_bar(
        &self,
        status: &StatusModel,
        snap: MetricsSnapshot,
        book_levels: usize,
        fps: f32,
        chrome_width: f32,
        cx: &App,
    ) -> impl IntoElement {
        // A picture of the interface should not differ from the last one because of the machine's load.
        #[cfg(uidoc)]
        let (snap, fps) = crate::uidoc::steady_status(snap, fps);
        let all_ok = status.total > 0 && status.ready == status.total;
        let p = MoonPalette::active(cx);
        let badge_col = if all_ok {
            p.green
        } else if status.any_failed {
            p.red
        } else {
            p.amber
        };

        // The caption is translated WHOLE, colon included (`%{value}`), rather than glued together
        // as "label + ': ' + tail": punctuation and word order are the translator's business. The
        // tail itself is "OK" or a pair of numbers, identical in every locale.
        let connection_value = if all_ok {
            "OK".to_string()
        } else {
            format!("{}/{}", status.ready, status.total)
        };
        let status_text = t!("status.connection", value = connection_value.clone()).to_string();
        // PRO/FREE are plan names, on the deliberately-untranslated list (locales/README.md).
        let (license_value, license_color) =
            if status.license_total == 0 || status.license_known == 0 {
                ("…".to_string(), p.text_muted)
            } else if status.license_known < status.license_total {
                (
                    format!("{}/{}", status.license_known, status.license_total),
                    p.amber,
                )
            } else if status.license_paid == status.license_total {
                ("PRO".to_string(), p.green)
            } else if status.license_free == status.license_total {
                ("FREE".to_string(), p.amber)
            } else {
                (
                    format!("PRO {}/{}", status.license_paid, status.license_total),
                    p.amber,
                )
            };
        let license_text = t!("status.license", value = license_value.clone()).to_string();

        // At the supported 520px minimum, full localized captions cannot coexist with every live
        // metric and both right actions. Compact only the captions and spacing; every value remains
        // visible, while the host tooltip expands the app/system CPU meaning and exact units.
        let compact = chrome_width < design::font_w(cx, 880.0).max(design::ui_value(cx, 880.0));
        let status_gap = if compact { 4.0 } else { 8.0 };
        let inner_gap = if compact { 2.0 } else { 5.0 };
        let value_gap = if compact { 4.0 } else { 8.0 };
        let group_gap = if compact { 5.0 } else { 10.0 };
        let tooltip_text = if compact {
            let mut text = format!(
                "{status_text}\n{license_text}\nBOOK {book_levels} · FPS {fps:.0}\n\
                 CPU APP/SYS {:.1}% / {:.1}% · GPU {:.1}%\n\
                 RAM {:.1} MiB · Δ5s {:+.1} MiB",
                snap.cpu_process, snap.cpu_system, snap.gpu_process, snap.mem_mb, snap.mem_delta_mb
            );
            if !status.down_text.is_empty() {
                text.push_str("\n\n");
                text.push_str(status.down_text.as_ref());
            }
            Some(SharedString::from(text))
        } else if status.down_text.is_empty() {
            None
        } else {
            Some(status.down_text.clone())
        };

        let mut right_items = Vec::new();
        #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
        {
            let backend = self.backend.clone();
            let group = self.group.clone();
            right_items.extend([
                MoonStatusItem::new("debug")
                    .id("debug-status-open")
                    .color(p.amber)
                    .gap_after(8.0)
                    .tooltip("debug")
                    .on_click(move |_, window, cx| {
                        crate::diagnostics::debug_window::open_debug_perf_window(
                            cx,
                            backend.clone(),
                            group.clone(),
                            Some(window.window_handle()),
                        )
                    }),
                MoonStatusItem::group_separator().gap_after(8.0),
            ]);
        }
        // The running build's own version, immediately left of the moonbot.pro link.
        //
        // It reads the SAME embedded baseline the updater compares against
        // (`update.rs:start_polling`), so what the user sees here is exactly what decides whether
        // the header offers an update — a separate source could disagree with the button beside
        // it. `ReleaseVersion`'s Display owns the `vMAJOR.MINOR.PATCH` form; a build with no
        // canonical tag reachable from HEAD embeds `unknown`, which fails to parse and is shown
        // as `dev` rather than as a version the updater never trusted.
        //
        // Kept to its bare value in both layouts: the footer degrades by clipping a
        // priority-ordered tail, so a caption here would cost `moonbot.pro` its place long
        // before it earned one. The build line the tooltip carries is the same triple
        // `startup.rs` logs at boot, which is what a tester is asked for.
        let release_base = option_env!("MOONTERMINAL_RELEASE_BASE").unwrap_or("unknown");
        let version_label = match BuildIdentity::from_release_base(release_base).baseline() {
            Some(version) => version.to_string(),
            None => "dev".to_string(),
        };
        // Every release changes this figure, and the status row stands in nearly every picture.
        #[cfg(uidoc)]
        let version_label = crate::uidoc::steady_version(version_label);
        // The tooltip carries the RAW `release_base`, not the rendered label, so its three facts
        // are byte-identical to the boot line in `startup.rs`. The two genuinely differ: a legacy
        // two-component tag parses with patch 0 (`release.rs:ReleaseVersion::parse`) and renders
        // as `v0.5.0`, and an untagged build embeds `unknown` and renders as `dev`. The short
        // label is for glancing; the tooltip is what a tester is asked to quote back, so it must
        // not normalize the fact away.
        let version_tooltip = t!(
            "status.version_tooltip",
            release = release_base,
            build = option_env!("MOONTERMINAL_GIT_REV").unwrap_or("unknown"),
            moonui = option_env!("MOONUI_GIT_REV").unwrap_or("unknown")
        )
        .to_string();
        // Compact drops the group rule and takes the inner gap, exactly as the left groups do.
        // The right region is `overflow_hidden` with no `flex_none` in the pinned MoonUI
        // (`moon-ui-components/src/status_bar.rs`), so its trailing `moonbot.pro` is what a
        // clipped bar loses first — every unit of width this readout does not spend is width the
        // link keeps. Hence a bare value, no caption, and no rule at the width where it matters.
        right_items.push(
            MoonStatusItem::new(version_label)
                .id("terminal-version")
                .color(p.text_soft)
                .gap_after(if compact { inner_gap } else { group_gap })
                .tooltip(version_tooltip),
        );
        if !compact {
            right_items.push(MoonStatusItem::group_separator().gap_after(group_gap));
        }
        right_items.push(
            MoonStatusItem::new("moonbot.pro")
                .id("moonbot-link")
                .color(p.blue)
                .gap_after(0.0)
                .tooltip("moonbot.pro")
                .on_click(|_, _window, cx| cx.open_url("https://moonbot.pro")),
        );

        let mut items = vec![
            MoonStatusItem::new(if compact {
                connection_value
            } else {
                status_text
            })
            .color(badge_col)
            .weight(600.0)
            .gap_after(status_gap),
        ];
        if let Some(value) = &status.sync_value {
            items.push(MoonStatusItem::separator().gap_after(status_gap));
            items.push(
                MoonStatusItem::new(if compact {
                    format!("↓{value}")
                } else {
                    t!("status.reports_sync", value = value).to_string()
                })
                .color(p.amber)
                .gap_after(status_gap),
            );
        }
        items.extend([
            MoonStatusItem::separator().gap_after(status_gap),
            MoonStatusItem::new(if compact { license_value } else { license_text })
                .color(license_color)
                .weight(600.0)
                .gap_after(group_gap),
            MoonStatusItem::group_separator().gap_after(group_gap),
            MoonStatusItem::new("BOOK")
                .color(p.text_muted)
                .gap_after(inner_gap),
            MoonStatusItem::new(format!("{book_levels}"))
                .color(p.text_soft)
                .gap_after(value_gap),
            MoonStatusItem::new("FPS")
                .color(p.text_muted)
                .gap_after(inner_gap),
            MoonStatusItem::new(format!("{fps:.0}"))
                .color(p.text_soft)
                .gap_after(group_gap),
            MoonStatusItem::group_separator().gap_after(group_gap),
            MoonStatusItem::new(if compact { "CPU" } else { "CPU APP/SYS" })
                .color(p.text_muted)
                .gap_after(inner_gap),
            MoonStatusItem::new(if compact {
                format!("{:.0}/{:.0}%", snap.cpu_process, snap.cpu_system)
            } else {
                format!("{:.0}% / {:.0}%", snap.cpu_process, snap.cpu_system)
            })
            .color(p.text_soft)
            .gap_after(value_gap),
            MoonStatusItem::new("GPU")
                .color(p.text_muted)
                .gap_after(inner_gap),
            MoonStatusItem::new(format!("{:.0}%", snap.gpu_process))
                .color(p.text_soft)
                .gap_after(value_gap),
            MoonStatusItem::new("RAM")
                .color(p.text_muted)
                .gap_after(inner_gap),
            MoonStatusItem::new(if compact {
                format!("{:.1}GiB", snap.mem_mb / 1024.0)
            } else {
                format!("{:.1} GiB", snap.mem_mb / 1024.0)
            })
            .color(p.text_soft)
            .gap_after(value_gap),
            MoonStatusItem::new(if compact {
                format!("Δ5s{:+.1}MiB", snap.mem_delta_mb)
            } else {
                format!("Δ5s {:+.1} MiB", snap.mem_delta_mb)
            })
            .color(p.text_muted)
            .gap_after(0.0),
        ]);

        let mut host = div()
            .id("status-bar-host")
            .w_full()
            // Mirror MoonStatusBar's default 22/13/4.5 fit triple so the aggregate telemetry
            // tooltip owns the same full-height region as the scaled status row.
            .h(design::fit_h_px(cx, design::STATUS_H, 13.0, 4.5))
            .child(
                MoonStatusBar::new("status-bar")
                    .indicator(
                        MoonStatusIndicator::new(badge_col)
                            .alpha(0.685)
                            .size(6.0)
                            .glow(8.0, 0.30),
                    )
                    .items(items)
                    .right_items(right_items)
                    .render(),
            );
        if let Some(tooltip_text) = tooltip_text {
            host = host.tooltip(move |_window, cx| {
                cx.new(|_| MoonTooltipView::new(tooltip_text.clone()).max_width(420.0))
                    .into()
            });
        }
        host
    }
}
