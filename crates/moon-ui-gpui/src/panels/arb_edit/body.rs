//! The window's layout: what every row prints, and the roster itself.
//!
//! One line per venue, and the line IS the settings: order, visibility, name, colour. There is no
//! per-venue pane to open, because a venue has four settings and three of them are one click.

use gpui::*;
use moon_core::config::{ArbShow, ArbVenueCfg};
use moon_ui::{MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::ArbEditState;
use crate::chart_tabs::seg_row;
use crate::design::{self, moon};
use crate::panels::{micro_button, popup_group, toggle_variant};

/// Colours a venue can take, matching the caption editor's own set so the two windows offer one
/// palette rather than two.
const VENUE_COLORS: [u32; 8] = [
    0xffffff, 0xffd166, 0xef476f, 0x06d6a0, 0x4cc9f0, 0xb388ff, 0xff9f1c, 0x8d99ae,
];

/// Width of one micro glyph button.
const MICRO_W: f32 = 20.0;
/// Width of the venue name column.
const NAME_W: f32 = 110.0;

/// Venue columns the window is sized for.
///
/// The roster is twenty-five venues — every named exchange plus the deployer indices — and one
/// column of them is a window taller than most screens. TWO is what the dialog's width fits, so the
/// lines-per-column follows from the roster rather than the other way round: a fixed twelve turned
/// twenty-five venues into three columns inside a two-column window, and every line overflowed.
const COLUMNS: usize = 2;

/// Spread floors offered, in percent. `0` shows every venue and is the default.
///
/// Steps rather than a free field, like the caption editor's own thresholds: the window holds one
/// text input already — the venue name — and a second one for a number is a focus target nobody
/// asked for.
const MIN_ABS_STEPS: [f32; 6] = [0.0, 0.1, 0.25, 0.5, 1.0, 2.0];

pub(super) fn dialog_body(state: &Entity<ArbEditState>, cx: &mut App) -> AnyElement {
    let p = MoonPalette::active(cx);
    let (cfg, dex_names) = {
        let s = state.read(cx);
        (s.cfg.clone(), s.dex_names.clone())
    };

    // What every row prints. One choice for the whole column: a roster where one venue showed a
    // price and the next a percentage would not be a column, it would be a list of unlike things.
    let show_row = {
        let state = state.clone();
        seg_row(
            "arb-show".to_string(),
            t!("arb.show").to_string(),
            ArbShow::ALL
                .iter()
                .map(|s| (t!(s.locale_key()).to_string(), *s == cfg.show))
                .collect(),
            96.0,
            p,
            cx,
            move |pick, cx| {
                if let Some(show) = ArbShow::ALL.get(pick).copied() {
                    ArbEditState::write(&state, cx, |cfg| cfg.show = show);
                }
            },
        )
    };

    let blocked_cb = {
        let state = state.clone();
        let on = cfg.mark_blocked;
        MoonCheckbox::new("arb-blocked")
            .label(t!("arb.mark_blocked").to_string())
            .checked(on)
            .size(MoonCheckboxSize::Compact)
            .on_change(move |v: &bool, _w, cx| {
                let v = *v;
                ArbEditState::write(&state, cx, |cfg| cfg.mark_blocked = v);
            })
    };

    // Which spreads are worth a LINE at all. A different question from the caption's colour
    // threshold — that one keeps the line and takes the paint off it — and the two are set in
    // different places for that reason.
    let min_row = {
        let state = state.clone();
        let current = MIN_ABS_STEPS
            .iter()
            .position(|v| (v - cfg.min_abs_pct).abs() < 0.001);
        seg_row(
            "arb-min".to_string(),
            t!("arb.min_abs").to_string(),
            MIN_ABS_STEPS
                .iter()
                .enumerate()
                .map(|(n, v)| {
                    let label = match *v == 0.0 {
                        true => t!("arb.min_abs_off").to_string(),
                        false => format!("{v}%"),
                    };
                    (label, Some(n) == current)
                })
                .collect(),
            48.0,
            p,
            cx,
            move |pick, cx| {
                if let Some(v) = MIN_ABS_STEPS.get(pick).copied() {
                    ArbEditState::write(&state, cx, |cfg| cfg.min_abs_pct = v);
                }
            },
        )
    };

    // The roster in COLUMNS. Filled column-first, so reading down a column follows the print order
    // the chart uses — a list that wrapped row-first would put the second venue beside the first
    // and read as a different order from the one it sets.
    let per_column = cfg.venues.len().div_ceil(COLUMNS).max(1);
    let mut list = h_flex().w_full().gap(design::ui_px(cx, 10.0));
    for column in 0..COLUMNS {
        let first = column * per_column;
        let last = ((column + 1) * per_column).min(cfg.venues.len());
        if first >= last {
            continue;
        }
        let mut lines = v_flex().flex_1().min_w_0().gap(design::ui_px(cx, 3.0));
        for ix in first..last {
            lines = lines.child(venue_line(state, &cfg.venues[ix], ix, &dex_names, p, cx));
        }
        list = list.child(lines);
    }

    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 8.0))
        .text_size(design::t_body(cx))
        .child(
            popup_group("arb-modes", t!("arb.printing").to_string())
                .child(show_row)
                .child(min_row)
                .child(blocked_cb),
        )
        .child(popup_group("arb-venues", t!("arb.venues").to_string()).child(list))
        .into_any_element()
}

/// One venue: order, visibility, name, colour.
fn venue_line(
    state: &Entity<ArbEditState>,
    venue: &ArbVenueCfg,
    ix: usize,
    dex_names: &[String],
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let micro_w = design::font_w(cx, MICRO_W);
    let up = {
        let state = state.clone();
        micro_button(
            format!("arb-up-{ix}"),
            "↑",
            t!("arb.move_up").to_string(),
            MoonButtonVariant::Ghost,
            false,
            micro_w,
            move |_w, cx| {
                ArbEditState::write(&state, cx, |cfg| {
                    if ix > 0 {
                        cfg.venues.swap(ix, ix - 1);
                    }
                });
            },
        )
    };
    let down = {
        let state = state.clone();
        micro_button(
            format!("arb-down-{ix}"),
            "↓",
            t!("arb.move_down").to_string(),
            MoonButtonVariant::Ghost,
            false,
            micro_w,
            move |_w, cx| {
                ArbEditState::write(&state, cx, |cfg| {
                    if ix + 1 < cfg.venues.len() {
                        cfg.venues.swap(ix, ix + 1);
                    }
                });
            },
        )
    };
    let eye = {
        let state = state.clone();
        let on = venue.visible;
        micro_button(
            format!("arb-eye-{ix}"),
            if on { "👁" } else { "–" },
            t!(if on { "arb.hide" } else { "arb.show_venue" }).to_string(),
            toggle_variant(on),
            on,
            micro_w,
            move |_w, cx| {
                ArbEditState::write(&state, cx, |cfg| {
                    if let Some(v) = cfg.venues.get_mut(ix) {
                        v.visible = !on;
                    }
                });
            },
        )
    };

    // Just the name. It is the PROTOCOL's — `ArbPlatformCode::name` for an exchange, the core's
    // own `known_dexes` entry for a deployer — and nothing here edits it: a renamed venue would
    // print a word no core ever sent, and the next person to read that chart would have no way to
    // tell which venue it is. A code the protocol cannot name shows its number, which is the honest
    // answer and is also what identifies it.
    let name = div()
        .w(px(design::font_w(cx, NAME_W)))
        .px(design::ui_px(cx, 4.0))
        .text_color(moon(match venue.color {
            Some(rgb) => gpui::rgb(rgb).into(),
            None => p.text,
        }))
        .child(venue.label_with(dex_names))
        .into_any_element();

    let mut swatches = h_flex().gap(design::ui_px(cx, 3.0));
    // The first swatch is "no colour of its own": a venue that follows the chart's caption colour,
    // which is what every venue starts as.
    for value in std::iter::once(None).chain(VENUE_COLORS.map(Some)) {
        let state = state.clone();
        let picked = venue.color == value;
        swatches = swatches.child(
            div()
                .id(SharedString::from(format!(
                    "arb-sw-{ix}-{}",
                    value.unwrap_or(0)
                )))
                .w(design::ui_px(cx, 14.0))
                .h(design::ui_px(cx, 14.0))
                .rounded(design::ui_px(cx, 3.0))
                .bg(match value {
                    Some(rgb) => gpui::rgb(rgb).into(),
                    None => moon(p.surface),
                })
                .border_1()
                .border_color(moon(if picked { p.text } else { p.border }))
                .cursor_pointer()
                .on_click(move |_, _w, cx: &mut App| {
                    ArbEditState::write(&state, cx, |cfg| {
                        if let Some(v) = cfg.venues.get_mut(ix) {
                            v.color = value;
                        }
                    });
                }),
        );
    }

    h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 3.0))
        .child(up)
        .child(down)
        .child(eye)
        .child(name)
        .child(swatches)
        .into_any_element()
}
