use super::*;

/// A list that fits is one page holding every row; an empty list still has one (empty) page.
#[test]
fn a_fitting_list_is_one_page() {
    assert_eq!(fitting_page_size(&[1, 2, 3], |_| true), 3);
    assert_eq!(fitting_page_size::<u8>(&[], |_| false), 1);
}

/// An oversized list gets the largest ladder rung that fits, not a fixed small page.
///
/// Mutation: return a constant page size. A cap of 40 rows per message then yields six-row pages.
#[test]
fn an_oversized_list_gets_the_largest_fitting_rung() {
    let rows: Vec<u32> = (0..300).collect();
    assert_eq!(fitting_page_size(&rows, |page| page.len() <= 10), 8);
    assert_eq!(fitting_page_size(&rows, |page| page.len() <= 40), 32);
    assert_eq!(fitting_page_size(&rows, |page| page.len() <= 200), 128);
}

/// A small change in content keeps the same page size, so Next/Prev indices stay put.
///
/// Mutation: search for the exact largest size. The cap moving from 40 to 37 rows then moves
/// the page size from 40 to 37, and page 2 starts at a different row.
#[test]
fn a_small_content_change_keeps_the_page_size() {
    let rows: Vec<u32> = (0..300).collect();
    assert_eq!(
        fitting_page_size(&rows, |page| page.len() <= 40),
        fitting_page_size(&rows, |page| page.len() <= 37)
    );
}

/// Every page the answer makes fits, including the short last one and one heavy row.
#[test]
fn every_page_of_the_answer_fits() {
    // Row 7 is heavy: a page holding it fits only with at most 3 rows in total.
    let rows: Vec<u32> = (0..20).collect();
    let fits = |page: &[u32]| page.len() <= if page.contains(&7) { 3 } else { 8 };
    let size = fitting_page_size(&rows, fits);
    assert!(
        rows.chunks(size).all(fits),
        "size {size} makes a page that does not fit"
    );
}

/// When nothing but a single row fits, the size is 1, never 0.
#[test]
fn nothing_fitting_still_pages_by_one() {
    assert_eq!(fitting_page_size(&[1, 2, 3], |_| false), 1);
}
