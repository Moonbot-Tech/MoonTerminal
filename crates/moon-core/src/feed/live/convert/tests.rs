use super::*;

/// A future edit that wires machine-wide `system_cpu_percent` into the
/// process-CPU field, swaps process/system, or feeds `used_memory_mb` from
/// `free_physical_memory_mb` — the scope-vs-average confusion the panel must
/// never render — reddens here. Oracle independent of the converter: distinct
/// hand-chosen values per field can only land in the correctly-named one.
///
/// The literal names only the fields this converter reads and defaults the rest.
/// `moonproto` is tracked as a branch dependency, so upstream adds fields to
/// `KernelHealth` between releases; an exhaustive literal here turns every such
/// addition into a red CI run on a commit that touched nothing near it.
#[test]
fn kernel_health_maps_each_field_by_scope() {
    let h = moonproto::state::KernelHealth {
        process_cpu_percent: 12,
        system_cpu_percent: 77,
        used_memory_mb: Some(345),
        free_physical_memory_mb: Some(678),
        logical_cpu_count: Some(16),
        core_round_trip_ms: Some(250),
        order_api_latency_ms: Some(90),
        ..Default::default()
    };
    let sys = sys_status_from_proto(h, 1_780_000_000_000);
    assert_eq!(sys.process_cpu_percent, Some(12));
    assert_eq!(sys.system_cpu_percent, Some(77));
    assert_eq!(sys.used_memory_mb, Some(345));
    assert_eq!(sys.free_physical_memory_mb, Some(678));
    assert_eq!(sys.logical_cpu_count, Some(16));
    assert_eq!(sys.round_trip_ms, Some(250));
    assert_eq!(sys.order_api_latency_ms, Some(90));
    assert_eq!(sys.updated_ms, 1_780_000_000_000);
}

/// Memory / CPU-count Options stay `None` until the lower-rate profile arrives —
/// the converter must not fabricate a value on a CPU-only ping.
#[test]
fn kernel_health_preserves_absent_memory() {
    let h = moonproto::state::KernelHealth {
        process_cpu_percent: 5,
        system_cpu_percent: 9,
        used_memory_mb: None,
        free_physical_memory_mb: None,
        logical_cpu_count: None,
        ..Default::default()
    };
    let sys = sys_status_from_proto(h, 42);
    assert_eq!(sys.process_cpu_percent, Some(5));
    assert_eq!(sys.system_cpu_percent, Some(9));
    assert_eq!(sys.used_memory_mb, None);
    assert_eq!(sys.free_physical_memory_mb, None);
    assert_eq!(sys.logical_cpu_count, None);
}

/// A key the exchange reports NO expiration for answers successfully with `is_known() == false`,
/// and the wire carries `Some(0)` days beside it. Letting that zero through would put every
/// perpetual key one day from expiry and light a permanent warning on it.
///
/// Oracle independent of the converter: the input is built through moonproto's own constructor for
/// the "no known time" case, and the assertion is on the absence of a number, not on a value the
/// converter chose.
#[test]
fn a_key_without_an_expiration_reports_no_day_count() {
    // "No known time" is a zero day-count on MoonProto's Delphi scale, whose epoch is 25 569 days
    // before the Unix one. The assertion below re-checks that this input really is the unknown
    // case, so a moved epoch fails loudly here instead of quietly testing nothing.
    let delphi_epoch_unix_seconds = -25_569.0 * 86_400.0;
    let unknown = moonproto::ApiExpirationTime::from_time(
        moonproto::MoonTime::from_unix_seconds(delphi_epoch_unix_seconds)
            .expect("the Delphi epoch is representable"),
    );
    assert!(!unknown.is_known(), "a zero day-count means no expiry");

    let expiry = api_key_expiry_from_proto(unknown, std::time::UNIX_EPOCH);

    assert!(!expiry.known);
    assert_eq!(expiry.days_left, None, "no expiry is not zero days left");
    assert_eq!(expiry.at_unix, None);
}

/// A LEGACY answer (no `reported_days_left`) keeps a day count but NO absolute date: its timestamp
/// is the core's own local time, which nothing normalizes. Keeping it would make the un-normalized
/// value the preferred source in `days_left_at` and put a core in another time zone a day out; the
/// day count is aged from the receipt stamp instead.
#[test]
fn a_legacy_answer_keeps_a_count_but_no_date() {
    let in_ten_days = std::time::UNIX_EPOCH + std::time::Duration::from_secs(10 * 86_400);
    let legacy = moonproto::ApiExpirationTime::from_time(
        moonproto::MoonTime::from_system_time(in_ten_days).expect("representable"),
    );
    assert!(legacy.is_known());
    assert_eq!(
        legacy.reported_days_left(),
        None,
        "this constructor builds the legacy shape, which is the point of the test"
    );

    let expiry = api_key_expiry_from_proto(legacy, std::time::UNIX_EPOCH);

    assert!(expiry.known);
    assert_eq!(expiry.days_left, Some(10), "derived from the date");
    assert_eq!(expiry.at_unix, None, "the un-normalized date is not kept");
    // With no date, the reader ages the count from the receipt stamp.
    assert_eq!(expiry.days_left_at(4 * 86_400_000), Some(6));
}

/// A connected core cannot be trading on a key that expired years ago, so a large negative count is
/// a core-side placeholder, not a fact. Observed live: two connected cores answer `-1000` while the
/// rest of the terminal's cores report no expiry at all. Accepting it would paint a red "expired"
/// warning on two healthy cores.
///
/// A recently-expired key is a REAL state and must survive, so the guard only rejects the far side.
#[test]
fn an_implausible_negative_count_is_not_taken_as_expired() {
    let placeholder = moonproto::ApiExpirationTime::from_time(
        moonproto::MoonTime::from_system_time(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(365 * 86_400),
        )
        .expect("representable"),
    );
    // Read a thousand days after that date: the derived count is about -1000.
    let long_after = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_365 * 86_400);

    let expiry = api_key_expiry_from_proto(placeholder, long_after);

    assert!(expiry.known, "the answer still carries an expiration");
    assert_eq!(expiry.days_left, None, "-1000 days is not a day count");
    assert_eq!(expiry.at_unix, None);

    // Ten days past its date is an ordinary expired key and must be preserved.
    let recent = std::time::UNIX_EPOCH + std::time::Duration::from_secs(375 * 86_400);
    let expired = api_key_expiry_from_proto(placeholder, recent);
    assert_eq!(expired.days_left, Some(-10));
}

/// The parser zeroes the expiration DATE whenever the core's timestamp is unusable, but still
/// returns the day count that came beside it. Gating the count on the date would turn "no date,
/// but 42 days left" into an unlimited key — an infinity glyph over a key with six weeks to live.
#[test]
fn a_count_survives_an_unusable_date() {
    let (days, unlimited) = api_days_and_unlimited(false, Some(42), None);

    assert!(!unlimited, "42 days is not an unlimited key");
    assert_eq!(days, Some(42));
}

/// The same answer with a ZERO count is the unlimited key: no date, nothing counting down. The
/// wire always puts that zero there, so reading it as a countdown would warn on every such key.
#[test]
fn no_date_and_no_count_is_an_unlimited_key() {
    let (days, unlimited) = api_days_and_unlimited(false, Some(0), None);

    assert!(unlimited);
    assert_eq!(days, None, "the zero is not a countdown");

    // A legacy answer carries no count field at all; an empty date still means unlimited.
    let (days, unlimited) = api_days_and_unlimited(false, None, None);
    assert!(unlimited);
    assert_eq!(days, None);
}

/// A dated answer is never unlimited, and a zero count on it is a real "less than a day left".
#[test]
fn a_dated_answer_keeps_its_count_including_zero() {
    let (days, unlimited) = api_days_and_unlimited(true, Some(0), None);
    assert!(!unlimited, "the date exists, so the key expires");
    assert_eq!(days, Some(0), "zero days left, not 'no expiry'");

    // No count field: the date-derived fallback supplies it.
    let (days, unlimited) = api_days_and_unlimited(true, None, Some(10));
    assert!(!unlimited);
    assert_eq!(days, Some(10));
}

/// Implausible counts are dropped on both paths, and dropping the count of an answer that HAS a
/// date must not turn it into an unlimited key — it is simply unusable.
#[test]
fn an_implausible_count_never_becomes_unlimited() {
    let (days, unlimited) = api_days_and_unlimited(true, Some(-1000), None);
    assert_eq!(days, None);
    assert!(
        !unlimited,
        "a dated answer stays dated even when its count is junk"
    );
}

/// A count the plausibility range REJECTS still proves the core was talking about a lifetime, so
/// the answer must not fall through to "unlimited". Deciding that flag after the filter would put
/// an infinity glyph on a key whose only number we just threw away.
#[test]
fn a_rejected_count_does_not_become_unlimited() {
    let (days, unlimited) = api_days_and_unlimited(false, Some(-1000), None);

    assert_eq!(days, None, "the junk count is not shown");
    assert!(
        !unlimited,
        "and it is not evidence of an unlimited key either"
    );
}
