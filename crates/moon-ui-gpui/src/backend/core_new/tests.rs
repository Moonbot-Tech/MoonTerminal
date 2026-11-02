use super::unique_name;

#[test]
fn a_taken_name_gets_the_first_free_number() {
    let taken = ["mbcore", "mbcore 2", "BB1"];
    assert_eq!(unique_name("mbcore", taken.iter().copied()), "mbcore 3");
    assert_eq!(unique_name("mbnew", taken.iter().copied()), "mbnew");
}
