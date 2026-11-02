use super::default_dir;

/// The folder goes into a shell command on the server: a core name with spaces, punctuation or
/// non-Latin letters must still give a plain word.
#[test]
fn the_offered_folder_is_a_plain_word() {
    assert_eq!(default_dir("HL Spot (Ядро) #2"), "mbhlspot2");
}
