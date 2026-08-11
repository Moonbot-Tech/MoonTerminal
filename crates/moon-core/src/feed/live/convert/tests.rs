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

/// A percentage stop set from the terminal must not be drawn as an absolute price.
///
/// Live case, BB1 2026-08-11 17:18: a manual 10kSATS order filled at 0.00010458, the stop was
/// switched on from the table, and the level came from the core settings (`price_drop=-10.65`), so
/// `with_stop_loss_percent(10.65)` went out and the core kept the percent (`OrderCommand op=3
/// applied`, no `StopLoss applied` recalculation). Read as a price, 10.65 put the line at
/// `+10183491%` and took the auto-Y range with it.
///
/// Mutation: believe `sl_level` whenever `sl_fixed` is false. The line leaves the chart again and
/// flattens every other price on the pane.
///
/// Returns:
///     Nothing; the percent resolves to the price the core would compute.
#[test]
fn a_percentage_stop_level_is_not_drawn_as_a_price() {
    let entry = 0.000_104_58;
    let price = stop_loss_line_price(entry, false, false, 10.65, true).expect("percent resolves");
    assert!(
        (price - entry * (1.0 - 0.1065)).abs() < 1e-12,
        "expected the core's own -10.65% price, got {price}"
    );
    // Same field, same flag, on a five-figure market: the percent is far BELOW a plausible price.
    let btc = stop_loss_line_price(100_000.0, false, false, 10.65, true).expect("percent resolves");
    assert!((btc - 100_000.0 * (1.0 - 0.1065)).abs() < 1e-6);
    // And on the mid-priced markets in between, where the percent happens to sit near the entry:
    // above a long's entry it cannot be that long's stop price.
    let mid = stop_loss_line_price(2.0, false, false, 10.65, true).expect("percent resolves");
    assert!((mid - 2.0 * (1.0 - 0.1065)).abs() < 1e-9, "got {mid}");
    // A short takes the percent to the other side of the entry, on both price scales.
    let short = stop_loss_line_price(entry, true, false, 10.65, true).expect("percent resolves");
    assert!((short - entry * 1.1065).abs() < 1e-12);
    let short_mid = stop_loss_line_price(100.0, true, false, 10.65, true).expect("percent resolves");
    assert!((short_mid - 110.65).abs() < 1e-9, "got {short_mid}");
}

/// Before the fill the level is a percent whatever it looks like, with no plausibility test.
///
/// The core resolves a stop into a price at the fill and not before, which the 2026-08-11
/// diagnostic showed across every unfilled order carrying one: the field held a plain 10.65 against
/// entries from 0.0049 to 0.22. The heuristic below is only needed for orders that HAVE filled and
/// then had a stop switched on from the terminal.
///
/// Mutation: run the plausibility band on unfilled orders too. A working order priced near the
/// percent — a $10 coin with a 10.65% stop — draws its stop at 10.65 instead of 8.94.
///
/// Returns:
///     Nothing; an unfilled order always reads its level as a percent.
#[test]
fn an_unfilled_order_reads_its_stop_level_as_a_percent() {
    // A price that would pass every plausibility test: 9.0 sits just under a $10 entry.
    let price = stop_loss_line_price(10.0, false, false, 9.0, false).expect("percent resolves");
    assert!((price - 10.0 * (1.0 - 0.09)).abs() < 1e-9, "got {price}");
    // Once the same order fills, that value IS a plausible price and is drawn as one.
    assert_eq!(stop_loss_line_price(10.0, false, false, 9.0, true), Some(9.0));
}

/// A stop price the CORE resolved is drawn exactly where the core put it.
///
/// `BEAT: [4] StopLoss applied (buyPrice 0.89100 stop -3.00% => 0.91856)` — a short whose stop the
/// core computed as a price. Values like this sit beside the entry and must survive untouched,
/// including a stop deliberately far away (a -90% catastrophe stop is still a price).
///
/// Mutation: treat a resolved price as a percent. Every core-applied stop collapses to a fraction
/// of the entry, silently moving lines that were correct.
///
/// Returns:
///     Nothing; plausible prices pass through, fixed levels always do.
#[test]
fn a_core_resolved_stop_price_is_drawn_as_reported() {
    assert_eq!(
        stop_loss_line_price(0.891, true, false, 0.918_56, true),
        Some(0.918_56)
    );
    // Measured live on BB1: a filled short at mean 139.76 with the core's own stop price 140.03952,
    // which is exactly -0.2% and must survive untouched.
    assert_eq!(
        stop_loss_line_price(139.76, true, false, 140.039_52, true),
        Some(140.039_52)
    );
    // Long, stop 90% below entry: distant but still on the protective side, and inside the band.
    assert_eq!(stop_loss_line_price(2.0, false, false, 0.2, true), Some(0.2));
    // A fixed level is an absolute price the trader chose, on either side.
    assert_eq!(stop_loss_line_price(2.0, false, true, 55.0, true), Some(55.0));
    // No entry to reason against: draw the enabled stop where the core reported it rather than
    // dropping the line (a listing-sell before its position price arrives).
    assert_eq!(
        stop_loss_line_price(0.0, false, false, 0.918_56, true),
        Some(0.918_56)
    );
    // No usable level at all.
    assert_eq!(stop_loss_line_price(2.0, false, false, 0.0, true), None);
}

/// The diagnostic's percentage must read like the chart label, or it cannot settle which base the
/// label should use.
///
/// The chart flips the sign for a short so that a protective stop always reads negative
/// (`data_state::orders::signed_pct`); a diagnostic that skipped the flip would show a short's stop
/// as +6% and look like the wrong base rather than the same one.
///
/// Mutation: drop the short flip or the base guard. The comparison prints a sign that does not
/// match the label, or divides by a zero base on an order that has no entry yet.
///
/// Returns:
///     Nothing; signs match the label and an unusable base yields nothing.
#[test]
fn the_price_diagnostic_measures_like_the_chart_label() {
    // Long: stop 6% below the base reads -6%.
    let long = stop_distance_pct(Some(94.0), 100.0, false).expect("long distance");
    assert!((long + 6.0).abs() < 1e-9, "got {long}");
    // Short: stop 6% above the base also reads -6%.
    let short = stop_distance_pct(Some(106.0), 100.0, true).expect("short distance");
    assert!((short + 6.0).abs() < 1e-9, "got {short}");
    // The same stop measured from two bases is exactly the mismatch being chased.
    let from_other = stop_distance_pct(Some(94.0), 99.8, false).expect("other base");
    assert!((from_other + 5.81).abs() < 0.01, "got {from_other}");
    assert_eq!(stop_distance_pct(Some(94.0), 0.0, false), None);
    assert_eq!(stop_distance_pct(None, 100.0, false), None);
}

/// A short's percentage stop reported with the long formula is mirrored back above the entry.
///
/// The 2026-07-17 report: a market short drew its stop on the take-profit side while Moonbot showed
/// it above the entry, and the core's own `StopLoss applied` line confirmed the higher trigger.
///
/// Mutation: drop the mirror. A short's stop returns to the profit side, where it reads as a
/// protective level that would never trigger.
///
/// Returns:
///     Nothing; a short's stop price stays above its entry.
#[test]
fn a_short_stop_reported_below_entry_is_mirrored() {
    assert_eq!(
        stop_loss_line_price(100.0, true, false, 97.0, true),
        Some(103.0)
    );
    // Fixed levels are left alone: a dragged stop may sit anywhere on purpose.
    assert_eq!(
        stop_loss_line_price(100.0, true, true, 97.0, true),
        Some(97.0)
    );
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
