//! Report CSV export header regression: the file and clipboard header row must stay the raw DB
//! column name, never the restyled human label, because every downstream consumer of an exported
//! file lives outside this repo and parses that header verbatim.

use super::*;

/// Breakage: `columns::header_for` gets swapped for the pretty `columns::header_label` in
/// `write_csv` (or the XLSX writer, which shares the same call). Every user's exported CSV/Excel
/// file would then silently change its header row to localized text, breaking every downstream
/// consumer outside this repo. The companion static invariant in
/// `tests/theme_contract/report.rs` pins the same rule for both writers plus the clipboard TSV in
/// `selection.rs`; this test additionally proves the CSV bytes on disk actually carry the raw name.
#[test]
fn csv_header_row_stays_the_raw_db_column_names() {
    let idx: Vec<(usize, &str)> = vec![(0, "closedate"), (1, "coin"), (2, "profitbtc")];
    let core_uids = vec![7u64];
    let rows = vec![vec![
        Value::Integer(1_700_000_000),
        Value::Text("BTCUSDT".to_string()),
        Value::Real(1.5),
    ]];
    let axis = ReportAxis::identity_core_local();
    let path = std::env::temp_dir().join(format!(
        "moonterminal-report-export-test-{}-{}.csv",
        std::process::id(),
        rand_suffix()
    ));

    write_csv(&path, &idx, &core_uids, &rows, &axis, Tz::UTC).expect("csv write must succeed");
    let text = std::fs::read_to_string(&path).expect("csv file must be readable");
    std::fs::remove_file(&path).ok();

    let header = text
        .trim_start_matches('\u{FEFF}')
        .lines()
        .next()
        .expect("csv must have a header line");
    assert_eq!(
        header, "closedate;coin;profit",
        "export header must stay raw DB names, not localized labels"
    );
}

/// Cheap per-process uniqueness for a temp filename; a fixed name would collide across a parallel
/// test run in the same process.
fn rand_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}
