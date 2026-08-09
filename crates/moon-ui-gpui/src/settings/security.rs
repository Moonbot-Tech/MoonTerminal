//! Security block of the General tab: the launch password and the `servers.enc` encryption
//! password, each behind its own checkbox.
//!
//! The two are deliberately separate settings because they protect different things, and saying so
//! in the UI is part of the feature:
//! - the **launch password** is a local gate. It stops someone sitting down at an unlocked machine
//!   from opening the terminal. It does NOT encrypt anything: the core keys stay readable to
//!   anyone who can read the files, exactly as they are today.
//! - the **encryption password** adds a password-derived key slot to `servers.enc`, so a copy of
//!   that file is useless on a machine whose OS keyring cannot open it — and so the file is
//!   recoverable when the keyring is lost.
//!
//! Draft state lives here rather than in `AppConfig` because neither password is a setting: they
//! are key material owned by `moon_core::config::crypto`, reached through [`vault`]. The checkbox
//! positions come from the FILE — what it already carries — so reopening Settings shows the truth
//! rather than whatever was typed last time.

mod strength;
mod vault;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonCheckbox, MoonCheckboxSize, MoonInput, MoonInputEvent,
    MoonInputState, MoonPalette, MoonProgress, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::SettingsView;
use crate::{Backend, design};
use strength::{Issue, Level};

/// Label column width in unscaled pixels, shared by every password row so the fields line up.
const LABEL_W: f32 = 150.0;
/// Password field width in unscaled pixels.
const FIELD_W: f32 = 240.0;

/// One password and its confirmation field.
///
/// The confirmation is not optional politeness: a mistyped encryption password is only discovered
/// on the machine where it is the last way in, which is the worst possible moment to find out.
pub(super) struct PasswordPair {
    value: Entity<MoonInputState>,
    confirm: Entity<MoonInputState>,
}

impl PasswordPair {
    /// Current text of both fields.
    fn read(&self, cx: &App) -> (String, String) {
        (
            self.value.read(cx).value().to_string(),
            self.confirm.read(cx).value().to_string(),
        )
    }

    /// Empty both fields, dropping the typed password from the UI.
    fn clear(&self, window: &mut Window, cx: &mut App) {
        for field in [&self.value, &self.confirm] {
            field.update(cx, |state, cx| state.set_value("", window, cx));
        }
    }
}

/// Which of the two password blocks something refers to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Block {
    Launch,
    File,
}

/// Outcome of the last security action, shown inside this block.
///
/// Deliberately NOT the Settings footer status: the footer reports whether the configuration was
/// written, and the two are independent. Writing a security failure there turned a successful
/// config save into a red "failed", which is the opposite of what saving actually did.
pub(super) struct SecurityStatus {
    key: &'static str,
    error: bool,
    /// Block the message is anchored under, or `None` for the section as a whole.
    block: Option<Block>,
}

/// Editor state for the security block.
pub(super) struct SecurityEd {
    launch_on: bool,
    launch: PasswordPair,
    file_on: bool,
    file: PasswordPair,
    /// What `servers.enc` currently carries, read once when Settings opens.
    vault: vault::VaultState,
    /// Result of the last Save or machine-slot action.
    status: Option<SecurityStatus>,
}

impl SecurityEd {
    /// Describe what this draft asks the key backend to do.
    ///
    /// `None` means the draft matches what the file already carries, so Save must stay silent
    /// about security. `Err(key)` is a localization key naming the reason the request cannot be
    /// honoured. `Ok(change)` is the request itself.
    ///
    /// An enabled checkbox with both fields empty is deliberately NOT an error once the password
    /// is already stored: that is the state of every Settings visit after the password was set,
    /// and demanding the old password to change an unrelated setting would be absurd.
    ///
    /// A rejected block aborts the whole request rather than applying the other half. The two
    /// passwords are written by one file operation, and half-applying a pair the user submitted
    /// together is the kind of partial success nobody expects to have to check for. The error
    /// carries its [`Block`] so the message can be rendered under the offending fields — the two
    /// pairs are visually identical, and an unanchored "passwords do not match" names neither.
    fn pending(&self, cx: &App) -> Option<Result<vault::VaultChange, (Block, &'static str)>> {
        let launch = match resolve_pair(
            &self.launch,
            self.launch_on,
            self.vault.launch_password_set,
            Block::Launch,
            cx,
        ) {
            Ok(request) => request,
            Err(problem) => return Some(Err(problem)),
        };
        let file = match resolve_pair(
            &self.file,
            self.file_on,
            self.vault.file_password_set,
            Block::File,
            cx,
        ) {
            Ok(request) => request,
            Err(problem) => return Some(Err(problem)),
        };
        (launch.is_some() || file.is_some()).then_some(Ok(vault::VaultChange {
            launch_password: launch,
            file_password: file,
        }))
    }

    /// Empty every password field after the draft has been accepted.
    fn clear_fields(&self, window: &mut Window, cx: &mut App) {
        self.launch.clear(window, cx);
        self.file.clear(window, cx);
    }
}

/// Resolve one password pair into the change it asks for.
///
/// Returns `None` for "leave as is", `Some(None)` for "remove", and `Some(Some(password))` for
/// "set". `require_strength` is the single difference between the two blocks: the launch gate is
/// explicitly allowed to be short, while the encryption password is the last way into the file.
fn resolve_pair(
    pair: &PasswordPair,
    enabled: bool,
    stored: bool,
    block: Block,
    cx: &App,
) -> Result<Option<Option<String>>, (Block, &'static str)> {
    let require_strength = block == Block::File;
    let (value, confirm) = pair.read(cx);
    match (enabled, stored) {
        (false, true) => Ok(Some(None)),
        (true, _) if !value.is_empty() || !confirm.is_empty() => {
            if value != confirm {
                return Err((block, "security.err.mismatch"));
            }
            if require_strength && !strength::estimate(&value).accepted() {
                return Err((block, "security.err.weak"));
            }
            Ok(Some(Some(value)))
        }
        (true, false) => Err((block, "security.err.empty")),
        _ => Ok(None),
    }
}

/// Build a masked password field that repaints the section on every keystroke.
///
/// The repaint is required rather than incidental: the strength meter, the mismatch line, and the
/// Save-blocking state are siblings of the field, and `MoonInput` only invalidates itself.
fn password_field(window: &mut Window, cx: &mut Context<SettingsView>) -> Entity<MoonInputState> {
    let state = cx.new(|cx| MoonInputState::new(window, cx).masked(true));
    cx.subscribe(&state, |this, _emitter, ev: &MoonInputEvent, cx| {
        if matches!(ev, MoonInputEvent::Change) {
            // Editing answers the previous complaint, so drop it instead of leaving a stale red
            // line under a field the user is already fixing.
            this.security.status = None;
            cx.notify();
        }
    })
    .detach();
    state
}

/// Build the security block's editor state from the current `servers.enc` access state.
pub(super) fn build(
    _backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> SecurityEd {
    let vault = vault::state();
    SecurityEd {
        launch_on: vault.launch_password_set,
        launch: PasswordPair {
            value: password_field(window, cx),
            confirm: password_field(window, cx),
        },
        file_on: vault.file_password_set,
        file: PasswordPair {
            value: password_field(window, cx),
            confirm: password_field(window, cx),
        },
        vault,
        status: None,
    }
}

/// Build one `label · field` row.
fn password_row(
    cx: &App,
    id: &'static str,
    label: String,
    state: &Entity<MoonInputState>,
    enabled: bool,
) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    let color = if enabled { p.text_soft } else { p.text_muted };
    h_flex()
        .gap(design::ui_px(cx, 10.0))
        .items_center()
        .child(
            div()
                .w(design::font_w_px(cx, LABEL_W))
                .text_color(rgba_from(color, 1.0))
                .child(label),
        )
        .child(
            div().w(design::font_w_px(cx, FIELD_W)).child(
                MoonInput::new(id)
                    .state(state)
                    .small()
                    .disabled(!enabled)
                    // The eye is what makes a masked field usable; without it the only way to
                    // check a long password is to retype it.
                    .mask_toggle(),
            ),
        )
}

/// Build the red "the two fields differ" line, or nothing while there is nothing to compare.
///
/// Silent until the confirmation has content: complaining about a mismatch after the first typed
/// character is noise, since every partially typed confirmation differs.
fn mismatch_line(cx: &App, value: &str, confirm: &str) -> Option<AnyElement> {
    let p = MoonPalette::active(cx);
    (!confirm.is_empty() && confirm != value).then(|| {
        div()
            .text_color(rgba_from(p.red, 1.0))
            .child(t!("security.mismatch").to_string())
            .into_any_element()
    })
}

impl SettingsView {
    /// Build a checkbox that toggles local security state rather than the config draft.
    fn security_checkbox(
        &self,
        cx: &Context<Self>,
        id: &'static str,
        checked: bool,
        set: fn(&mut SecurityEd, bool),
    ) -> MoonCheckbox {
        MoonCheckbox::new(id)
            .checked(checked)
            .size(MoonCheckboxSize::Normal)
            // Nothing to attach a password to until the config file is open.
            .disabled(!self.security.vault.editable)
            .on_change(cx.listener(move |this, checked: &bool, _window, cx| {
                set(&mut this.security, *checked);
                cx.notify();
            }))
    }

    /// Build the launch-password group: checkbox, both fields, and the mismatch line.
    fn launch_group(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let on = self.security.launch_on;
        let (value, confirm) = self.security.launch.read(cx);
        v_flex()
            .gap(design::ui_px(cx, 4.0))
            .child(
                self.security_checkbox(cx, "sec-launch", on, |ed, v| ed.launch_on = v)
                    .label(t!("security.launch").to_string()),
            )
            .child(
                div()
                    .text_color(rgba_from(p.text_muted, 1.0))
                    .child(t!("security.launch_hint").to_string()),
            )
            .when(on, |this| {
                this.child(password_row(
                    cx,
                    "sec-launch-pw",
                    t!("security.password").to_string(),
                    &self.security.launch.value,
                    true,
                ))
                .child(password_row(
                    cx,
                    "sec-launch-confirm",
                    t!("security.confirm").to_string(),
                    &self.security.launch.confirm,
                    true,
                ))
                .children(mismatch_line(cx, &value, &confirm))
                .children(self.security_status_line(cx, Some(Block::Launch)))
            })
    }

    /// Build the encryption-password group: checkbox, both fields, strength meter, and the
    /// machine-slot row.
    fn file_group(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let on = self.security.file_on;
        let (value, confirm) = self.security.file.read(cx);
        v_flex()
            .gap(design::ui_px(cx, 4.0))
            .child(
                self.security_checkbox(cx, "sec-file", on, |ed, v| ed.file_on = v)
                    .label(t!("security.file").to_string()),
            )
            .child(
                div()
                    .text_color(rgba_from(p.text_muted, 1.0))
                    .child(t!("security.file_hint").to_string()),
            )
            .when(on, |this| {
                this.child(password_row(
                    cx,
                    "sec-file-pw",
                    t!("security.password").to_string(),
                    &self.security.file.value,
                    true,
                ))
                .child(strength_meter(&value, p, cx))
                .child(password_row(
                    cx,
                    "sec-file-confirm",
                    t!("security.confirm").to_string(),
                    &self.security.file.confirm,
                    true,
                ))
                .children(mismatch_line(cx, &value, &confirm))
                .children(self.security_status_line(cx, Some(Block::File)))
                .child(
                    div()
                        .text_color(rgba_from(p.text_muted, 1.0))
                        .child(t!("security.file_warning").to_string()),
                )
            })
            .child(self.machines_row(cx))
    }

    /// Build the row reporting how many machines can open `servers.enc` silently.
    ///
    /// Visible even with no password set, because the count is the honest answer to "who else can
    /// open this file" and it is the only place that question is asked.
    ///
    /// The count reads as a fact about the user's file, so in the preview build it shows a dash
    /// rather than the stub's number: nothing here has counted anything yet.
    fn machines_row(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let used = self.security.vault.machine_slots;
        // With no file open the count is not zero, it is unknown; printing "0 of 8" would be a
        // statement about the user's file that we did not read.
        let text = if self.security.vault.editable {
            t!(
                "security.machines",
                used = used,
                max = vault::MAX_MACHINE_SLOTS
            )
        } else {
            t!("security.machines_unknown", max = vault::MAX_MACHINE_SLOTS)
        };
        h_flex()
            .mt(design::ui_px(cx, 6.0))
            .gap(design::ui_px(cx, 10.0))
            .items_center()
            .child(
                div()
                    .text_color(rgba_from(p.text_soft, 1.0))
                    .child(text.to_string()),
            )
            .child(
                MoonButton::new("sec-forget-machines")
                    .outline()
                    .size(MoonButtonSize::Micro)
                    // One slot means only this machine, so there is nothing to forget.
                    .disabled(used <= 1 || !self.security.vault.editable)
                    .label(format!("  {}  ", t!("security.forget_machines")))
                    .on_click(cx.listener(|this, _, window, cx| this.forget_machines(window, cx)))
                    .render(),
            )
    }

    /// Build the message line for one anchor, or nothing when the last action said nothing there.
    fn security_status_line(&self, cx: &Context<Self>, block: Option<Block>) -> Option<AnyElement> {
        let p = MoonPalette::active(cx);
        let status = self.security.status.as_ref().filter(|s| s.block == block)?;
        let color = if status.error { p.red } else { p.green };
        Some(
            div()
                .text_color(rgba_from(color, 1.0))
                .child(t!(status.key).to_string())
                .into_any_element(),
        )
    }

    /// Handle the "forget other machines" button.
    ///
    /// Saves immediately rather than waiting for the Save button. Revocation is a security action
    /// the user expects to have taken effect when it reports success; leaving it in memory meant
    /// closing Settings silently kept every revoked machine's access.
    fn forget_machines(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = vault::forget_other_machines() {
            self.security.status = Some(SecurityStatus {
                key: error.message_key(),
                error: true,
                block: None,
            });
            cx.notify();
            return;
        }
        self.security.vault = vault::state();
        self.save(window, cx);
        // `save` reports the configuration write in the footer; this line reports what the write
        // meant here. A failed write leaves the revocation in memory only, and says so.
        let saved = matches!(self.status, Some((super::StatusMsg::Key(_), false)));
        self.security.status = Some(SecurityStatus {
            key: if saved {
                "security.machines_forgotten"
            } else {
                "security.err.not_saved"
            },
            error: !saved,
            block: None,
        });
        cx.notify();
    }

    /// Apply the security draft after the configuration itself has been saved.
    ///
    /// Called from `save` rather than owning its own button so both passwords land in the same
    /// user action as the rest of Settings. It runs BEFORE the config write, because that write is
    /// what persists the resulting key slots, and reports into the security block's own status
    /// line, so a security failure neither blocks the other settings nor makes a successful save
    /// look failed.
    /// Returns whether a change was accepted into key material and is now waiting for the config
    /// write. The caller reports the outcome once that write has actually happened — see
    /// [`Self::report_security_saved`].
    pub(super) fn apply_security(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(request) = self.security.pending(cx) else {
            return false;
        };
        let (status, accepted) = match request {
            Err((block, key)) => (
                SecurityStatus {
                    key,
                    error: true,
                    block: Some(block),
                },
                false,
            ),
            Ok(change) => match vault::apply(change) {
                Ok(()) => {
                    self.security.vault = vault::state();
                    // Empty the fields once they have been accepted. Leaving them filled makes
                    // every later Save re-submit the same password, which re-derives the key and
                    // replaces the slot for no reason — a visible pause and a new slot per click.
                    self.security.clear_fields(window, cx);
                    // No status yet: the passwords are in memory, not on disk.
                    (
                        SecurityStatus {
                            key: "security.pending_write",
                            error: false,
                            block: None,
                        },
                        true,
                    )
                }
                Err(error) => (
                    SecurityStatus {
                        key: error.message_key(),
                        error: true,
                        block: None,
                    },
                    false,
                ),
            },
        };
        self.security.status = Some(status);
        cx.notify();
        accepted
    }

    /// Report what happened to an accepted password change once the config write has finished.
    ///
    /// Split from [`Self::apply_security`] because the two facts are independent: the key material
    /// accepted the password, and the file write either stored it or did not. Reporting "saved"
    /// from the first alone put a green line over a password that a failed write left only in
    /// memory — where it survives until the process exits and then is gone.
    pub(super) fn report_security_saved(&mut self, saved: bool, cx: &mut Context<Self>) {
        self.security.status = Some(SecurityStatus {
            key: if saved {
                "security.applied"
            } else {
                "security.err.not_saved"
            },
            error: !saved,
            block: None,
        });
        cx.notify();
    }

    /// Build the complete security block for the General tab.
    pub(super) fn security_section(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        v_flex()
            .w_full()
            .gap(design::ui_px(cx, 4.0))
            .child(super::section(&t!("security.title"), p, cx))
            // Without an opened config file there is no key material to attach a password to.
            // Say so instead of collecting one that cannot be stored.
            .when(!self.security.vault.editable, |this| {
                this.child(
                    div()
                        .text_color(rgba_from(p.amber, 1.0))
                        .child(t!("security.not_editable").to_string()),
                )
            })
            .child(self.launch_group(cx))
            .child(super::separator(p, cx))
            .child(self.file_group(cx))
            // Outcome of the last Save or slot action. Anchored to the section, not the window
            // footer, which reports only whether the configuration itself was written.
            .children(self.security_status_line(cx, None))
    }
}

/// Build the strength meter: a bar plus its verdict, or nothing while the field is empty.
fn strength_meter(password: &str, p: MoonPalette, cx: &App) -> impl IntoElement {
    let verdict = strength::estimate(password);
    let (color, level_key) = match verdict.level {
        Level::Empty => (p.text_muted, "security.level.empty"),
        Level::Weak => (p.red, "security.level.weak"),
        Level::Fair => (p.orange, "security.level.fair"),
        Level::Good => (p.green, "security.level.good"),
        Level::Strong => (p.green, "security.level.strong"),
    };
    // Name the problem rather than only the score: "Слабый" tells the user nothing about what to
    // change, and an unexplained rejection reads as the field being broken.
    let note = match verdict.issue {
        _ if verdict.level == Level::Empty => {
            t!("security.rule", min = strength::MIN_LEN).to_string()
        }
        Some(Issue::TooShort) => t!("security.issue.short", min = strength::MIN_LEN).to_string(),
        Some(Issue::Common) => t!("security.issue.common").to_string(),
        Some(Issue::Repeats) => t!("security.issue.repeats").to_string(),
        Some(Issue::Sequence) => t!("security.issue.sequence").to_string(),
        Some(Issue::OneClass) => t!("security.issue.one_class").to_string(),
        Some(Issue::LowEntropy) => t!("security.issue.low_entropy").to_string(),
        None => t!("security.issue.none").to_string(),
    };
    v_flex()
        .gap(design::ui_px(cx, 2.0))
        .child(
            h_flex()
                .gap(design::ui_px(cx, 10.0))
                .items_center()
                .child(
                    div().w(design::font_w_px(cx, LABEL_W)).child(
                        MoonProgress::new("sec-file-strength")
                            .value(verdict.percent())
                            .color(color)
                            .height(design::ui_value(cx, 4.0))
                            .render(),
                    ),
                )
                .child(
                    div()
                        .text_color(rgba_from(color, 1.0))
                        .child(t!(level_key).to_string()),
                )
                .child(
                    div()
                        .text_color(rgba_from(p.text_muted, 1.0))
                        .text_size(design::t_caption(cx))
                        .child(t!("security.bits", bits = verdict.bits.round() as i32).to_string()),
                ),
        )
        .child(
            div()
                .text_color(rgba_from(p.text_muted, 1.0))
                .text_size(design::t_caption(cx))
                .child(note),
        )
}
