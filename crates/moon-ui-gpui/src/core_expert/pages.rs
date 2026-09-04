//! Bodies of the expert window's tabs, one module per Moonbot page.
//!
//! Each page reproduces its Moonbot original ROW FOR ROW, including the rows this terminal cannot
//! fill: those are drawn and disabled rather than hidden, so the window can be read side by side
//! with Moonbot's own dialog. What decides a row's fate is the projection
//! (`moon_core::feed::CoreConfig`) and whether this page's tab names that section in
//! [`super::ExpertTab::add_sections`] — a control outside either is dead until both are widened,
//! and drawing it as live would promise a value OK cannot carry.
//!
//! Controls that need a retained state (sliders, text fields) are declared as SPECS here and built
//! once per render by the window, exactly as the compact popup builds its own — see
//! [`crate::shell::core_settings::editors`]. A dead row still declares its spec: a `MoonSlider`
//! needs a state to draw at all, and its staging function is the one that writes nothing.

mod autobuy;
mod autostart;
mod general;
mod hotkeys;
mod interface;
mod login;
mod special;
mod telegram;

pub(super) use hotkeys::HotkeysSub;
pub(super) use special::SpecialSection;

use gpui::*;
use moon_ui::MoonPalette;

use moon_core::feed::CoreConfig;
use moon_core::session::CoreId;

use crate::Backend;
use crate::shell::editors::EditorStore;

use super::{CoreExpertView, ExpertTab};

/// One report counter as a page prints it: the sum and the trade count behind it, or `None` until
/// the core publishes them.
pub(super) type ProfitCounter = Option<(f64, i32)>;

/// What a page needs beyond the staged draft: the core it addresses and the report counters some
/// rows print beside their own setting.
///
/// Passed in rather than read from the view: a page is built from inside that view's own render,
/// where reading it back would panic.
pub(super) struct PageCtx<'a> {
    pub(super) backend: &'a Entity<Backend>,
    pub(super) group: &'a str,
    pub(super) seeded: Option<CoreId>,
    /// The core's trade-window and hourly report totals, each `None` until the core publishes them.
    pub(super) profit: (ProfitCounter, ProfitCounter),
    /// Which of Moonbot's inner Hotkeys tabs is open. Carried here because it belongs to the
    /// WINDOW, not to a page: a page is rebuilt on every render and could not remember it.
    pub(super) hotkeys_sub: HotkeysSub,
    /// Which of the Special page's four collapsible sections is open, for the same reason.
    pub(super) special_section: SpecialSection,
    /// Which row of the Telegram page's channel box is picked, for the same reason.
    pub(super) selected_channel: Option<usize>,
}

/// Ids of the page's text boxes that hold a value on its way somewhere else rather than a setting.
///
/// Built through `editors::scratch_input_state`, which does not subscribe: see its doc for why a
/// box like this must not stage.
pub(super) fn scratch_specs(tab: ExpertTab) -> &'static [&'static str] {
    match tab {
        ExpertTab::Telegram => telegram::SCRATCH_FIELDS,
        _ => &[],
    }
}

/// Text fields the page on screen needs, as `(id, current value, staging function)`.
///
/// Per TAB, not per window: a control belongs to the page that draws it, and building the others'
/// would pay for every page ever ported on every repaint of whichever one is open.
#[allow(clippy::type_complexity)]
pub(super) fn field_specs(
    tab: ExpertTab,
    draft: &CoreConfig,
) -> Vec<(&'static str, String, fn(&mut CoreConfig, &str))> {
    match tab {
        ExpertTab::General => general::field_specs(draft),
        ExpertTab::Login => login::field_specs(draft),
        ExpertTab::AutoBuy => autobuy::field_specs(draft),
        ExpertTab::AutoStart => autostart::field_specs(draft),
        ExpertTab::Interface => interface::field_specs(draft),
        ExpertTab::Special => special::field_specs(draft),
        _ => Vec::new(),
    }
}

/// Sliders the page on screen needs, as `(id, bounds, current value, staging function, mirror)`.
#[allow(clippy::type_complexity)]
pub(super) fn slider_specs(
    tab: ExpertTab,
    draft: &CoreConfig,
) -> Vec<(
    &'static str,
    (f32, f32, f32),
    f32,
    fn(&mut CoreConfig, f32),
    Option<&'static str>,
)> {
    match tab {
        ExpertTab::General => general::slider_specs(draft),
        ExpertTab::AutoBuy => autobuy::slider_specs(draft),
        ExpertTab::AutoStart => autostart::slider_specs(draft),
        ExpertTab::Interface => interface::slider_specs(draft),
        ExpertTab::Special => special::slider_specs(draft),
        _ => Vec::new(),
    }
}

/// The body of one page, or `None` while that page has not been ported yet.
///
/// Args:
///     tab: Page to draw.
///     view: Window entity the controls stage through.
///     store: Controls built for this render.
///     draft: Staged page every control reads and writes.
///     p: Active palette.
///     cx: Application context used for font-scaled geometry and for reading control states.
///
/// Returns:
///     The page body, or `None` when the window should render its own placeholder note instead.
#[allow(clippy::too_many_arguments)]
pub(super) fn page(
    tab: ExpertTab,
    view: &Entity<CoreExpertView>,
    store: &EditorStore,
    draft: &CoreConfig,
    ctx: &PageCtx<'_>,
    p: MoonPalette,
    cx: &App,
) -> Option<AnyElement> {
    match tab {
        ExpertTab::General => Some(general::body(view, store, draft, p, cx)),
        ExpertTab::Login => Some(login::body(view, store, p, cx)),
        ExpertTab::Telegram => Some(telegram::body(
            view,
            store,
            draft,
            ctx.selected_channel,
            p,
            cx,
        )),
        ExpertTab::AutoBuy => Some(autobuy::body(view, store, draft, p, cx)),
        ExpertTab::AutoStart => Some(autostart::body(
            view,
            store,
            draft,
            ctx.backend,
            ctx.group,
            ctx.seeded,
            ctx.profit,
            p,
            cx,
        )),
        ExpertTab::Interface => Some(interface::body(view, store, draft, p, cx)),
        ExpertTab::Hotkeys => Some(hotkeys::body(view, ctx.hotkeys_sub, draft, p, cx)),
        ExpertTab::Special => Some(special::body(view, store, ctx.special_section, p, cx)),
    }
}
