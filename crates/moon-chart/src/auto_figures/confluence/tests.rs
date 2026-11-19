use super::*;

fn placed(row: usize, tf: TfId, price: f64, pct: f32) -> PlacedLine {
    PlacedLine {
        row,
        tf,
        price,
        pct,
    }
}

fn run(lines: &[PlacedLine]) -> Vec<bool> {
    let mut out = vec![true; 3];
    confluence(lines, &mut out);
    out
}

/// By hand, 0.3 % bands: a 1h line at 100 (band 99.7..100.3) has the 4h line at 100.2 → both flagged;
/// a second 1h line at 100.25 also sees the 4h one (band 99.95..100.55); a 4h line at 105 has
/// nothing within 0.315; a NaN line is never flagged and flags nobody.
///
/// Breakage: the band measured from the wrong price, a NaN poisoning the sort, or the output not
/// in input order.
#[test]
fn flags_by_hand() {
    let lines = [
        placed(0, TfId::H1, 100.0, 0.3),
        placed(1, TfId::H4, 100.2, 0.3),
        placed(2, TfId::H1, 100.25, 0.3),
        placed(3, TfId::H4, 105.0, 0.3),
        placed(4, TfId::M5, f64::NAN, 0.3),
    ];
    assert_eq!(run(&lines), vec![true, true, true, false, false]);
}

/// Only another row on ANOTHER timeframe counts. Two 1h rows side by side are not a confluence;
/// one row's lines on two timeframes (a row the caller placed twice) are not either — until a
/// different row's 4h line arrives in the band, which the run-of-rows check must see past the same
/// row's own 4h line sitting first in that band. By hand for that last case: row 0's 1h line at
/// 100 finds row 1's 4h line at 100.15 behind row 0's own 4h one (true); row 0's 4h line at 100.1
/// has only its own row on 1h (false); row 1's 4h line at 100.15 reaches 100.15 ± 0.30045, which
/// holds row 0's 1h line at 100 (true).
///
/// Breakage: comparing timeframes only, rows only, or looking at the band's first line only.
#[test]
fn another_row_and_another_timeframe() {
    let same_tf = [
        placed(0, TfId::H1, 100.0, 0.3),
        placed(1, TfId::H1, 100.1, 0.3),
    ];
    assert_eq!(run(&same_tf), vec![false, false]);

    let same_row = [
        placed(0, TfId::H1, 100.0, 0.3),
        placed(0, TfId::H4, 100.1, 0.3),
    ];
    assert_eq!(run(&same_row), vec![false, false]);

    let third = [
        placed(0, TfId::H1, 100.0, 0.3),
        placed(0, TfId::H4, 100.1, 0.3),
        placed(1, TfId::H4, 100.15, 0.3),
    ];
    assert_eq!(run(&third), vec![true, false, true]);
}

/// Each line is judged by its OWN row's distance: a 0.1 % row at 100 does not reach a line 0.5
/// away, a 1 % row at 100.5 does.
///
/// Breakage: one distance applied to every line.
#[test]
fn each_row_uses_its_own_distance() {
    let lines = [
        placed(0, TfId::H1, 100.0, 0.1),
        placed(1, TfId::H4, 100.5, 1.0),
    ];
    assert_eq!(run(&lines), vec![false, true]);
}

/// Against an O(n²) brute force on 300 pseudo-random lines over seven timeframes and twenty rows.
///
/// Breakage: any shortcut in the sorted search that drops a neighbour (an off-by-one band edge, a
/// group skipped, a run of rows misread).
#[test]
fn matches_brute_force() {
    let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let lines: Vec<PlacedLine> = (0..300)
        .map(|_| {
            let row = (next() % 20) as usize;
            let tf = TfId::ALL[(next() % 7) as usize];
            // Prices on a coarse grid, so exact ties and band edges occur.
            let price = 100.0 + (next() % 400) as f64 * 0.025;
            let pct = [0.05f32, 0.1, 0.3][(next() % 3) as usize];
            placed(row, tf, price, pct)
        })
        .collect();
    let mut fast = Vec::new();
    confluence(&lines, &mut fast);
    for (i, a) in lines.iter().enumerate() {
        // The rule's own band (`pct` as a fraction of the line's price), spelled out.
        let reach = a.price * (f64::from(a.pct) / 100.0);
        let brute = lines.iter().any(|b| {
            b.row != a.row
                && b.tf != a.tf
                && b.price >= a.price - reach
                && b.price <= a.price + reach
        });
        assert_eq!(fast[i], brute, "line {i}: {a:?}");
    }
    assert!(fast.iter().any(|&f| f) && fast.iter().any(|&f| !f));
}

/// A reused scratch answers exactly like a fresh one, run after run: a timeframe group left over
/// from an earlier run (here the 4h lines of the first) must not flag anything in the next.
///
/// Breakage: the groups not cleared between runs, or a stale group's sorted runs read against the
/// new lines.
#[test]
fn a_reused_scratch_carries_nothing_between_runs() {
    let first = [
        placed(0, TfId::H1, 100.0, 0.3),
        placed(1, TfId::H4, 100.1, 0.3),
    ];
    let second = [
        placed(0, TfId::H1, 100.0, 0.3),
        placed(1, TfId::M5, 105.0, 0.3),
    ];
    let mut scratch = ConfluenceScratch::default();
    let mut out = Vec::new();
    confluence_in(&first, &mut scratch, &mut out);
    assert_eq!(out, vec![true, true]);
    confluence_in(&second, &mut scratch, &mut out);
    assert_eq!(out, vec![false, false]);
    let mut fresh = Vec::new();
    confluence(&second, &mut fresh);
    assert_eq!(out, fresh);
}
