//! Runtime state and compact command vocabulary.

/// Core runtime state from moonproto `RuntimeState`: whether the market runtime is running and
/// automatic detection is active. Passive mode is specifically `is_started=true` with
/// `auto_detect_active=false`; a false value alone does not identify passive mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RuntimeState {
    pub is_started: bool,
    pub auto_detect_active: bool,
}

/// Targeted `ClientSettings` edit from the toolbar.
///
/// The feed applies it through public helpers to the retained moonproto snapshot at
/// `client.snapshot().settings().client_settings`, preserving append-only tails and AutoStart
/// blobs invisible to the UI. It then sends the full snapshot back to the core through
/// `settings().send`.
#[derive(Debug, Clone, Copy)]
pub enum ClientSettingsEdit {
    /// Main take-profit percentage and extended-range mode from `x_tmode` or `s9`. With
    /// `extended`, writes `x_tmode=true` and `x_sell=round(pct/10)` for 100..900%; otherwise writes
    /// `x_tmode=false` and `x_sell=round(pct)` for 1..100%. Clears fixed-sell and scalp modes.
    TakeProfit { pct: f64, extended: bool },
    /// Stop-loss or price-drop level as a signed core percentage in -20..+1.
    StopLossPct(f32),
    /// Scalp take profit for the fine TP slider, stored as a sub-percent value through
    /// `x_sell_scalp` with `x_sell=0`. The core's actual step is 1/50, or 0.02%. Clears fixed-sell.
    ScalpTakeProfit(f64),
    /// Select a fixed-sell slot in 1..=6 from buttons S1-S6, enabling `fixed_sell_mode`.
    SelectFixedSellSlot(usize),
    /// Return control to the main TP by setting `fixed_sell_mode=false` without changing the TP
    /// value in `x_sell` or scalp. Triggered by the TP button or a second click on the active S slot.
    EngageMainTakeProfit,
    /// Fixed-sell preset value as a slot in 1..=6 and visible percentage, edited by the wheel or
    /// inline editing on an S button.
    SetFixedSellPct { slot: usize, pct: f64 },
    /// Use a stop-market rather than stop-limit order through `use_stop_market`.
    UseStopMarket(bool),
    /// Panic on price drop through `panic_if_price_drop`.
    PanicIfPriceDrop(bool),
    /// Order signing through `sign_orders`.
    SignOrders(bool),
    /// Core emulator mode through `emu_mode`.
    EmuMode(bool),
}

/// Profit counter to reset through moonproto `ResetProfitKind`, selected by the Session or
/// All-Time buttons in the core-settings popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetProfitKind {
    /// Current trading session.
    Session,
    /// All accumulated time.
    All,
}

/// Which build to ask a core's own updater to install, mirroring moonproto's
/// `request_release_update`/`request_version_update` split on `MoonSettings`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum UpdateTarget {
    /// The latest ordinary release build, through `request_release_update`.
    Release,
    /// A named beta or test build, through `request_version_update`.
    Named(String),
}

impl UpdateTarget {
    /// Letter a named install must report, or `None` for a release.
    ///
    /// Strips a case-insensitive `MoonBot-` prefix and keeps the remainder as written
    /// (`MoonBot-R3` -> `R3`, `moonbot-r3` -> `r3`). A prefix that leaves nothing, or a name
    /// with no such prefix, is the whole name. [`UpdateTarget::Release`] has no letter.
    ///
    /// Args:
    ///     self: The install target whose name, if any, is read.
    ///
    /// Returns:
    ///     The expected handshake letter, or `None` when the target is a release.
    pub fn expected_suffix(&self) -> Option<&str> {
        match self {
            UpdateTarget::Release => None,
            UpdateTarget::Named(name) => Some(named_build_label(name)),
        }
    }
}

/// Letter of a named build: the part after a case-insensitive `MoonBot-` prefix, or the whole
/// name when that prefix is absent or leaves nothing.
///
/// `MoonBot-R3` is `R3` and `moonbot-r3` is `r3`. The remainder keeps the case it was written in.
///
/// Args:
///     name: Build name as stored on [`UpdateTarget::Named`], not a display string.
///
/// Returns:
///     A subslice of `name`.
pub fn named_build_label(name: &str) -> &str {
    const PREFIX: &str = "MoonBot-";
    name.get(..PREFIX.len())
        .filter(|head| head.eq_ignore_ascii_case(PREFIX))
        .map(|_| &name[PREFIX.len()..])
        .filter(|rest| !rest.is_empty())
        .unwrap_or(name)
}

/// Leading command word accepted in the [`UpdateTarget::Named`] field when a tester pastes a
/// complete install command. Keep it here so the normalizer, placeholder, and hint share one
/// spelling rather than drifting from the protocol convention.
pub const CORE_UPDATE_COMMAND_WORD: &str = "InstallTestVersion";

/// Trailing broadcast token of a pasted install command (`InstallTestVersion MoonBot-R2 ALL`),
/// meaning "all bots" in the Telegram form. See [`normalize_named_build`].
pub const CORE_UPDATE_ALL_WORD: &str = "ALL";

/// Protocol error code the core answers with when it refuses a named/test build target. Machine-
/// stable and language-independent, unlike the prose sentence that rides beside it — a core build
/// can reword the sentence, never this code. See [`is_core_update_rejection`].
pub const CORE_UPDATE_REJECT_CODE: &str = "BGF-SUB4";

/// Strip a leading [`CORE_UPDATE_COMMAND_WORD`] typed into the named/test build field, so both
/// `MoonBot-F8` and `InstallTestVersion MoonBot-F8` reach the updater as the bare version name.
/// Trims, collapses internal whitespace runs to a single space, drops a case-insensitive leading
/// command-word TOKEN (whitespace-delimited,
/// so `installtestversion-foo` is one token and is left alone), then drops a case-insensitive
/// trailing [`CORE_UPDATE_ALL_WORD`] token when a name remains before it. Returns `None`
/// when nothing remains — a value that is only the command word is refused rather than sent as an
/// empty name.
pub fn normalize_named_build(raw: &str) -> Option<String> {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim();
    let without_command = trimmed
        .split_once(char::is_whitespace)
        .filter(|(head, _)| head.eq_ignore_ascii_case(CORE_UPDATE_COMMAND_WORD))
        .map(|(_, rest)| rest.trim())
        .unwrap_or_else(|| {
            if trimmed.eq_ignore_ascii_case(CORE_UPDATE_COMMAND_WORD) {
                ""
            } else {
                trimmed
            }
        });
    // `InstallTestVersion MoonBot-R2 ALL` is the broadcast form ("every bot"); the trailing
    // standalone token is not part of the build name. Kept when it is the only token left.
    let without_all = without_command
        .rsplit_once(' ')
        .filter(|(_, tail)| tail.eq_ignore_ascii_case(CORE_UPDATE_ALL_WORD))
        .map_or(without_command, |(head, _)| head);
    if without_all.is_empty() {
        None
    } else {
        Some(without_all.to_string())
    }
}

/// Whether a raw `ServerLogEvent.msg` line is the core refusing a named build.
///
/// `msg` is arbitrary decoded core text. Returns true only when
/// [`CORE_UPDATE_REJECT_CODE`] is written as an error code: its own token, then
/// `:`, then the refusal text, as in `BGF-SUB4: Wrong version name!`. A line
/// where those letters are only the core's name (`BGF-SUB4`) or a folder
/// (`Updater prepared: C:\...\BGF-SUB4\updater.exe`) returns false — that token
/// standing alone is how a core of this name used to close a real install as
/// refused. The prose after the colon is not compared; a core build can reword
/// it, and a colon with no text is not a refusal.
///
/// The character before the code must be absent or outside an identifier (ASCII
/// alphanumeric, `-`, or `_`). Case-sensitive. A longer sibling (`BGF-SUB40:`),
/// a prefixed token (`XBGF-SUB4:`), and an underscore or hyphen join
/// (`BGF-SUB4_foo:`, `foo-BGF-SUB4:`) do not match. Checking only the colon
/// would still let `XBGF-SUB4:` free a lane while an update is running.
pub fn is_core_update_rejection(msg: &str) -> bool {
    let code = CORE_UPDATE_REJECT_CODE;
    let is_token_char = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    for (start, _) in msg.match_indices(code) {
        let end = start + code.len();
        let before_ok = msg[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !is_token_char(c));
        if !before_ok {
            continue;
        }
        // The code is the line's error code only when `:` introduces the refusal text.
        let Some(rest) = msg[end..].strip_prefix(':') else {
            continue;
        };
        if rest.chars().any(|c| !c.is_whitespace()) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests;
