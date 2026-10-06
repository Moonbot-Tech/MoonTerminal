//! News, diagnostics, Telegram and license projections with shared trace helpers.

use super::*;

/// Project moonproto's retained `NewsState` into a moonproto-free [`NewsSnapshot`]: reduce its flat
/// frame ring into logical items. Called only when an `Event::News` arrives, mirroring the
/// license/settings snapshot idiom.
pub(in crate::feed::live) fn news_snapshot_from_proto(
    news: &moonproto::state::NewsState,
) -> NewsSnapshot {
    NewsSnapshot {
        items: crate::feed::news::reduce(news.items()),
        catalog: crate::feed::news::parse_catalog(news.tags_json()),
    }
}

pub(super) fn trace_point(p: OrderTraceChartPoint) -> OrderTracePoint {
    OrderTracePoint {
        time_ms: p.unix_millis() as f64,
        price: p.price,
    }
}

pub(super) fn valid_trace_point(p: &OrderTracePoint) -> bool {
    p.time_ms > 1.0 && p.price.is_finite() && p.price > 0.0
}

pub(super) fn valid_trace_tmp_point(p: OrderTraceChartPoint) -> Option<OrderTracePoint> {
    let time_ms = p.unix_millis() as f64;
    (time_ms > 1.0 && p.price.is_finite() && p.price > 0.0).then_some(OrderTracePoint {
        time_ms,
        price: p.price,
    })
}

pub(super) fn moon_time_to_unix_millis_f64(time: moonproto::MoonTime) -> f64 {
    let millis = time.unix_millis();
    if millis > 0 { millis as f64 } else { 0.0 }
}

/// Whether a core has proved it can report diagnostics at all.
///
/// A delivered finding proves the capability just as well as a complete list does, and it can
/// arrive FIRST: the protocol notes that a live notification may land before the initial list.
/// Reading only `snapshot_received` there counts a core as silent while its own row is on screen —
/// the two halves of `CoreProblems` contradicting each other.
///
/// A free function rather than an inline `||` because it is the rule the whole surface rests on and
/// `ProblemsState` cannot be constructed outside moonproto, so this is the only shape in which the
/// rule can be asserted at all.
///
/// Args:
///     snapshot_received: Whether a complete list has arrived on this connection.
///     item_count: How many findings the retained state currently holds.
///
/// Returns:
///     `true` when the core has demonstrably answered.
pub(super) fn problems_supported(snapshot_received: bool, item_count: usize) -> bool {
    snapshot_received || item_count > 0
}

/// Longest `kind_name` kept. It is a machine key (`paging`, `region-blocked`), not prose.
const PROBLEM_KIND_MAX_CHARS: usize = 64;

/// Longest problem heading kept, sized for a table cell rather than for a paragraph.
const PROBLEM_TITLE_MAX_CHARS: usize = 200;

/// Longest problem body or evidence text kept.
///
/// The wire allows 65535 BYTES per string and the core owns what it puts there, so an unclamped
/// projection lets one core decide this terminal's retained footprint. Generous enough that a real
/// detector message survives whole; the clamp exists for the pathological case, not the normal one.
const PROBLEM_TEXT_MAX_CHARS: usize = 2_000;

/// Make one wire-supplied string safe to retain and to draw.
///
/// Three separate jobs, and skipping any one of them has been a bug in this tree before:
///
/// - **Invisible formatting marks are removed.** A bidi override inside a label reverses the text
///   drawn AFTER it, including text the row never supplied. `moon_core::venue::is_invisible_format`
///   is public precisely so every display path filters the same set — its own doc says two
///   spellings of "what is printable" would let a name pass one check and vanish at the other.
/// - **Control characters are removed**, newlines and tabs included: these strings land in
///   fixed-height table cells, where a newline draws as a box or silently eats the rest of the line.
/// - **Length is clamped**, because the sender chooses it and this value is retained per core.
///
/// Args:
///     text: The string exactly as the core sent it.
///     max_chars: Longest form to keep, in characters rather than bytes.
///
/// Returns:
///     The printable, bounded form; empty when nothing printable remained.
pub(super) fn wire_text(text: &str, max_chars: usize) -> String {
    let clean: String = text
        .trim()
        .chars()
        .filter(|c| !c.is_control() && !crate::venue::is_invisible_format(*c))
        .take(max_chars)
        .collect();
    clean.trim().to_string()
}

/// A wire time as unix ms, or `None` when the core sent none.
///
/// Separate from [`moon_time_to_unix_millis_f64`], which answers `0.0` for an absent time because
/// its callers plot on an axis where zero is off-screen anyway. A diagnostic prints its times, and
/// "1 January 1970" is a worse answer than no answer.
fn moon_time_to_unix_ms(time: moonproto::MoonTime) -> Option<i64> {
    let millis = time.unix_millis();
    (millis > 0).then_some(millis)
}

/// Project the core's confirmed diagnostics into the terminal's own model.
///
/// The category wildcard is load-bearing rather than defensive: moonproto already models an
/// unrecognised category as `Unknown(u8)`, and the protocol asks consumers to preserve it. Folding
/// it into `Other` would erase the distinction between "the core placed this in its catch-all
/// bucket" and "this build is older than the category" — and it is the second one that tells a
/// reader the terminal needs updating, not the core.
///
/// `supported` comes from `snapshot_received()` — see [`CoreProblems::supported`] for why arrival,
/// rather than a version comparison, is the capability test.
///
/// Args:
///     problems: The core's retained problems state from the client snapshot.
///
/// Returns:
///     The projection, with `supported` false and no items for a core that has never answered.
pub(in crate::feed::live) fn problems_from_proto(
    problems: &moonproto::state::ProblemsState,
) -> CoreProblems {
    CoreProblems {
        supported: problems_supported(problems.snapshot_received(), problems.items().len()),
        items: problems
            .items()
            .iter()
            .map(|p| CoreProblem {
                kind: p.kind,
                kind_name: wire_text(&p.kind_name, PROBLEM_KIND_MAX_CHARS),
                category: match p.category {
                    moonproto::state::ProblemCategory::Machine => CoreProblemCategory::Machine,
                    moonproto::state::ProblemCategory::Exchange => CoreProblemCategory::Exchange,
                    moonproto::state::ProblemCategory::Network => CoreProblemCategory::Network,
                    moonproto::state::ProblemCategory::Other => CoreProblemCategory::Other,
                    moonproto::state::ProblemCategory::Unknown(raw) => {
                        CoreProblemCategory::Unknown(raw)
                    }
                },
                title: wire_text(&p.title, PROBLEM_TITLE_MAX_CHARS),
                message: wire_text(&p.message, PROBLEM_TEXT_MAX_CHARS),
                technical_details: wire_text(&p.technical_details, PROBLEM_TEXT_MAX_CHARS),
                first_seen_ms: moon_time_to_unix_ms(p.first_seen),
                confirmed_ms: moon_time_to_unix_ms(p.confirmed),
                confirmations: p.confirmations,
            })
            .collect(),
    }
}

/// Project the core's built-in Telegram reader snapshot into the terminal's own model.
///
/// Field-for-field, including absents: each protocol snapshot REPLACES the previous one, so a
/// field dropped here would read as "unavailable" forever. Strings are cloned; this is the one
/// place that sees both moonproto and the moon-core mirror.
///
/// Args:
///     state: The core's retained Telegram snapshot from the client.
///
/// Returns:
///     The moonproto-free projection the UI and the store consume.
pub(in crate::feed::live) fn telegram_from_proto(state: &TelegramState) -> CoreTelegramState {
    CoreTelegramState {
        enabled: state.enabled,
        proxy_type: state.proxy_type,
        proxy_host: state.proxy_host.clone(),
        proxy_port: state.proxy_port,
        proxy_user: state.proxy_user.clone(),
        proxy_password_set: state.proxy_password_set,
        client_state: state.client_state.clone(),
        service_online: state.service_online,
        state_supported: state.state_supported,
        service_version: state.service_version.clone(),
        setup_error: state.setup_error.clone(),
        client_error: state.client_error.clone(),
        service: state.service.as_ref().map(telegram_service_from_proto),
    }
}

/// Project the nested service snapshot, cloning strings and mapping nested optionals.
fn telegram_service_from_proto(service: &TelegramServiceState) -> CoreTelegramService {
    CoreTelegramService {
        auth_state: service.auth_state.clone(),
        details: telegram_auth_details_from_proto(&service.details),
        connection: service.connection.clone(),
        phone: service.phone.clone(),
        error: service.error.as_ref().map(telegram_error_from_proto),
        proxy: service.proxy.as_ref().map(telegram_active_proxy_from_proto),
        proxy_error: service.proxy_error.as_ref().map(telegram_error_from_proto),
    }
}

/// Project step-specific auth hints, including the QR link, without logging them.
fn telegram_auth_details_from_proto(details: &TelegramAuthDetails) -> CoreTelegramAuthDetails {
    CoreTelegramAuthDetails {
        qr_link: details.qr_link.clone(),
        phone: details.phone.clone(),
        code_type: details
            .code_type
            .as_ref()
            .map(telegram_code_type_from_proto),
        next_code_type: details
            .next_code_type
            .as_ref()
            .map(telegram_code_type_from_proto),
        resend_at: details.resend_at,
        password_hint: details.password_hint.clone(),
        recovery_email_pattern: details.recovery_email_pattern.clone(),
        email_pattern: details.email_pattern.clone(),
        code_length: details.code_length,
        terms: details.terms.clone(),
        min_user_age: details.min_user_age,
        show_popup: details.show_popup,
        support_email: details.support_email.clone(),
        support_subject: details.support_subject.clone(),
    }
}

/// Project a code-delivery method; unknown `kind` strings are preserved.
fn telegram_code_type_from_proto(code_type: &TelegramCodeType) -> CoreTelegramCodeType {
    CoreTelegramCodeType {
        kind: code_type.kind.clone(),
        length: code_type.length,
        first_letter: code_type.first_letter.clone(),
        first_word: code_type.first_word.clone(),
        pattern: code_type.pattern.clone(),
        prefix: code_type.prefix.clone(),
        url: code_type.url.clone(),
    }
}

/// Project a service error next to the current step or proxy settings.
fn telegram_error_from_proto(error: &TelegramError) -> CoreTelegramError {
    CoreTelegramError {
        code: error.code,
        message: error.message.clone(),
    }
}

/// Project the proxy the service currently has enabled.
fn telegram_active_proxy_from_proto(proxy: &TelegramActiveProxy) -> CoreTelegramActiveProxy {
    CoreTelegramActiveProxy {
        mode: proxy.mode.clone(),
        host: proxy.host.clone(),
        port: proxy.port,
    }
}

pub(in crate::feed::live) fn license_state_from_proto(
    license: moonproto::KernelLicenseStateCommand,
) -> LicenseState {
    LicenseState {
        paid_version: license.paid_version,
        reg_id: license.reg_id,
        moon_credits: license.moon_credits,
        moon_credits_hold: license.moon_credits_hold,
        moon_credits_auction: license.moon_credits_auction,
        can_use_watcher: license.can_use_watcher,
        // News-module subscription/trial, previously dropped; surfaced in the News panel footer.
        // Guard non-positive (pre-1970 Delphi) times to None, matching the sibling time converters,
        // so a malformed stamp reads as "no subscription" rather than "expired".
        news_valid_until: license
            .news_valid_until
            .map(|t| t.unix_millis())
            .filter(|&ms| ms > 0),
        news_trial_used: license.news_trial_used,
    }
}
