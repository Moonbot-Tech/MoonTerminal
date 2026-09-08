//! The coin dropdown's tabs: the lists the field offers besides a search.
//!
//! A typed query always searches — that is what the field is — so these decide what the EMPTY
//! field shows: the suggestions it has always shown, the markets the user marked, and the coins
//! their cores are currently holding out of trading. Only the chart tab strip asks for them; the
//! header ticker and the Report token filter are query-only consumers and pass no configuration,
//! which is why every tab-aware part of the popup hangs off one optional value rather than a flag.
//!
//! The temporary-ban list is read from the CORE's own snapshot ([`moon_core::session::CoreData`]),
//! never from anything this popup remembers: a ban can be placed from MoonBot itself, by a cloud
//! signal, or by an exchange rate limit, and a list assembled from our own writes would show the
//! user a different world than the one their cores are in.

use std::collections::HashSet;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonAccent, MoonButton, MoonButtonSize, MoonButtonVariant, MoonPalette, MoonSegmentItem,
    MoonSegmentedControl, h_flex,
};
use rust_i18n::t;

use super::{CoinHit, coin_row_h, cores_for, hits_for};
use crate::Backend;
use crate::design;
use crate::display_text::fmt_ban_left;
use moon_core::config::ChartBucket;
use moon_core::session::CoreId;

/// Which list the coin dropdown is showing while the field is empty.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CoinTab {
    /// What the field has always offered: recently opened markets, then the top movers.
    #[default]
    All,
    /// Markets the user marked. Not yet fillable — the tab exists so the place it will live is
    /// decided once, with the others, instead of rearranging the popup later.
    Favorites,
    /// Coins the cores in this field's scope are holding out of trading, with what is left to run.
    Banned,
}

impl CoinTab {
    /// Every tab, in the order the strip draws them. The click handler resolves an index through
    /// THIS array, so the order here is the only place that decides it.
    pub(crate) const ALL: [CoinTab; 3] = [CoinTab::All, CoinTab::Favorites, CoinTab::Banned];

    /// Whether pressing this tab has anything to do, given what the field currently holds.
    ///
    /// NOT simply "a different tab": typing sends the strip back to [`CoinTab::All`] while the text
    /// stands, so pressing the already-highlighted All is the gesture that drops the query and
    /// returns the list the highlight is promising. Without this the one tab a reader presses to
    /// get back out of a search is the one that does nothing.
    ///
    /// Args:
    ///     open: The tab currently highlighted.
    ///     query_empty: Whether the field is empty, which is when a tab's own list is on screen.
    ///
    /// Returns:
    ///     Whether the press changes what the popup shows.
    pub(crate) fn press_acts(self, open: CoinTab, query_empty: bool) -> bool {
        self != open || !query_empty
    }

    /// Locale key of the strip caption.
    fn locale_key(self) -> &'static str {
        match self {
            CoinTab::All => "chart.coin.tab_all",
            CoinTab::Favorites => "chart.coin.tab_favorites",
            CoinTab::Banned => "chart.coin.tab_banned",
        }
    }
}

/// Chooses a tab, carrying the window because switching tabs also empties the field — see
/// `ChartTabs::select_coin_tab`.
pub(crate) type TabSelectFn = Rc<dyn Fn(CoinTab, &mut Window, &mut App)>;

/// Lifts the temporary ban one row is showing, on the core that holds it.
pub(crate) type UnbanFn = Rc<dyn Fn(CoreId, String, &mut App)>;

/// What a tabbed host hands the popup: which tab is open, and the two commands its tabs need.
///
/// One optional value rather than three parameters: a host either has tabs and all of this, or has
/// none of it, and there is no state in between for a caller to get half right.
#[derive(Clone)]
pub(crate) struct CoinTabsCfg {
    /// The open tab, whose list the host has already resolved into the results it passed.
    pub active: CoinTab,
    /// Called with the tab the user pressed.
    pub on_select: TabSelectFn,
    /// Called with the core and market of the ban row whose lift button was pressed.
    pub on_unban: UnbanFn,
}

/// One temporarily banned market, ready to draw.
pub(crate) struct CoinBan {
    /// The market itself, labelled exactly as a search hit for it would be.
    pub hit: CoinHit,
    /// The ban as its core listed it, carrying the deadline and what this window may do about it.
    pub ban: BanSource,
}

/// One core's listed ban, before the market catalogue has named it.
pub(crate) struct BanSource {
    /// Core holding it.
    pub core: CoreId,
    /// The market as THAT core spells it — the key the ban is listed under and the value a lift has
    /// to send back. See `coin_menu::temp_ban_symbol` on why a coin name cannot stand in for it.
    pub market: String,
    /// When the ban runs out, Unix ms; see `moon_core::session::CoreData::temp_bans` on why the
    /// deadline rather than the remainder.
    pub until_ms: i64,
    /// Whether its core is behind on its settings, which makes the countdown an extrapolation
    /// rather than a report. The row says so with a tilde and a tooltip, as the coin menu does.
    pub stale: bool,
    /// Whether this group may still command that core. A row stays VISIBLE when it may not — the
    /// ban is real and the reader should see it — but its lift is disabled rather than silently
    /// refused when the press arrives; see `ChartTabs::lift_temp_ban`.
    pub allowed: bool,
}

/// Returns every temporary ban held by the cores feeding this field, soonest to expire first.
///
/// Scoped through the same [`cores_for`] the typed search and the suggestions use: a coin field
/// belonging to one tab must not offer to lift a ban on a core it would never open a chart for.
///
/// Args:
///     b: Backend holding the sessions, the market catalogue and the core snapshots.
///     group: Window group whose cores feed this field.
///     bucket: Chart bucket narrowing those cores, or `None` for the whole group.
///
/// Returns:
///     Labelled ban rows; empty when nothing in scope is banned.
pub(crate) fn banned(b: &Backend, group: &str, bucket: Option<&ChartBucket>) -> Vec<CoinBan> {
    let mut sources: Vec<BanSource> = Vec::new();
    // One row per market even if a core lists it twice: two rows would offer the same lift twice
    // and disagree with the chart's own lock, which reads the first match.
    let mut seen: HashSet<(CoreId, String)> = HashSet::new();
    for core in cores_for(b, group, bucket) {
        let Some(data) = b.session.store().core(core) else {
            continue;
        };
        let mut core_facts = None;
        for (market, until_ms) in data.temp_bans() {
            if !seen.insert((core, market.to_ascii_uppercase())) {
                continue;
            }
            // Resolved on the first row and not before: `workspace_action_allows_core` walks the
            // group's whole Auto scope, and most cores in a scope hold no ban at all.
            let (stale, allowed) = *core_facts.get_or_insert_with(|| {
                (
                    // The state, not the raw latch: a core that has gone quiet since its last
                    // snapshot is extrapolating just as much as one whose write is outstanding.
                    data.client_settings_state() != moon_core::feed::CoreConfigState::Live,
                    b.workspace_action_allows_core(Some(group), core),
                )
            });
            sources.push(BanSource {
                core,
                market: market.to_string(),
                until_ms,
                stale,
                allowed,
            });
        }
    }
    let hits = hits_for(b, sources.iter().map(|src| (src.core, src.market.clone())));
    pair_bans(hits, sources)
}

/// Joins labelled hits back to the bans they were built from, soonest first.
///
/// Split from [`banned`] because it is the half that can be tested: [`hits_for`] needs a live
/// `Backend`, while the pairing and the ordering are what a reader actually depends on.
///
/// A hit with no source is dropped rather than drawn at zero — `hits_for` may return fewer rows
/// than it was given, and a row with no deadline would read as a ban that has just run out while
/// in fact nothing is known about it.
///
/// Args:
///     hits: Labelled markets, as `hits_for` produced them.
///     sources: The listed bans, in the order they were collected.
///
/// Returns:
///     Rows ordered by deadline, ties broken by market so equal deadlines keep a stable order.
fn pair_bans(hits: Vec<CoinHit>, mut sources: Vec<BanSource>) -> Vec<CoinBan> {
    let mut rows: Vec<CoinBan> = hits
        .into_iter()
        .filter_map(|hit| {
            // Matched case-insensitively, the rule every other reader of these symbols uses; the
            // labels come back carrying the exact string they were asked about, so this is a
            // backstop rather than the mechanism. Taken OUT of the sources, so one market cannot
            // hand its deadline to two rows.
            let ix = sources.iter().position(|src| {
                src.core == hit.core && src.market.eq_ignore_ascii_case(&hit.market)
            })?;
            Some(CoinBan {
                ban: sources.swap_remove(ix),
                hit,
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        a.ban
            .until_ms
            .cmp(&b.ban.until_ms)
            .then_with(|| a.hit.market.cmp(&b.hit.market))
            // The same market on two cores at one deadline: ordered outright rather than left to
            // sort stability over whatever order the cores were walked in.
            .then_with(|| a.ban.core.cmp(&b.ban.core))
    });
    rows
}

/// Renders the tab strip above the result list.
///
/// Drawn OUTSIDE the scrolling list, like the footer button: the viewport cap counts fixed-height
/// direct children of that list ([`super::direct_row_count`]), and a control of another height
/// inside it would make the cap lie about how many rows fit.
///
/// Args:
///     id: Stable popup identity used to derive the control's own id.
///     cfg: The host's tab configuration, carrying the open tab and the select command.
///     p: Active palette used by the separating border.
///     cx: Application context used to resolve scaled design tokens.
///
/// Returns:
///     The strip as one bordered row.
pub(super) fn render_tab_strip(
    id: &'static str,
    cfg: &CoinTabsCfg,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    let active = cfg.active;
    let on_select = cfg.on_select.clone();
    let items: Vec<MoonSegmentItem> = CoinTab::ALL
        .iter()
        .map(|tab| {
            MoonSegmentItem::new("", t!(tab.locale_key()).to_string())
                // Fitted rather than fixed: three captions in three languages do not share a
                // width, and a fixed one either clips the Russian or pads the English. The ceiling
                // keeps three of them inside the popup's own width whatever a translation says.
                .fit_width(cx, TAB_MIN_W, TAB_MAX_W)
                .selected(*tab == active)
        })
        .collect();
    div()
        .w_full()
        .flex_none()
        .overflow_hidden()
        .px(design::ui_px(cx, 6.0))
        .py(design::ui_px(cx, 4.0))
        .border_b_1()
        .border_color(rgb(p.border))
        .child(
            MoonSegmentedControl::new(SharedString::from(format!("{id}-tabs")))
                .accent(MoonAccent::Blue)
                .items(items)
                .on_click(move |ix, _, window, app| {
                    // Resolved through the same array the items were built from, so a tab added
                    // there cannot land on the wrong index here.
                    if let Some(tab) = CoinTab::ALL.get(ix) {
                        on_select(*tab, window, app);
                    }
                })
                .render(),
        )
}

/// Design-reference width bounds of one tab caption. Three at the ceiling plus the row's own
/// padding stay inside [`super::COIN_POPUP_W`].
const TAB_MIN_W: f32 = 52.0;
const TAB_MAX_W: f32 = 88.0;

/// Appends the temporary-ban rows to the scrolling list, one fixed-height row each.
///
/// A FLAT list rather than the exchange/coin tree the search results use: a ban is one core's
/// decision about one of its markets, so there is nothing to fold — two cores banning the same
/// coin are two facts, each with its own clock and its own lift.
///
/// Args:
///     list: Stateful scrolling list that receives the rows.
///     id: Stable popup identity used to derive row ids.
///     rows: The bans, already ordered by [`banned`].
///     show_server_per_row: Whether a row names the core holding the ban; `false` when the popup
///         already names the one server above the list.
///     p: Active palette used by row text and hover states.
///     cx: Application context used to resolve scaled design tokens.
///     on_pick: Callback for opening a row's market, the same one the search rows use.
///     on_unban: Callback for the lift button, or `None` to show the rows without one.
///
/// Returns:
///     The same list with one row appended per ban.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_ban_rows<F>(
    mut list: Stateful<Div>,
    id: &'static str,
    rows: Vec<CoinBan>,
    show_server_per_row: bool,
    p: MoonPalette,
    cx: &App,
    on_pick: F,
    on_unban: Option<UnbanFn>,
) -> Stateful<Div>
where
    F: Fn(CoreId, String, &mut Window, &mut App) + Clone + 'static,
{
    // One clock for the whole list, so two rows a millisecond apart cannot print figures that
    // disagree by a minute.
    let now_ms = moon_core::util::now_unix_ms_i64();
    for row in rows {
        list = list.child(ban_row(
            id,
            row,
            now_ms,
            show_server_per_row,
            p,
            cx,
            on_pick.clone(),
            on_unban.clone(),
        ));
    }
    list
}

/// Draws one ban: what is banned, on which core, how much is left, and the button that lifts it.
///
/// Args:
///     id: Stable popup identity used to derive this row's ids.
///     row: The ban itself.
///     now_ms: Clock the whole list counts down against.
///     show_server_per_row: Whether to name the core here.
///     p: Active palette.
///     cx: Application context used to resolve scaled design tokens.
///     on_pick: Callback for opening the market.
///     on_unban: Callback for the lift button, or `None` for a list without one.
///
/// Returns:
///     One fixed-height row.
#[allow(clippy::too_many_arguments)]
fn ban_row<F>(
    id: &'static str,
    row: CoinBan,
    now_ms: i64,
    show_server_per_row: bool,
    p: MoonPalette,
    cx: &App,
    on_pick: F,
    on_unban: Option<UnbanFn>,
) -> impl IntoElement
where
    F: Fn(CoreId, String, &mut Window, &mut App) + Clone + 'static,
{
    let CoinBan {
        hit:
            CoinHit {
                core,
                market,
                server,
                label,
                ..
            },
        ban:
            BanSource {
                until_ms,
                stale,
                allowed,
                ..
            },
    } = row;
    let left = fmt_ban_left(until_ms.saturating_sub(now_ms));
    let left = match stale {
        true => format!("~{left}"),
        false => left,
    };
    let pair = SharedString::from(label.pair());
    // Identity, not POSITION: this list re-sorts itself as bans arrive, are lifted and expire, and
    // an index-keyed row hands a press begun on one market to whichever market inherited the slot
    // — on a control whose whole job is to act on one named coin.
    let row_id = format!("{id}-ban-{core}-{market}");
    let market_pick = market.clone();
    div()
        .id(SharedString::from(row_id.clone()))
        .w_full()
        .h(px(coin_row_h(cx)))
        .flex_none()
        .flex()
        .items_center()
        .px(design::ui_px(cx, 8.0))
        .hover(move |s| s.bg(rgb(p.shell_high)))
        .child(
            h_flex()
                .w_full()
                .gap(design::ui_px(cx, 6.0))
                .items_center()
                // Opening the coin is the row; lifting the ban is the button. They are separate
                // targets so a misplaced click cannot untrade a coin — and the pointer cursor
                // covers exactly the half that opens.
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(design::ui_px(cx, 6.0))
                        .items_baseline()
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, move |_, window, app| {
                            on_pick(core, market_pick.clone(), window, app);
                            app.stop_propagation();
                        })
                        .child(
                            div()
                                .flex_none()
                                .text_size(design::t_body(cx))
                                .text_color(rgb(p.text))
                                .child(pair),
                        )
                        .when(show_server_per_row, |row| {
                            row.child(
                                // On the CELL, not the row: a core name wider than the popup clips
                                // and its whole form has to stay reachable, but a row-wide tooltip
                                // would sit over the lift button — which is the rule the contract
                                // test `the coin result row must not attach a tooltip` states for
                                // the search rows beside these. A plain `Div` carries no tooltip,
                                // hence the id.
                                div()
                                    .id(SharedString::from(format!("{row_id}-server")))
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(design::t_caption(cx))
                                    .text_color(rgb(p.text_muted))
                                    .tooltip(crate::panels::common::text_tooltip(server.clone()))
                                    .child(format!("@{server}")),
                            )
                        })
                        .child(
                            div()
                                .id(SharedString::from(format!("{row_id}-left")))
                                .flex_none()
                                .text_size(design::t_caption(cx))
                                .text_color(rgb(p.text_soft))
                                // What the tilde means, rather than a figure that merely looks
                                // rounded. The same fact the coin menu spells out.
                                .when(stale, |cell| {
                                    cell.tooltip(crate::panels::common::text_tooltip(
                                        t!("chart.coin.ban_stale").to_string(),
                                    ))
                                })
                                .child(left),
                        ),
                )
                .children(on_unban.map(|unban| {
                    MoonButton::new(SharedString::from(format!("{row_id}-lift")))
                        .label(design::GLYPH_CLOSE)
                        .size(MoonButtonSize::Micro)
                        .variant(MoonButtonVariant::Soft)
                        .tooltip(match allowed {
                            true => t!("chart.coin.unban").to_string(),
                            false => t!("chart.coin.unban_denied").to_string(),
                        })
                        // Disabled rather than silently refused when this group may no longer
                        // command the core: the press has to say something, and a control that
                        // answers nothing is the worse of the two.
                        .disabled(!allowed)
                        .on_click(move |_, _window, app| {
                            unban(core, market.clone(), app);
                            app.stop_propagation();
                        })
                        .render()
                })),
        )
}

#[cfg(test)]
mod tests;
