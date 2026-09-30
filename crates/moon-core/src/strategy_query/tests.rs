//! Unit tests for the shared strategy-name query syntax.
//!
//! The oracle is a hand-written expectation per case (what a user typing the query must see for
//! each synthetic strategy name), never a value computed by the parser under test.

use super::*;

/// Whether `query_text` selects `name`, through the public entry every window uses.
fn hit(query_text: &str, name: &str) -> bool {
    StrategyQuery::parse(query_text).matches(name)
}

/// Dropping the trim or the fold in `parse`/`matches` would break the old plain-word search.
#[test]
fn a_plain_word_is_a_trimmed_case_insensitive_contains() {
    assert!(hit("  ema ", "Long EMA fast"));
    assert!(hit("EMA", "long_ema_fast"));
    assert!(!hit("ema", "Hook_M1"));
}

/// Replacing `fold` with ASCII-only lowering would break Cyrillic strategy names.
#[test]
fn cyrillic_names_match_without_regard_to_case() {
    assert!(hit("СТРАТЕГИЯ", "Стратегия_1"));
    assert!(!hit("СТРАТЕГИЯ", "Drops 1m"));
}

/// In `StrategyQuery::matches_folded`, changing `.any(|term|` into `.all(|term|` makes a
/// comma-separated query need every term at once, so `EMA, Hook` finds nothing in the Strategies
/// tree, the Report and Analytics.
#[test]
fn a_comma_separates_alternatives() {
    assert!(hit("EMA, Hook", "EMA_Fast"));
    assert!(hit("EMA, Hook", "Hook_M1"));
    assert!(!hit("EMA, Hook", "Drops 1m"));
}

/// Whitespace inside one term is AND: every word must appear, in any order.
#[test]
fn whitespace_inside_a_term_requires_every_word() {
    assert!(hit("drops 1m", "Drops 1m"));
    assert!(hit("drops 1m", "1m x drops"));
    assert!(!hit("drops 1m", "Drops 5m"));
}

/// An exclusion applies to the whole query: it removes a row even when another term matched it.
#[test]
fn an_exclusion_is_global_across_terms() {
    // Terms `a` and `b`, exclusion `x` written inside the second term.
    let query = "a, b !x";
    assert!(hit(query, "a1"));
    assert!(hit(query, "b1"));
    assert!(!hit(query, "ax"), "matched through a, excluded by x");
    assert!(!hit(query, "bx"));
    assert!(!hit(query, "c"), "matches no positive term");
}

/// A query made only of exclusions means "everything except", so unrelated names survive it.
#[test]
fn only_exclusions_mean_everything_except() {
    let query = StrategyQuery::parse("!test");
    assert!(!query.is_empty());
    assert!(!query.has_positive());
    assert!(query.matches("Drops 1m"));
    assert!(query.matches(""));
    assert!(!query.matches("My TEST one"));
    // A positive word next to the exclusion flips it to a real selection.
    assert!(StrategyQuery::parse("ema !test").has_positive());
}

/// A lone `!` negates the next token, so `! test` reads exactly like `!test`.
#[test]
fn a_bang_then_space_negates_the_next_word() {
    assert_eq!(
        StrategyQuery::parse("! test"),
        StrategyQuery::parse("!test")
    );
    assert!(!hit("! test", "a test"));
    assert!(hit("! test", "a"));
}

/// Text that carries no word selects everything, which lets callers emit no predicate at all.
#[test]
fn text_without_a_word_is_empty() {
    for text in ["", "   ", "\t\n", "!", ",", " , ,", "! ,", " ! "] {
        assert!(
            StrategyQuery::parse(text).is_empty(),
            "{text:?} carries no word"
        );
        assert!(hit(text, "anything"), "{text:?} must select every name");
    }
}

/// Only one leading `!` is stripped, so `!!x` excludes the literal text `!x`.
#[test]
fn a_double_bang_excludes_a_name_containing_a_bang() {
    assert!(!hit("!!x", "a!x"));
    assert!(hit("!!x", "ax"), "contains x but not !x");
    assert!(hit("!!x", "a!"));
}

/// Repeats collapse, so re-typing a word never changes the parsed query.
#[test]
fn duplicate_words_and_terms_collapse() {
    assert_eq!(StrategyQuery::parse("a a"), StrategyQuery::parse("a"));
    assert_eq!(StrategyQuery::parse("A a"), StrategyQuery::parse("a"));
    assert_eq!(StrategyQuery::parse("a, a"), StrategyQuery::parse("a"));
    assert_eq!(StrategyQuery::parse("!x !X"), StrategyQuery::parse("!x"));
}

/// Builds `count` words `w000`, `w001`, ... none of which contains another.
fn words(count: usize) -> Vec<String> {
    (0..count).map(|i| format!("w{i:03}")).collect()
}

/// Dropping the `MAX_WORDS` check in `parse` would keep every word of a pasted novel, and keeping
/// the cap one word too low or high would change which names match.
#[test]
fn words_past_the_cap_are_dropped() {
    let seventy = words(70).join(" ");
    let sixty_four = words(64).join(" ");
    assert_eq!(
        StrategyQuery::parse(&seventy),
        StrategyQuery::parse(&sixty_four)
    );
    // A name carrying exactly the first 64 words matches; the tail words are not required.
    assert!(hit(&seventy, &sixty_four));
    // The 64th word is still enforced (boundary pair with the dropped 65th).
    assert!(!hit(&seventy, &words(63).join(" ")));
    // Exclusions count in reading order too: one past the cap is ignored.
    assert!(hit(
        &format!("{sixty_four} !zzz"),
        &format!("{sixty_four} zzz")
    ));
    // A repeat within its bucket is free, so the 64th distinct word is still kept.
    let repeated = format!("{} w000 w063", words(63).join(" "));
    assert!(!hit(&repeated, &words(63).join(" ")));
}

/// SQL `LIKE` wildcards and the escape character are plain text in this syntax.
#[test]
fn percent_underscore_and_backslash_are_literal() {
    assert!(hit("%", "100% sure"));
    assert!(!hit("%", "abc"));
    assert!(hit("a_c", "xa_cx"));
    assert!(!hit("a_c", "abc"));
    assert!(hit("\\", "a\\b"));
    assert!(!hit("\\", "ab"));
}

/// Full Unicode case folding expands `ß` into `ss`, which plain lowercasing does not.
#[test]
fn full_case_folding_matches_multi_character_equivalents() {
    assert!(hit("ss", "Straße"));
    assert!(hit("straße", "STRASSE"));
    assert!(!hit("strase", "Straße"));
}

/// A NUL byte separates words like whitespace, because SQL literals cannot carry it.
#[test]
fn a_nul_byte_separates_words() {
    assert_eq!(StrategyQuery::parse("a\0b"), StrategyQuery::parse("a b"));
    assert!(hit("a\0b", "b then a"));
    assert!(!hit("a\0b", "a only"));
    assert!(!hit("\0!x", "has x"));
}

/// `phrase_for_name` must give back text whose parse finds the very name it came from, whatever
/// syntax characters the name carries.
#[test]
fn a_phrase_for_a_name_always_finds_that_name() {
    for (name, phrase) in [
        ("EMA, Hook", "EMA Hook"),
        ("!Bang one", "Bang one"),
        ("!!x", "x"),
        ("Drops 1m", "Drops 1m"),
        ("a,,b", "a b"),
        ("Стратегия, 1", "Стратегия 1"),
        ("a! b", "a! b"),
    ] {
        assert_eq!(StrategyQuery::phrase_for_name(name), phrase, "{name:?}");
        assert!(hit(phrase, name), "{name:?} must match its own phrase");
    }
}

/// A name made only of `!`, commas and whitespace has no word to search for.
#[test]
fn a_name_without_a_word_has_an_empty_phrase() {
    for name in ["!!!", "!", ",", "  ", "! ! ,", ""] {
        assert_eq!(StrategyQuery::phrase_for_name(name), "", "{name:?}");
    }
}
