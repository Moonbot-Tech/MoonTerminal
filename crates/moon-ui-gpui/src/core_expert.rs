//! Expert core-settings window — Moonbot's own Settings dialog, reproduced over the wire, for one
//! core or for many at once.
//!
//! The gear beside the header core selector has two faces. The compact popover
//! (`shell::core_settings_popup`) is the default: two tabs, the settings a trader touches daily.
//! With "Expert mode" ticked the same gear opens THIS window instead, which reproduces Moonbot's
//! full Settings dialog tab for tab, so a trader who knows that dialog finds every page where they
//! expect it.
//!
//! Where the popup edits the group's active core, this window edits the cores picked in ITS OWN
//! list, drawn down the left: one click shows a core's page, Ctrl and Shift build a selection, and
//! OK writes the changed parameters to every selected core. The page is always ONE core's — the
//! anchor, see [`cores::anchor`] — and what OK carries is not that page but the [`CoreChangeSet`]
//! staged on it: the parameters the user actually moved, laid over each target's own latest
//! snapshot. That is what lets a trader change take-profit on twelve cores that otherwise differ
//! in a hundred fields, and see beforehand how many parameters and how many cores the OK reaches.
//! The changes belong to the cores selected when they were made: adding a core keeps them, a
//! plain click that replaces the selection drops them, as leaving a page without OK always has.
//! Where the selected cores disagree, the page says so in place — a checkbox neither on nor off,
//! a box left empty, a slider framed — and the strip counts such parameters per tab, so the ones
//! worth unifying are in view before anything is typed; see [`mixed`].
//!
//! The contract the two faces still share:
//!
//! * ONE staged page, seeded from a core's projected configuration and committed only on a
//!   button — OK, or this window's Apply, which sends and stays — because a write sends the
//!   core's WHOLE safe-share page: nothing may reach the wire while the user types.
//! * Both buttons travel through `shell::send_core_config_to` — the same function the popup's OK
//!   ends in, not a second copy of the clamp, the field mask and the client-side filter halves.
//!
//! The popup's third rule — refuse the write when the group's active core moved underneath the
//! page — does not apply here, and deliberately: this window's cores are chosen by name in a list,
//! not resolved from the group, so nothing can move underneath it. What CAN happen is a core losing
//! its page (a link that is no longer Ready, a replaced MoonBot process), and a window has room to
//! say so instead of closing: [`PageState`] is that answer for the anchor, recomputed by
//! [`CoreExpertView::sync_from_core`], and it alone decides whether OK can be pressed. A selected
//! core OTHER than the anchor that has no page is skipped by OK, and the footer says how many.
//!
//! What this window does NOT reproduce is Moonbot's ability to edit everything on those pages: the
//! wire carries a SAFE subset, and this terminal projects a subset of THAT.
//! [`tabs::TabSource`] records whether a whole PAGE is behind the first of those limits; a row
//! behind either answers for itself, by being drawn disabled with its reason stated beside it.
//! Every page it DOES carry is drawn, in Moonbot's own slot, because hiding one would renumber the
//! rest for a trader who reaches for a page by position — the two Moonbot tabs that are pure
//! actions of that process (its setup wizard and its PRO purchase) are left out entirely instead,
//! having no setting to mirror.

mod commit;
mod cores;
mod mixed;
mod pages;
mod render;
mod sidebar;
mod sync;
mod tabs;
mod widgets;

use gpui::*;
use moon_ui::{MoonBackgroundPolicy, Root};
use rust_i18n::t;

use moon_core::feed::{CoreChangeSet, CoreConfig};
use moon_core::session::CoreId;

use crate::Backend;
use crate::controls::row_selection::RowSelection;
use crate::shell::editors::EditorStore;

use cores::CoreRoster;
use mixed::MixedScope;
pub(crate) use tabs::{ExpertTab, TabSource};

/// Default window size: wide enough for the core list, Moonbot's tab strip and the pages.
const DEFAULT_SIZE: (f32, f32) = (1180.0, 720.0);
/// Smallest usable size. Below it the strip hands most of its tabs to the overflow menu and the
/// footer buttons start to crowd.
const MIN_SIZE: (f32, f32) = (860.0, 480.0);

/// What the window can do with the anchor core right now — the one input to whether OK is live.
///
/// Every variant but [`PageState::Ready`] is a state the compact popup handles by closing itself.
/// A window has room to explain instead, and must: closing on its own is indistinguishable from a
/// successful save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageState {
    /// Nothing is selected in the list, so there is no core to draw.
    NoCore,
    /// The anchor's full configuration has not arrived yet. The runtime fetches it in the
    /// background after Ready and retries until it lands, so this is a wait, not a failure.
    Waiting,
    /// A DIFFERENT MoonBot process now answers on the anchor's connection: the store dropped the
    /// retained configuration, and a page copied from it describes the instance that went away.
    Replaced,
    /// A page is retained, but the link behind it is not Ready — the store's own `Stale`
    /// classification. Sending it would write values whose freshness the store itself doubts.
    Stale,
    /// A page is staged and may be sent.
    Ready,
}

impl PageState {
    /// Whether OK may send the staged page.
    pub(crate) fn can_send(self) -> bool {
        self == Self::Ready
    }
}

/// One core's share of an Apply that its write queue has not worked through yet; see
/// [`CoreExpertView::pending`].
struct PendingSend {
    /// The fields sent and the values they were sent at.
    sent: CoreChangeSet,
    /// The core's `core_config_drained_rev` when the FIRST send of this entry went out.
    ///
    /// The entry is done once that revision has moved: the queue ran empty after the send was
    /// enqueued, so everything sent has either landed in the retained page or been given up on
    /// — and a give-up is a difference the page must show again. Read from the store, never
    /// reconstructed: an edit the core already holds leaves the queue with no packet and no
    /// echo, so neither packets nor snapshots can be counted, and a timeout would hold a field
    /// out of the attention list for as long as the timeout lasts.
    drained_rev: u64,
}

/// State of the singleton expert core-settings window.
///
/// Fields are private to this module tree: the submodules that render and sync it are its
/// children and see them, and nothing outside `core_expert` reaches past [`open`].
pub struct CoreExpertView {
    backend: Entity<Backend>,
    /// Every core the terminal runs, as the list draws it; rebuilt by the sync when its inputs
    /// move.
    roster: CoreRoster,
    /// Which rows are selected — the cores OK writes to — and which was clicked last.
    selection: RowSelection<CoreId>,
    /// The core the page follows, resolved from the selection by [`cores::anchor`] on every sync.
    ///
    /// Held so a change of anchor can be told from a notification that moved nothing: the page
    /// belongs to the core it was seeded from and is dropped the moment the anchor is another.
    /// Whether that core's page has actually arrived is [`Self::seeded`], read off the state.
    anchor: Option<CoreId>,
    /// Selected page.
    tab: ExpertTab,
    /// Selected inner tab of the Hotkeys page. Moonbot's own page is split six ways, and the choice
    /// has to outlive a render.
    hotkeys_sub: pages::HotkeysSub,
    /// Open section of the Special page, which Moonbot splits into four collapsible blocks.
    special_section: pages::SpecialSection,
    /// Row picked in the Telegram page's channel box, as an index into the list that page draws.
    ///
    /// Belongs to the WINDOW for the same reason the open Special section does: a page is rebuilt
    /// on every render and could not remember it. Held as an index rather than a name because the
    /// list is what the page draws, and the page bounds-checks it against the draft it has.
    selected_channel: Option<usize>,
    /// Staged page, present only in [`PageState::Ready`]: the anchor's snapshot with
    /// [`Self::changes`] laid over it.
    draft: Option<CoreConfig>,
    /// What every edit is measured against to decide whether a field is a change at all: the
    /// anchor's snapshot as it was seeded, with what Apply sent it and its queue has not worked
    /// through laid over ([`Self::pending`]) — after an Apply, the page as applied. Present
    /// exactly when [`Self::draft`] is.
    base: Option<CoreConfig>,
    /// The parameters OK writes, and the values it writes them to. Outlives the page: it is
    /// carried onto the next anchor's snapshot when the user picks another core.
    changes: CoreChangeSet,
    /// What Apply sent to each core that has not echoed it yet.
    ///
    /// Laid over every page sent to THAT core until then, because the session's write queue
    /// carries each edit's WHOLE projection and stamps an area from it wholesale
    /// (`feed::live::shared_config`): a second Apply built from the store's not-yet-updated
    /// snapshot would otherwise put the first Apply's field back to its old value, silently, on
    /// the wire. Per core, not per window: a core added to the selection after an Apply was not
    /// sent those values and must not inherit them. The fields pending on any selected core are
    /// also kept out of the attention list, so a field just unified does not read as mixed until
    /// the queue has worked through it. An entry is dropped once its core's write queue ran
    /// empty after the send ([`PendingSend::drained_rev`]) or the core lost its page. It mirrors
    /// the queue and nothing else: a queue parked behind the compact settings channel (that
    /// channel's known limit, logged by `note_gated`) parks the entry with it, as it parks every
    /// OK on that core — no timer here second-guesses that.
    pending: std::collections::BTreeMap<CoreId, PendingSend>,
    /// What the window can do with the anchor right now.
    state: PageState,
    /// Controls the drawn pages have built, through the store the compact popup also uses.
    editors: EditorStore,
    /// This view's own window, needed to write a field from a slider drag.
    window: AnyWindowHandle,
    /// Whether a page for the anchor has ever arrived.
    ///
    /// [`PageState::Replaced`] is "the page I had is gone", and the page itself cannot answer that
    /// — entering a blocked state discards it, so the very next sync would read the same core as
    /// one that had simply never answered. Cleared when the anchor moves.
    had_page: bool,
    /// Whether the controls were dropped while one of them may have held focus.
    ///
    /// Dropping a focused editor leaves focus on a handle nothing draws, and the window's dispatch
    /// path goes empty with it — every hotkey dead until the next click, the failure
    /// `Shell::render` documents for the same situation. The drop happens on a `&App` path with no
    /// window, so the blur is deferred to the next render, which has one.
    needs_blur: bool,
    /// How the last OK went, when it did not reach every core it was meant to: `(sent, targets)`.
    ///
    /// [`Self::state`] already keeps OK dark in every case this window can see coming for the
    /// anchor; this covers a target refusing the page at the moment of the click. Cleared by the
    /// next successful send, a re-seed, or a change of selection.
    write_refused: Option<(usize, usize)>,
    /// `CoreData::core_config_rev` the seed was taken at.
    ///
    /// The store bumps that revision only when the projected page actually CHANGES, which makes it
    /// the cheap answer to "is my seed current" — where comparing the projections themselves means
    /// walking a hundred fields and a dozen heap strings, several times a second, to almost always
    /// conclude nothing moved.
    seen_rev: u64,
    /// Whether the anchor's page must be re-seeded although the store's revision has not moved:
    /// set when the anchor's pending entry is dropped, because the page has been showing what
    /// Apply sent, and a write the core refused leaves the store's page — and its revision —
    /// exactly where they were. Cleared by the re-seed.
    page_behind: bool,
    /// The report counters last drawn, as raw bits; see the gate in the sync.
    seen_profit: Option<(u64, i32, u64, i32)>,
    /// The anchor's `core_config_edit_rev` last drawn: the store bumps it on every verdict of a
    /// write, including a give-up that moves no data revision, and the banner drawn from that
    /// row has no other way to be repainted.
    seen_edit_rev: u64,
    /// The selection and page revisions [`Self::diff`] was computed for; see [`cores::diff_key`].
    diff_key: Vec<cores::DiffKeyEntry>,
    /// Table fields on which the selected cores' pages disagree, ascending.
    diff: Vec<usize>,
    /// How the page's controls learn that their parameter is one of [`Self::diff`]; see
    /// [`mixed`].
    mixed: MixedScope,
    /// Text boxes the window emptied to show a mixed value, so it can refill them from the page
    /// the moment they stop being mixed — the editors re-read the page only on a re-seed.
    emptied: std::collections::HashSet<&'static str>,
    focus: FocusHandle,
}

impl CoreExpertView {
    /// Build the window's state, selecting the opening group's active core.
    ///
    /// Args:
    ///     backend: Application state read for the cores' configuration and written on OK.
    ///     group: Group whose active trading core is selected at first; the list shows every core.
    ///     window: Window being created, observed for geometry.
    ///     cx: View context.
    ///
    /// Returns:
    ///     The view, seeded when the selected core's configuration has already arrived.
    fn new(
        backend: Entity<Backend>,
        group: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // A core's configuration arrives in the background after Ready, and every later change to
        // it is published the same way; both reach this window as a Backend notification. That
        // notification fires far more often than anything here changes — a few times a second on a
        // large fleet — so the repaint is gated on the sync having actually found something, the
        // way every other Backend observer in this tree gates its own.
        cx.observe(&backend, |this, _, cx| {
            if this.sync_from_core(cx) {
                cx.notify();
            }
        })
        .detach();

        // Persist position and size in the layout, as every other tool window here does.
        cx.observe_window_bounds(window, |this, window, cx| {
            let geom = crate::window::windowing::window_geom_rect(window, cx);
            this.backend.update(cx, |b, _| {
                let geom = geom.keeping_display_of(b.layout.core_expert_window);
                if b.layout.core_expert_window != Some(geom) {
                    b.layout.core_expert_window = Some(geom);
                    b.layout_dirty = true;
                }
            });
        })
        .detach();

        // The singleton's handles are the window's own to clear, and EVERY close path ends here:
        // OK, Cancel, unticking expert mode, the OS close button, and the owner window going away.
        // Without this, `open` would keep probing a handle whose window is gone.
        //
        // Guarded by the window id, as the Profit Monitor and the detached hosts guard theirs: a
        // view released AFTER its replacement registered — close and reopen inside one effect
        // flush — would otherwise clear the handles of a window that is on screen, and the next
        // gear press would open a second one beside it.
        let window_id = window.window_handle().window_id();
        cx.on_release(move |this, app| {
            this.backend.update(app, |b, _| {
                if b.core_expert_window
                    .is_none_or(|handle| handle.window_id() == window_id)
                {
                    b.core_expert_window = None;
                    b.core_expert_view = None;
                }
            });
        })
        .detach();

        let mut this = Self {
            backend,
            roster: CoreRoster::default(),
            selection: RowSelection::default(),
            anchor: None,
            tab: ExpertTab::default(),
            hotkeys_sub: pages::HotkeysSub::default(),
            special_section: pages::SpecialSection::default(),
            selected_channel: None,
            draft: None,
            base: None,
            changes: CoreChangeSet::default(),
            pending: std::collections::BTreeMap::new(),
            state: PageState::NoCore,
            needs_blur: false,
            write_refused: None,
            editors: EditorStore::default(),
            window: window.window_handle(),
            had_page: false,
            seen_rev: 0,
            page_behind: false,
            seen_profit: None,
            seen_edit_rev: 0,
            diff_key: Vec::new(),
            diff: Vec::new(),
            mixed: MixedScope::default(),
            emptied: std::collections::HashSet::new(),
            focus: cx.focus_handle(),
        };
        this.select_group_core(group, cx);
        this
    }

    /// Core the page was seeded from — the anchor, once its page has arrived and while it may be
    /// sent. `None` in every blocked state, which is exactly when the page is gone.
    pub(super) fn seeded(&self) -> Option<CoreId> {
        self.state.can_send().then_some(self.anchor).flatten()
    }

    /// The differing fields the trader can still do something about, ascending: [`Self::diff`]
    /// less the staged fields — OK will unify those — less the fields Apply just sent and the
    /// cores have not all echoed, and less the fields that are copies of another on the drawn
    /// page (a short gesture under the mirror), which differ only because the field they copy
    /// does.
    pub(super) fn attention(&self) -> impl Iterator<Item = usize> + '_ {
        // Pending on ANY selected core, not the anchor's alone: the anchor may have echoed first,
        // and the field would read as mixed against the cores still owed one. Resolved once, not
        // per field.
        let held: Vec<&CoreChangeSet> = self
            .pending
            .iter()
            .filter(|(core, _)| self.selection.contains(Some(**core)))
            .map(|(_, entry)| &entry.sent)
            .collect();
        self.diff
            .iter()
            .copied()
            .filter(move |&field| {
                !self.changes.contains(field) && !held.iter().any(|sent| sent.contains(field))
            })
            .filter(|&field| {
                self.draft
                    .as_ref()
                    .is_none_or(|page| !moon_core::feed::CORE_FIELDS[field].is_derived(page))
            })
    }
}

impl EventEmitter<()> for CoreExpertView {}

impl Focusable for CoreExpertView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Open the singleton expert core-settings window, focusing an existing one instead of duplicating
/// it.
///
/// One window for the whole application, like every other tool window here: its list already
/// carries every core, so a second window would only be a second selection over the same cores
/// with no way for two OKs to agree on what was sent.
///
/// Args:
///     backend: Application state the window reads its pages from and writes on OK.
///     owner: Window this one belongs to, for placement.
///     owner_display: Display the owner sits on.
///     group: Group whose gear was pressed; its active core is what the window selects.
///     cx: Application context.
pub(crate) fn open(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    group: String,
    cx: &mut App,
) {
    // A live window is pointed at this group's core and focused, never duplicated. The window
    // being live is decided by the HANDLE alone — a second window beside a live one is the worse
    // outcome, so an unreachable view costs the reselect and nothing more.
    if let Some(handle) = backend.read(cx).core_expert_window
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        let view = backend
            .read(cx)
            .core_expert_view
            .clone()
            .and_then(|v| v.upgrade());
        match view {
            Some(view) => view.update(cx, |this, cx| this.rebind(&group, cx)),
            None => log::warn!(
                "expert core settings window focused without reselecting: its view is unreachable"
            ),
        }
        return;
    }
    let saved = backend.read(cx).layout.core_expert_window;
    let bounds = saved.map_or(
        Bounds {
            origin: point(px(140.0), px(100.0)),
            size: size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)),
        },
        |g| Bounds {
            origin: point(px(g.x as f32), px(g.y as f32)),
            size: size(px(g.w as f32), px(g.h as f32)),
        },
    );
    // Choose a display from the saved position when supported, otherwise from the owner: without a
    // display id GPUI creates the window on the primary display and may discard off-screen bounds.
    let display_id = crate::window::windowing::saved_or_owner_display_id(
        saved.and_then(|g| g.display_uuid),
        saved.map(|g| point(px(g.x as f32), px(g.y as f32))),
        owner,
        owner_display,
        cx,
    );
    let mut opts = crate::window::windowing::tool_window_options(
        t!("core_expert.window_title").to_string(),
        crate::window::windowing::restored_window_bounds(saved, bounds),
        Some(size(px(MIN_SIZE.0), px(MIN_SIZE.1))),
        owner,
    );
    opts.display_id = display_id;
    let b = backend.clone();
    let created = cx.open_window(opts, move |window, cx| {
        crate::window::windowing::configure_shell_clear_color(window, cx);
        let view = cx.new(|cx| CoreExpertView::new(b.clone(), &group, window, cx));
        // Registered from inside the constructor, the way the Report window registers its panel:
        // this is the only place the view exists, and that handle is what a second group's gear
        // needs in order to reselect in this window instead of opening another.
        b.update(cx, |backend, _| {
            backend.core_expert_view = Some(view.downgrade());
        });
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    });
    match created {
        Ok(handle) => {
            backend.update(cx, |bk, _| bk.core_expert_window = Some(handle));
            crate::window::windowing::activate_new_window(handle.into(), cx);
        }
        Err(error) => {
            // The preference points the gear at a window that cannot be created, and the only
            // control that clears it lives INSIDE that window: leaving it set would make the gear
            // do nothing at all, permanently.
            log::warn!("expert core settings window could not be opened: {error:#}");
            backend.update(cx, |bk, bcx| {
                bk.core_expert_view = None;
                // The handle we fell through can only be a dead one; leaving it set would make a
                // later reader believe this window is live.
                bk.core_expert_window = None;
                bk.set_core_settings_expert(false, bcx);
            });
        }
    }
}
