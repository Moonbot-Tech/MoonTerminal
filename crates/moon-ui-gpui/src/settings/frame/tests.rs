//! Coverage for the save bar's dirty sentence.

use super::dirty_text;
use crate::settings::Tab;

const PAGES: [Tab; 5] = [
    Tab::Cores,
    Tab::Appearance,
    Tab::Hotkeys,
    Tab::Lines,
    Tab::Badges,
];

/// Catches a wrong plural form or a lost page name: Russian takes "странице" for one page and
/// "страницах" for three and five, and every dirty page is named, comma-separated, in order.
#[test]
fn dirty_sentence_counts_and_names_the_pages() {
    let _locale = crate::test_locale::force("ru");
    let one = dirty_text(&PAGES[..1]);
    assert!(one.contains("1 странице — Ядра"), "{one}");

    let three = dirty_text(&PAGES[..3]);
    assert!(
        three.contains("3 страницах — Ядра, Тема и цвета, Горячие клавиши"),
        "{three}"
    );

    let five = dirty_text(&PAGES);
    assert!(five.contains("5 страницах"), "{five}");
    assert!(five.contains("Линии ордеров, Бейджи детектов"), "{five}");
}

/// Catches the sentence falling back to its raw key in a language without Slavic plurals.
#[test]
fn dirty_sentence_resolves_in_every_language() {
    for lang in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(lang.code());
        for n in [1, 3, 5] {
            let text = dirty_text(&PAGES[..n]);
            assert!(!text.starts_with("settings."), "{}: {text}", lang.code());
            assert!(text.contains(&n.to_string()), "{}: {text}", lang.code());
            assert!(
                text.contains(&Tab::Cores.title()),
                "{}: {text}",
                lang.code()
            );
        }
    }
}
