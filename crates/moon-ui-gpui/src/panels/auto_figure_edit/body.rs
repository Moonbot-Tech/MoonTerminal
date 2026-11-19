//! The editor's layout: the name, then two panes — what the row looks for on the left, how its
//! lines look on the right — and the preview under both.
//!
//! The panes WRAP: each has a floor width, and a dialog narrower than two of them (a small window,
//! a large Font setting) stacks the look pane under the search pane instead of squeezing both. The
//! whole body scrolls inside a height bound by the window, so a tall detector (`Squeeze` shows a
//! dozen numbers) never pushes OK off the screen.

use gpui::*;
use moon_chart::auto_figures::LineTag;
use moon_core::config::LabelColor;
use moon_core::config::auto_figures::{
    AutoEvent, AutoFigureRow, BrokenPolicy, CaptionPart, CaptionPlace, CaptionShow, DetectorKind,
    DetectorParams, EventMarks, Extend, ExtremePeriod, ExtremeWhich, ProfilePeriod, RoundGrid,
    RowDetector, StrengthBasis, TakeOrder, TakeSide, TfId, TfSource, ToleranceUnit, TrendSide,
};
use moon_core::figures::LineKind;
use moon_ui::{
    MoonAccent, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonDropdown, MoonInput,
    MoonInputState, MoonMenuItem, MoonPalette, MoonSegmentDensity, MoonSegmentItem,
    MoonSegmentedControl, h_flex, v_flex,
};
use rust_i18n::t;

use super::fields::{NumField, broken_of, broken_of_mut, unit_of, unit_of_mut};
use super::preview::{self, PreviewData};
use super::{AutoFigureEditState, kind_key, reseed_inputs, switch_detector};
use crate::chartdx::auto_figures_sync::caption::{event_key, tag_key, tf_key};
use crate::design::{self, moon};
use crate::panels::popup_group;

/// Width the dialog asks for when the window has room: two panes side by side.
pub(super) const DIALOG_W: f32 = 820.0;
/// What the dialog takes from its width before the content: MoonUI's 16 px padding on each side,
/// plus room for the body's scrollbar.
const DIALOG_CHROME_W: f32 = 40.0;
/// Floor of one pane; below two of these the panes stack.
const PANE_MIN_W: f32 = 280.0;
/// Width of a numeric input.
const NUM_W: f32 = 72.0;
/// Height of the preview plot.
const PREVIEW_H: f32 = 150.0;

/// Edit the working row and repaint, without touching the inputs — for edits that cannot change
/// which numbers the row has (a colour, a checkbox of the caption).
fn write_row(
    state: &Entity<AutoFigureEditState>,
    cx: &mut App,
    f: impl FnOnce(&mut AutoFigureRow),
) {
    state.update(cx, |s, cx| {
        f(&mut s.row);
        cx.notify();
    });
}

/// Edit the working row, then put every numeric input back to the row's values.
///
/// For the edits that change WHICH numbers the row has — the detector kind, the broken-line
/// policy, the zone switch. Safe to reseed all of them: these come from a click elsewhere, which
/// already took the focus out of any field being typed into.
fn write_row_reseed(
    state: &Entity<AutoFigureEditState>,
    window: &mut Window,
    cx: &mut App,
    f: impl FnOnce(&mut AutoFigureRow),
) {
    state.update(cx, |s, cx| {
        f(&mut s.row);
        cx.notify();
    });
    let (row, nums) = {
        let s = state.read(cx);
        (s.row.clone(), s.nums.clone())
    };
    reseed_inputs(&row, &nums, window, cx);
}

/// The label for a timeframe source: `Auto`, or the timeframe in the active language.
pub(crate) fn tf_source_label(tf: TfSource) -> String {
    match tf {
        TfSource::Auto => t!("auto_figures.tf.auto").to_string(),
        TfSource::Fixed(tf) => t!(tf_key(tf)).to_string(),
    }
}

/// Locale key of one dash pattern.
fn line_kind_key(kind: LineKind) -> &'static str {
    match kind {
        LineKind::Solid => "auto_figures.edit.line.solid",
        LineKind::Dash => "auto_figures.edit.line.dash",
        LineKind::Dot => "auto_figures.edit.line.dot",
        LineKind::DashDot => "auto_figures.edit.line.dash_dot",
        LineKind::DashDotDot => "auto_figures.edit.line.dash_dot_dot",
    }
}

/// Locale key of one caption part.
fn part_key(part: CaptionPart) -> &'static str {
    match part {
        CaptionPart::RowName => "auto_figures.edit.part.row_name",
        CaptionPart::Price => "auto_figures.edit.part.price",
        CaptionPart::DistancePct => "auto_figures.edit.part.distance",
        CaptionPart::Touches => "auto_figures.edit.part.touches",
        CaptionPart::Tf => "auto_figures.edit.part.tf",
        CaptionPart::Age => "auto_figures.edit.part.age",
        CaptionPart::Event => "auto_figures.edit.part.event",
    }
}

/// Locale key of one caption place.
fn place_key(place: CaptionPlace) -> &'static str {
    match place {
        CaptionPlace::LeftEnd => "auto_figures.edit.place.left",
        CaptionPlace::RightEnd => "auto_figures.edit.place.right",
        CaptionPlace::AtPriceAxis => "auto_figures.edit.place.axis",
        CaptionPlace::AboveLine => "auto_figures.edit.place.above",
        CaptionPlace::BelowLine => "auto_figures.edit.place.below",
    }
}

/// Locale key of one calendar period.
fn period_key(period: ExtremePeriod) -> &'static str {
    match period {
        ExtremePeriod::Day => "auto_figures.edit.period.day",
        ExtremePeriod::PrevDay => "auto_figures.edit.period.prev_day",
        ExtremePeriod::Week => "auto_figures.edit.period.week",
        ExtremePeriod::PrevWeek => "auto_figures.edit.period.prev_week",
    }
}

/// Apply `f` to the row's detector parameters, when this build knows them.
fn edit_params(r: &mut AutoFigureRow, f: impl FnOnce(&mut DetectorParams)) {
    if let RowDetector::Known(d) = &mut r.detector {
        f(d);
    }
}

/// Pull a pinned timeframe under the coarsest one the row's detector can answer on — after an edit
/// that may have lowered it (a day's open needs intraday bars), now rather than at OK, so the
/// timeframe dropdown never shows a choice the row refuses.
fn cap_tf(r: &mut AutoFigureRow) {
    if let (Some(d), TfSource::Fixed(tf)) = (r.detector.known(), &mut r.tf) {
        *tf = (*tf).min(d.coarsest_tf());
    }
}

/// Where a row's switch for `event` is stored.
fn mark_mut(marks: &mut EventMarks, event: AutoEvent) -> &mut bool {
    match event {
        AutoEvent::Approach => &mut marks.approach,
        AutoEvent::Squeeze => &mut marks.squeeze,
        AutoEvent::Breakout => &mut marks.breakout,
        AutoEvent::FalseBreak => &mut marks.false_break,
        AutoEvent::Bounce => &mut marks.bounce,
        AutoEvent::RoleFlip => &mut marks.role_flip,
        AutoEvent::StopHunt => &mut marks.stop_hunt,
        AutoEvent::Confluence => &mut marks.confluence,
    }
}

/// Locale key of a volume profile's day.
fn profile_period_key(period: ProfilePeriod) -> &'static str {
    match period {
        ProfilePeriod::Day => "auto_figures.edit.period.day",
        ProfilePeriod::PrevDay => "auto_figures.edit.period.prev_day",
    }
}

/// One labelled checkbox whose change is handed the window.
fn checkbox(
    id: String,
    label: String,
    checked: bool,
    on_change: impl Fn(bool, &mut Window, &mut App) + 'static,
) -> AnyElement {
    MoonCheckbox::new(SharedString::from(id))
        .label(label)
        .checked(checked)
        .on_change(move |v: &bool, window, cx| on_change(*v, window, cx))
        .into_any_element()
}

/// A caption over a wrapping row of checkboxes — the caption parts' layout.
fn check_group(caption: String, boxes: Vec<AnyElement>, p: MoonPalette, cx: &App) -> AnyElement {
    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 2.0))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text))
                .child(caption),
        )
        .child(
            h_flex()
                .w_full()
                .flex_wrap()
                .gap(design::ui_px(cx, 8.0))
                .children(boxes),
        )
        .into_any_element()
}

/// A caption over a row of segments, the pick handed back with the window — the captions
/// editor's `seg_row`, with the window kept because some picks here reseed the inputs.
fn seg_row(
    id: &str,
    caption: String,
    labels: Vec<(String, bool)>,
    p: MoonPalette,
    cx: &App,
    on_pick: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let on_pick = std::rc::Rc::new(on_pick);
    let cells = labels
        .into_iter()
        .enumerate()
        .map(|(ix, (label, selected))| {
            let item = MoonSegmentItem::new("", label)
                .density(MoonSegmentDensity::Compact)
                .selected(selected)
                .fit_width(cx, 48.0, 560.0);
            let width = item.resolved_width();
            let on_pick = on_pick.clone();
            div().flex_none().w(px(width)).child(
                MoonSegmentedControl::new(SharedString::from(format!("{id}-{ix}")))
                    .accent(MoonAccent::Blue)
                    .density(MoonSegmentDensity::Compact)
                    .item(item)
                    .on_click(move |_, _, window, cx| on_pick(ix, window, cx)),
            )
        });
    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 2.0))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text))
                .child(caption),
        )
        .child(h_flex().w_full().flex_wrap().children(cells))
}

/// A caption beside a dropdown of choices.
fn dropdown_row(
    id: &'static str,
    label: String,
    current: String,
    items: Vec<MoonMenuItem>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap(design::ui_px(cx, 6.0))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text))
                .child(label),
        )
        .child(
            div().flex_none().child(
                MoonDropdown::new(id)
                    .label(current)
                    .trigger_caret(true)
                    .trigger_variant(MoonButtonVariant::Soft)
                    .trigger_size(MoonButtonSize::density(cx))
                    .fit_trigger_width(84.0, 200.0)
                    .fit_menu_width(104.0, 560.0)
                    .items(items),
            ),
        )
}

/// Mutually exclusive menu items whose pick is handed the window.
fn pick_items<T: Copy + PartialEq + 'static>(
    id: &str,
    options: impl IntoIterator<Item = (T, String)>,
    current: T,
    on_pick: impl Fn(T, &mut Window, &mut App) + Clone + 'static,
) -> Vec<MoonMenuItem> {
    options
        .into_iter()
        .enumerate()
        .map(|(ix, (value, label))| {
            let on_pick = on_pick.clone();
            MoonMenuItem::with_key(
                SharedString::from(format!("{id}-{ix}")),
                SharedString::from(label),
            )
            .checked(value == current)
            .on_click(move |_, window, app| on_pick(value, window, app))
        })
        .collect()
}

/// One numeric field: its label (with the unit) and its input.
fn num_row(
    field: NumField,
    input: &Entity<MoonInputState>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text))
                .child(t!(field.locale_key()).to_string()),
        )
        .child(
            div().flex_none().w(design::font_w_px(cx, NUM_W)).child(
                MoonInput::new(SharedString::from(field.element_id()))
                    .state(input)
                    .size(design::INPUT_SIZE),
            ),
        )
}

/// Render the dialog's body: name, both panes and the preview, scrolling inside `max_h`.
pub(super) fn dialog_body(
    state: &Entity<AutoFigureEditState>,
    width: f32,
    max_h: f32,
    cx: &mut App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let data = state.update(cx, |s, _| s.preview());
    let (row, name_input, nums, theme) = {
        let s = state.read(cx);
        (
            s.row.clone(),
            s.name_input.clone(),
            s.nums.clone(),
            s.theme.clone(),
        )
    };
    let input_of = |field: NumField| {
        nums.iter()
            .find(|(f, _)| *f == field)
            .map(|(_, i)| i.clone())
    };
    // A preset row shows ITS OWN name as the placeholder, a plain row its detector's: the field is
    // empty because the name comes from the dictionary, and typing here overrides it.
    let name_hint = row
        .title_key()
        .map(|key| t!(key).to_string())
        .or_else(|| {
            row.detector
                .known()
                .map(|d| t!(kind_key(d.kind())).to_string())
        })
        .unwrap_or_default();
    let name_row = h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(t!("auto_figures.edit.name").to_string()),
        )
        .child(
            div().flex_1().child(
                MoonInput::new("afe-name")
                    .state(&name_input)
                    .placeholder(name_hint)
                    .size(design::INPUT_SIZE),
            ),
        );
    // Every width here is DEFINITE, computed from the dialog's own: inside a vertically scrolling
    // box a percentage or flex width has nothing to resolve against, and the panes' rows then
    // laid out at their max-content width — controls painted over the neighbouring pane and the
    // name field collapsed. Two columns when two pane floors fit, one under the other otherwise.
    let content_w = (width - DIALOG_CHROME_W).max(1.0);
    let gap = f32::from(design::ui_px(cx, 8.0));
    let two_cols = content_w >= 2.0 * design::font_w(cx, PANE_MIN_W) + gap;
    let pane_w = if two_cols {
        (content_w - gap) / 2.0
    } else {
        content_w
    };
    let mut panes = match two_cols {
        true => h_flex().items_start(),
        false => v_flex(),
    };
    panes = panes
        .w(px(content_w))
        .gap(px(gap))
        .child(
            div()
                .flex_none()
                .w(px(pane_w))
                .child(search_pane(state, &row, &input_of, p, cx)),
        )
        .child(
            div()
                .flex_none()
                .w(px(pane_w))
                .child(look_pane(state, &row, &input_of, &theme, p, cx)),
        );
    div()
        .id("afe-body")
        .w(px(content_w))
        .max_h(px(max_h))
        .overflow_y_scroll()
        .child(
            v_flex()
                .w(px(content_w))
                .gap(design::ui_px(cx, 8.0))
                .child(name_row)
                .child(panes)
                .child(preview_group(data, &theme, p, cx)),
        )
        .into_any_element()
}

/// What the row looks for: the detector and its numbers, the timeframe, which lines it keeps and
/// what happens to a broken one.
fn search_pane(
    state: &Entity<AutoFigureEditState>,
    row: &AutoFigureRow,
    input_of: &dyn Fn(NumField) -> Option<Entity<MoonInputState>>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let Some(params) = row.detector.known().copied() else {
        return div().into_any_element();
    };
    let mut col = v_flex().w_full().gap(design::ui_px(cx, 6.0));
    let current_kind = params.kind();
    col = col.child({
        let state = state.clone();
        dropdown_row(
            "afe-kind",
            t!("auto_figures.edit.detector").to_string(),
            t!(kind_key(current_kind)).to_string(),
            pick_items(
                "afe-kind-item",
                DetectorKind::ALL
                    .into_iter()
                    .map(|k| (k, t!(kind_key(k)).to_string())),
                current_kind,
                move |k, window, cx| {
                    write_row_reseed(&state, window, cx, |r| switch_detector(r, k))
                },
            ),
            p,
            cx,
        )
    });
    match params {
        DetectorParams::PeriodExtreme(pe) => {
            let state_p = state.clone();
            col = col.child(dropdown_row(
                "afe-period",
                t!("auto_figures.edit.period").to_string(),
                t!(period_key(pe.period)).to_string(),
                pick_items(
                    "afe-period-item",
                    [
                        ExtremePeriod::Day,
                        ExtremePeriod::PrevDay,
                        ExtremePeriod::Week,
                        ExtremePeriod::PrevWeek,
                    ]
                    .into_iter()
                    .map(|v| (v, t!(period_key(v)).to_string())),
                    pe.period,
                    move |v, window, cx| {
                        write_row_reseed(&state_p, window, cx, |r| {
                            if let moon_core::config::auto_figures::RowDetector::Known(
                                DetectorParams::PeriodExtreme(x),
                            ) = &mut r.detector
                            {
                                x.period = v;
                            }
                            // A day row cannot answer on daily bars; pull a pinned timeframe down
                            // now rather than at OK.
                            if let (Some(d), TfSource::Fixed(tf)) = (r.detector.known(), &mut r.tf)
                            {
                                *tf = (*tf).min(d.coarsest_tf());
                            }
                        })
                    },
                ),
                p,
                cx,
            ));
            let state_w = state.clone();
            let which = [ExtremeWhich::High, ExtremeWhich::Low, ExtremeWhich::Both];
            col = col.child(seg_row(
                "afe-which",
                t!("auto_figures.edit.which").to_string(),
                which
                    .iter()
                    .map(|w| {
                        let key = match w {
                            ExtremeWhich::High => "auto_figures.edit.which.high",
                            ExtremeWhich::Low => "auto_figures.edit.which.low",
                            ExtremeWhich::Both => "auto_figures.edit.which.both",
                        };
                        (t!(key).to_string(), *w == pe.which)
                    })
                    .collect(),
                p,
                cx,
                move |ix, _window, cx| {
                    if let Some(v) = which.get(ix).copied() {
                        write_row(&state_w, cx, |r| {
                            if let moon_core::config::auto_figures::RowDetector::Known(
                                DetectorParams::PeriodExtreme(x),
                            ) = &mut r.detector
                            {
                                x.which = v;
                            }
                        });
                    }
                },
            ));
        }
        DetectorParams::Trend(tp) => {
            let state_s = state.clone();
            let sides = [TrendSide::Support, TrendSide::Resistance, TrendSide::Both];
            col = col.child(seg_row(
                "afe-trend-side",
                t!("auto_figures.edit.trend_side").to_string(),
                sides
                    .iter()
                    .map(|s| {
                        let key = match s {
                            TrendSide::Support => "auto_figures.edit.trend_side.support",
                            TrendSide::Resistance => "auto_figures.edit.trend_side.resistance",
                            TrendSide::Both => "auto_figures.edit.trend_side.both",
                        };
                        (t!(key).to_string(), *s == tp.side)
                    })
                    .collect(),
                p,
                cx,
                move |ix, _window, cx| {
                    if let Some(v) = sides.get(ix).copied() {
                        write_row(&state_s, cx, |r| {
                            if let moon_core::config::auto_figures::RowDetector::Known(
                                DetectorParams::Trend(x),
                            ) = &mut r.detector
                            {
                                x.side = v;
                            }
                        });
                    }
                },
            ));
        }
        DetectorParams::VolumeProfile(vp) => {
            let state_p = state.clone();
            col = col.child(dropdown_row(
                "afe-profile-period",
                t!("auto_figures.edit.period").to_string(),
                t!(profile_period_key(vp.period)).to_string(),
                pick_items(
                    "afe-profile-period-item",
                    [ProfilePeriod::Day, ProfilePeriod::PrevDay]
                        .into_iter()
                        .map(|v| (v, t!(profile_period_key(v)).to_string())),
                    vp.period,
                    move |v, _window, cx| {
                        write_row(&state_p, cx, |r| {
                            edit_params(r, |d| {
                                if let DetectorParams::VolumeProfile(x) = d {
                                    x.period = v;
                                }
                            })
                        })
                    },
                ),
                p,
                cx,
            ));
            // Which of the profiled day's lines are drawn, named by the same word their caption
            // prints. Naked POCs are a count of days below, independent of these.
            let boxes = [
                (LineTag::Poc, vp.poc),
                (LineTag::Vah, vp.vah),
                (LineTag::Val, vp.val),
            ]
            .into_iter()
            .map(|(tag, on)| {
                let state = state.clone();
                checkbox(
                    format!("afe-vp-{tag:?}").to_lowercase(),
                    t!(tag_key(tag)).to_string(),
                    on,
                    move |v, _window, cx| {
                        write_row(&state, cx, |r| {
                            edit_params(r, |d| {
                                if let DetectorParams::VolumeProfile(x) = d {
                                    match tag {
                                        LineTag::Poc => x.poc = v,
                                        LineTag::Vah => x.vah = v,
                                        _ => x.val = v,
                                    }
                                }
                            })
                        })
                    },
                )
            })
            .collect();
            col = col.child(check_group(
                t!("auto_figures.edit.profile_lines").to_string(),
                boxes,
                p,
                cx,
            ));
        }
        DetectorParams::PeriodLevel(pl) => {
            let boxes = [
                (LineTag::PrevDayClose, pl.prev_day_close),
                (LineTag::DayOpen, pl.day_open),
                (LineTag::WeekOpen, pl.week_open),
                (LineTag::PrevWeekClose, pl.prev_week_close),
            ]
            .into_iter()
            .map(|(tag, on)| {
                let state = state.clone();
                checkbox(
                    format!("afe-pl-{tag:?}").to_lowercase(),
                    t!(tag_key(tag)).to_string(),
                    on,
                    move |v, _window, cx| {
                        write_row(&state, cx, |r| {
                            edit_params(r, |d| {
                                if let DetectorParams::PeriodLevel(x) = d {
                                    match tag {
                                        LineTag::PrevDayClose => x.prev_day_close = v,
                                        LineTag::DayOpen => x.day_open = v,
                                        LineTag::WeekOpen => x.week_open = v,
                                        _ => x.prev_week_close = v,
                                    }
                                }
                            });
                            // The day's open cannot be read on daily bars.
                            cap_tf(r);
                        })
                    },
                )
            })
            .collect();
            col = col.child(check_group(
                t!("auto_figures.edit.period_lines").to_string(),
                boxes,
                p,
                cx,
            ));
        }
        DetectorParams::RoundLevels(rl) => {
            let state_g = state.clone();
            let grids = [RoundGrid::Major, RoundGrid::Half, RoundGrid::Minor];
            col = col.child(seg_row(
                "afe-round-grid",
                t!("auto_figures.edit.grid").to_string(),
                grids
                    .iter()
                    .map(|g| {
                        let key = match g {
                            RoundGrid::Major => "auto_figures.edit.grid.major",
                            RoundGrid::Half => "auto_figures.edit.grid.half",
                            RoundGrid::Minor => "auto_figures.edit.grid.minor",
                        };
                        (t!(key).to_string(), *g == rl.grid)
                    })
                    .collect(),
                p,
                cx,
                move |ix, _window, cx| {
                    if let Some(v) = grids.get(ix).copied() {
                        write_row(&state_g, cx, |r| {
                            edit_params(r, |d| {
                                if let DetectorParams::RoundLevels(x) = d {
                                    x.grid = v;
                                }
                            })
                        });
                    }
                },
            ));
        }
        DetectorParams::Swing(_) | DetectorParams::Squeeze(_) => {}
    }
    // The unit of a pivot detector's touch tolerance — one for both halves of a squeeze. It
    // decides WHICH tolerance fields exist below, so a pick reseeds the inputs.
    if let Some(unit) = unit_of(&params) {
        let state_u = state.clone();
        let units = [ToleranceUnit::Pct, ToleranceUnit::Atr];
        col = col.child(seg_row(
            "afe-tolerance-unit",
            t!("auto_figures.edit.tolerance_unit").to_string(),
            units
                .iter()
                .map(|u| {
                    let key = match u {
                        ToleranceUnit::Pct => "auto_figures.edit.tolerance_unit.pct",
                        ToleranceUnit::Atr => "auto_figures.edit.tolerance_unit.atr",
                    };
                    (t!(key).to_string(), *u == unit)
                })
                .collect(),
            p,
            cx,
            move |ix, window, cx| {
                if let Some(v) = units.get(ix).copied() {
                    write_row_reseed(&state_u, window, cx, |r| {
                        edit_params(r, |d| {
                            if let Some(u) = unit_of_mut(d) {
                                *u = v;
                            }
                        })
                    });
                }
            },
        ));
    }
    for field in NumField::DETECTOR {
        if field.has(row)
            && let Some(input) = input_of(field)
        {
            col = col.child(num_row(field, &input, p, cx));
        }
    }

    // Which bars: Auto, or a pinned timeframe the detector can answer on.
    let coarsest = params.coarsest_tf();
    let mut tf_options = vec![(TfSource::Auto, tf_source_label(TfSource::Auto))];
    tf_options.extend(
        TfId::ALL
            .into_iter()
            .filter(|tf| *tf <= coarsest)
            .map(|tf| (TfSource::Fixed(tf), tf_source_label(TfSource::Fixed(tf)))),
    );
    col = col.child({
        let state = state.clone();
        dropdown_row(
            "afe-tf",
            t!("auto_figures.edit.tf").to_string(),
            tf_source_label(row.tf),
            pick_items("afe-tf-item", tf_options, row.tf, move |tf, _w, cx| {
                write_row(&state, cx, |r| r.tf = tf)
            }),
            p,
            cx,
        )
    });

    // Which of the found lines the row keeps.
    let mut take = v_flex().w_full().gap(design::ui_px(cx, 6.0));
    if let Some(input) = input_of(NumField::TakeCount) {
        take = take.child(num_row(NumField::TakeCount, &input, p, cx));
    }
    take = take.child({
        let state = state.clone();
        let sides = [TakeSide::Above, TakeSide::Below, TakeSide::Both];
        seg_row(
            "afe-take-side",
            t!("auto_figures.edit.take_side").to_string(),
            sides
                .iter()
                .map(|s| {
                    let key = match s {
                        TakeSide::Above => "auto_figures.edit.take_side.above",
                        TakeSide::Below => "auto_figures.edit.take_side.below",
                        TakeSide::Both => "auto_figures.edit.take_side.both",
                    };
                    (t!(key).to_string(), *s == row.take.side)
                })
                .collect(),
            p,
            cx,
            move |ix, _w, cx| {
                if let Some(v) = sides.get(ix).copied() {
                    write_row(&state, cx, |r| r.take.side = v);
                }
            },
        )
    });
    take = take.child({
        let state = state.clone();
        let orders = [TakeOrder::Nearest, TakeOrder::Strongest];
        seg_row(
            "afe-take-order",
            t!("auto_figures.edit.take_order").to_string(),
            orders
                .iter()
                .map(|o| {
                    let key = match o {
                        TakeOrder::Nearest => "auto_figures.edit.take_order.nearest",
                        TakeOrder::Strongest => "auto_figures.edit.take_order.strongest",
                    };
                    (t!(key).to_string(), *o == row.take.order)
                })
                .collect(),
            p,
            cx,
            move |ix, _w, cx| {
                if let Some(v) = orders.get(ix).copied() {
                    write_row(&state, cx, |r| r.take.order = v);
                }
            },
        )
    });
    // What "strongest" ranks by — read only under that order, so offered only there.
    if row.take.order == TakeOrder::Strongest {
        let state = state.clone();
        let bases = [StrengthBasis::Touches, StrengthBasis::Volume];
        take = take.child(seg_row(
            "afe-take-strength",
            t!("auto_figures.edit.take_strength").to_string(),
            bases
                .iter()
                .map(|b| {
                    let key = match b {
                        StrengthBasis::Touches => "auto_figures.edit.take_strength.touches",
                        StrengthBasis::Volume => "auto_figures.edit.take_strength.volume",
                    };
                    (t!(key).to_string(), *b == row.take.strength)
                })
                .collect(),
            p,
            cx,
            move |ix, _w, cx| {
                if let Some(v) = bases.get(ix).copied() {
                    write_row(&state, cx, |r| r.take.strength = v);
                }
            },
        ));
    }

    let mut pane = v_flex()
        .w_full()
        .gap(design::ui_px(cx, 6.0))
        .child(popup_group("afe-search", t!("auto_figures.edit.frame_search")).child(col))
        .child(popup_group("afe-take", t!("auto_figures.edit.frame_take")).child(take));

    // What happens to a line once a bar closed beyond it — offered only by the kinds that have a
    // policy; a period's extreme is a fact of the calendar and is never dropped.
    if let Some(policy) = broken_of(&params) {
        let state = state.clone();
        let keep = matches!(policy, BrokenPolicy::Keep { .. });
        let mut broken = v_flex().w_full().gap(design::ui_px(cx, 6.0)).child(seg_row(
            "afe-broken",
            t!("auto_figures.edit.broken").to_string(),
            vec![
                (t!("auto_figures.edit.broken.remove").to_string(), !keep),
                (t!("auto_figures.edit.broken.keep").to_string(), keep),
            ],
            p,
            cx,
            move |ix, window, cx| {
                write_row_reseed(&state, window, cx, |r| {
                    if let moon_core::config::auto_figures::RowDetector::Known(d) = &mut r.detector
                        && let Some(b) = broken_of_mut(d)
                    {
                        *b = match ix {
                            0 => BrokenPolicy::Remove,
                            // Back to the shipped policy's bar count, not zero: "keep" with
                            // nothing kept would read as a remove.
                            _ => match *b {
                                BrokenPolicy::Keep { bars } => BrokenPolicy::Keep { bars },
                                BrokenPolicy::Remove => BrokenPolicy::default(),
                            },
                        };
                    }
                })
            },
        ));
        if keep && let Some(input) = input_of(NumField::BrokenBars) {
            broken = broken.child(num_row(NumField::BrokenBars, &input, p, cx));
        }
        pane = pane.child(
            popup_group("afe-broken-group", t!("auto_figures.edit.frame_broken")).child(broken),
        );
    }
    pane.into_any_element()
}

/// How the row's lines look, the caption printed on them and the event marks.
fn look_pane(
    state: &Entity<AutoFigureEditState>,
    row: &AutoFigureRow,
    input_of: &dyn Fn(NumField) -> Option<Entity<MoonInputState>>,
    theme: &moon_core::config::ChartTheme,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let style = row.style;
    let mut line = v_flex().w_full().gap(design::ui_px(cx, 6.0));
    // Colour MODE: the chart's caption colour, by side of price, or the reader's own.
    line = line.child({
        let state = state.clone();
        let current = match style.color {
            LabelColor::Theme => 0,
            LabelColor::BySign => 1,
            LabelColor::Fixed(_) => 2,
        };
        let fallback = design::rgb_to_u32(theme.caption_label);
        seg_row(
            "afe-color",
            t!("auto_figures.edit.color").to_string(),
            vec![
                (
                    t!("auto_figures.edit.color.theme").to_string(),
                    current == 0,
                ),
                (t!("auto_figures.edit.color.sign").to_string(), current == 1),
                (
                    t!("auto_figures.edit.color.fixed").to_string(),
                    current == 2,
                ),
            ],
            p,
            cx,
            move |pick, _w, cx| {
                write_row(&state, cx, |r| {
                    r.style.color = match pick {
                        0 => LabelColor::Theme,
                        1 => LabelColor::BySign,
                        // Keep a fixed colour the row already had; start a new one from the
                        // theme's caption colour, which is what the line was just drawn in.
                        _ => match r.style.color {
                            LabelColor::Fixed(rgb) => LabelColor::Fixed(rgb),
                            _ => LabelColor::Fixed(fallback),
                        },
                    };
                })
            },
        )
    });
    if let LabelColor::Fixed(picked) = style.color {
        let state = state.clone();
        line = line.child(crate::controls::color_picker::ColorPicker::new(
            format!("afe-color-pick-{}", state.entity_id()),
            design::hsla_to_rgb8(rgb(picked).into()),
            move |color, cx| {
                write_row(&state, cx, |r| {
                    r.style.color = LabelColor::Fixed(design::rgb_to_u32(color));
                });
            },
        ));
    }
    line = line.child({
        let state = state.clone();
        dropdown_row(
            "afe-line-kind",
            t!("auto_figures.edit.line_kind").to_string(),
            t!(line_kind_key(style.line_kind)).to_string(),
            pick_items(
                "afe-line-kind-item",
                LineKind::ALL
                    .into_iter()
                    .map(|k| (k, t!(line_kind_key(k)).to_string())),
                style.line_kind,
                move |k, _w, cx| write_row(&state, cx, |r| r.style.line_kind = k),
            ),
            p,
            cx,
        )
    });
    if let Some(input) = input_of(NumField::Thickness) {
        line = line.child(num_row(NumField::Thickness, &input, p, cx));
    }
    line = line.child({
        let state = state.clone();
        seg_row(
            "afe-extend",
            t!("auto_figures.edit.extend").to_string(),
            Extend::ALL
                .iter()
                .map(|e| {
                    let key = match e {
                        Extend::RightEdge => "auto_figures.edit.extend.right",
                        Extend::BothEdges => "auto_figures.edit.extend.both",
                        Extend::BetweenTouches => "auto_figures.edit.extend.between",
                    };
                    (t!(key).to_string(), *e == style.extend)
                })
                .collect(),
            p,
            cx,
            move |ix, _w, cx| {
                if let Some(e) = Extend::ALL.get(ix).copied() {
                    write_row(&state, cx, |r| r.style.extend = e);
                }
            },
        )
    });
    // The tolerance band: on/off, and its opacity while on.
    line = line.child({
        let state = state.clone();
        MoonCheckbox::new("afe-zone")
            .label(t!("auto_figures.edit.zone").to_string())
            .checked(style.zone.is_some())
            .on_change(move |on: &bool, window, cx| {
                let on = *on;
                write_row_reseed(&state, window, cx, |r| {
                    r.style.zone = on.then_some(ZONE_DEFAULT_ALPHA);
                });
            })
    });
    if style.zone.is_some()
        && let Some(input) = input_of(NumField::ZoneOpacity)
    {
        line = line.child(num_row(NumField::ZoneOpacity, &input, p, cx));
    }
    // The pattern of a broken line still kept — shown only while the row keeps one.
    let keeps_broken = row
        .detector
        .known()
        .and_then(broken_of)
        .is_some_and(|b| matches!(b, BrokenPolicy::Keep { .. }));
    if keeps_broken {
        let state = state.clone();
        line = line.child(dropdown_row(
            "afe-broken-kind",
            t!("auto_figures.edit.broken_kind").to_string(),
            t!(line_kind_key(style.broken_kind)).to_string(),
            pick_items(
                "afe-broken-kind-item",
                LineKind::ALL
                    .into_iter()
                    .map(|k| (k, t!(line_kind_key(k)).to_string())),
                style.broken_kind,
                move |k, _w, cx| write_row(&state, cx, |r| r.style.broken_kind = k),
            ),
            p,
            cx,
        ));
    }

    // The caption printed on each line: which parts (in print order), where, and when.
    let mut caption = v_flex().w_full().gap(design::ui_px(cx, 6.0));
    let mut parts = h_flex().w_full().flex_wrap().gap(design::ui_px(cx, 8.0));
    for part in CaptionPart::ALL {
        let state = state.clone();
        let on = row.caption.parts.contains(&part);
        parts = parts.child(
            MoonCheckbox::new(SharedString::from(format!("afe-part-{part:?}")))
                .label(t!(part_key(part)).to_string())
                .checked(on)
                .on_change(move |v: &bool, _w, cx| {
                    let v = *v;
                    write_row(&state, cx, |r| {
                        r.caption.parts.retain(|p| *p != part);
                        // A part switched on goes to the END: the order is the print order, and
                        // the newest choice reads last.
                        if v {
                            r.caption.parts.push(part);
                        }
                    });
                }),
        );
    }
    caption = caption.child(
        v_flex()
            .w_full()
            .gap(design::ui_px(cx, 2.0))
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text))
                    .child(t!("auto_figures.edit.parts").to_string()),
            )
            .child(parts),
    );
    caption = caption.child({
        let state = state.clone();
        dropdown_row(
            "afe-place",
            t!("auto_figures.edit.place").to_string(),
            t!(place_key(row.caption.place)).to_string(),
            pick_items(
                "afe-place-item",
                CaptionPlace::ALL
                    .into_iter()
                    .map(|v| (v, t!(place_key(v)).to_string())),
                row.caption.place,
                move |v, _w, cx| write_row(&state, cx, |r| r.caption.place = v),
            ),
            p,
            cx,
        )
    });
    caption = caption.child({
        let state = state.clone();
        let shows = [CaptionShow::Always, CaptionShow::OnHover];
        seg_row(
            "afe-show",
            t!("auto_figures.edit.show").to_string(),
            vec![
                (
                    t!("auto_figures.edit.show.always").to_string(),
                    row.caption.show == CaptionShow::Always,
                ),
                (
                    t!("auto_figures.edit.show.hover").to_string(),
                    row.caption.show == CaptionShow::OnHover,
                ),
            ],
            p,
            cx,
            move |ix, _w, cx| {
                if let Some(v) = shows.get(ix).copied() {
                    write_row(&state, cx, |r| r.caption.show = v);
                }
            },
        )
    });

    // Which events get a mark on the line. The squeeze mark is offered only by a squeeze row:
    // nothing else can be "pressed".
    let is_squeeze = matches!(row.detector.known(), Some(DetectorParams::Squeeze(_)));
    let mut events = h_flex().w_full().flex_wrap().gap(design::ui_px(cx, 8.0));
    // Named through the same `event_key` the line captions print, so a mark and its checkbox
    // cannot disagree on the word.
    // The element ids are the atlas's names for these controls (`afe-ev-approach`, …): kept as
    // they were, snake case, so a reshoot finds the old checkboxes where they stood.
    let marks: [(&str, AutoEvent, bool); 8] = [
        ("approach", AutoEvent::Approach, true),
        ("squeeze", AutoEvent::Squeeze, is_squeeze),
        ("breakout", AutoEvent::Breakout, true),
        ("false_break", AutoEvent::FalseBreak, true),
        ("bounce", AutoEvent::Bounce, true),
        ("role_flip", AutoEvent::RoleFlip, true),
        ("stop_hunt", AutoEvent::StopHunt, true),
        ("confluence", AutoEvent::Confluence, true),
    ];
    for (id, event, shown) in marks {
        if !shown {
            continue;
        }
        let state = state.clone();
        events = events.child(checkbox(
            format!("afe-ev-{id}"),
            t!(event_key(event)).to_string(),
            row.events.marks(event),
            move |v, _w, cx| write_row(&state, cx, |r| *mark_mut(&mut r.events, event) = v),
        ));
    }
    let mut events_col = v_flex().w_full().gap(design::ui_px(cx, 6.0)).child(events);
    for field in [
        NumField::ApproachPct,
        NumField::StopHuntPct,
        NumField::ConfluencePct,
    ] {
        if let Some(input) = input_of(field) {
            events_col = events_col.child(num_row(field, &input, p, cx));
        }
    }

    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 6.0))
        .child(popup_group("afe-line", t!("auto_figures.edit.frame_line")).child(line))
        .child(popup_group("afe-caption", t!("auto_figures.edit.frame_caption")).child(caption))
        .child(popup_group("afe-events", t!("auto_figures.edit.frame_events")).child(events_col))
        .into_any_element()
}

/// Opacity a zone starts at when it is switched on.
const ZONE_DEFAULT_ALPHA: f32 = 0.12;

/// The demo plot and one line saying what the detector found on it.
fn preview_group(
    data: std::rc::Rc<Option<PreviewData>>,
    theme: &moon_core::config::ChartTheme,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let summary = match data.as_ref() {
        Some(d) if d.kept > 0 => t!(
            "auto_figures.edit.preview_found",
            found = d.found,
            kept = d.kept,
            tf = t!(tf_key(d.tf))
        )
        .to_string(),
        Some(_) => t!("auto_figures.edit.preview_none").to_string(),
        None => String::new(),
    };
    let theme = theme.clone();
    let plot = canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            if let Some(d) = data.as_ref() {
                preview::paint(d, &theme, b, window);
            }
        },
    )
    .w_full()
    .h(design::ui_px(cx, PREVIEW_H));
    popup_group("afe-preview", t!("auto_figures.edit.preview"))
        .child(
            v_flex()
                .w_full()
                .gap(design::ui_px(cx, 4.0))
                .child(
                    div()
                        .w_full()
                        .rounded(design::ui_px(cx, 4.0))
                        .overflow_hidden()
                        .child(plot),
                )
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_muted))
                        .child(summary),
                ),
        )
        .into_any_element()
}
