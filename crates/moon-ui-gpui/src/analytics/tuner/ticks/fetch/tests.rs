use super::schema_default;

/// moonproto sends a default only when it is not zero (`FLAG_DEFAULT_NZ`): a number or switch
/// field without one defaults to zero, which the table must hold — `PriceToSwitch2Stop`, left out
/// of every strategy, read as "no default" and the search turned `UseSecondStop` on without it.
#[test]
fn a_field_without_a_default_defaults_to_zero() {
    for type_name in [
        "Bool", "Int32", "Int64", "Double", "Single", "Byte", "Word", "UInt32",
    ] {
        assert_eq!(schema_default(type_name, None), Some(0.0), "{type_name}");
    }
    assert_eq!(schema_default("String", None), None);
    assert_eq!(schema_default("Unknown", None), None);
}

/// A switch's default arrives spelled `Yes`/`No` (`feed::strategies::fmt_field`) and is kept as
/// 1/0, the way `StrategyValues::bool` reads the table; a number keeps its value.
#[test]
fn a_switch_default_is_one_or_zero_and_a_number_keeps_its_value() {
    assert_eq!(schema_default("Bool", Some("Yes")), Some(1.0));
    assert_eq!(schema_default("Bool", Some("No")), Some(0.0));
    assert_eq!(schema_default("Double", Some("-2,5")), Some(-2.5));
    assert_eq!(schema_default("Double", Some("1.5%")), Some(1.5));
    assert_eq!(schema_default("String", Some("abc")), None);
}
