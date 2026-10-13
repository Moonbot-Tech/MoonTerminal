//! Security page: the launch password and the `servers.enc` encryption password, each in its own
//! card behind its own checkbox, and the machines that open the file without a password.
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
use std::rc::Rc;

use moon_core::config::AppConfig;
use moon_ui::{
    MoonAlert, MoonButton, MoonButtonVariant, MoonCheckbox, MoonInput, MoonInputEvent,
    MoonInputState, MoonPalette, MoonProgress, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::SettingsView;
use crate::{Backend, design};
use strength::{Issue, Level};

/// Label column width in unscaled pixels, shared by every password row so the fields line up.
const LABEL_W: f32 = 90.0;
/// Widest a password field grows, in unscaled pixels; narrower cards shrink it.
const FIELD_W: f32 = 320.0;
/// Narrowest a password card gets before the two cards stack, in unscaled pixels.
const CARD_MIN_W: f32 = 300.0;

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
    /// Key slots as they were before a change was accepted into key material that no config write
    /// has stored yet. Set by [`SettingsView::apply_security`] and the machine revocation; a
    /// successful write drops it, a failed one or a discard puts it back, so key material never
    /// keeps a change that is not on disk for some later, unrelated write to persist.
    pub(super) unpersisted: Option<vault::Snapshot>,
}

/// What [`SettingsView::apply_security`] did with the password draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SecurityApply {
    /// No password change was asked for.
    Nothing,
    /// The change is in key material and waits for the config write.
    Accepted,
    /// The change was refused; the locale key names why.
    Rejected(&'static str),
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

    /// Whether this draft is asking Save to do anything at all.
    ///
    /// Exposed for the Save button's dirty indicator, which must count a pending password change:
    /// the password draft lives OUTSIDE `AppConfig` by the design stated at the top of this file,
    /// and `save` applies it BEFORE the config write, so a password-only edit makes the very same
    /// button write key slots to disk. An `AppConfig`-only comparison would render it clean.
    ///
    /// A boolean rather than the request itself, deliberately: [`Self::pending`] returns the
    /// private `vault::VaultChange`, and the footer has no business reaching it -- it needs to
    /// know THAT there is work, never what the work is.
    ///
    /// An INVALID request (mismatched pair) counts as pending: the user typed something, Save
    /// will act on it and report the error, so "no changes" would be false there too.
    ///
    /// Args:
    ///     cx: Application context used to read the password input states.
    ///
    /// Returns:
    ///     `true` when Save must process a password change or report an invalid request.
    pub(super) fn has_pending_request(&self, cx: &App) -> bool {
        self.pending(cx).is_some()
    }

    /// Whether the Security page holds work a Save still has to do: a typed request, or an
    /// accepted change whose config write has not succeeded yet.
    pub(super) fn has_unsaved(&self, cx: &App) -> bool {
        self.unpersisted.is_some() || self.has_pending_request(cx)
    }

    /// Empty every password field after the draft has been accepted.
    pub(super) fn clear_fields(&self, window: &mut Window, cx: &mut App) {
        self.launch.clear(window, cx);
        self.file.clear(window, cx);
    }

    /// Put both checkboxes back on what the file carries, so a revert drops a ticked or unticked
    /// box together with the typed passwords.
    pub(super) fn revert_toggles(&mut self) {
        (self.launch_on, self.file_on) = saved_toggles(&self.vault);
    }
}

/// The checkbox states a fresh Security page shows for `vault`: ticked where a password is stored.
pub(super) fn saved_toggles(vault: &vault::VaultState) -> (bool, bool) {
    (vault.launch_password_set, vault.file_password_set)
}

/// What a config write means for key slots changed before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Settle {
    /// Nothing was waiting for the write.
    Nothing,
    /// The slots reached disk: forget the snapshot, empty the password fields.
    Commit,
    /// The slots did not reach disk: put the snapshot back, keep the typed passwords for a retry.
    Rollback,
}

/// Decide what a finished config write does to accepted key-slot changes.
///
/// Args:
///     held: A pre-change snapshot is held, i.e. a change waits for this write.
///     write_ok: The config write succeeded.
///
/// Returns:
///     What to do with the held snapshot.
pub(super) fn settle(held: bool, write_ok: bool) -> Settle {
    match (held, write_ok) {
        (false, _) => Settle::Nothing,
        (true, true) => Settle::Commit,
        (true, false) => Settle::Rollback,
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
    let (value, confirm) = pair.read(cx);
    resolve(value, confirm, enabled, stored, block)
}

/// The request one password block makes, from its typed texts and checkbox.
///
/// Args:
///     value: The password field.
///     confirm: The confirmation field.
///     enabled: The block's checkbox.
///     stored: The file already carries this password.
///     block: Which block, for the error anchor and the strength rule.
///
/// Returns:
///     `Ok(None)` when nothing changes, `Ok(Some(None))` to remove, `Ok(Some(Some(pw)))` to set.
pub(super) fn resolve(
    value: String,
    confirm: String,
    enabled: bool,
    stored: bool,
    block: Block,
) -> Result<Option<Option<String>>, (Block, &'static str)> {
    let require_strength = block == Block::File;
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
        unpersisted: None,
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
        .w_full()
        .child(
            div()
                .w(design::font_w_px(cx, LABEL_W))
                .flex_none()
                .text_color(rgba_from(color, 1.0))
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .max_w(design::font_w_px(cx, FIELD_W))
                .child(
                    MoonInput::new(id)
                        .state(state)
                        .size(design::INPUT_SIZE)
                        .disabled(!enabled)
                        // The Settings root renders prose in the UI face and `MoonInput` inherits it.
                        // A secret must not: the eye below reveals it, and a key is read character by
                        // character, where a proportional face merges 0 with O and l with 1. Same pin
                        // as `window/login.rs`'s password field, for the same reason.
                        .mono(true)
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
            // Nothing to attach a password to until the config file is open.
            .disabled(!self.security.vault.editable)
            .on_change(cx.listener(move |this, checked: &bool, _window, cx| {
                set(&mut this.security, *checked);
                cx.notify();
            }))
    }

    /// Build the launch-password card: title and checkbox, both fields, and the mismatch line.
    fn launch_group(&self, cx: &Context<Self>) -> AnyElement {
        let on = self.security.launch_on;
        let (value, confirm) = self.security.launch.read(cx);
        let mut rows = Vec::new();
        if on {
            rows.push(
                password_row(
                    cx,
                    "sec-launch-pw",
                    t!("security.password").to_string(),
                    &self.security.launch.value,
                    true,
                )
                .into_any_element(),
            );
            rows.push(
                password_row(
                    cx,
                    "sec-launch-confirm",
                    t!("security.confirm").to_string(),
                    &self.security.launch.confirm,
                    true,
                )
                .into_any_element(),
            );
            rows.extend(mismatch_line(cx, &value, &confirm));
            rows.extend(self.security_status_line(cx, Some(Block::Launch)));
        }
        password_card(
            "security-launch-card",
            t!("security.launch.title").to_string(),
            t!("security.launch.hint").to_string(),
            self.security_checkbox(cx, "sec-launch", on, |ed, v| ed.launch_on = v)
                .into_any_element(),
            rows,
            cx,
        )
    }

    /// Build the encryption-password card: title and checkbox, both fields, the strength meter,
    /// and the warning that this password cannot be recovered.
    fn file_group(&self, cx: &Context<Self>) -> AnyElement {
        let p = MoonPalette::active(cx);
        let on = self.security.file_on;
        let (value, confirm) = self.security.file.read(cx);
        let mut rows = Vec::new();
        if on {
            rows.push(
                password_row(
                    cx,
                    "sec-file-pw",
                    t!("security.password").to_string(),
                    &self.security.file.value,
                    true,
                )
                .into_any_element(),
            );
            rows.push(strength_meter(&value, p, cx).into_any_element());
            rows.push(
                password_row(
                    cx,
                    "sec-file-confirm",
                    t!("security.confirm").to_string(),
                    &self.security.file.confirm,
                    true,
                )
                .into_any_element(),
            );
            rows.extend(mismatch_line(cx, &value, &confirm));
            rows.extend(self.security_status_line(cx, Some(Block::File)));
            rows.push(unrecoverable_alert());
        }
        password_card(
            "security-file-card",
            t!("security.file.title").to_string(),
            t!("security.file.hint").to_string(),
            self.security_checkbox(cx, "sec-file", on, |ed, v| ed.file_on = v)
                .into_any_element(),
            rows,
            cx,
        )
    }

    /// Build the control reporting how many machines can open `servers.enc` silently, with the
    /// button that revokes all but this one.
    ///
    /// Visible even with no password set, because the count is the honest answer to "who else can
    /// open this file" and it is the only place that question is asked.
    ///
    /// The count reads as a fact about the user's file, so with no file open it shows a dash
    /// rather than a number: nothing here has counted anything yet.
    fn machines_row(&self, cx: &Context<Self>) -> AnyElement {
        let p = MoonPalette::active(cx);
        let used = self.security.vault.machine_slots;
        let editable = self.security.vault.editable;
        let count = if editable {
            used.to_string()
        } else {
            "\u{2014}".to_string()
        };
        let text = t!(
            "security.machines.count",
            n = count,
            max = vault::MAX_MACHINE_SLOTS
        );
        h_flex()
            .gap(design::ui_px(cx, 10.0))
            .items_center()
            .child(
                div()
                    // MIXED NODE: the locale string welds "of" to the used/max slot FIGURES, and
                    // half a node cannot be styled. The figure wins.
                    .font_family(design::mono())
                    .text_color(rgba_from(p.text, 1.0))
                    .child(text.to_string()),
            )
            .child(
                MoonButton::new("sec-forget-machines")
                    .variant(MoonButtonVariant::OutlineRed)
                    .size(design::CONTROL_TIER)
                    // One slot means only this machine, so there is nothing to forget.
                    .disabled(used <= 1 || !editable)
                    .when(!editable, |b| {
                        b.tooltip(t!("security.not_editable").to_string())
                    })
                    .when(editable && used <= 1, |b| {
                        b.tooltip(t!("security.machines.keep_only_off").to_string())
                    })
                    .label(t!("security.machines.keep_only").to_string())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.confirm_forget_machines(window, cx)),
                    )
                    .render(),
            )
            .into_any_element()
    }

    /// Ask before revoking every other machine: the revocation is written at once and there is
    /// no Save to back out of.
    fn confirm_forget_machines(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        let forget: super::common::Action = Rc::new(move |window, cx| {
            let _ = this.update(cx, |view, cx| view.forget_machines(window, cx));
        });
        super::common::confirm(
            window,
            cx,
            super::common::Confirm {
                title: t!("security.machines.confirm_title").to_string(),
                body: t!("security.machines.confirm_body").to_string(),
                buttons: vec![
                    super::common::ConfirmBtn {
                        label: t!("dialogs.cancel").to_string(),
                        kind: super::common::ConfirmKind::Cancel,
                        on: Rc::new(|_, _| {}),
                    },
                    super::common::ConfirmBtn {
                        label: t!("security.machines.confirm_btn").to_string(),
                        kind: super::common::ConfirmKind::DangerSolid,
                        on: forget,
                    },
                ],
            },
        );
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

    /// Revoke every other machine and write that at once.
    ///
    /// Writes immediately rather than waiting for the Save button. Revocation is a security action
    /// the user expects to have taken effect when it reports success; leaving it in memory meant
    /// closing Settings silently kept every revoked machine's access.
    ///
    /// What is written is the SAVED configuration, never the draft: the key slots ride along in
    /// `servers.enc`, so any config write stores them, and writing the draft here would quietly
    /// save every other pending edit too. The draft stays as it was, still dirty, still the
    /// user's to save or drop. Typed passwords stay in their fields for the same reason.
    fn forget_machines(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let before = vault::snapshot();
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
        // An earlier held snapshot is older and covers this revocation too.
        self.security.unpersisted.get_or_insert(before);
        let saved =
            self.backend
                .update(cx, |b, bcx| match write_saved(&b.config, AppConfig::save) {
                    Ok(written) => {
                        b.config = written;
                        bcx.notify();
                        true
                    }
                    Err(error) => {
                        log::warn!("settings: writing the machine revocation failed: {error:#}");
                        false
                    }
                });
        match settle(self.security.unpersisted.is_some(), saved) {
            Settle::Nothing => {}
            Settle::Commit => self.security.unpersisted = None,
            Settle::Rollback => self.discard_security_slots(),
        }
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
    /// Returns what happened to the draft. An accepted change marks the page unpersisted until a
    /// config write stores it; the caller reports the outcome once that write has actually
    /// happened — see [`Self::report_security_saved`].
    pub(super) fn apply_security(&mut self, cx: &mut Context<Self>) -> SecurityApply {
        let Some(request) = self.security.pending(cx) else {
            return SecurityApply::Nothing;
        };
        let (status, outcome) = match request {
            Err((block, key)) => (
                SecurityStatus {
                    key,
                    error: true,
                    block: Some(block),
                },
                SecurityApply::Rejected(key),
            ),
            Ok(change) => match (vault::snapshot(), vault::apply(change)) {
                (before, Ok(())) => {
                    self.security.vault = vault::state();
                    // The fields are emptied only once the write stores the change (see
                    // `report_security_saved`); a failed write rolls the slots back and keeps
                    // them for the retry. No status yet: the passwords are in memory only.
                    self.security.unpersisted.get_or_insert(before);
                    (
                        SecurityStatus {
                            key: "security.pending_write",
                            error: false,
                            block: None,
                        },
                        SecurityApply::Accepted,
                    )
                }
                (_, Err(error)) => (
                    SecurityStatus {
                        key: error.message_key(),
                        error: true,
                        block: None,
                    },
                    SecurityApply::Rejected(error.message_key()),
                ),
            },
        };
        self.security.status = Some(status);
        cx.notify();
        outcome
    }

    /// Report what happened to an accepted password change once the config write has finished.
    ///
    /// Split from [`Self::apply_security`] because the two facts are independent: the key material
    /// accepted the password, and the file write either stored it or did not. Reporting "saved"
    /// from the first alone put a green line over a password that a failed write left only in
    /// memory — where it survives until the process exits and then is gone.
    pub(super) fn report_security_saved(
        &mut self,
        saved: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match settle(self.security.unpersisted.is_some(), saved) {
            Settle::Nothing => return,
            // Empty the fields once they are on disk. Leaving them filled makes every later Save
            // re-submit the same password, which re-derives the key and replaces the slot for no
            // reason: a visible pause and a new slot per click.
            Settle::Commit => {
                self.security.unpersisted = None;
                self.security.clear_fields(window, cx);
            }
            Settle::Rollback => self.discard_security_slots(),
        }
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

    /// Put back the key slots of an accepted change that never reached disk. Called on a failed
    /// write and whenever the window drops its draft (revert, close without saving).
    pub(super) fn discard_security_slots(&mut self) {
        if let Some(before) = self.security.unpersisted.take() {
            vault::restore(before);
            self.security.vault = vault::state();
        }
    }

    /// Build the Security page body from the two password cards and the machines row.
    pub(super) fn security_section(&self, cx: &Context<Self>) -> AnyElement {
        security_body(
            SecurityParts {
                not_editable: !self.security.vault.editable,
                launch: self.launch_group(cx),
                file: self.file_group(cx),
                machines: self.machines_row(cx),
                // Outcome of the last Save or slot action. Anchored to the page, not the window
                // footer, which reports only whether the configuration itself was written.
                status: self.security_status_line(cx, None),
            },
            cx,
        )
    }
}

/// Write the saved configuration as it stands, leaving the draft alone.
///
/// Args:
///     saved: The configuration on disk, as the backend holds it.
///     write: The writer, [`AppConfig::save`] outside tests.
///
/// Returns:
///     The written configuration (the writer may normalize it), or the write's error.
fn write_saved(
    saved: &AppConfig,
    write: impl FnOnce(&mut AppConfig) -> anyhow::Result<()>,
) -> anyhow::Result<AppConfig> {
    let mut written = saved.clone();
    write(&mut written)?;
    Ok(written)
}

/// The parts of the Security page, built by the view and laid out by [`security_body`].
pub(super) struct SecurityParts {
    /// No config file is open, so there is no key material to attach a password to.
    pub(super) not_editable: bool,
    pub(super) launch: AnyElement,
    pub(super) file: AnyElement,
    pub(super) machines: AnyElement,
    pub(super) status: Option<AnyElement>,
}

/// The Security page body: two password cards side by side (stacked when the body is narrow),
/// then the machines with access.
pub(super) fn security_body(parts: SecurityParts, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    let noop: super::common::Action = Rc::new(|_, _| {});
    v_flex()
        .w_full()
        .min_w_0()
        .gap(design::ui_px(cx, 16.0))
        // Say so instead of collecting a password that cannot be stored.
        .when(parts.not_editable, |this| {
            this.child(
                div()
                    .text_color(rgba_from(p.amber, 1.0))
                    .child(t!("security.not_editable").to_string()),
            )
        })
        .child(
            h_flex()
                .w_full()
                .flex_wrap()
                .items_start()
                .gap(design::ui_px(cx, 14.0))
                .debug_selector(|| "security-cards".into())
                .child(parts.launch)
                .child(parts.file),
        )
        .child(super::general::page_section(
            "security-machines",
            t!("security.machines.title").to_string(),
            vec![
                div()
                    .debug_selector(|| "security-machines-row".into())
                    .child(super::common::hint_row(
                        "security-machines-row",
                        t!("security.machines.label").to_string(),
                        Some(t!("security.machines.hint").to_string()),
                        None,
                        parts.machines,
                        false,
                        noop,
                        cx,
                    ))
                    .into_any_element(),
            ],
            cx,
        ))
        .children(parts.status)
        .into_any_element()
}

/// One password card: title and hint beside the block's checkbox, then the block's rows.
fn password_card(
    id: &'static str,
    title: String,
    hint: String,
    toggle: AnyElement,
    rows: Vec<AnyElement>,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let header = h_flex()
        .w_full()
        .items_start()
        .gap(design::ui_px(cx, 12.0))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(design::ui_px(cx, 2.0))
                .child(
                    div()
                        .debug_selector(move || format!("{id}-title"))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(
                    div()
                        .debug_selector(move || format!("{id}-hint"))
                        .text_color(rgba_from(p.text_muted, 1.0))
                        .child(hint),
                ),
        )
        .child(div().flex_none().child(toggle))
        .into_any_element();
    let mut children = vec![header];
    children.extend(rows);
    super::general::page_box(children, cx)
        .flex_1()
        .min_w(design::font_w_px(cx, CARD_MIN_W))
        .gap(design::ui_px(cx, 8.0))
        .py(design::ui_px(cx, 10.0))
        .debug_selector(move || id.to_string())
        .into_any_element()
}

/// The red note that the encryption password cannot be recovered.
fn unrecoverable_alert() -> AnyElement {
    div()
        .debug_selector(|| "security-unrecoverable".into())
        .child(
            MoonAlert::error(
                "sec-unrecoverable",
                t!("security.unrecoverable").to_string(),
            )
            .render(),
        )
        .into_any_element()
}

/// Build the strength meter under the password field: a bar, its verdict and entropy, and what
/// to change.
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
    // Indented to the field column, so the bar reads as part of the field above it.
    h_flex()
        .w_full()
        .gap(design::ui_px(cx, 10.0))
        .child(div().w(design::font_w_px(cx, LABEL_W)).flex_none())
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .max_w(design::font_w_px(cx, FIELD_W))
                .gap(design::ui_px(cx, 2.0))
                .child(
                    h_flex()
                        .w_full()
                        .flex_wrap()
                        .gap(design::ui_px(cx, 8.0))
                        .items_center()
                        .child(
                            div().flex_1().min_w(design::ui_px(cx, 60.0)).child(
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
                                // MIXED NODE: an entropy FIGURE welded to its unit in one locale
                                // string.
                                .font_family(design::mono())
                                .text_color(rgba_from(p.text_muted, 1.0))
                                .text_size(design::t_caption(cx))
                                .child(
                                    t!("security.bits", bits = verdict.bits.round() as i32)
                                        .to_string(),
                                ),
                        ),
                )
                .child(
                    div()
                        .text_color(rgba_from(p.text_muted, 1.0))
                        .text_size(design::t_caption(cx))
                        .child(note),
                ),
        )
}

impl SettingsView {
    /// The Security page: two password cards and the machines with access.
    pub(super) fn security_page(&self, cx: &Context<Self>) -> super::frame::PageParts {
        let body = self.security_section(cx);
        self.page_parts(super::Tab::Security, body, Vec::new(), cx)
    }
}

#[cfg(test)]
mod tests;
