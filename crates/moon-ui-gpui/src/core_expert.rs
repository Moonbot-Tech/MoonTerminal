//! Expert core-settings window — Moonbot's own Settings dialog, reproduced over the wire.
//!
//! The gear beside the header core selector has two faces. The compact popover
//! (`shell::core_settings_popup`) is the default: two tabs, the settings a trader touches daily.
//! With "Expert mode" ticked the same gear opens THIS window instead, which reproduces Moonbot's
//! full Settings dialog tab for tab, so a trader who knows that dialog finds every page where they
//! expect it.
//!
//! Both faces share one contract, and it is the popup's:
//!
//! * ONE staged draft, seeded from the core's projected configuration and committed only on OK,
//!   because a write sends the core's WHOLE safe-share page — nothing may reach the wire while the
//!   user types.
//! * The draft belongs to the core it was seeded from. The active trading core can move underneath
//!   an open window (the header selector, a Main-chart coin switch, a core going away), and
//!   `shell::core_settings::resolve_core_settings_write` refuses the write when it has.
//! * OK travels through `shell::core_settings::draft::send_core_config` — the same function the
//!   popup's OK calls, not a second copy of the clamp, the guard and the field mask.
//!
//! Where the popup answers those hazards by CLOSING — a popover has nowhere to put an explanation —
//! this window stays up and says which one it is in: a window that vanished on its own would look
//! like it had saved. [`PageState`] is that answer, recomputed by [`CoreExpertView::sync_from_core`]
//! and the only thing that decides whether OK can be pressed.
//!
//! What this window does NOT reproduce is Moonbot's ability to edit everything on those pages: the
//! wire carries a SAFE subset, and this terminal projects a subset of THAT. [`tabs::TabSource`]
//! records which of the two limits a page sits behind. Every page is still drawn, in Moonbot's own
//! slot, because hiding one would renumber every tab after it for a trader who reaches for a page
//! by position.

mod render;
mod tabs;

use gpui::*;
use moon_ui::{MoonBackgroundPolicy, Root};
use rust_i18n::t;

use moon_core::feed::{CoreConfig, CoreConfigState};
use moon_core::session::CoreId;

use crate::Backend;
use crate::shell::{resolve_core_settings_write, send_core_config};

pub(crate) use tabs::{ExpertTab, TabSource};

/// Default window size: wide enough for Moonbot's ten-tab strip and tall enough for the pages that
/// will follow.
const DEFAULT_SIZE: (f32, f32) = (980.0, 700.0);
/// Smallest usable size. Below it the strip hands most of its tabs to the overflow menu and the
/// footer buttons start to crowd.
const MIN_SIZE: (f32, f32) = (720.0, 460.0);

/// What the window can do with the core right now — the one input to whether OK is live.
///
/// Every variant but [`PageState::Ready`] is a state the compact popup handles by closing itself.
/// A window has room to explain instead, and must: closing on its own is indistinguishable from a
/// successful save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageState {
    /// The group has no active trading core to edit.
    NoCore,
    /// The group is in Auto Overview, where the header draws no per-core cluster at all and
    /// `Backend::active_trade_core` falls through to the group's FIRST core — an address this
    /// window must never write to.
    Overview,
    /// The active trading core moved since the page was seeded. The values on screen describe the
    /// core that WAS active, so they may not be sent anywhere.
    CoreMoved,
    /// The core's full configuration has not arrived yet. The runtime fetches it in the background
    /// after Ready and retries until it lands, so this is a wait, not a failure.
    Waiting,
    /// A DIFFERENT MoonBot process now answers on this connection: the store dropped the retained
    /// configuration, and a page copied from it describes the instance that went away.
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

/// State of the singleton expert core-settings window.
pub struct CoreExpertView {
    pub(super) backend: Entity<Backend>,
    /// Group whose active trading core this window edits.
    ///
    /// Carried rather than resolved per event: `Backend::active_trade_core` is answered per group,
    /// and this window has to keep asking about the SAME group it was opened from even after focus
    /// moved to another group's window.
    pub(super) group: String,
    /// Core the draft was seeded from; the only core OK may reach.
    pub(super) seeded: Option<CoreId>,
    /// Selected page.
    pub(super) tab: ExpertTab,
    /// Staged page, present only in [`PageState::Ready`].
    pub(super) draft: Option<CoreConfig>,
    /// The projection the draft was seeded with, which is what tells an untouched window from an
    /// edited one — exactly as the popup does.
    pub(super) seed: Option<CoreConfig>,
    /// What the window can do with the core right now.
    pub(super) state: PageState,
    /// Whether a page for [`Self::seeded`] has ever arrived.
    ///
    /// [`PageState::Replaced`] is "the page I had is gone", and the page itself cannot answer that
    /// — entering a blocked state discards it, so the very next sync would read the same core as
    /// one that had simply never answered. Cleared with the binding, in [`Self::rebind`].
    had_page: bool,
    /// Whether the last OK was refused by the shared send.
    ///
    /// [`Self::state`] already keeps OK dark in every case this window can see coming; this covers
    /// the one it cannot — the core moving between the render that drew the button and the click
    /// that pressed it.
    pub(super) write_refused: bool,
    /// Configured name of [`Self::seeded`], resolved when the binding changes rather than per
    /// frame: naming a core is a linear scan of the configured servers plus a `String` clone.
    pub(super) core_name: Option<String>,
    /// `CoreData::core_config_rev` the seed was taken at.
    ///
    /// The store bumps that revision only when the projected page actually CHANGES, which makes it
    /// the cheap answer to "is my seed current" — where comparing the projections themselves means
    /// walking a hundred fields and a dozen heap strings, several times a second, to almost always
    /// conclude nothing moved.
    seen_rev: u64,
    focus: FocusHandle,
}

impl CoreExpertView {
    /// Build the window's state over one group's active core.
    ///
    /// Args:
    ///     backend: Application state read for the core's configuration and written on OK.
    ///     group: Group whose active trading core this window edits.
    ///     window: Window being created, observed for geometry.
    ///     cx: View context.
    ///
    /// Returns:
    ///     The view, seeded when the core's configuration has already arrived.
    fn new(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The core's configuration arrives in the background after Ready, and every later change to
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
            group,
            seeded: None,
            tab: ExpertTab::default(),
            draft: None,
            seed: None,
            state: PageState::NoCore,
            write_refused: false,
            core_name: None,
            had_page: false,
            seen_rev: 0,
            focus: cx.focus_handle(),
        };
        this.sync_from_core(cx);
        this
    }

    /// Resolve what the window can do with the core, and take the core's newest page when it may.
    ///
    /// This is the whole reconciliation the popup performs across `reconcile_core_settings_popup`
    /// and `refresh_untouched_core_draft`, in one pass, because a window answers all of it the same
    /// way: by changing what the page SAYS rather than by disappearing.
    ///
    /// Re-seeding happens only while the draft still equals what it was seeded with. Past that the
    /// user's edits outrank the core's newer values, and overwriting them mid-edit is the worse
    /// failure — the rule the popup states for the same reason.
    ///
    /// Runs on EVERY backend notification, so the cheap disqualifications come first and a
    /// `CoreConfig` comparison is reached only for a core that is still the one this window
    /// addresses.
    ///
    /// Args:
    ///     cx: Application context used to read the backend.
    ///
    /// Returns:
    ///     Whether anything the window draws changed, which is what gates the repaint.
    fn sync_from_core(&mut self, cx: &App) -> bool {
        let b = self.backend.read(cx);

        // Overview draws no per-core cluster and `active_trade_core` falls through to the group's
        // first core there, so resolving an address at all would invent one.
        if b.is_auto_overview_scope(&self.group) {
            return self.enter_state(PageState::Overview);
        }
        let Some(active) = b.active_trade_core(&self.group) else {
            return self.enter_state(PageState::NoCore);
        };
        // A window opened before the group had a core binds to the first one that appears; one
        // already bound never follows a move, because its page describes the core it was seeded
        // from.
        let core = match self.seeded {
            None => {
                self.seeded = Some(active);
                self.core_name = b
                    .config
                    .servers
                    .iter()
                    .find(|server| server.id == active)
                    .map(|server| server.name.clone());
                active
            }
            // Through the shared guard rather than a second `seeded == active` written here: the
            // gate that lights OK and the guard that lets the page onto the wire must be the same
            // predicate, or the window offers a send its own send refuses.
            Some(seeded) => match resolve_core_settings_write(seeded.into(), Some(active)) {
                Some(core) => core,
                None => return self.enter_state(PageState::CoreMoved),
            },
        };

        let entry = b.session.store().core(core);
        let rev = entry.map_or(0, |d| d.core_config_rev);
        // The store's own classification, not a second reading of the same facts: `Awaiting` covers
        // both "no page yet" and the drop that happens when a DIFFERENT MoonBot process answers on
        // this connection, and `Stale` covers a page retained across a link that is no longer Ready
        // — a state a hand-rolled `core_config.is_none()` test cannot see at all, and one whose
        // page must not be sent.
        match entry.map(|d| d.core_config_state()) {
            None | Some(CoreConfigState::Awaiting) => {
                let state = if self.had_page {
                    PageState::Replaced
                } else {
                    PageState::Waiting
                };
                return self.enter_state(state);
            }
            Some(CoreConfigState::Stale) => return self.enter_state(PageState::Stale),
            Some(CoreConfigState::Live) => {}
        }
        let Some(latest) = entry.and_then(|d| d.core_config.as_ref()) else {
            // `Live` guarantees a page; this arm cannot run, and returns the wait rather than
            // unwrapping into a panic on the frame path if that ever stops holding.
            return self.enter_state(PageState::Waiting);
        };
        let mut reseeded = false;
        // The revision gate first: it is one integer, and it is false on almost every wake. Only a
        // page that really moved is worth asking the far more expensive question below — whether
        // the user has edits that outrank it.
        if self.seen_rev != rev {
            let untouched = match (&self.draft, &self.seed) {
                (Some(draft), Some(seed)) => draft == seed,
                _ => true,
            };
            if untouched {
                self.seed = Some(latest.clone());
                self.draft = self.seed.clone();
                self.seen_rev = rev;
                self.had_page = true;
                reseeded = true;
            }
        }
        if reseeded {
            // The page under the banner is not the page the refusal was about any more.
            self.write_refused = false;
        }
        let state_changed = self.enter_state(PageState::Ready);
        reseeded || state_changed
    }

    /// Move to a state, dropping a page — and, where it would only get in the way, the binding —
    /// that state may no longer use.
    ///
    /// The binding is released in every blocked state but [`PageState::CoreMoved`], so the window
    /// recovers on its own the moment the obstacle clears: picking a core out of Overview, or the
    /// replacement MoonBot sending its configuration, then reaches [`PageState::Ready`] instead of
    /// being read as yet another move away from a core whose page is already gone. `CoreMoved`
    /// keeps its binding on purpose — it is the one state whose note tells the trader to press the
    /// gear again, and rebinding underneath them would swap the core silently.
    ///
    /// Args:
    ///     state: State the sync resolved.
    ///
    /// Returns:
    ///     Whether anything the window draws changed.
    fn enter_state(&mut self, state: PageState) -> bool {
        let before = (self.state, self.seeded, self.core_name.clone());
        if !state.can_send() && (self.draft.is_some() || self.seed.is_some()) {
            if self.draft != self.seed {
                log::info!(
                    "expert core settings dropped unsaved edits: the page can no longer be sent ({state:?})"
                );
            }
            self.draft = None;
            self.seed = None;
        }
        if !state.can_send() && state != PageState::CoreMoved {
            self.seeded = None;
            self.core_name = None;
        }
        // A dropped page is no longer at any revision: the next Ready must seed again rather than
        // conclude it is already current.
        if !state.can_send() {
            self.seen_rev = 0;
        }
        if state != self.state {
            self.write_refused = false;
        }
        self.state = state;
        (self.state, self.seeded, self.core_name.clone()) != before
    }

    /// Select a page.
    ///
    /// Every tab opens, including the ones with no wire values behind them: the window reproduces
    /// Moonbot's dialog, and what is blocked there is the CONTROL without a value, not the page.
    fn set_tab(&mut self, tab: ExpertTab, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        cx.notify();
    }

    /// Send the staged page to the core and close the window, as Moonbot's OK does.
    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.state.can_send().then(|| self.draft.clone()).flatten() else {
            // Nothing to send. Closing would read as a save; the page keeps saying what it is
            // waiting for instead.
            return;
        };
        if !send_core_config(&self.backend, &self.group, self.seeded, draft, cx) {
            // The core moved, or the session refused the page, between the render that drew OK and
            // this click. Say so and stay open.
            self.write_refused = true;
            cx.notify();
            return;
        }
        window.remove_window();
    }

    /// Close the window without sending, as Moonbot's Cancel does.
    ///
    /// The staged page needs no explicit discard: it lives in this view, which the window drops.
    fn cancel(&mut self, window: &mut Window) {
        window.remove_window();
    }

    /// Turn expert mode off: the gear goes back to the compact popup, so this window closes with
    /// it rather than staying up as the only surface the preference no longer points at.
    ///
    /// The staged page is DISCARDED, exactly as Cancel discards it — leaving expert mode is not a
    /// confirmation of the values on screen.
    fn leave_expert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.backend.update(cx, |b, bcx| {
            b.set_core_settings_expert(false, bcx);
        });
        window.remove_window();
    }

    /// Point an already-open window at another group's active core.
    ///
    /// The window is an application-wide singleton, so the gear of a SECOND group window has to
    /// reach the one that exists. Focusing it unchanged would show that trader the first group's
    /// core under their own group's gear — the same confusion `resolve_core_settings_write` keeps
    /// off the wire, except visible on screen.
    ///
    /// The staged page is dropped: it describes a core this window no longer addresses. Rebinding
    /// is unconditional, so a second press of the gear is also how a window stranded on a departed
    /// MoonBot instance is recovered.
    fn rebind(&mut self, group: String, cx: &mut Context<Self>) {
        if self.draft.is_some() && self.draft != self.seed {
            log::info!(
                "expert core settings rebound with unsaved edits: the gear of another group opened it"
            );
        }
        self.group = group;
        self.seeded = None;
        self.core_name = None;
        self.draft = None;
        self.seed = None;
        self.seen_rev = 0;
        self.had_page = false;
        self.write_refused = false;
        self.sync_from_core(cx);
        cx.notify();
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
/// One window for the whole application, like every other tool window here: it edits ONE core's
/// page, and a second window over a second core would give two drafts of the same dialog no way to
/// agree on which core OK writes to.
///
/// Args:
///     backend: Application state the window reads its page from and writes on OK.
///     owner: Window this one belongs to, for placement.
///     owner_display: Display the owner sits on.
///     group: Group whose active trading core the window edits.
///     cx: Application context.
pub(crate) fn open(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    group: String,
    cx: &mut App,
) {
    // A live window is REBOUND to this group and focused, never duplicated: the singleton edits one
    // core, and the gear that reached it here may belong to another group's window. The window
    // being live is decided by the HANDLE alone — a second window beside a live one is the worse
    // outcome, so an unreachable view costs the rebind and nothing more.
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
            Some(view) => view.update(cx, |this, cx| this.rebind(group, cx)),
            None => log::warn!(
                "expert core settings window focused without rebinding: its view is unreachable"
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
        let view = cx.new(|cx| CoreExpertView::new(b.clone(), group, window, cx));
        // Registered from inside the constructor, the way the Report window registers its panel:
        // this is the only place the view exists, and that handle is what a second group's gear
        // needs in order to rebind this window instead of showing it another group's core.
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
                bk.set_core_settings_expert(false, bcx);
            });
        }
    }
}
