//! One explanation of each deal's tape, model findings and place in the search sample.

use moon_core::db::tuner::ticks::exit::UnmodelledRule;
use moon_core::db::tuner::ticks::verify::{EntryFinding, ExitFinding, ExitMiss, Unjudged};
use moon_core::market::trade_replay::TickStatus;
use rust_i18n::t;

use super::state::{DealRow, TapeStatus};

/// Mutually exclusive sample buckets, plus the overlapping tape-presence filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analytics::tuner) enum DealChip {
    WithTape,
    NoTape,
    Fit,
    ShortTail,
    Hole,
    ModelMiss,
    Unjudged,
}

impl DealChip {
    /// Footer order includes the overlapping tape count before the exclusive buckets.
    pub(in crate::analytics::tuner) const ALL: [Self; 7] = [
        Self::WithTape,
        Self::NoTape,
        Self::Fit,
        Self::ShortTail,
        Self::Hole,
        Self::ModelMiss,
        Self::Unjudged,
    ];

    /// Stable suffix shared by localized captions, hover explanations and element ids.
    pub(in crate::analytics::tuner) fn key(self) -> &'static str {
        match self {
            Self::WithTape => "with_tape",
            Self::NoTape => "no_tape",
            Self::Fit => "fit",
            Self::ShortTail => "short_tail",
            Self::Hole => "hole",
            Self::ModelMiss => "model_miss",
            Self::Unjudged => "unjudged",
        }
    }

    /// Group the sample first, then the remaining buckets in declaration order.
    pub(in crate::analytics::tuner) fn rank(self) -> u8 {
        match self {
            Self::Fit => 0,
            Self::WithTape => 1,
            Self::NoTape => 2,
            Self::ShortTail => 3,
            Self::Hole => 4,
            Self::ModelMiss => 5,
            Self::Unjudged => 6,
        }
    }

    /// Keep all footer text localized while the glyphs remain in code.
    pub(in crate::analytics::tuner) fn label(self, min_tail_s: u32) -> String {
        let key = format!("analytics.ticks.chip.{}", self.key());
        let label = t!(key.as_str(), min = min_tail_s).to_string();
        match self {
            Self::ModelMiss => format!("{label} ✗"),
            Self::Unjudged => format!("{label} ·"),
            _ => label,
        }
    }

    /// The overlapping tape bucket is independent of the reason for exclusion.
    pub(in crate::analytics::tuner) fn tape(row: &DealRow) -> Option<Self> {
        (row.tape == TapeStatus::Covered).then_some(Self::WithTape)
    }
}

/// Semantic colours, resolved against the active palette by the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analytics::tuner) enum Tone {
    Good,
    Warning,
    Muted,
}

/// Hover content keeps the user's next action separate from the explanation.
#[derive(Clone, Debug)]
pub(in crate::analytics::tuner) struct Tip {
    pub(in crate::analytics::tuner) what: String,
    pub(in crate::analytics::tuner) why: String,
    pub(in crate::analytics::tuner) todo: Option<String>,
    pub(in crate::analytics::tuner) not_your_fault: bool,
}

impl Tip {
    /// Render the same four-part explanation for both table marks.
    pub(in crate::analytics::tuner) fn text(self) -> String {
        let mut lines = vec![self.what, self.why];
        if let Some(todo) = self.todo {
            lines.push(t!("analytics.ticks.why.todo", action = todo).to_string());
        }
        if self.not_your_fault {
            lines.push(t!("analytics.ticks.why.not_your_fault").to_string());
        }
        lines.join("\n")
    }
}

/// The first reason a deal cannot teach the search, in the sample's exclusion order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analytics::tuner) enum DealWhy {
    Fit,
    NoTape { fetchable: bool },
    NoAddress,
    Fetching,
    Refused(TickStatus),
    EntryMissed(EntryFinding),
    ExitMissed(ExitMiss),
    Unjudged(Unjudged),
    NoVerdict,
    ShortTail { trail_ms: i64, min_s: u32 },
    Hole { len_ms: i64 },
    LostTape,
}

impl DealWhy {
    /// Classify a row using one captured tail floor and clock for the whole population.
    pub(in crate::analytics::tuner) fn of(row: &DealRow, min_tail_s: u32, now_ms: i64) -> Self {
        match row.tape {
            TapeStatus::Missing => {
                return Self::NoTape {
                    fetchable: row.fetchable_at(now_ms, min_tail_s),
                };
            }
            TapeStatus::NoAddress => return Self::NoAddress,
            TapeStatus::Fetching => return Self::Fetching,
            TapeStatus::Refused(status) => return Self::Refused(status),
            TapeStatus::Covered => {}
        }
        let Some(verdict) = row.verdict.as_ref() else {
            return Self::NoVerdict;
        };
        match verdict.entry_finding {
            EntryFinding::Fact | EntryFinding::Hit => {}
            missed @ (EntryFinding::Unfilled | EntryFinding::Off) => {
                return Self::EntryMissed(missed);
            }
        }
        if let Some(why) = Self::of_exit(Some(verdict)) {
            return why;
        }
        if let Some(why) = Self::tail_or_gap(row, min_tail_s) {
            return why;
        }
        if row.ticks.is_none() {
            return Self::LostTape;
        }
        Self::Fit
    }

    /// Project the exit independently of entry, as the accuracy denominator requires.
    pub(in crate::analytics::tuner) fn of_exit(
        verdict: Option<&moon_core::db::tuner::ticks::Verdict>,
    ) -> Option<Self> {
        match verdict.map(|v| v.exit_finding) {
            None => Some(Self::NoVerdict),
            Some(ExitFinding::Hit) => None,
            Some(ExitFinding::Miss(miss)) => Some(Self::ExitMissed(miss)),
            Some(ExitFinding::Unjudged(why)) => Some(Self::Unjudged(why)),
        }
    }

    /// Tape completeness is visible even when a model finding takes exclusion precedence.
    pub(in crate::analytics::tuner) fn tape_of(
        row: &DealRow,
        min_tail_s: u32,
        now_ms: i64,
    ) -> Self {
        if row.tape == TapeStatus::Covered {
            Self::tail_or_gap(row, min_tail_s).unwrap_or(Self::Fit)
        } else {
            Self::of(row, min_tail_s, now_ms)
        }
    }

    /// Share the tape exclusions between the primary reason and the tape-only projection.
    fn tail_or_gap(row: &DealRow, min_tail_s: u32) -> Option<Self> {
        let trail_ms = row.held.map_or(0, |(_, trail)| trail.max(0));
        if !row.held.is_some_and(|(_, trail)| {
            moon_core::db::tuner::ticks::tail::reaches(trail, i64::from(min_tail_s) * 1_000)
        }) {
            return Some(Self::ShortTail {
                trail_ms,
                min_s: min_tail_s,
            });
        }
        if let Some(gap) = &row.deal.gap {
            return Some(Self::Hole {
                len_ms: gap.to_ms.saturating_sub(gap.from_ms).max(0),
            });
        }
        None
    }

    /// Partition all rows; dropped retained tape belongs to the unjudged bucket.
    pub(in crate::analytics::tuner) fn chip(self) -> DealChip {
        match self {
            Self::Fit => DealChip::Fit,
            Self::NoTape { .. } | Self::NoAddress | Self::Fetching | Self::Refused(_) => {
                DealChip::NoTape
            }
            Self::EntryMissed(_) | Self::ExitMissed(_) => DealChip::ModelMiss,
            Self::Unjudged(_) | Self::NoVerdict | Self::LostTape => DealChip::Unjudged,
            Self::ShortTail { .. } => DealChip::ShortTail,
            Self::Hole { .. } => DealChip::Hole,
        }
    }

    /// Stable translation identity; every upstream finding is matched explicitly.
    fn key(self) -> &'static str {
        match self {
            Self::Fit => "fit",
            Self::NoTape { fetchable: true } => "missing_fetchable",
            Self::NoTape { fetchable: false } => "missing",
            Self::NoAddress => "no_address",
            Self::Fetching => "fetching",
            Self::Refused(status) => match status {
                TickStatus::Pending => "pending",
                TickStatus::Streaming => "streaming",
                TickStatus::AwaitingCore => "awaiting_core",
                TickStatus::ContextUnavailable => "context_unavailable",
                TickStatus::NoRoute => "no_route",
                TickStatus::ArchivePending { .. } => "archive_pending",
                TickStatus::Disabled => "disabled",
                TickStatus::OutOfRetention { .. } => "retention",
                TickStatus::NoTrades => "no_trades",
                TickStatus::RateLimited { .. } => "rate_limited",
                TickStatus::Failed => "failed",
                TickStatus::Served => "served",
            },
            Self::EntryMissed(finding) => entry_key(finding),
            Self::ExitMissed(miss) => miss_key(miss),
            Self::Unjudged(why) => unjudged_key(why),
            Self::NoVerdict => "no_verdict",
            Self::ShortTail { .. } => "short_tail",
            Self::Hole { .. } => "hole",
            Self::LostTape => "lost_tape",
        }
    }

    /// Localize a reason with its measured durations and retry metadata.
    fn translated(self, suffix: &str) -> String {
        let key = format!("analytics.ticks.why.{}{}", self.key(), suffix);
        let (trail_ms, min_s, len_ms, retry_at_ms, retention_ms, retry_in_s) = match self {
            Self::ShortTail { trail_ms, min_s } => (trail_ms, min_s, 0, 0, 0, 0),
            Self::Hole { len_ms } => (0, 0, len_ms, 0, 0, 0),
            Self::Refused(TickStatus::ArchivePending { retry_at_ms }) => {
                (0, 0, 0, retry_at_ms, 0, 0)
            }
            Self::Refused(TickStatus::OutOfRetention { retention_ms }) => {
                (0, 0, 0, 0, retention_ms, 0)
            }
            Self::Refused(TickStatus::RateLimited { retry_in_s }) => (0, 0, 0, 0, 0, retry_in_s),
            Self::Fit
            | Self::NoTape { .. }
            | Self::NoAddress
            | Self::Fetching
            | Self::Refused(
                TickStatus::Pending
                | TickStatus::Streaming
                | TickStatus::AwaitingCore
                | TickStatus::ContextUnavailable
                | TickStatus::NoRoute
                | TickStatus::Disabled
                | TickStatus::NoTrades
                | TickStatus::Failed
                | TickStatus::Served,
            )
            | Self::EntryMissed(_)
            | Self::ExitMissed(_)
            | Self::Unjudged(_)
            | Self::NoVerdict
            | Self::LostTape => (0, 0, 0, 0, 0, 0),
        };
        let expected = chrono::DateTime::from_timestamp_millis(retry_at_ms)
            .map(|time| time.format("%Y-%m-%d %H:%M UTC").to_string())
            .unwrap_or_else(|| retry_at_ms.to_string());
        t!(
            key.as_str(),
            trail = duration(trail_ms),
            min = duration(i64::from(min_s) * 1_000),
            length = duration(len_ms),
            expected = expected,
            hours = retention_ms / 3_600_000,
            seconds = retry_in_s
        )
        .to_string()
    }

    /// Compact reason shared by hover headings and the reason column.
    pub(in crate::analytics::tuner) fn short(self) -> String {
        self.translated("")
    }

    /// Only actions that can improve this deal are offered.
    fn todo(self) -> Option<String> {
        match self {
            Self::NoTape { fetchable: true } => Some(t!("analytics.ticks.why.fetch").to_string()),
            Self::NoAddress => Some(t!("analytics.ticks.why.connect").to_string()),
            Self::ShortTail { .. } => Some(t!("analytics.ticks.why.extend_tail").to_string()),
            Self::Refused(TickStatus::ArchivePending { .. }) => Some(self.translated(".todo")),
            Self::Fit
            | Self::NoTape { fetchable: false }
            | Self::Fetching
            | Self::Refused(
                TickStatus::Pending
                | TickStatus::Streaming
                | TickStatus::AwaitingCore
                | TickStatus::ContextUnavailable
                | TickStatus::NoRoute
                | TickStatus::Disabled
                | TickStatus::OutOfRetention { .. }
                | TickStatus::NoTrades
                | TickStatus::RateLimited { .. }
                | TickStatus::Failed
                | TickStatus::Served,
            )
            | Self::EntryMissed(_)
            | Self::ExitMissed(_)
            | Self::Unjudged(_)
            | Self::NoVerdict
            | Self::Hole { .. }
            | Self::LostTape => None,
        }
    }

    /// Tape and sample state without exposing debug variant names.
    pub(in crate::analytics::tuner) fn tape_tip(self) -> Tip {
        if self == Self::Fit {
            return Tip {
                what: t!("analytics.ticks.why.covered").to_string(),
                why: t!("analytics.ticks.why.covered.why").to_string(),
                todo: None,
                not_your_fault: false,
            };
        }
        Tip {
            what: self.short(),
            why: self.translated(".why"),
            todo: self.todo(),
            not_your_fault: !matches!(self, Self::ShortTail { .. }),
        }
    }

    /// Explain both model marks, including a factual entry that is allowed in the sample.
    pub(in crate::analytics::tuner) fn model_tip(
        self,
        verdict: Option<&moon_core::db::tuner::ticks::Verdict>,
    ) -> Tip {
        let mut tip = Tip {
            what: self.short(),
            why: self.translated(".why"),
            todo: self.todo(),
            not_your_fault: !matches!(self, Self::Fit | Self::ShortTail { .. }),
        };
        if let Some(verdict) = verdict {
            let entry = format!(
                "analytics.ticks.why.{}.why",
                entry_key(verdict.entry_finding)
            );
            let exit = format!(
                "analytics.ticks.why.{}.why",
                match verdict.exit_finding {
                    ExitFinding::Hit => "exit_hit",
                    ExitFinding::Miss(miss) => miss_key(miss),
                    ExitFinding::Unjudged(why) => unjudged_key(why),
                }
            );
            tip.why = format!("{}\n{}\n{}", t!(entry.as_str()), t!(exit.as_str()), tip.why);
            tip.not_your_fault |= verdict.exit != Some(true);
        }
        tip
    }

    /// A covered but incomplete tape is amber, so it cannot look ready for the search.
    pub(in crate::analytics::tuner) fn tape_glyph(self) -> (&'static str, Tone) {
        match self {
            Self::ShortTail { .. } => ("●", Tone::Warning),
            Self::Hole { .. } => ("◐", Tone::Warning),
            Self::Fit
            | Self::EntryMissed(_)
            | Self::ExitMissed(_)
            | Self::Unjudged(_)
            | Self::NoVerdict
            | Self::LostTape => ("●", Tone::Good),
            Self::NoTape { .. } | Self::Refused(TickStatus::ArchivePending { .. }) => {
                ("○", Tone::Muted)
            }
            Self::Fetching => ("…", Tone::Warning),
            Self::NoAddress => ("·", Tone::Muted),
            Self::Refused(
                TickStatus::Pending
                | TickStatus::Streaming
                | TickStatus::AwaitingCore
                | TickStatus::ContextUnavailable
                | TickStatus::NoRoute
                | TickStatus::Disabled
                | TickStatus::OutOfRetention { .. }
                | TickStatus::NoTrades
                | TickStatus::RateLimited { .. }
                | TickStatus::Failed
                | TickStatus::Served,
            ) => ("✕", Tone::Muted),
        }
    }
}

/// Entry findings remain distinct even when the entry is taken from the fact.
fn entry_key(finding: EntryFinding) -> &'static str {
    match finding {
        EntryFinding::Fact => "entry_fact",
        EntryFinding::Hit => "entry_hit",
        EntryFinding::Unfilled => "entry_unfilled",
        EntryFinding::Off => "entry_off",
    }
}

/// Preserve all same-rule disagreement details in one explanatory family.
fn miss_key(miss: ExitMiss) -> &'static str {
    match miss {
        ExitMiss::StopNotFired => "stop_not_fired",
        ExitMiss::NoLevel => "no_level",
        ExitMiss::Off(_) => "exit_off",
    }
}

/// A rule added upstream must acquire an explanation before this module compiles.
fn unjudged_key(why: Unjudged) -> &'static str {
    match why {
        Unjudged::Rule(rule) => match rule {
            UnmodelledRule::SellShot => "sell_shot",
            UnmodelledRule::SellSpread => "sell_spread",
            UnmodelledRule::NoAutoSell => "no_auto_sell",
        },
        Unjudged::InGap => "in_gap",
        Unjudged::TakeUnknown => "take_unknown",
        Unjudged::OtherRule => "other_rule",
    }
}

/// Reuse the tuner's localized duration units for measured tape lengths.
fn duration(ms: i64) -> String {
    if ms <= 0 {
        return t!("analytics.ticks.dur_s", s = 0).to_string();
    }
    let secs = (ms as u64) / 1_000;
    if (60..3_600).contains(&secs) && !secs.is_multiple_of(60) {
        format!(
            "{} {}",
            super::estimate::duration_text(std::time::Duration::from_secs(secs / 60 * 60)),
            super::estimate::duration_text(std::time::Duration::from_secs(secs % 60))
        )
    } else {
        super::estimate::duration_text(std::time::Duration::from_millis(ms as u64))
    }
}

#[cfg(test)]
mod tests;
