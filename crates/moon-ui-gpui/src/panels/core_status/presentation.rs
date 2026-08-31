//! Shared connection and metric presentation rules for both Core Status modes.

use moon_core::feed::{ConnStatus, Diagnosis};
use moon_core::session::core_update::{CoreUpdateOutcome, CoreUpdatePhase, UpdateFailure};
use moon_ui::MoonPalette;
use rust_i18n::t;

use super::model::{ApiKeyState, GroupUpdate, GroupVersion, TzOffsetGroup};
use super::time_offset::{TzOffsetCell, tz_offset_cell_text};
use crate::backend::core_warn::LatencySeverity;
use crate::conn_diag::fault_short;

/// Visual metadata shared by Flat and By IP connection rows.
pub(super) struct ConnectionPresentation {
    /// Localized lifecycle label, including stage or failure details.
    pub(super) label: String,
}

/// Resolve one connection state into its shared lifecycle label.
///
/// The verdict, when there is one, REPLACES the raw payload the two in-progress states used to
/// interpolate. Those payloads are built in `moon-core`, which cannot localize: `Stage` carried an
/// English phrase such as "connected, init..." and `Failed` carried MoonProto's own error text, and
/// both reached the screen untranslated. The verdict is the same fact, classified and worded here.
///
/// Args:
///     status: Latest core connection state.
///     diag: The verdict for this core, when `moon_core::feed::diagnose` returned one.
///
/// Returns:
///     A consistent label for either Core Status presentation.
pub(super) fn connection_presentation(
    status: &ConnStatus,
    diag: Option<&Diagnosis>,
) -> ConnectionPresentation {
    let label = match (status, diag) {
        (ConnStatus::Ready, _) => t!("conn.status.ready").to_string(),
        (ConnStatus::Failed(_), Some(d)) => {
            t!("conn.status.failed", err = fault_short(&d.class)).to_string()
        }
        (_, Some(d)) => t!("conn.status.stage", stage = fault_short(&d.class)).to_string(),
        (ConnStatus::Connecting, None) => t!("conn.status.connecting").to_string(),
        // No verdict behind an in-progress or failed state: there is nothing classified to show,
        // so fall back to the coarse phase rather than to the untranslatable payload.
        (ConnStatus::Stage(_), None) => t!("conn.status.connecting").to_string(),
        (ConnStatus::Failed(_), None) => t!("conn.status.failed", err = "-").to_string(),
        (ConnStatus::Disconnected, None) => t!("conn.status.disconnected").to_string(),
    };
    ConnectionPresentation { label }
}

/// Return the localized connection label for reuse outside the Core Status panel.
///
/// Args:
///     status: Latest core connection state.
///     diag: The verdict for this core, when one was derived.
///
/// Returns:
///     The same lifecycle label rendered by the Core Status status column.
pub(crate) fn connection_status_text(status: &ConnStatus, diag: Option<&Diagnosis>) -> String {
    connection_presentation(status, diag).label
}

/// Format an optional integer percentage.
///
/// Args:
///     value: Integer percentage from MoonProto.
///
/// Returns:
///     Percentage text or an ASCII unavailable marker.
pub(super) fn percent(value: Option<u8>) -> String {
    value
        .map(|value| format!("{value}%"))
        .unwrap_or_else(|| "-".to_string())
}

/// Format optional per-process or machine memory.
///
/// Args:
///     value: Decimal megabytes from MoonProto.
///
/// Returns:
///     Localized memory text or an ASCII unavailable marker.
pub(super) fn memory_u16(value: Option<u16>) -> String {
    value
        .map(|value| format!("{} {}", value, t!("core_status.mb")))
        .unwrap_or_else(|| "-".to_string())
}

/// Format an optional client↔core round-trip time in milliseconds, e.g. `142 ms`.
///
/// Args:
///     value: Round-trip time from `Event::KernelHealth`.
///
/// Returns:
///     Localized latency text or an ASCII unavailable marker.
pub(super) fn ping(value: Option<u32>) -> String {
    value
        .map(|value| format!("{} {}", value, t!("core_status.ms")))
        .unwrap_or_else(|| "-".to_string())
}

/// Format a latency in milliseconds WITHOUT the unit, e.g. `142`, for the By IP tree where the
/// column header carries the unit. `-` when unavailable.
///
/// Args:
///     value: Round-trip time from `Event::KernelHealth`.
///
/// Returns:
///     Bare millisecond number, or an ASCII unavailable marker.
pub(super) fn ping_plain(value: Option<u32>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

/// Format one core's API-key state for its column.
///
/// A bare number: the unit lives in the column heading, so a column of counts reads as a column
/// instead of repeating "дн" on every row. The infinity glyph lives here, not in the dictionaries,
/// because `locales/README.md` keeps glyphs out of translated values.
///
/// Args:
///     state: The key's state as classified for this frame.
///
/// Returns:
///     Column text for the key's remaining lifetime.
pub(super) fn api_expiry_text(state: ApiKeyState) -> String {
    match state {
        ApiKeyState::Unknown => "-".to_string(),
        ApiKeyState::Perpetual => "\u{221e}".to_string(),
        ApiKeyState::Days(days) if days < 0 => t!("core_status.api_expired").to_string(),
        ApiKeyState::Days(days) => days.to_string(),
    }
}

/// Format one core's remaining API request quota for its column.
///
/// A bare number for the same reason as [`api_expiry_text`]: the unit belongs to the heading. `-`
/// covers both a core whose exchange publishes no quota at all and a HyperLiquid core that has not
/// answered yet — the protocol does not separate those, so neither may this.
///
/// Args:
///     quota: Remaining requests, when the core published a number.
///
/// Returns:
///     Column text for the remaining quota.
pub(super) fn api_quota_text(quota: Option<u64>) -> String {
    match quota {
        Some(left) => left.to_string(),
        None => "-".to_string(),
    }
}

/// Colour for the API-quota cell.
///
/// Args:
///     quota: Remaining requests, when the core published a number.
///     warn: Whether the engine currently warns about this quota.
///
/// Returns:
///     Yellow while the engine warns, else no colour — and always `Normal` when there is no number,
///     so a stale flag on a core that stopped publishing cannot paint an absence as a problem.
pub(super) fn api_quota_level(quota: Option<u64>, warn: bool) -> LoadLevel {
    if quota.is_some() && warn {
        LoadLevel::Warning
    } else {
        LoadLevel::Normal
    }
}

/// Format one core's reported MoonBot build for its column.
///
/// No noun — that lives in the column heading, exactly as [`api_expiry_text`] drops "дн" and
/// [`ping_plain`] drops "ms"; the fault tooltip spells out "MoonBot %{server}" because that one is
/// a sentence and a column is not.
///
/// The number ITSELF is dotted, through [`moon_core::util::fmt::core_build`]: the wire payload is a
/// flat `u32`, but the product names its builds `7.69` and `7.70`, so printing the raw `769` makes
/// the reader convert. That convention is the formatter's to state and is documented there — do not
/// re-derive it here. The terminal's own `vX.Y.Z` release version is real SemVer and a different
/// fact entirely.
///
/// Args:
///     version: The build this core reported, when it reported one.
///
/// Returns:
///     Decimal text, or the panel's ASCII unavailable marker.
pub(super) fn version_text(version: Option<u32>) -> String {
    version
        .map(moon_core::util::fmt::core_build)
        .unwrap_or_else(|| "-".to_string())
}

/// Format a server row's rolled-up build.
///
/// `Mixed` is an ellipsis rather than a number or a dash: it carries exactly one instruction to the
/// user — expand the group — and neither a number nor a blank would. The glyph lives here rather
/// than in the dictionaries, per `locales/README.md`, the same reason `∞` lives in
/// [`api_expiry_text`].
///
/// Args:
///     version: The group's agreement state.
///
/// Returns:
///     The agreed build, an ellipsis, or the unavailable marker.
pub(super) fn version_group_text(version: GroupVersion) -> String {
    match version {
        GroupVersion::Uniform(version) => moon_core::util::fmt::core_build(version),
        GroupVersion::Mixed => "\u{2026}".to_string(),
        GroupVersion::Absent => "-".to_string(),
    }
}

/// Format a server row's rolled-up clock offset.
///
/// `Mixed` is the same ellipsis [`version_group_text`] uses for a disagreeing build: it carries
/// exactly one instruction — expand the group — and neither a number nor the never-measured marker
/// would say that. `Absent` reuses the never-measured cell text: no core on the server has ever
/// measured an offset, which is exactly what a lone `Unknown` core also means.
///
/// Args:
///     group: The server's rolled-up agreement state.
///
/// Returns:
///     The text to show on the collapsed server row.
pub(super) fn tz_offset_group_text(group: TzOffsetGroup) -> String {
    match group {
        TzOffsetGroup::Uniform(offset_secs) => {
            tz_offset_cell_text(TzOffsetCell::Measured { offset_secs })
        }
        TzOffsetGroup::Mixed => "\u{2026}".to_string(),
        TzOffsetGroup::Absent => tz_offset_cell_text(TzOffsetCell::Unknown),
    }
}

/// Colour for an API-key cell.
///
/// The WARNING decision is the engine's — it owns the user's day threshold — so this reads that
/// decision instead of re-deriving one from its own day literals. A second set of steps here would
/// let the number stay grey under a lit warning triangle the moment the threshold is not the
/// default, the exact disagreement `lat_level` exists to prevent for the ping axes.
///
/// The panel-side `notice` band IS a second set of day steps, and that is legitimate rather than a
/// repeat of the same mistake: `warn` is tested BEFORE `notice` and returns `Warning`
/// unconditionally, so the banned failure mode (`warn` true but the colour stays grey) is
/// unreachable no matter what the two horizons are — the notice branch is never even reached while
/// `warn` holds. That is a stronger guarantee than widening the notice horizon to match the
/// configured one would have bought, and it needs neither the configured horizon nor a
/// shared-surface widening to hold.
///
/// Args:
///     state: The key's state as classified for this frame.
///     warn: Whether the engine currently warns about this key.
///     notice: Whether the key is inside the purely visual notice horizon.
///
/// Returns:
///     Red once the key is past its date, yellow while the engine warns, blue inside the notice
///     horizon, else no colour. Always `Normal` when there is no day count to speak of — an unknown
///     fact must never render as a problem, whatever either flag claims.
pub(super) fn api_expiry_level(state: ApiKeyState, warn: bool, notice: bool) -> LoadLevel {
    if state.is_expired() {
        return LoadLevel::Critical;
    }
    if state.days().is_none() {
        return LoadLevel::Normal;
    }
    if warn {
        LoadLevel::Warning
    } else if notice {
        LoadLevel::Notice
    } else {
        LoadLevel::Normal
    }
}

/// Map the engine's latency severity (already computed against the core's baseline and the axis
/// thresholds) to the shared load level for colouring. The engine is the single source of truth, so
/// the row colour and the ping/exch warning always agree — a core whose high ping IS its normal (the
/// 20/60/200 ms case) stays `Normal`.
pub(super) fn lat_level(sev: LatencySeverity) -> LoadLevel {
    match sev {
        LatencySeverity::Normal => LoadLevel::Normal,
        LatencySeverity::Warning => LoadLevel::Warning,
        LatencySeverity::Critical => LoadLevel::Critical,
    }
}

/// Format machine CPU load with the machine's logical-core count, e.g. `34% (16 core)`.
///
/// Args:
///     system_cpu: Whole-machine CPU percentage.
///     cores: Logical CPU count of the machine, appended in parentheses when known.
///
/// Returns:
///     Localized CPU line; the core count is dropped when it has not arrived yet.
pub(super) fn cpu_load(system_cpu: Option<u8>, cores: Option<u8>) -> String {
    match cores {
        Some(cores) => t!(
            "core_status.cpu_load",
            value = percent(system_cpu),
            n = cores
        )
        .to_string(),
        None => percent(system_cpu),
    }
}

/// Format free-memory percent and the machine's TOTAL RAM in gigabytes, e.g. `12% free (2 GB)`.
///
/// MoonProto never reports total RAM, so it is reconstructed as `process RAM sum + free physical`
/// and rounded UP to the next whole gigabyte, because real RAM is always a whole number of
/// gigabytes. The parenthetical is that reconstructed total; the percentage is free of it.
///
/// Args:
///     process_mem_mb: Sum of per-process resident memory on the machine, decimal MB.
///     free_mb: Free physical memory on the machine, decimal MB.
///
/// Returns:
///     Localized "free% (total GB)" line, or an ASCII marker until free memory has arrived.
pub(super) fn memory_free(process_mem_mb: Option<u64>, free_mb: Option<u16>) -> String {
    let Some(free_mb) = free_mb else {
        return "-".to_string();
    };
    let free_mb = u64::from(free_mb);
    let total_mb = process_mem_mb.unwrap_or(0) + free_mb;
    let total_gb = total_mb.div_ceil(1024).max(1);
    let pct = (free_mb as f64 / (total_gb as f64 * 1024.0) * 100.0).round() as u64;
    t!("core_status.mem_free", pct = pct, gb = total_gb).to_string()
}

/// Operational load state of a core along one axis (CPU load or free memory).
///
/// Ordered `Normal < Notice < Warning < Critical`, so severities combine with `max`. Exposed for
/// reuse: today it colors the metric numbers and drives the warnings sort; later it can drive core
/// badges and a tab indicator without recomputing the thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum LoadLevel {
    /// Comfortable headroom.
    Normal,
    /// Purely visual "look at this" mark: colour only, no episode, no sound.
    Notice,
    /// Getting tight.
    Warning,
    /// Near exhaustion.
    Critical,
}

/// Classify CPU load, where a higher percentage is worse.
///
/// Args:
///     percent: CPU percentage, process or whole-machine.
///
/// Returns:
///     `Warning` from 70%, `Critical` from 90%, else `Normal` (including unknown).
pub(super) fn cpu_level(percent: Option<u8>) -> LoadLevel {
    match percent {
        Some(percent) if percent >= 90 => LoadLevel::Critical,
        Some(percent) if percent >= 70 => LoadLevel::Warning,
        _ => LoadLevel::Normal,
    }
}

/// Classify free memory, where a lower free share is worse.
///
/// Uses the same reconstructed total as [`memory_free`] (process RAM sum + free physical).
///
/// Args:
///     process_mem_mb: Sum of per-process resident memory on the machine, decimal MB.
///     free_mb: Free physical memory on the machine, decimal MB.
///
/// Returns:
///     `Warning` below 10% free, `Critical` below 5% free, else `Normal` (including unknown).
pub(super) fn free_mem_level(process_mem_mb: Option<u64>, free_mb: Option<u16>) -> LoadLevel {
    let Some(free_mb) = free_mb else {
        return LoadLevel::Normal;
    };
    let free_mb = u64::from(free_mb);
    let total_mb = process_mem_mb.unwrap_or(0) + free_mb;
    if total_mb == 0 {
        return LoadLevel::Normal;
    }
    let free_pct = free_mb * 100 / total_mb;
    if free_pct < 5 {
        LoadLevel::Critical
    } else if free_pct < 10 {
        LoadLevel::Warning
    } else {
        LoadLevel::Normal
    }
}

/// Text color for a load level: soft text when normal, then yellow, then red.
///
/// Args:
///     level: Classified load state.
///     palette: Active Moon palette.
///
/// Returns:
///     A palette color the metric number is drawn in.
pub(super) fn level_color(level: LoadLevel, palette: MoonPalette) -> u32 {
    match level {
        LoadLevel::Normal => palette.text_soft,
        LoadLevel::Notice => notice_color(palette),
        LoadLevel::Warning => palette.yellow,
        LoadLevel::Critical => palette.red,
    }
}

/// The panel's colour-only "look at this" mark, shared by the API notice band and the
/// behind-the-fleet build. NOT a triangle: a triangle means an engine episode.
///
/// BLUE, deliberately, and NOT amber. Amber already carries one meaning throughout this panel --
/// something is wrong RIGHT NOW -- across five call sites: the dock-tab warning badge (`mod.rs`),
/// the `Degraded` connectivity dot and the dropout triangle (`server_view.rs`), a core still
/// starting (`StartupCell::Progress`), and the sustained-warning triangle in [`metric_cell`]. A
/// notice is the opposite claim: no episode, no sound, nothing broken yet. Painting it amber too
/// would leave those two meanings separated only by the presence of a small glyph, and a user
/// scanning 56 rows reads hue long before icon-presence.
///
/// It also keeps the tiers apart in the LIGHT theme, where amber (`0xB97824`) and yellow
/// (`0xB8860B`) are near-identical ochres -- roughly 9 degrees apart in hue at the same lightness
/// -- so an amber notice beside a yellow alert would have been effectively one colour there. Blue
/// separates cleanly in both themes.
///
/// The resulting ramp reads as one scale: blue = worth knowing, yellow + triangle = the engine is
/// alerting, red = already expired.
pub(super) fn notice_color(p: MoonPalette) -> u32 {
    p.blue
}

/// Colour for a reported-build cell: [`notice_color`] when this build is behind the fleet's
/// newest, `text_soft` for a build that is current, `text_muted` for absence or disagreement.
///
/// `behind` can only be true where a build was actually reported, so the notice colour can
/// never paint an absence -- but the order here makes that structural rather than merely true
/// today.
///
/// Args:
///     behind: Whether this cell's build is behind the fleet's newest reported build.
///     reported: Whether there is a build to show at all.
///     p: Active Moon palette.
///
/// Returns:
///     The notice colour when behind, soft text when current, muted text for absence or
///     disagreement.
pub(super) fn version_color(behind: bool, reported: bool, p: MoonPalette) -> u32 {
    if behind {
        notice_color(p)
    } else if reported {
        p.text_soft
    } else {
        p.text_muted
    }
}

/// Hover text for a per-core reported build that is behind the fleet's newest.
///
/// Args:
///     have: This core's own reported build, when it has one.
///     newest: The newest build currently reported across the fleet.
///
/// Returns:
///     Localized hover text naming both builds.
pub(super) fn version_behind_tooltip(have: Option<u32>, newest: u32) -> String {
    t!(
        "core_status.version_behind",
        have = version_text(have),
        newest = moon_core::util::fmt::core_build(newest)
    )
    .to_string()
}

/// Hover text for a collapsed server row whose cores all agree on a build that is behind the
/// fleet's newest.
///
/// It NAMES the group's own build rather than telling the user to expand the group. A group is
/// only ever marked when [`GroupVersion::Uniform`] holds -- a `Mixed` group states no build, so it
/// has none to be behind with -- and expanding a Uniform group therefore reveals N cores all
/// reporting the number already printed on the collapsed row. "Expand me" is the ELLIPSIS cell's
/// instruction, and this is not that cell.
///
/// Args:
///     have: The build every core on this server reported.
///     newest: The newest build currently reported across the fleet.
///
/// Returns:
///     Localized hover text naming both builds.
pub(super) fn version_behind_group_tooltip(have: u32, newest: u32) -> String {
    t!(
        "core_status.version_behind_group",
        have = moon_core::util::fmt::core_build(have),
        newest = moon_core::util::fmt::core_build(newest)
    )
    .to_string()
}

/// Small `Copy` visual for one update-queue state: a glyph, the load level to color it, and the
/// locale key naming it for a hover.
///
/// The moon-core -> UI localization boundary the module doc describes: `moon-core` hands back a
/// typed [`CoreUpdatePhase`]/[`GroupUpdate`], never a `String`, and this is the one place that
/// turns it into something to paint. The glyph lives here rather than in the dictionaries, per
/// `locales/README.md` -- the same reason `∞` lives in [`api_expiry_text`].
#[derive(Debug, Clone, Copy)]
pub(super) struct UpdateBadge {
    pub(super) glyph: &'static str,
    pub(super) level: LoadLevel,
    pub(super) locale_key: &'static str,
}

/// Locale key for one failed attempt, by the reason it failed.
fn update_failure_locale_key(failure: UpdateFailure) -> &'static str {
    match failure {
        UpdateFailure::NotSent => "core_update.phase.failed.not_sent",
        UpdateFailure::NeverDropped => "core_update.phase.failed.never_dropped",
        UpdateFailure::NotReady => "core_update.phase.failed.not_ready",
        UpdateFailure::Timeout => "core_update.phase.failed.timeout",
        UpdateFailure::Gone => "core_update.phase.failed.gone",
        UpdateFailure::Abandoned => "core_update.phase.failed.abandoned",
    }
}

/// Badge for one core's own update-queue phase.
///
/// Args:
///     phase: This core's tracked phase, or `None` when it has never been enqueued.
///
/// Returns:
///     `None` for a core the queue has never touched; otherwise the badge for its current phase.
pub(super) fn update_badge(phase: Option<&CoreUpdatePhase>) -> Option<UpdateBadge> {
    let phase = phase?;
    Some(match phase {
        CoreUpdatePhase::Queued { held: false, .. } => UpdateBadge {
            glyph: "\u{2026}", // …
            level: LoadLevel::Normal,
            locale_key: "core_update.phase.queued",
        },
        CoreUpdatePhase::Queued { held: true, .. } => UpdateBadge {
            glyph: "\u{2016}", // ‖
            level: LoadLevel::Warning,
            locale_key: "core_update.phase.held",
        },
        CoreUpdatePhase::Sent { .. } => UpdateBadge {
            glyph: "\u{2191}", // ↑
            level: LoadLevel::Notice,
            locale_key: "core_update.phase.sent",
        },
        CoreUpdatePhase::Waiting { .. } => UpdateBadge {
            glyph: "\u{21bb}", // ↻
            level: LoadLevel::Notice,
            locale_key: "core_update.phase.waiting",
        },
        CoreUpdatePhase::Done(CoreUpdateOutcome::Succeeded { .. }) => UpdateBadge {
            glyph: "\u{2713}", // ✓
            level: LoadLevel::Normal,
            locale_key: "core_update.phase.succeeded",
        },
        CoreUpdatePhase::Done(CoreUpdateOutcome::Unchanged { .. }) => UpdateBadge {
            glyph: "=",
            level: LoadLevel::Normal,
            locale_key: "core_update.phase.unchanged",
        },
        CoreUpdatePhase::Done(CoreUpdateOutcome::Failed(failure)) => UpdateBadge {
            glyph: "!",
            level: LoadLevel::Critical,
            locale_key: update_failure_locale_key(*failure),
        },
    })
}

/// Badge for a collapsed server row's rolled-up update state — see [`GroupUpdate`].
///
/// Args:
///     update: The group's rollup over its own cores.
///
/// Returns:
///     `None` while `Idle`, so a server with no update activity draws no badge at all.
pub(super) fn update_badge_for_group(update: GroupUpdate) -> Option<UpdateBadge> {
    match update {
        GroupUpdate::Idle => None,
        GroupUpdate::Active(_) => Some(UpdateBadge {
            glyph: "\u{21bb}", // ↻
            level: LoadLevel::Notice,
            locale_key: "core_update.summary.updating",
        }),
        // No dedicated `summary.held` key exists (see the phase list in `branch-P3.md`); a held
        // core is a queued one blocked behind a stalled lane, so it borrows `summary.queued`.
        GroupUpdate::Held(_) => Some(UpdateBadge {
            glyph: "\u{2016}", // ‖
            level: LoadLevel::Warning,
            locale_key: "core_update.summary.queued",
        }),
        GroupUpdate::Failed(_) => Some(UpdateBadge {
            glyph: "!",
            level: LoadLevel::Critical,
            locale_key: "core_update.summary.failed",
        }),
    }
}

/// Full localized hover text for one core's own update-queue phase.
///
/// Args:
///     phase: This core's tracked phase.
///
/// Returns:
///     The phrase naming that phase, from the same locale keys [`update_badge`] points at.
pub(super) fn update_tooltip(phase: &CoreUpdatePhase) -> String {
    match phase {
        CoreUpdatePhase::Queued { held: false, .. } => t!("core_update.phase.queued").to_string(),
        CoreUpdatePhase::Queued { held: true, .. } => t!("core_update.phase.held").to_string(),
        CoreUpdatePhase::Sent { .. } => t!("core_update.phase.sent").to_string(),
        CoreUpdatePhase::Waiting { .. } => t!("core_update.phase.waiting").to_string(),
        CoreUpdatePhase::Done(CoreUpdateOutcome::Succeeded { .. }) => {
            t!("core_update.phase.succeeded").to_string()
        }
        CoreUpdatePhase::Done(CoreUpdateOutcome::Unchanged { .. }) => {
            t!("core_update.phase.unchanged").to_string()
        }
        CoreUpdatePhase::Done(CoreUpdateOutcome::Failed(failure)) => {
            t!(update_failure_locale_key(*failure)).to_string()
        }
    }
}

#[cfg(test)]
mod tests;
