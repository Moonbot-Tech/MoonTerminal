//! Password-strength estimate for the `servers.enc` encryption password.
//!
//! PROVISIONAL: this is a deliberately small, dependency-free estimator that exists so the
//! Settings meter shows something truthful while the encryption backend is still a stub. Phase 2
//! replaces it with a proper estimator (`zxcvbn`) living beside the key-slot code in `moon-core`,
//! because the same verdict has to gate the actual key derivation, not just the meter. Keep the
//! public shape (`estimate` returning a [`Verdict`] with an [`Issue`]) so that swap touches one
//! module.
//!
//! Why an estimate at all: this password is the ONLY way back into `servers.enc` on a machine
//! whose OS keyring cannot open it. A four-character password there is not a weak lock, it is a
//! published one — the file travels, and an offline attacker gets unlimited attempts against it.

/// Shortest accepted encryption password.
///
/// Deliberately independent of the entropy floor: a short password can score well on charset
/// variety alone (`aA1!aA1!` is eight characters over four classes), and length is what actually
/// costs an offline attacker.
pub(super) const MIN_LEN: usize = 10;

/// Entropy floor for acceptance, in bits of the estimate below.
///
/// 50 bits is not "uncrackable" — with a memory-hard KDF in front of it, it is the point where an
/// offline attack stops being cheap. It is a floor for a local trading terminal, not for a secret
/// that must survive a funded adversary.
pub(super) const MIN_BITS: f32 = 50.0;

/// Bits at which the meter shows a full bar. Only presentation; acceptance uses [`MIN_BITS`].
const FULL_BAR_BITS: f32 = 80.0;

/// Coarse strength band shown by the meter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Level {
    /// No password typed yet.
    Empty,
    Weak,
    Fair,
    Good,
    Strong,
}

/// The single most useful thing to tell the user about a rejected password.
///
/// Only one is reported: a list of complaints reads as a puzzle, and the first one fixed usually
/// resolves the rest.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Issue {
    TooShort,
    /// Contains a well-known password, product, or keyboard run as a whole chunk.
    Common,
    /// A long run of one repeated character.
    Repeats,
    /// A long ascending or descending character run (`abcdef`, `4321`).
    Sequence,
    /// Only one character class, however long.
    OneClass,
    /// Below the entropy floor with no single dominant cause.
    LowEntropy,
}

/// Estimated strength of one candidate password.
pub(super) struct Verdict {
    /// Estimated entropy in bits after run and dictionary penalties.
    pub bits: f32,
    pub level: Level,
    /// Main reason the password is not accepted, or `None` when it is.
    pub issue: Option<Issue>,
}

impl Verdict {
    /// Whether this password may be used as the encryption password.
    pub fn accepted(&self) -> bool {
        self.issue.is_none()
    }

    /// Meter fill in percent, clamped to the component's `0..=100` range.
    pub fn percent(&self) -> f32 {
        (self.bits / FULL_BAR_BITS * 100.0).clamp(0.0, 100.0)
    }
}

/// Well-known chunks that make a password guessable regardless of its length.
///
/// Deliberately short and domain-flavoured rather than a real dictionary: a 100k-word list belongs
/// in the phase-2 estimator, while these are the strings this product's users actually reach for.
/// Matching is case-insensitive and substring-based, so `Moonbot2024!` is caught by `moonbot`.
const COMMON_CHUNKS: [&str; 34] = [
    "password",
    "passwort",
    "parol",
    "qwerty",
    "qwertz",
    "asdf",
    "zxcv",
    "1234",
    "12345",
    "123456",
    "admin",
    "root",
    "login",
    "letmein",
    "welcome",
    "master",
    "secret",
    "dragon",
    "monkey",
    "iloveyou",
    "sunshine",
    "princess",
    "football",
    "baseball",
    "trustno1",
    "whatever",
    "moonbot",
    "moonterminal",
    "binance",
    "bitcoin",
    "crypto",
    "trader",
    "abcabc",
    "test123",
];

/// Estimate the strength of `password`.
///
/// The model is the classic `length × log2(alphabet)` with the parts an attacker gets for free
/// discounted: a repeated character and a continued `+1`/`-1` run each contribute a fraction of a
/// character, and a well-known chunk collapses the whole estimate. It intentionally errs toward
/// pessimism — an over-estimate here is a password the user believes is safe and is not.
pub(super) fn estimate(password: &str) -> Verdict {
    let chars: Vec<char> = password.chars().collect();
    if chars.is_empty() {
        return Verdict {
            bits: 0.0,
            level: Level::Empty,
            issue: Some(Issue::TooShort),
        };
    }

    let classes = Classes::of(&chars);
    let (effective_len, dominant_run) = effective_length(&chars);
    let mut bits = effective_len * (classes.pool() as f32).log2();

    // A recognizable chunk is guessed as ONE token, so only what surrounds it still carries
    // entropy. `to_lowercase` allocates once per keystroke on a Settings field, which is not a
    // hot path.
    let lowered = password.to_lowercase();
    let common_hit = COMMON_CHUNKS
        .iter()
        .filter(|chunk| lowered.contains(**chunk))
        .map(|chunk| chunk.chars().count())
        .max();
    if let Some(hit_len) = common_hit {
        let rest = chars.len().saturating_sub(hit_len) as f32;
        bits = bits.min(10.0 + rest * 2.5);
    }

    // Rejection is decided by the SCORE, and the issue only explains it. Blaming a property
    // directly is what makes an estimator obnoxious: `MyLongSecureRootPassphrase99!` contains
    // "root" and is still a fine password, and rejecting it teaches users that the rule is
    // arbitrary. The one exception below the score is length, which no amount of variety buys.
    //
    // A single character class is rejected even above the floor. Twelve random lowercase letters
    // really do carry ~56 bits, but nobody types random lowercase — single-class passwords in the
    // wild are words, and no word list is complete enough for the formula above to catch them.
    let issue = if chars.len() < MIN_LEN {
        Some(Issue::TooShort)
    } else if bits < MIN_BITS {
        // Name whichever property actually cost the most, most specific first.
        Some(match dominant_run {
            _ if common_hit.is_some() => Issue::Common,
            Some(Run::Repeat) => Issue::Repeats,
            Some(Run::Sequence) => Issue::Sequence,
            None if classes.count() < 2 => Issue::OneClass,
            None => Issue::LowEntropy,
        })
    } else if classes.count() < 2 {
        Some(Issue::OneClass)
    } else {
        None
    };

    Verdict {
        bits,
        level: displayed_level(bits, issue.is_some()),
        issue,
    }
}

/// Which kind of cheap run dominates a password, used to pick the message.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Run {
    Repeat,
    Sequence,
}

/// Discount for the second and later characters of a repeated run (`aaaa`).
const REPEAT_WEIGHT: f32 = 0.2;
/// Discount for each character continuing an ascending/descending run (`abcd`, `9876`).
const SEQUENCE_WEIGHT: f32 = 0.35;

/// Return the run-discounted character count and the run kind that removed the most length.
///
/// A guesser walking `aaaa` or `1234` spends one decision, not four, so those characters must not
/// be paid for at full alphabet price. The returned run kind is only for messaging.
fn effective_length(chars: &[char]) -> (f32, Option<Run>) {
    let mut total = 0.0f32;
    let mut repeat_saved = 0.0f32;
    let mut sequence_saved = 0.0f32;
    let mut previous_step: Option<i32> = None;

    for (index, current) in chars.iter().enumerate() {
        let Some(previous) = index.checked_sub(1).map(|i| chars[i]) else {
            total += 1.0;
            continue;
        };
        let step = *current as i32 - previous as i32;
        if step == 0 {
            total += REPEAT_WEIGHT;
            repeat_saved += 1.0 - REPEAT_WEIGHT;
            previous_step = Some(0);
        } else if (step == 1 || step == -1) && previous_step.is_none_or(|last| last == step) {
            total += SEQUENCE_WEIGHT;
            sequence_saved += 1.0 - SEQUENCE_WEIGHT;
            previous_step = Some(step);
        } else {
            total += 1.0;
            previous_step = Some(step);
        }
    }

    // Only call a run "dominant" once it has eaten a meaningful part of the password; otherwise a
    // single doubled letter in an otherwise fine password would explain its rejection.
    let threshold = chars.len() as f32 * 0.25;
    let dominant = if repeat_saved >= sequence_saved && repeat_saved > threshold {
        Some(Run::Repeat)
    } else if sequence_saved > threshold {
        Some(Run::Sequence)
    } else {
        None
    };
    (total, dominant)
}

/// Character classes present in a password, which set the alphabet an attacker must search.
struct Classes {
    lower: bool,
    upper: bool,
    digit: bool,
    symbol: bool,
    other: bool,
    /// A letter outside ASCII appeared, so the letter alphabet is larger than Latin's 26.
    /// Not a class of its own — it only widens the pool.
    wide_alphabet: bool,
}

impl Classes {
    /// Classify letters by CASE rather than by script.
    ///
    /// This product's users type Russian. Bucketing every non-ASCII character into one "other"
    /// class made `МоёОченьДлинноеСлово` a single-class password and told the user to add a
    /// capital it already had — the two-class rule has to be satisfiable without switching
    /// keyboard layout. A caseless script has neither `is_uppercase` nor `is_lowercase`; those
    /// letters land in `lower` deliberately, counting as one class rather than none.
    fn of(chars: &[char]) -> Self {
        let mut classes = Self {
            lower: false,
            upper: false,
            digit: false,
            symbol: false,
            other: false,
            wide_alphabet: false,
        };
        for c in chars {
            if c.is_ascii_digit() {
                classes.digit = true;
            } else if c.is_alphabetic() {
                if c.is_uppercase() {
                    classes.upper = true;
                } else {
                    classes.lower = true;
                }
                classes.wide_alphabet |= !c.is_ascii();
            } else if c.is_ascii_graphic() || *c == ' ' {
                classes.symbol = true;
            } else {
                classes.other = true;
            }
        }
        classes
    }

    /// Alphabet size implied by the classes present, never below 2 so `log2` stays positive.
    fn pool(&self) -> u32 {
        let mut pool = 0;
        if self.lower {
            pool += 26;
        }
        if self.upper {
            pool += 26;
        }
        if self.digit {
            pool += 10;
        }
        if self.symbol {
            pool += 33;
        }
        if self.other {
            // Non-ASCII non-letters are credited conservatively: an attacker who knows the
            // language searches a far smaller set than the full codepoint space.
            pool += 100;
        }
        if self.wide_alphabet {
            // A non-Latin alphabet is bigger than 26 (Cyrillic is 33), and its letters are not in
            // the set an ASCII-only attack enumerates at all.
            pool += 40;
        }
        pool.max(2)
    }

    fn count(&self) -> u32 {
        u32::from(self.lower)
            + u32::from(self.upper)
            + u32::from(self.digit)
            + u32::from(self.symbol)
            + u32::from(self.other)
    }
}

/// Band shown to the user, which is never allowed to contradict acceptance.
///
/// A rejected password must not be painted green. `aA1!aA1!` scores well over the entropy floor
/// on charset variety alone and is still refused for length — a green "Strong" over a field Save
/// rejects is the loudest wrong thing on the panel, because the bar is read before the text.
fn displayed_level(bits: f32, rejected: bool) -> Level {
    match (level_of(bits), rejected) {
        (Level::Empty, _) => Level::Empty,
        (Level::Good | Level::Strong, true) => Level::Fair,
        (level, _) => level,
    }
}

/// Map estimated bits to the entropy band, before acceptance is taken into account.
fn level_of(bits: f32) -> Level {
    if bits <= 0.0 {
        Level::Empty
    } else if bits < 28.0 {
        Level::Weak
    } else if bits < MIN_BITS {
        Level::Fair
    } else if bits < 70.0 {
        Level::Good
    } else {
        Level::Strong
    }
}

#[cfg(test)]
mod tests;
