//! The two modals behind the core picker's saved groups: naming a new one, and managing the list.
//!
//! Both live here rather than in any consuming panel because a saved group is application state,
//! not panel state: the six pickers read the same list out of `AppConfig`, so an editor owned by
//! whichever panel happened to open it would be six editors of one thing. Each modal creates its
//! own state entity on open; the dialog layer retains the builder closure, so that entity lives
//! exactly as long as the dialog and needs no home anywhere else.
//!
//! Every mutation from these modals funnels through `Backend::edit_core_groups`, which sanitizes
//! and raises `config_dirty` — never a direct write to `config.core_groups` here.

use gpui::{
    App, AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement as _, Render,
    Styled as _, Subscription, Window, div, px,
};
use moon_core::config::CoreGroup;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonInput, MoonInputEvent, MoonInputState,
    MoonNotification, MoonPalette, MoonWindowExt as _, h_flex, v_flex,
};
use rust_i18n::t;

use moon_core::config::{move_group, unique_group_name};

use crate::design::moon;
use crate::{Backend, design};

/// Unique dialog id for the save modal.
const SAVE_DIALOG_ID: &str = "core-group-save";
/// Unique dialog id for the management modal.
const MANAGE_DIALOG_ID: &str = "core-groups-manage";

/// Apply the shared modal chrome and header both group dialogs use.
///
/// One definition rather than two byte-identical blocks in one file. The same block exists in four
/// other dialogs in this crate; extracting it for all of them is a separate cleanup, and this at
/// least stops the count from growing by two.
///
/// Args:
///     dialog: The dialog under construction.
///     width: Fixed dialog width in logical pixels.
///     title: Localized header text.
///     cx: Application context supplying palette and radius.
///
/// Returns:
///     The dialog with chrome and header applied.
fn group_dialog_chrome(
    dialog: moon_ui::MoonDialog,
    width: f32,
    title: String,
    cx: &App,
) -> moon_ui::MoonDialog {
    let p = MoonPalette::active(cx);
    dialog
        .w(px(width))
        .close_button(true)
        .overlay(true)
        .overlay_closable(true)
        .bg(moon(p.shell_high))
        .border_color(moon(p.border))
        .rounded(design::r_container(cx))
        .text_color(moon(p.text))
        .header(
            div()
                .w_full()
                .py_2()
                .border_b_1()
                .border_color(moon(p.border))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
}

/// Naming state for a group about to be created.
struct SaveCoreGroup {
    backend: Entity<Backend>,
    name: Entity<MoonInputState>,
    /// The member uids already resolved against the configured cores by the caller.
    cores: Vec<u64>,
}

impl SaveCoreGroup {
    /// Create the group under the typed name.
    ///
    /// Args:
    ///     cx: This view's context.
    ///
    /// Returns:
    ///     Whether a group was actually created. `false` keeps the dialog open — either the name
    ///     held no visible characters, or the saved list is already at `CORE_GROUPS_MAX` and the
    ///     sanitizer dropped the append. Reporting success in the second case would close the
    ///     dialog over a group that does not exist.
    fn confirm(&mut self, cx: &mut Context<Self>) -> bool {
        let typed = self.name.read(cx).value().to_string();
        let existing: Vec<String> = self
            .backend
            .read(cx)
            .config
            .core_groups
            .iter()
            .map(|group| group.name.clone())
            .collect();
        let name = unique_group_name(&existing, &typed);
        if name.is_empty() {
            return false;
        }
        let cores = self.cores.clone();
        self.backend.update(cx, |backend, _| {
            backend.edit_core_groups(move |groups| {
                groups.push(CoreGroup { name, cores });
                true
            })
        })
    }
}

impl Render for SaveCoreGroup {
    /// Render the saved member count and the group-name input.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        v_flex()
            .w_full()
            .gap_2()
            .child(
                div()
                    .text_color(moon(p.text_muted))
                    .child(t!("common.core_pick.group_cores_n", n = self.cores.len()).to_string()),
            )
            .child(MoonInput::new("core-group-name").state(&self.name).small())
    }
}

/// Open the "save the current selection as a group" modal.
///
/// Args:
///     backend: Shared terminal state holding the saved groups.
///     cores: Member uids to store, already resolved against the configured cores.
///     window: The window opening the dialog.
///     app: Application context.
pub(crate) fn open_save_dialog(
    backend: Entity<Backend>,
    cores: Vec<u64>,
    window: &mut Window,
    app: &mut App,
) {
    let state = app.new(|cx| SaveCoreGroup {
        backend,
        name: cx.new(|cx| {
            MoonInputState::new(window, cx)
                .placeholder(t!("common.core_pick.group_name_ph").to_string())
        }),
        cores,
    });
    // The dialog focuses its own handle on open and MoonInput does not self-focus, so without this
    // the single field of a one-field modal has to be clicked before it accepts a keystroke.
    state
        .read(app)
        .name
        .clone()
        .update(app, |input, cx| input.focus(window, cx));
    window.open_unique_moon_dialog(SAVE_DIALOG_ID, app, move |dialog, _window, cx| {
        let p = MoonPalette::active(cx);
        let body = state.clone();
        let confirm = state.clone();
        group_dialog_chrome(
            dialog,
            420.0,
            t!("common.core_pick.save_title").to_string(),
            cx,
        )
        .content(move |content, _window, _cx| content.child(body.clone()))
        .footer(save_footer(confirm, p))
    });
}

/// Cancel and Create for the save modal.
fn save_footer(create: Entity<SaveCoreGroup>, p: MoonPalette) -> gpui::AnyElement {
    h_flex()
        .w_full()
        .justify_end()
        .gap_2()
        .text_color(moon(p.text))
        .child(
            MoonButton::new("core-group-cancel")
                .ghost()
                .size(MoonButtonSize::Micro)
                .label(t!("dialogs.cancel").to_string())
                .on_click(move |_, window, cx| window.close_dialog(cx))
                .render(),
        )
        .child(
            MoonButton::new("core-group-create")
                .size(MoonButtonSize::Micro)
                .variant(MoonButtonVariant::Blue)
                .label(t!("dialogs.create").to_string())
                .on_click(move |_, window, cx| {
                    if create.update(cx, |this, cx| this.confirm(cx)) {
                        window.close_dialog(cx);
                    } else {
                        // Not `dialogs.name_required` — that string names a STRATEGY in all three
                        // languages, and this modal is about a core group.
                        window.push_notification(
                            MoonNotification::warning(
                                t!("common.core_pick.group_name_required").to_string(),
                            ),
                            cx,
                        );
                    }
                })
                .render(),
        )
        .into_any_element()
}

/// One editable row: the saved group it stands for, its rename field, and that field's listener.
///
/// The subscription lives HERE so dropping the row drops its listener, and so a surviving row
/// keeps both — a reorder must not destroy a field the user is still typing into.
struct GroupRowState {
    /// The saved name this row currently edits, updated by a successful rename.
    ///
    /// A NAME, not an index. The saved list is application state while a dialog is window-scoped,
    /// so a second window can insert, reorder or delete underneath this one, and a retained index
    /// would then rename or delete a different group. Names are unique case-insensitively
    /// (`sanitize_core_groups`), so they are the only stable identity a group has.
    key: String,
    input: Entity<MoonInputState>,
    _sub: Subscription,
}

/// Editing state for the saved-group list.
struct ManageCoreGroups {
    backend: Entity<Backend>,
    /// One row per saved group, in the saved order.
    rows: Vec<GroupRowState>,
    /// Name whose Delete button is armed, if any.
    ///
    /// A two-click delete instead of a confirmation dialog: MoonUI paints one dialog at a time,
    /// so a confirm modal opened from this modal would replace the list the user is editing.
    armed: Option<String>,
}

impl ManageCoreGroups {
    /// Reconcile the rows to the saved list, keeping every field that still has a group.
    ///
    /// Called at the top of every render, so an edit made in ANOTHER window reaches this list too,
    /// and cheap when nothing moved: the name sequence is compared first and an unchanged list
    /// returns immediately. Rows are matched BY NAME, so a reorder permutes the existing fields
    /// rather than rebuilding them — rebuilding would discard text the user had typed into some
    /// other row and never committed, since MoonUI buttons take no focus and fire no `Blur` first.
    ///
    /// Deliberately does NOT notify: it runs during render, and a notify there would repaint
    /// forever.
    ///
    /// Args:
    ///     window: The window owning the inputs.
    ///     cx: This view's context.
    fn sync_rows(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        {
            let groups = &self.backend.read(cx).config.core_groups;
            if self.rows.len() == groups.len()
                && self
                    .rows
                    .iter()
                    .zip(groups.iter())
                    .all(|(row, group)| row.key == group.name)
            {
                return;
            }
        }
        let names: Vec<String> = self
            .backend
            .read(cx)
            .config
            .core_groups
            .iter()
            .map(|group| group.name.clone())
            .collect();
        let mut previous = std::mem::take(&mut self.rows);
        for name in names {
            let row = match previous.iter().position(|row| row.key == name) {
                Some(at) => previous.remove(at),
                None => self.new_row(name, window, cx),
            };
            self.rows.push(row);
        }
        self.armed = self
            .armed
            .take()
            .filter(|armed| self.rows.iter().any(|row| &row.key == armed));
    }

    /// Build one row and wire its commit-on-finish listener.
    ///
    /// The listener resolves its row by INPUT ENTITY at event time, so it never holds a position
    /// and never goes stale, however the list is later reordered.
    fn new_row(&self, name: String, window: &mut Window, cx: &mut Context<Self>) -> GroupRowState {
        let input = cx.new(|cx| MoonInputState::new(window, cx).default_value(name.clone()));
        let sub = cx.subscribe_in(
            &input,
            window,
            move |this, input, event: &MoonInputEvent, window, cx| {
                if !matches!(
                    event,
                    MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }
                ) {
                    return;
                }
                let Some(key) = this
                    .rows
                    .iter()
                    .find(|row| &row.input == input)
                    .map(|row| row.key.clone())
                else {
                    return;
                };
                let typed = input.read(cx).value().to_string();
                this.commit_rename(key, typed, window, cx);
            },
        );
        GroupRowState {
            key: name,
            input,
            _sub: sub,
        }
    }

    /// Apply a finished rename, or put the previous name back.
    ///
    /// An empty name is refused rather than deleting the group: the sanitizer drops a nameless
    /// group, so accepting it would turn a stray Ctrl+A into silent data loss. A name colliding
    /// with another group gets a numbered suffix, and the input is rewritten to whatever was
    /// actually stored so the field never shows a name the file does not hold.
    ///
    /// Args:
    ///     key: The group's current saved name.
    ///     typed: The user's text.
    ///     window: The window owning the inputs.
    ///     cx: This view's context.
    fn commit_rename(
        &mut self,
        key: String,
        typed: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.index_of(&key, cx) else {
            return;
        };
        let existing: Vec<String> = self
            .backend
            .read(cx)
            .config
            .core_groups
            .iter()
            .enumerate()
            .filter(|(other, _)| *other != index)
            .map(|(_, group)| group.name.clone())
            .collect();
        let name = unique_group_name(&existing, &typed);
        let settled = if name.is_empty() { key.clone() } else { name };
        let stored = settled.clone();
        self.backend.update(cx, |backend, _| {
            backend.edit_core_groups(move |groups| match groups.get_mut(index) {
                Some(group) if group.name != stored => {
                    group.name = stored;
                    true
                }
                _ => false,
            })
        });
        if let Some(row) = self.rows.iter_mut().find(|row| row.key == key) {
            row.key = settled.clone();
            row.input.update(cx, |input, cx| {
                input.set_value(settled, window, cx);
            });
        }
        // An arm belongs to the click that made it, not to the session: leaving it set would let
        // a much later single click delete a group with no second confirmation.
        self.armed = None;
        cx.notify();
    }

    /// Current position of a saved group, or `None` when it no longer exists.
    fn index_of(&self, key: &str, cx: &App) -> Option<usize> {
        self.backend
            .read(cx)
            .config
            .core_groups
            .iter()
            .position(|group| group.name == key)
    }

    /// Move one group by one position, resolving its place at click time.
    fn reorder(&mut self, key: String, up: bool, cx: &mut Context<Self>) {
        let Some(from) = self.index_of(&key, cx) else {
            return;
        };
        let Some(to) = (if up {
            from.checked_sub(1)
        } else {
            Some(from + 1)
        }) else {
            return;
        };
        self.backend.update(cx, |backend, _| {
            backend.edit_core_groups(|groups| move_group(groups, from, to))
        });
        self.armed = None;
        cx.notify();
    }

    /// Arm a row's Delete, or perform the deletion when that row is already armed.
    fn delete(&mut self, key: String, cx: &mut Context<Self>) {
        if self.armed.as_deref() != Some(key.as_str()) {
            self.armed = Some(key);
            cx.notify();
            return;
        }
        let Some(index) = self.index_of(&key, cx) else {
            return;
        };
        self.backend.update(cx, |backend, _| {
            backend.edit_core_groups(|groups| {
                groups.remove(index);
                true
            })
        });
        self.armed = None;
        cx.notify();
    }
}

impl Render for ManageCoreGroups {
    /// Reconcile external edits, then render one editable row per surviving group.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Reconcile FIRST: this is what makes an edit from another window show up here, and it
        // returns immediately when the saved list has not moved.
        self.sync_rows(window, cx);

        let p = MoonPalette::active(cx);
        let backend = self.backend.read(cx);
        let configured = super::core_groups::configured_uids(&backend.config);
        // Keyed by NAME like everything else in this file. `sync_rows` has just run, so a
        // positional join would work — and would be the one index join in a module whose whole
        // thesis is that positions are not identity.
        let facts: std::collections::HashMap<&str, String> = backend
            .config
            .core_groups
            .iter()
            .map(|group| {
                (
                    group.name.as_str(),
                    super::core_groups::group_facts(
                        t!("common.core_pick.group_cores_n", n = group.cores.len()).to_string(),
                        super::core_groups::group_dead_count(&group.cores, &configured),
                    ),
                )
            })
            .collect();
        let last = self.rows.len().saturating_sub(1);

        let mut list = v_flex().w_full().gap_1();
        for (index, row) in self.rows.iter().enumerate() {
            let Some(facts) = facts.get(row.key.as_str()).cloned() else {
                continue;
            };
            let armed = self.armed.as_deref() == Some(row.key.as_str());
            let up_key = row.key.clone();
            let down_key = row.key.clone();
            let delete_key = row.key.clone();
            let this = cx.entity();
            let remove = this.clone();
            list = list.child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_2()
                    .child(
                        div().w(px(220.0)).child(
                            MoonInput::new(format!("core-group-row-{index}"))
                                .state(&row.input)
                                .small(),
                        ),
                    )
                    .child(div().flex_1().text_color(moon(p.text_muted)).child(facts))
                    .child(move_button(&this, index, up_key, true, index == 0))
                    .child(move_button(&this, index, down_key, false, index == last))
                    .child(
                        MoonButton::new(("core-group-delete", index))
                            .size(MoonButtonSize::Micro)
                            .variant(if armed {
                                MoonButtonVariant::Danger
                            } else {
                                MoonButtonVariant::Ghost
                            })
                            .label(if armed {
                                t!("dialogs.delete_q").to_string()
                            } else {
                                t!("common.core_pick.group_delete").to_string()
                            })
                            .on_click(move |_, _window, cx| {
                                remove.update(cx, |this, cx| this.delete(delete_key.clone(), cx));
                            })
                            .render(),
                    ),
            );
        }
        list
    }
}

/// One reorder button: the up and down arrows differ only in direction and end-stop.
///
/// Args:
///     view: The modal, for the click to act on.
///     index: Row position, used only for the element key.
///     key: The group's saved name, the identity the click resolves through.
///     up: Whether this button moves the row toward the start.
///     at_end: Whether the row is already at this direction's end.
///
/// Returns:
///     The rendered button.
fn move_button(
    view: &Entity<ManageCoreGroups>,
    index: usize,
    key: String,
    up: bool,
    at_end: bool,
) -> impl IntoElement {
    let view = view.clone();
    let id = if up {
        "core-group-up"
    } else {
        "core-group-down"
    };
    MoonButton::new((id, index))
        .ghost()
        .size(MoonButtonSize::Micro)
        .label(if up { "↑" } else { "↓" })
        .disabled(at_end)
        .on_click(move |_, _window, cx| {
            view.update(cx, |this, cx| this.reorder(key.clone(), up, cx));
        })
        .render()
}

/// Open the saved-group management modal.
///
/// Args:
///     backend: Shared terminal state holding the saved groups.
///     window: The window opening the dialog.
///     app: Application context.
pub(crate) fn open_manage_dialog(backend: Entity<Backend>, window: &mut Window, app: &mut App) {
    let state = app.new(|cx: &mut Context<ManageCoreGroups>| {
        let mut this = ManageCoreGroups {
            backend,
            rows: Vec::new(),
            armed: None,
        };
        this.sync_rows(window, cx);
        this
    });
    window.open_unique_moon_dialog(MANAGE_DIALOG_ID, app, move |dialog, _window, cx| {
        let p = MoonPalette::active(cx);
        let body = state.clone();
        group_dialog_chrome(
            dialog,
            560.0,
            t!("common.core_pick.manage_title").to_string(),
            cx,
        )
        .content(move |content, _window, _cx| content.child(body.clone()))
        .footer(
            h_flex()
                .w_full()
                .justify_end()
                .text_color(moon(p.text))
                .child(
                    MoonButton::new("core-groups-close")
                        .ghost()
                        .size(MoonButtonSize::Micro)
                        .label(t!("dialogs.close").to_string())
                        .on_click(move |_, window, cx| window.close_dialog(cx))
                        .render(),
                )
                .into_any_element(),
        )
    });
}
