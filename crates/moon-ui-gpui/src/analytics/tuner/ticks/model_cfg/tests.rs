use super::*;

#[test]
fn every_field_reads_back_what_it_writes() {
    for field in MODEL_FIELDS {
        let mut settings = ModelSettings::default();
        (field.set)(&mut settings, 7.0);
        assert_eq!((field.get)(&settings), 7.0, "{}", field.id);
    }
}

#[test]
fn a_setting_reads_as_its_box_shows_it() {
    let settings = ModelSettings::default();
    let latency = MODEL_FIELDS.iter().find(|f| f.id == "latency").unwrap();
    assert_eq!(field_text(latency, &settings), "100");
    let price = MODEL_FIELDS.iter().find(|f| f.id == "price").unwrap();
    assert_eq!(field_text(price, &settings), "0.05");
    assert_eq!(parse_field("0,3"), Some(0.3));
    assert_eq!(parse_field("-1"), None);
    assert_eq!(parse_field("abc"), None);
}

#[test]
fn every_field_has_a_distinct_id() {
    let mut ids: Vec<&str> = MODEL_FIELDS.iter().map(|f| f.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), MODEL_FIELDS.len());
}
