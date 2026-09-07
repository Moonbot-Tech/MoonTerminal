//! Publishing the core's temporary blacklist, and the test that decides when it is worth saying.
//!
//! Its own module rather than another block in the feed loop for the reason every sibling here
//! exists: `run()` already carries more than one screen of unrelated concerns, and a change test
//! belongs beside the state it compares, not a thousand lines from it.
//!
//! What the test is FOR: a TempBL row counts itself down on every snapshot. Publishing that would
//! wake every reader once per snapshot to say nothing, so decay is subtracted before two states are
//! compared, and only what is left over — a row appearing, going away, being respelled, extended,
//! or cut short — reaches the UI.

use std::time::{Duration, Instant};

use moonproto::MoonClient;

use crate::feed::TempBlacklistRow;

#[cfg(test)]
mod tests;

/// Slack around the expected decay: the snapshot was taken before it reached us, and the core
/// rounds. Below this a difference is the clock, not an edit.
const DECAY_SLACK: Duration = Duration::from_secs(60);

/// Per-core publisher: what the UI was last told, and what the diagnostic channel last wrote.
pub(super) struct TempBlacklistPublisher {
    /// Rows last handed to the UI, with the moment they were handed over — the baseline decay is
    /// measured from.
    published: Option<(Instant, Vec<TempBlacklistRow>)>,
    /// Last state written to `channels.settings`, compared only on its GLOBAL half; the rows use
    /// the same test the UI does, so the log and the panel cannot disagree about what happened.
    logged: Option<crate::settings_diag::BlacklistShot>,
    /// Rows as they stood at that line, with its moment.
    ///
    /// A baseline of its own, because the log and the UI advance at different times: the channel
    /// reports the state a core was already holding when it was switched on, while the UI is only
    /// told on a settings event. Sharing one baseline made the first event after the switch write
    /// the same rows a second time.
    logged_rows: Option<(Instant, Vec<TempBlacklistRow>)>,
    /// Whether the channel has already reported the state a core was holding when it was switched
    /// on. Without this the block would read a snapshot on every wake of the feed loop, which is
    /// exactly the per-batch cost the channel's own documentation promises it does not have.
    diag_primed: bool,
}

impl TempBlacklistPublisher {
    pub(super) fn new() -> Self {
        Self {
            published: None,
            logged: None,
            logged_rows: None,
            diag_primed: false,
        }
    }

    /// Look at the core's retained settings and decide what to say about its temporary blacklist.
    ///
    /// Writes the diagnostic line itself — that is a side effect on a file, not state anyone else
    /// reads — and hands back the rows the caller must publish, if any.
    ///
    /// Args:
    ///     client: Connected client whose retained snapshot holds the rows.
    ///     server_id: Core the line is stamped with.
    ///     settings_arrived: Whether this batch carried a `ClientSettingsUpdated`.
    ///
    /// Returns:
    ///     Rows to send as `FeedMsg::TempBlacklist`, or `None` when nothing changed.
    pub(super) fn poll(
        &mut self,
        client: &MoonClient,
        server_id: u64,
        settings_arrived: bool,
    ) -> Option<Vec<TempBlacklistRow>> {
        let diag_on = crate::settings_diag::enabled();
        if !diag_on {
            // Switched off: the next switch-on reports what the core holds then, not what it held
            // when the channel was last on.
            self.diag_primed = false;
        }
        let prime_diag = diag_on && !self.diag_primed;
        if !settings_arrived && !prime_diag {
            return None;
        }

        let held = client.snapshot().and_then(|state| {
            state.settings().client_settings.as_ref().map(|settings| {
                let rows = crate::feed::temp_blacklist_rows(settings);
                // The shot is built only for a channel that is on: it clones the whole blacklist
                // text and a string per row.
                let shot = diag_on.then(|| crate::settings_diag::BlacklistShot {
                    global_on: settings.use_coins_black_list,
                    global_text: settings.coins_black_list_text.clone(),
                    temp: settings
                        .temp_blacklist_entries()
                        .map(|row| (row.symbol.to_string(), row.remaining_days()))
                        .collect(),
                });
                (rows, shot)
            })
        });
        let (rows, shot) = held?;

        let since = self
            .published
            .as_ref()
            .map(|(at, _)| at.elapsed())
            .unwrap_or_default();
        let news = worth_publishing(
            self.published.as_ref().map(|(_, rows)| rows.as_slice()),
            since,
            &rows,
        );

        if let Some(shot) = shot {
            // Judged against the LOG's own baseline, by the same test the UI uses for its own: what
            // must never be compared directly is a remainder, which ticks on every snapshot.
            let global_changed = self.logged.as_ref().is_none_or(|prev| {
                prev.global_on != shot.global_on || prev.global_text != shot.global_text
            });
            let rows_changed = worth_publishing(
                self.logged_rows.as_ref().map(|(_, rows)| rows.as_slice()),
                self.logged_rows
                    .as_ref()
                    .map(|(at, _)| at.elapsed())
                    .unwrap_or_default(),
                &rows,
            );
            if global_changed || rows_changed {
                crate::settings_diag::line(&format!(
                    "core={} {}",
                    crate::feed::core_label(server_id),
                    shot.describe()
                ));
                self.logged = Some(shot);
                self.logged_rows = Some((Instant::now(), rows.clone()));
            }
            self.diag_primed = true;
        }

        if !settings_arrived || !news {
            return None;
        }
        self.published = Some((Instant::now(), rows.clone()));
        Some(rows)
    }
}

/// Whether this TempBL state is news, or the same rows one snapshot older.
///
/// Args:
///     published: Rows last handed to the UI, or `None` before the first publication.
///     since_published: Time elapsed since that publication, which is how much decay is expected.
///     held: Rows the core's snapshot holds now.
///
/// Returns:
///     `true` when the rows must be published.
fn worth_publishing(
    published: Option<&[TempBlacklistRow]>,
    since_published: Duration,
    held: &[TempBlacklistRow],
) -> bool {
    let Some(published) = published else {
        return true;
    };
    if published.len() != held.len() {
        return true;
    }
    // Keyed by symbol rather than by position: the wire order is the core's to choose, and a list
    // it merely reordered says nothing new.
    held.iter().any(|now| {
        let Some(was) = published
            .iter()
            .find(|was| was.symbol.eq_ignore_ascii_case(&now.symbol))
        else {
            return true;
        };
        // What the row WOULD hold if it had merely been counting down since the last publication.
        let expected = was.remaining.saturating_sub(since_published);
        now.remaining.abs_diff(expected) > DECAY_SLACK
    })
}
