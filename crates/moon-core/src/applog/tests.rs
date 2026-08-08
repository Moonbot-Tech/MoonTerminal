use super::*;

/// One stored TSV line as [`DatedWriter::write`] emits it.
fn stored(hms: &str, level: &str, target: &str, msg: &str) -> String {
    format!("{hms}\t{level}\t{target}\t{msg}\n")
}

#[test]
fn scan_keeps_only_lines_holding_the_marker() {
    let file = stored(
        "09:29:25.434",
        "INFO",
        "",
        "UAI: [1] (17062) Task 17062 started",
    ) + &stored(
        "09:29:26.000",
        "INFO",
        "",
        "PROM: [2] (16922) BUY order replaced !",
    ) + &stored(
        "09:29:27.100",
        "ERR",
        "",
        "UAI: [1] (17062) BUY order canceled",
    );

    let found = scan_reader(file.as_bytes(), "2026-08-03", "(17062)", 100);

    assert!(!found.truncated);
    assert_eq!(found.lines.len(), 2, "две строки задачи из трёх");
    assert_eq!(found.lines[0].ts, "2026-08-03 09:29:25.434");
    assert_eq!(found.lines[0].msg, "UAI: [1] (17062) Task 17062 started");
    assert_eq!(found.lines[1].level, log::Level::Error);
}

#[test]
fn scan_stops_at_the_cap_and_says_so() {
    let mut file = String::new();
    for i in 0..5 {
        file += &stored("09:29:25.434", "INFO", "", &format!("(17062) line {i}"));
    }

    let found = scan_reader(file.as_bytes(), "2026-08-03", "(17062)", 3);

    assert!(found.truncated, "обрезали — сказать об этом");
    assert_eq!(found.lines.len(), 3);
    assert_eq!(found.lines[2].msg, "(17062) line 2");
}

#[test]
fn scan_of_exactly_the_cap_is_not_truncated() {
    let file = stored("09:29:25.434", "INFO", "", "(17062) only line");

    let found = scan_reader(file.as_bytes(), "2026-08-03", "(17062)", 1);

    assert!(
        !found.truncated,
        "ровно cap совпадений — это полный результат"
    );
    assert_eq!(found.lines.len(), 1);
}

#[test]
fn scan_of_a_similar_number_does_not_match() {
    let file = stored("09:29:25.434", "INFO", "", "AAA: [1] (170620) other task");

    // Через `task_marker`, а не через свой литерал: тест обязан ломаться, если изменится ФОРМАТ
    // метки, которым ищут вживую, а не только эта строка.
    let found = scan_reader(file.as_bytes(), "2026-08-03", &task_marker(17062), 100);

    assert!(
        found.lines.is_empty(),
        "скобки отделяют номер от его префикса"
    );
}

#[test]
fn file_name_matches_the_writer_spelling() {
    assert_eq!(file_name("2026-08-03", "BinF1"), "2026-08-03_BinF1.log");
    assert_eq!(
        file_name("2026-08-03", "Bin F1/x"),
        "2026-08-03_Bin_F1_x.log"
    );
}

/// The cursor arithmetic behind [`super::since`], in the three shapes it has to survive.
#[test]
fn unseen_counts_what_the_reader_missed() {
    use super::unseen;

    assert_eq!(unseen(10, 5, 8), 2, "the ordinary case is a difference");
    assert_eq!(unseen(10, 5, 10), 0, "a caught-up reader gets nothing");
    assert_eq!(
        unseen(100, 5, 10),
        5,
        "a reader further behind than the ring holds gets the survivors, not 90 phantom rows"
    );
    assert_eq!(
        unseen(3, 3, 9_000),
        0,
        "a cursor ahead of the counter must not underflow into a huge count"
    );
}
