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
    /// Markets the cores in scope have MARKED, read from each core's own favourites list — the
    /// same one the chart's star writes.
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

/// Which acting list a row belongs to.
///
/// One value carrying the three things that differ between the favourites tab and the ban tab —
/// the id namespace, the note an empty one prints, and what its button does — so a third such tab
/// is one variant rather than a third copy of the same twenty lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MarkedList {
    Favorites,
    Banned,
}

impl MarkedList {
    /// Id namespace, so a reader switching tabs cannot hand one list's row state to the other.
    fn id_kind(self) -> &'static str {
        match self {
            MarkedList::Favorites => "fav",
            MarkedList::Banned => "ban",
        }
    }

    /// Locale key of the note an empty list prints.
    pub(super) fn empty_key(self) -> &'static str {
        match self {
            MarkedList::Favorites => "chart.coin.no_favorites",
            MarkedList::Banned => "chart.coin.no_banned",
        }
    }

    /// Locale key of the row button's tooltip while it can act.
    fn act_key(self) -> &'static str {
        match self {
            MarkedList::Favorites => "chart.coin.unfav",
            MarkedList::Banned => "chart.coin.unban",
        }
    }
}

/// Chooses a tab, carrying the window because switching tabs also empties the field — see
/// `ChartTabs::select_coin_tab`.
pub(crate) type TabSelectFn = Rc<dyn Fn(CoinTab, &mut Window, &mut App)>;

/// Acts on the market one row is showing, on the core that holds it: lifting its ban, or taking
/// the mark off it.
pub(crate) type RowActionFn = Rc<dyn Fn(CoreId, String, &mut App)>;

/// What a tabbed host hands the popup: which tab is open, and the commands its tabs need.
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
    pub on_unban: RowActionFn,
    /// Called with the core and market of the favourite row whose remove button was pressed.
    pub on_unfav: RowActionFn,
}

impl CoinTabsCfg {
    /// The command one acting list's row button carries.
    pub(crate) fn row_action(&self, list: MarkedList) -> RowActionFn {
        match list {
            MarkedList::Favorites => self.on_unfav.clone(),
            MarkedList::Banned => self.on_unban.clone(),
        }
    }
}

/// One row of a tab that ACTS on its market: the coin, an optional figure, and the button that
/// takes it off the list the tab is showing.
///
/// One shape for both lists, because they are one shape: a ban row and a favourite row differ in
/// the figure between the coin and the button, and nowhere else. Two renderers would be two places
/// to change a row height, a hover colour or a padding — and the first change would land in one.
pub(crate) struct CoinActRow {
    /// The market itself, labelled exactly as a search hit for it would be.
    pub hit: CoinHit,
    /// The figure between the coin and the button — what is left of a ban — or `None` for a list
    /// that counts nothing down.
    pub note: Option<RowNote>,
    /// Whether this window may still command the row's core. A row stays VISIBLE when it may not —
    /// the fact is real and the reader should see it — but its button is disabled rather than
    /// silently refused when the press arrives.
    pub allowed: bool,
    /// What the row's BUTTON acts on, which is not always what the row opens: a temporary ban is
    /// keyed by the market, a favourite by the core's own `market_currency`. The two lists disagree
    /// about that on purpose — see `coin_menu::temp_ban_symbol` and the core's own matching rule —
    /// so the key travels with the row rather than being re-derived where the press lands.
    pub action_key: String,
}

/// One temporarily banned market, before it becomes a [`CoinActRow`].
struct CoinBan {
    /// The market itself, labelled exactly as a search hit for it would be.
    pub hit: CoinHit,
    /// The ban as its core listed it, carrying the deadline and what this window may do about it.
    pub ban: BanSource,
}

/// The figure one acting row prints between the coin and its button.
pub(crate) struct RowNote {
    /// The text itself.
    pub text: String,
    /// Whether it is an extrapolation rather than a report; it then says so in a tooltip. Kept WITH
    /// the text, because a staleness with nothing to qualify is a state no row can draw.
    pub stale: bool,
}

/// One core's listed ban, before the market catalogue has named it.
struct BanSource {
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
pub(crate) fn banned(b: &Backend, group: &str, bucket: Option<&ChartBucket>) -> Vec<CoinActRow> {
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
    // One clock for the whole list, so two rows a millisecond apart cannot print figures that
    // disagree by a minute. Read HERE rather than in the renderer: the deadline is what travels,
    // and the text is what a row is drawn from.
    let now_ms = moon_core::util::now_unix_ms_i64();
    pair_bans(hits, sources)
        .into_iter()
        .map(|row| {
            let left = fmt_ban_left(row.ban.until_ms.saturating_sub(now_ms));
            CoinActRow {
                action_key: row.hit.market.clone(),
                hit: row.hit,
                note: Some(RowNote {
                    text: match row.ban.stale {
                        true => format!("~{left}"),
                        false => left,
                    },
                    stale: row.ban.stale,
                }),
                allowed: row.ban.allowed,
            }
        })
        .collect()
}

/// Returns every market the cores feeding this field have MARKED, in core order.
///
/// The core's own list (`trading.fav_markets`), the same one MoonBot's star writes — see
/// `Backend::fav_markets_of`. A core that has not reported its configuration yet contributes
/// nothing rather than an empty list drawn as "nothing marked": the tab's empty note says what is
/// known, and what is not known is silence.
///
/// Args:
///     b: Backend holding the sessions, the market catalogue and the core snapshots.
///     group: Window group whose cores feed this field.
///     bucket: Chart bucket narrowing those cores, or `None` for the whole group.
///
/// Returns:
///     Labelled rows; empty when nothing in scope is marked.
pub(crate) fn favorites(b: &Backend, group: &str, bucket: Option<&ChartBucket>) -> Vec<CoinActRow> {
    let mut rows: Vec<CoinActRow> = Vec::new();
    for core in cores_for(b, group, bucket) {
        // `None` is the core's SILENCE — it has reported no configuration — and contributes
        // nothing, exactly as an empty list does. The two differ to a reader of the store; to a
        // list of marked coins they are the same answer.
        let Some(coins) = b.fav_markets_of(core) else {
            continue;
        };
        let coins = dedup_markets(coins);
        if coins.is_empty() {
            continue;
        }
        let allowed = b.workspace_action_allows_core(Some(group), core);
        for coin in coins {
            // A row NAMES a market — it opens a chart — while the list names a coin, so each entry
            // is resolved against the core's own catalogue. An entry the catalogue cannot name is
            // skipped rather than drawn: there is no chart behind it and no label to draw it with.
            let Some(hit) = market_of_coin(b, group, bucket, core, &coin) else {
                continue;
            };
            rows.push(CoinActRow {
                action_key: coin,
                hit,
                note: None,
                allowed,
            });
        }
    }
    rows
}

/// The market one core would open for a coin its favourites list names.
///
/// The list holds `market_currency` — the core's own name for the coin, which is what it matches
/// its favourites against — and a chart needs a market. The catalogue answers that, through the
/// same search the coin field runs, filtered to the ONE core and to hits whose label carries this
/// exact coin: a text search for `ICX` also finds `ICXUP` and `OMICX`, and neither is this row.
///
/// Args:
///     b: Backend holding the market catalogue.
///     group: Window group whose cores feed this field.
///     bucket: Chart bucket narrowing those cores.
///     core: Core whose list named the coin.
///     coin: The coin, as that core spells it.
///
/// Returns:
///     The first market of that core carrying this coin, or `None` when its catalogue has none.
fn market_of_coin(
    b: &Backend,
    group: &str,
    bucket: Option<&ChartBucket>,
    core: CoreId,
    coin: &str,
) -> Option<CoinHit> {
    super::search_limited(b, group, bucket, coin, super::COIN_MATCH_LIMIT)
        .into_iter()
        .find(|hit| hit.core == core && hit.label.coin.eq_ignore_ascii_case(coin))
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

/// One entry per market the core names, however many times it names it.
///
/// A duplicate is not merely an ugly second row: both rows would carry the SAME element id, which
/// GPUI refuses inside one frame. The core's list is a hand-edited string on the other side of a
/// wire, so this is a shape we receive rather than one we can rule out.
///
/// Args:
///     markets: One core's list, in its own order.
///
/// Returns:
///     The same order, first spelling kept, later repeats dropped.
fn dedup_markets(markets: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(markets.len());
    for market in markets {
        if !out.iter().any(|held| held.eq_ignore_ascii_case(&market)) {
            out.push(market);
        }
    }
    out
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

/// Appends the rows of an acting tab to the scrolling list, one fixed-height row each.
///
/// A FLAT list rather than the exchange/coin tree the search results use: a ban and a mark are one
/// core's statement about one of its markets, so there is nothing to fold — two cores that banned
/// or marked the same coin are two facts, each with its own button.
///
/// Args:
///     list: Stateful scrolling list that receives the rows.
///     id: Stable popup identity used to derive row ids.
///     list: Which list this is — its id namespace and its button's caption.
///     rows: The rows, already ordered by their builder.
///     show_server_per_row: Whether a row names the core; `false` when the popup already names the
///         one server above the list.
///     p: Active palette used by row text and hover states.
///     cx: Application context used to resolve scaled design tokens.
///     on_pick: Callback for opening a row's market, the same one the search rows use.
///     on_press: Callback for the row button, or `None` to show the rows without one.
///
/// Returns:
///     The same list with one row appended per entry.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_marked_rows<F>(
    mut rendered: Stateful<Div>,
    id: &'static str,
    list: MarkedList,
    rows: Vec<CoinActRow>,
    show_server_per_row: bool,
    p: MoonPalette,
    cx: &App,
    on_pick: F,
    on_press: Option<RowActionFn>,
) -> Stateful<Div>
where
    F: Fn(CoreId, String, &mut Window, &mut App) + Clone + 'static,
{
    for row in rows {
        rendered = rendered.child(marked_row(
            id,
            list,
            row,
            show_server_per_row,
            p,
            cx,
            on_pick.clone(),
            on_press.clone(),
        ));
    }
    rendered
}

/// Draws one such row: what it names, on which core, its figure if it has one, and its button.
///
/// Args:
///     id: Stable popup identity used to derive this row's ids.
///     list: Which list this is.
///     row: The row itself.
///     show_server_per_row: Whether to name the core here.
///     p: Active palette.
///     cx: Application context used to resolve scaled design tokens.
///     on_pick: Callback for opening the market.
///     on_press: Callback for the button, or `None` for a list without one.
///
/// Returns:
///     One fixed-height row.
#[allow(clippy::too_many_arguments)]
fn marked_row<F>(
    id: &'static str,
    list: MarkedList,
    row: CoinActRow,
    show_server_per_row: bool,
    p: MoonPalette,
    cx: &App,
    on_pick: F,
    on_press: Option<RowActionFn>,
) -> impl IntoElement
where
    F: Fn(CoreId, String, &mut Window, &mut App) + Clone + 'static,
{
    let CoinActRow {
        hit:
            CoinHit {
                core,
                market,
                server,
                label,
                ..
            },
        note,
        allowed,
        action_key,
    } = row;
    let pair = SharedString::from(label.pair());
    // Identity, not POSITION: these lists re-sort themselves as bans expire and marks arrive, and
    // an index-keyed row hands a press begun on one market to whichever market inherited the slot
    // — on a control whose whole job is to act on one named coin.
    let row_id = format!("{id}-{}-{core}-{market}", list.id_kind());
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
                // Opening the coin is the row; the list button is the button. They are separate
                // targets so a misplaced click cannot act on a coin — and the pointer cursor covers
                // exactly the half that opens.
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
                                // would sit over the button — which is the rule the contract test
                                // `the coin result row must not attach a tooltip` states for the
                                // search rows beside these. A plain `Div` carries no tooltip, hence
                                // the id.
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
                        .children(note.map(|note| {
                            div()
                                .id(SharedString::from(format!("{row_id}-note")))
                                .flex_none()
                                .text_size(design::t_caption(cx))
                                .text_color(rgb(p.text_soft))
                                // What the tilde means, rather than a figure that merely looks
                                // rounded. The same fact the coin menu spells out.
                                .when(note.stale, |cell| {
                                    cell.tooltip(crate::panels::common::text_tooltip(
                                        t!("chart.coin.ban_stale").to_string(),
                                    ))
                                })
                                .child(note.text)
                        })),
                )
                .children(on_press.map(|press| {
                    MoonButton::new(SharedString::from(format!("{row_id}-act")))
                        .label(design::GLYPH_CLOSE)
                        .size(MoonButtonSize::Micro)
                        .variant(MoonButtonVariant::Soft)
                        // What it does, or — while this window may no longer command the core —
                        // why it cannot. The refusal reads the same on every acting list, so it is
                        // named here rather than carried per list.
                        .tooltip(match allowed {
                            true => t!(list.act_key()).to_string(),
                            false => t!("chart.coin.act_denied").to_string(),
                        })
                        // Disabled rather than silently refused when this group may no longer
                        // command the core: the press has to say something, and a control that
                        // answers nothing is the worse of the two.
                        .disabled(!allowed)
                        .on_click(move |_, _window, app| {
                            press(core, action_key.clone(), app);
                            app.stop_propagation();
                        })
                        .render()
                })),
        )
}

#[cfg(test)]
mod tests;
