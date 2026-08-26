//! Proves the `db::valuation` reads that must stay on the RAW `closedate` column forever cannot
//! be silently routed through [`moon_core`]'s report time axis by a future edit.
//!
//! `worker.rs`'s own module doc names these locations as load-bearing IDENTITY or ORDERING reads,
//! mirroring `report_axis.rs`'s "What this type must never be applied to": the reconciliation
//! keyset query and its cursor, the `trade_values` coverage join, the `trade_values` upsert key,
//! `trade_key`, and the two `TradeInput` decode sites that populate `closedate` in the first
//! place. This is a SOURCE-TEXT test because all of them sit inside functions private to
//! `moon-core`, unreachable from an integration test's public-API-only view — a behavioural test
//! cannot anchor on them from here. The funnel side of the same contract (`valuation_minute`
//! itself converts before flooring, and is the ONLY seam that may call the axis) is asserted
//! behaviourally in the sibling unit test `db/valuation/worker/tests.rs`, which has access to the
//! private symbol.
//!
//! Each assertion anchors on the EXACT source text of one never-routed read today. A future edit
//! that routes any of them through the axis — via `axis.to_utc`, `valuation_minute`, or an
//! equivalent — has to change that text, which is exactly what makes the anchor stop matching and
//! reddens this test. Block comments are removed (nesting-aware, so this half is exact), and each
//! remaining line's tail is cut from its first `//` outside a SINGLE-LINE string literal, before
//! any anchor is compared — see [`strip_trailing_line_comment`] for the exact limits of that
//! second half. Those limits do not affect this file's own anchors today (neither scanned file
//! has a raw string literal, a `'"'` char literal, or a multi-line string literal on a line an
//! anchor needs), but a future change to either file could silently reintroduce one.

use std::path::{Path, PathBuf};

/// `moon-core`'s own `src/` root, resolved the same way for every scan below.
fn moon_core_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Remove every `/* ... */` block comment, including ones spanning multiple lines, before any
/// per-line stripping runs — otherwise a commented-out line inside a block survives untouched and
/// can still satisfy a positive anchor. Newlines inside a removed block are preserved so line
/// numbers and the line-based matching below stay unaffected. Nesting is honoured because Rust
/// itself allows nested block comments.
///
/// This scan carries no notion of a string literal at all, unlike
/// [`strip_trailing_line_comment`]: a `/*` that happens to sit inside a string (an SQL `/*+ hint
/// */` clause, say) is removed the same as a real block comment. The `text.len() > 500` guard in
/// [`code_only`] runs on the raw, unstripped text and would not catch that.
fn strip_block_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut depth = 1;
            while depth > 0 {
                match chars.next() {
                    Some('*') if chars.peek() == Some(&'/') => {
                        chars.next();
                        depth -= 1;
                    }
                    Some('/') if chars.peek() == Some(&'*') => {
                        chars.next();
                        depth += 1;
                    }
                    Some('\n') => out.push('\n'),
                    Some(_) => {}
                    None => break,
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Cut a line's tail from its first `//` outside a SINGLE-LINE `"..."` string literal.
///
/// Args:
///     line: One physical line, already past block-comment removal.
///
/// Returns:
///     The line up to (but excluding) that `//`, or the whole line when none is found outside a
///     string. A whole-line comment (`//...`, `///...`, `//!...`) is therefore reduced to an empty
///     string rather than merely left intact for a later `starts_with("//")` filter to catch, and
///     a trailing comment (`code(); // ...`) no longer survives just because it does not itself
///     start with `//`.
///
/// This is a quote-toggle scan, not a lexer, and knows three real gaps: a `'"'` char literal
/// flips `in_string` with no closing quote to match it, so a genuine trailing `//` later on that
/// same line survives uncut; a raw string ending in a backslash (`r"C:\"`) is walked by the same
/// backslash-escape rule real strings use, which eats the closing quote and leaves `in_string`
/// stuck; and `in_string` always starts `false` at the top of a line, so a string literal that
/// spans more than one physical line is invisible to it. None of the three is worth a fuller
/// lexer here — see the module doc for why they do not reach this file's own anchors today.
fn strip_trailing_line_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_string => i += 2,
            b'"' => {
                in_string = !in_string;
                i += 1;
            }
            b'/' if !in_string && i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                return &line[..i];
            }
            _ => i += 1,
        }
    }
    line
}

/// Read one source file relative to `src/`, with block comments and single-line trailing
/// comments removed (see [`strip_trailing_line_comment`] for the scan's known limits).
///
/// Args:
///     relative: Path under `moon-core/src/`, forward-slash separated.
///
/// Returns:
///     The file's code lines only, each trimmed of surrounding whitespace and rejoined with `\n`
///     so a multi-line anchor compares independently of indentation.
fn code_only(relative: &str) -> String {
    let path = moon_core_src().join(relative);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    assert!(
        text.len() > 500,
        "{} read suspiciously short ({} bytes); a broken path would make this test vacuous",
        path.display(),
        text.len()
    );
    strip_block_comments(&text)
        .lines()
        .map(strip_trailing_line_comment)
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether `haystack` contains `needle_lines` as a contiguous, trimmed run.
///
/// Args:
///     haystack: Output of [`code_only`].
///     needle_lines: Expected trimmed source lines, in order.
///
/// Returns:
///     `true` when the exact run is present.
fn contains_lines(haystack: &str, needle_lines: &[&str]) -> bool {
    haystack.contains(&needle_lines.join("\n"))
}

/// The startup/live reconciliation walk's descending `(closedate, core_uid, row_id)` cursor must
/// advance from the RAW stored `closedate`. Its total monotonicity is what makes the walk
/// terminate exactly once; converting the seeded value would reorder rows under a cursor already
/// past them, so a batch would be re-visited or skipped outright, and a later, better offset
/// estimate would do it again to all of history.
#[test]
fn reconciliation_cursor_still_carries_the_raw_closedate() {
    let worker = code_only("db/valuation/worker.rs");
    assert!(
        contains_lines(
            &worker,
            &[".map(|input| (input.closedate, input.core_uid, input.row_id));"]
        ),
        "worker.rs:reconcile_step must seed the next descending cursor from the RAW closedate; \
         routing it through the axis (e.g. via valuation_minute or axis.to_utc) would desync the \
         cursor from the stored column it walks"
    );
}

/// `coverage_sql`'s `trade_values` coverage join must compare the STORED `closedate` on both
/// sides. Converting one side and not the other matches nothing, so every row would look
/// permanently uncovered and get re-valued from scratch on every pass.
#[test]
fn coverage_join_still_compares_the_raw_stored_columns() {
    let valuation_mod = code_only("db/valuation/mod.rs");
    assert!(
        contains_lines(
            &valuation_mod,
            &["AND v.closedate={alias}.closedate AND v.quote_ordinal=({quote})"]
        ),
        "coverage_sql's coverage join must compare the raw stored closedate against itself; \
         routing either side through the axis breaks the join it exists to satisfy"
    );
}

/// `store_trade_value`'s `trade_values` upsert must persist the RAW `closedate` under the same
/// column the coverage join above compares against — a value corrected through the axis here
/// would desync the cache key from every future coverage check.
#[test]
fn trade_values_upsert_still_stores_the_raw_closedate() {
    let valuation_mod = code_only("db/valuation/mod.rs");
    assert!(
        contains_lines(
            &valuation_mod,
            &["ALGORITHM_VERSION,", "input.closedate,", "input.quote_ordinal,"]
        ),
        "store_trade_value's upsert must bind the raw input.closedate as the persisted cache \
         identity, not a value corrected through the axis"
    );
}

/// `trade_key`, the in-memory deferred-row identity, must stay exactly `(source, core_uid,
/// row_id)` with no time-axis read folded in — adding one would give the same physical row two
/// identities across a restart whenever the axis changes.
#[test]
fn trade_key_still_carries_no_time_axis_read_at_all() {
    let worker = code_only("db/valuation/worker.rs");
    assert!(
        contains_lines(
            &worker,
            &[
                "fn trade_key(input: &TradeInput) -> (i64, i64, i64) {",
                "(input.source.code(), input.core_uid, input.row_id)",
                "}",
            ]
        ),
        "trade_key must stay closedate-free and axis-free; folding a time-axis read into it \
         would give one deferred row two identities depending on when the axis last changed"
    );
}

/// The two `TradeInput` DECODE sites — `load_trade`'s single-row read and `reconciliation_batch`'s
/// keyset scan — must both still bind `closedate` straight from the replica column. Anchoring on
/// the shared decode text alone is not enough: both sites emit the exact same trimmed line, so a
/// route inserted at ONE of them would still leave that text present at the other and every
/// assertion above — keyed on `input.closedate` downstream of here — would stay green while the
/// cursor, the coverage join, the upsert key and `trade_key` were all silently fed a converted
/// value. Counting occurrences is what makes converting either site alone visible.
#[test]
fn trade_input_decode_sites_still_both_carry_the_raw_closedate() {
    let worker = code_only("db/valuation/worker.rs");
    let occurrences = worker.matches("closedate: row.get(2)?,").count();
    assert_eq!(
        occurrences, 2,
        "expected exactly 2 raw TradeInput decode sites (load_trade and reconciliation_batch) \
         still binding closedate straight from row.get(2); found {occurrences} instead — a \
         decode-site route (e.g. `closedate: axis.to_utc(row.get(2)?, core_uid as u64)?`) would \
         defeat every downstream anchor in this file while leaving each of them green"
    );
}

/// `reconciliation_batch`'s own keyset predicate — the `(closedate, core_uid, row_id) < (?1, ?2,
/// ?3)` comparison against the descending cursor — must compare the RAW stored column. Wrapping it
/// in an axis conversion would desync the predicate from the cursor values it is compared against,
/// which are seeded from the same raw column one batch earlier.
#[test]
fn reconciliation_batch_keyset_predicate_still_carries_the_raw_closedate() {
    let worker = code_only("db/valuation/worker.rs");
    assert!(
        contains_lines(
            &worker,
            &["AND (r.closedate, r.core_uid, r.{id_column}) < (?1, ?2, ?3)"]
        ),
        "reconciliation_batch's keyset predicate must compare the raw stored closedate against \
         the raw cursor; routing either side through the axis desyncs the walk from the column it \
         is ordered on"
    );
}

/// `reconciliation_batch`'s own `ORDER BY` must sort by the RAW stored `closedate`. Wrapping the
/// column in an axis conversion here would reorder rows under a cursor seeded from the raw column,
/// silently re-visiting or skipping rows exactly as a converted cursor seed would.
#[test]
fn reconciliation_batch_order_by_still_sorts_the_raw_closedate() {
    let worker = code_only("db/valuation/worker.rs");
    assert!(
        contains_lines(
            &worker,
            &["ORDER BY r.closedate DESC, r.core_uid DESC, r.{id_column} DESC LIMIT ?4\","]
        ),
        "reconciliation_batch's ORDER BY must sort by the raw stored closedate; routing it \
         through the axis would reorder rows under a cursor already past them, so a batch would \
         be re-visited or skipped, and a later, better offset estimate would do it again"
    );
}

/// `reconciliation_batch` runs its OWN staleness join against `trade_values` inline in its keyset
/// query — a second, separate join from `coverage_sql`'s, in a different file with a literal `r`
/// alias rather than `{alias}`. Both sides must stay on the stored value for the same reason
/// `coverage_join_still_compares_the_raw_stored_columns` requires it of `coverage_sql`.
#[test]
fn reconciliation_batch_staleness_join_still_compares_the_raw_stored_columns() {
    let worker = code_only("db/valuation/worker.rs");
    assert!(
        contains_lines(
            &worker,
            &["AND v.closedate=r.closedate AND v.quote_ordinal=({quote})"]
        ),
        "reconciliation_batch's own staleness join must compare the raw stored closedate against \
         itself; routing either side through the axis makes every row look permanently stale"
    );
}

/// `worker.rs` must call the axis from exactly ONE place: [`valuation_minute`]'s own body
/// (`axis.to_utc(input.closedate, input.core_uid as u64)`), and nowhere else in the file. Every
/// anchor above proves one specific never-routed read still carries the raw column; this one
/// closes the gap between them — a NEW route added anywhere else in the file (through
/// `to_utc`, `from_utc`, `offset_secs`, or an equivalent) adds a second `axis.` call without
/// having to touch any of the exact text those per-site anchors match, so it would stay
/// invisible to all eight of them at once.
#[test]
fn worker_calls_the_axis_from_exactly_one_place() {
    let worker = code_only("db/valuation/worker.rs");
    let occurrences = worker.matches("axis.").count();
    assert_eq!(
        occurrences, 1,
        "expected exactly 1 call through the axis in worker.rs (valuation_minute's own \
         `axis.to_utc`); found {occurrences} instead — a second axis. call anywhere else in the \
         file is a new, unreviewed route that the per-site anchors above cannot see"
    );
}

/// No function in `worker.rs` may assign a converted value back into a `TradeInput.closedate`
/// field — every one of the eight anchors above assumes the DECODED value never moves again, so
/// an assignment-after-decode would leave the exact text each anchor matches untouched (the two
/// `closedate: row.get(2)?,` decode sites, the cursor seed, the coverage/staleness joins and the
/// `trade_key` identity all read `input.closedate` by reference, not by re-deriving it) while
/// every one of those reads silently started seeing a converted value. Anchoring on the assignment
/// form itself is the only way to catch a route inserted right after the decode, including through
/// the free function `valuation_minute(axis, input)`, which never itself writes `axis.` to the
/// call site.
#[test]
fn no_closedate_field_is_ever_reassigned_after_decode() {
    let worker = code_only("db/valuation/worker.rs");
    let occurrences = worker.matches(".closedate =").count();
    assert_eq!(
        occurrences, 0,
        "expected zero `.closedate =` assignments in worker.rs; found {occurrences} — a \
         `TradeInput.closedate` field written after decode (e.g. `input.closedate = \
         axis.to_utc(input.closedate, input.core_uid as u64);`) converts the value once under \
         text every other anchor in this file still matches unchanged, so the cursor seed, both \
         coverage joins and a second valuation_minute conversion on top would all silently start \
         reading a converted closedate"
    );
}
