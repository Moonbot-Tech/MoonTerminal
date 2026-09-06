//! Confirmed core diagnostics: what the core's own detectors have concluded about themselves.
//!
//! A DIFFERENT thing from this terminal's warning episodes (`backend::core_warn`), and the
//! difference is not a detail of presentation. Our episodes are ours: we sample every core once a
//! second, hold the threshold, open and close an interval, and keep it forever. These are the
//! CORE's findings — its thresholds, its wording, its decision about when a suspicion has become a
//! fact. The core sends only what it has confirmed; hypotheses that never reached confirmation are
//! never transmitted at all.
//!
//! Neither replaces the other, and they can describe the same incident from two sides: a machine
//! under memory pressure is `MemGrowth` to us and `Machine`/`paging` to the core, at different
//! moments and with different evidence. Keeping them apart is what lets a reader tell "the terminal
//! measured this" from "the core concluded this".

/// What a core problem is about, as the core classifies it.
///
/// `Unknown` is not a parse failure — it is a category this build has never heard of, kept as its
/// raw byte so a newer core's finding still renders instead of being silently dropped. The
/// protocol's own guidance is to preserve unknown categories rather than fold them into `Other`,
/// which is a real category the core assigns deliberately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreProblemCategory {
    /// The machine the core runs on: memory pressure, paging, disk.
    Machine,
    /// The exchange side: API restrictions, regional blocks.
    Exchange,
    /// Connectivity between the core and what it talks to.
    Network,
    /// A confirmed finding the core places in none of the above.
    Other,
    /// A category newer than this build, kept verbatim.
    Unknown(u8),
}

/// One confirmed finding a core reports about itself.
///
/// Text fields arrive in the CORE's language, not the terminal's — the core is i18n-agnostic and
/// says so. A surface that shows them beside localized chrome is mixing two languages on purpose;
/// the alternative is to key off [`Self::kind_name`], which is a stable machine string, and treat
/// the core's own text as detail rather than as the heading.
///
/// `technical_details` is detector evidence and thresholds. It is meant to be READ, not parsed:
/// there is no schema behind it and the core is free to change its shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreProblem {
    /// Identity of this finding. One retained row per kind, never one per market.
    pub kind: u8,
    /// Stable textual key such as `paging`, `region-blocked` or `test`. Safe to match on; unlike
    /// the display text it does not move with the core's language.
    pub kind_name: String,
    /// What the finding is about.
    pub category: CoreProblemCategory,
    /// Display heading, in the core's language.
    pub title: String,
    /// Display body, in the core's language.
    pub message: String,
    /// Detector evidence and thresholds, for a details view. Not structured data.
    pub technical_details: String,
    /// When the core first saw the underlying symptom, as unix ms, or `None` when it sent no time.
    pub first_seen_ms: Option<i64>,
    /// When the core CONFIRMED it as a fact, as unix ms, or `None` when it sent no time.
    pub confirmed_ms: Option<i64>,
    /// Confirmation count as of the last row the core sent.
    ///
    /// It can read as frozen, and that is the protocol working as designed: repeated confirmations
    /// of an already-known fact are deliberately NOT broadcast, so this figure — and both times
    /// above — stay at whatever the last full list carried until the next one arrives.
    pub confirmations: i32,
}

/// Everything one core has said about its own health, plus whether it can say anything at all.
///
/// The two halves answer different questions and a surface must not collapse them. `items` empty
/// with `supported == true` means the core looked and found nothing. `items` empty with
/// `supported == false` means NOTHING IS KNOWN — the core has never sent a list, because it is
/// older than the extension or because its first list has not arrived yet. Rendering the second as
/// "healthy" is the one failure mode this type exists to prevent, and the protocol documentation
/// calls it out in as many words: an absent list is not proof that the core is healthy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoreProblems {
    /// Whether this core has ever delivered a complete list.
    ///
    /// This is the capability answer, and it is deliberately the ARRIVAL of a list rather than a
    /// version comparison. The protocol carries no build number for the extension and the core's
    /// reported version is a flat integer that cannot express a beta build, so a version gate would
    /// need a threshold nobody can supply. A list that arrived is proof that cannot be wrong in the
    /// dangerous direction: it never claims support the core does not have.
    ///
    /// The cost is one-sided and small — a core that DOES support the extension reads as unknown
    /// for the seconds between `Ready` and its first list, because delivery deliberately does not
    /// block readiness.
    pub supported: bool,
    /// Confirmed findings, in the order the core listed them.
    pub items: Vec<CoreProblem>,
}
