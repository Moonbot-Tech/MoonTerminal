//! One line of chips in a row of the "WL distribution" tab — a whitelist, what an empty
//! whitelist trades, or a blacklist: its caption, the chips in the chosen order with their
//! colour, marks and menu, and the "+N" that folds a long line. Split from `view` to keep each
//! file readable.
//!
//! While any list on the board holds a draft, what Apply would change leads each line — removals,
//! then additions — at full brightness, and the unchanged chips step back.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonButton, MoonPalette, MoonTag, MoonTone, h_flex};
use rust_i18n::t;

use moon_core::session::CoreId;

use super::view::ListKind;
use super::{Chip, ChipState, Edit, Look, by_profit, look};
use crate::design;
use crate::design::{moon, moon_alpha};
use crate::strategies::StrategiesView;

/// Opacity of an unchanged chip while the board holds drafts, so what Apply would change stands
/// out. Not a theme colour: it scales whatever tone the chip already has.
const UNCHANGED_OPACITY: f32 = 0.4;

/// Chips a collapsed list draws before its "+N" button. A first core holding everybody else's
/// coins as its blacklist runs to hundreds of chips, and a hover repaints the whole window.
const CHIP_CAP: usize = 60;

impl StrategiesView {
    /// One list of one core as a wrapped line of chips; the blacklist sits on a red tint.
    pub(super) fn list_line(
        &self,
        core: CoreId,
        kind: ListKind,
        chips: &[Chip],
        edited: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let black = kind == ListKind::Black;
        let expanded = self.dist.expanded.contains(&(core, kind));
        let shown = if expanded {
            chips.len()
        } else {
            chips.len().min(CHIP_CAP)
        };
        // The count is what the list will HOLD — a struck-out chip is on its way out — and the
        // change apart from it.
        let removed = chips.iter().filter(|c| c.edit == Edit::Removed).count();
        let added = chips.iter().filter(|c| c.edit == Edit::Added).count();
        let held = chips.len() - removed;
        // A whitelist entry the core does not trade — blacklisted too (⊘), or gone from the
        // exchange — still counts as an entry; the bracket says how many it actually trades, the
        // figure the "Trades" line of an empty whitelist shows.
        let trading = chips
            .iter()
            .filter(|c| {
                c.edit != Edit::Removed
                    && matches!(c.state, ChipState::Normal | ChipState::Duplicate)
            })
            .count();
        let caption = match kind {
            ListKind::White if held > 0 => {
                t!("strat.dist_white_trading", n = held, traded = trading)
            }
            ListKind::White => t!("strat.dist_white", n = held),
            ListKind::Traded => t!("strat.dist_traded", n = held),
            ListKind::Black => t!("strat.dist_black", n = held),
        }
        .to_string();
        // On a line of its own under the count: the caption column is narrow.
        let changes = (removed + added > 0)
            .then(|| t!("strat.dist_changes", removed = removed, added = added).to_string());
        let mut line = h_flex()
            .w_full()
            .flex_wrap()
            .items_center()
            .gap(design::ui_px(cx, 4.0))
            .p(design::ui_px(cx, 4.0))
            .rounded(design::ui_px(cx, 4.0))
            .when(black, |el| el.bg(moon_alpha(p.red, 0.08)))
            .child(
                div()
                    .id(ElementId::Name(
                        format!("strat-dist-caption-{core}-{}", kind.id()).into(),
                    ))
                    .flex_none()
                    // A floor, not a width: a larger font or a longer locale widens the column
                    // instead of running the unbreakable count into the chips.
                    .min_w(design::ui_px(cx, 104.0))
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .when(kind == ListKind::Traded, |el| {
                        el.tooltip(crate::panels::common::text_tooltip(
                            t!("strat.dist_traded_tip").to_string(),
                        ))
                    })
                    .when(kind == ListKind::White && held > 0, |el| {
                        el.tooltip(crate::panels::common::text_tooltip(
                            t!("strat.dist_white_trading_tip").to_string(),
                        ))
                    })
                    // The count and its bracket are one figure: never split across lines.
                    .child(div().whitespace_nowrap().child(caption))
                    .children(changes.map(|c| div().child(c))),
            );
        if chips.is_empty() {
            let empty = match kind {
                ListKind::White => t!("strat.dist_white_empty"),
                ListKind::Traded => t!("strat.dist_traded_empty"),
                ListKind::Black => t!("strat.dist_black_empty"),
            };
            return line
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_faint))
                        .child(empty.to_string()),
                )
                .into_any_element();
        }
        let stats = self.dist.stats.coin_stats();
        let stat_of = |coin: &str| stats.and_then(|s| s.get(coin));
        let mut ordered: Vec<&Chip> = match self.dist.stats.by_profit {
            true => by_profit(chips, |coin| {
                stat_of(coin).and_then(|s| s.comparable_profit())
            }),
            false => chips.iter().collect(),
        };
        if self.dist.stats.reversed {
            ordered.reverse();
        }
        // What Apply would change leads: removals, then additions, then the rest — each group in
        // the order chosen above (the sort is stable).
        ordered.sort_by_key(|c| match c.edit {
            Edit::Removed => 0,
            Edit::Added => 1,
            Edit::Same => 2,
        });
        let selected = self.dist.stats.coin.as_deref();
        let side = kind.id();
        line = line.children(ordered[..shown].iter().map(|chip| {
            let stat = stat_of(&chip.coin);
            let tone = match look(chip.state, stat.and_then(|s| s.comparable_profit())) {
                Look::Gone => MoonTone::Muted,
                Look::Blocked => MoonTone::Danger,
                Look::Duplicate => MoonTone::Warning,
                Look::Profit => MoonTone::Positive,
                Look::Loss => MoonTone::Negative,
                Look::Flat => MoonTone::Default,
            };
            let blocked = chip.state == ChipState::Blocked;
            let edit = chip.edit;
            let coin = chip.coin.clone();
            // The selection is a tint BEHIND the chip, not a tone of its own: a selected coin that
            // is gone from the exchange or traded twice must still say so.
            let is_selected = selected == Some(chip.coin.as_str());
            div()
                .id(ElementId::Name(
                    format!("strat-dist-chip-{core}-{side}-{}", chip.coin).into(),
                ))
                .flex_none()
                .rounded_full()
                .cursor_pointer()
                .when(is_selected, |el| el.bg(moon_alpha(p.accent, 0.35)))
                .when(edited && edit == Edit::Same && !is_selected, |el| {
                    el.opacity(UNCHANGED_OPACITY)
                })
                // No tooltip (02.10, the developer's call): the chip's figures are in the trades
                // table its click opens. Left click shows that coin's trades and a second one
                // hides them; right click shows them and opens the edit menu.
                .on_click({
                    let coin = coin.clone();
                    cx.listener(move |this, _, _, cx| this.toggle_distribution_coin(&coin, cx))
                })
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.show_distribution_coin(&coin, cx);
                        this.open_chip_menu(core, kind, coin.clone(), edit, window, cx);
                    }),
                )
                .child({
                    // A mark the colour alone could not carry: the blacklist line under it is drawn
                    // on a red tint, so a red chip by itself would read as "this is the BL".
                    let text = match (blocked, edit) {
                        (_, Edit::Added) => format!("+ {}", chip.coin),
                        (true, _) => format!("⊘ {}", chip.coin),
                        (false, _) => chip.coin.clone(),
                    };
                    let tag = MoonTag::new().tone(tone);
                    // A coin the drafts take off the list stays in place, struck through, until
                    // Apply or Revert settles it.
                    match edit {
                        Edit::Removed => tag.child(div().line_through().child(text)),
                        _ => tag.label(text),
                    }
                })
        }));
        if chips.len() > CHIP_CAP {
            let label = match expanded {
                true => t!("strat.dist_collapse").to_string(),
                false => format!("+{}", chips.len() - shown),
            };
            line = line.child(
                MoonButton::new(ElementId::Name(
                    format!("strat-dist-more-{core}-{side}").into(),
                ))
                .ghost()
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.dist.expanded.remove(&(core, kind)) {
                        this.dist.expanded.insert((core, kind));
                    }
                    cx.notify();
                }))
                .render(),
            );
        }
        line.into_any_element()
    }
}
