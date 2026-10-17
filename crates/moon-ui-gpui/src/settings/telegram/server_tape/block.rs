//! The production tape page composition, shared by Settings and fixture-only GPUI tests.

use super::keep::{KeepState, db_tile, keep_note, keep_state, period, purge_counts};
use super::{
    TapeEd, catchup_breakdown, catchup_sentence, catchup_state, catchup_tiles, plural_key,
    tape_breakdown, tape_status_strip, tape_tiles,
};
use crate::design;
use gpui::{prelude::FluentBuilder as _, *};
use moon_core::config::storage::LONG_POSITION_MIN_RANGE;
use moon_core::station_api::{DEFAULT_MAX_AGE_DAYS, TapeCatchup, TapeKeep};
use moon_ui::{
    MoonAccent, MoonButton, MoonButtonVariant, MoonCheckbox, MoonPalette, MoonProgress,
    MoonSegmentItem, MoonSegmentSelection, MoonSegmentedControl, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;
use std::rc::Rc;

/// Changes enter Settings through these same callbacks in the production page and tests.
pub(super) struct Actions {
    pub days: Rc<dyn Fn(u16, &mut Window, &mut App)>,
    pub emulator: Rc<dyn Fn(bool, &mut Window, &mut App)>,
    pub tuner: Rc<dyn Fn(bool, &mut Window, &mut App)>,
    pub confirm: Rc<dyn Fn(&mut Window, &mut App)>,
    pub cancel: Rc<dyn Fn(&mut Window, &mut App)>,
}

/// Existing window, switches and reach controls keep their production implementations.
pub(super) struct Controls {
    pub margin: AnyElement,
    pub long: AnyElement,
    pub switches: Option<AnyElement>,
    pub venues: AnyElement,
}

/// Station facts and geometry are immutable during one render.
pub(super) struct Snapshot<'a> {
    pub catchup: Option<&'a TapeCatchup>,
    pub busy: bool,
    pub width: f32,
    pub read_at: Option<String>,
}

/// Keep the minutes field at the shared Settings compact width, even in a growing row.
pub(super) fn long_control(
    input: &Entity<moon_ui::MoonInputState>,
    enabled: bool,
    cx: &App,
) -> AnyElement {
    let range =
        f64::from(*LONG_POSITION_MIN_RANGE.start())..=f64::from(*LONG_POSITION_MIN_RANGE.end());
    let unit = t!("storage.unit_min").to_string();
    let width = crate::settings::common::num_field_min_w(&range, Some(&unit), cx);
    div()
        .w(px(width))
        .max_w_full()
        .flex_none()
        .debug_selector(|| "server-tape-long-compact".into())
        .child(crate::settings::general::num_input_on(
            "server-tape-long",
            input,
            range,
            1.0,
            Some(unit),
            enabled,
        ))
        .into_any_element()
}

/// Wrapping prose owns its width directly, preventing nested flex truncation.
fn prose(text: String, color: u32, cx: &App) -> Div {
    crate::settings::common::cap_prose(div().w_full(), cx)
        .min_w_0()
        .font_family(design::ui_font())
        .text_size(design::t_caption(cx))
        .text_color(rgba_from(color, 1.0))
        .child(text)
}

/// Stack a label and its controls when the column cannot hold both.
fn row(label: String, control: AnyElement, caption: Option<String>, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    h_flex()
        .w_full()
        .min_w_0()
        .flex_wrap()
        .gap(design::ui_px(cx, 12.0))
        .items_start()
        .child(
            div()
                .w(design::font_w_px(cx, 190.0))
                .max_w_full()
                .flex_shrink_0()
                .pt(design::ui_px(cx, 4.0))
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .flex_basis(design::font_w_px(cx, 220.0))
                .min_w(design::font_w_px(cx, 220.0))
                .max_w_full()
                .child(control),
        )
        .children(caption.map(|text| {
            div()
                .flex_1()
                .min_w(design::font_w_px(cx, 220.0))
                .max_w_full()
                .child(prose(text, p.text_muted, cx))
        }))
        .into_any_element()
}

/// Each fitted segment wraps as a whole, so the longest localized forever label stays readable.
fn keep_controls(ed: &TapeEd, disabled: bool, actions: &Actions, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    let keep = ed.draft_keep.unwrap_or(TapeKeep {
        max_age_days: DEFAULT_MAX_AGE_DAYS,
        record_emulator: true,
        tuner_only: None,
        min_tail_s: None,
    });
    let presets = [30, 90, 180, 365, 1825, 0];
    let chips = h_flex()
        .w_full()
        .min_w_0()
        .flex_wrap()
        .gap(design::ui_px(cx, 4.0))
        .debug_selector(|| "server-tape-days-chips".into())
        .children(presets.into_iter().map(|days| {
            let on_days = actions.days.clone();
            let label = match days {
                30 | 90 | 180 => t!("telegram.server.keep_chip_days", n = days).to_string(),
                _ => period(days),
            };
            MoonSegmentedControl::new(("server-tape-day", days as usize))
                .accent(MoonAccent::Blue)
                .selection(MoonSegmentSelection::Filled)
                .item(
                    MoonSegmentItem::new("", label)
                        .fit_width(cx, 36.0, 240.0)
                        .selected(keep.max_age_days == days)
                        .disabled(disabled),
                )
                .on_click(move |_, _, window, app| on_days(days, window, app))
                .into_any_element()
        }));
    let custom = h_flex()
        .w_full()
        .min_w_0()
        .flex_wrap()
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .child(t!("telegram.server.keep_custom").to_string())
        .children(ed.days_input.as_ref().map(|input| {
            crate::settings::general::num_input_on(
                "server-tape-days",
                input,
                1.0..=3650.0,
                1.0,
                None,
                !disabled,
            )
        }))
        .child(t!("telegram.server.keep_days_unit").to_string());
    v_flex()
        .w_full()
        .min_w_0()
        .gap(design::ui_px(cx, 6.0))
        .child(chips)
        .child(custom)
        .child(
            prose(keep_note(keep.max_age_days), p.text_muted, cx)
                .debug_selector(|| "server-tape-days-note".into()),
        )
        .into_any_element()
}

/// Render the real page's entire tape block without constructing a network/storage backend.
pub(super) fn server_tape_block(
    ed: &TapeEd,
    facts: Snapshot<'_>,
    controls: Controls,
    actions: Actions,
    cx: &App,
) -> AnyElement {
    let Snapshot {
        catchup: c,
        busy,
        width,
        read_at: snapshot,
    } = facts;
    let p = MoonPalette::active(cx);
    let state = keep_state(c);
    let disabled = busy
        || ed.confirm
        || ed.awaiting_keep.is_some()
        || matches!(state, KeepState::Old | KeepState::Purging { .. });
    let kind = catchup_state(c);
    let mut tiles = c.map(|c| catchup_tiles(kind, c)).unwrap_or_default();
    if let Some(c) = c {
        tiles.push(db_tile(c));
    }
    let keep = ed.draft_keep.unwrap_or(TapeKeep {
        max_age_days: DEFAULT_MAX_AGE_DAYS,
        record_emulator: true,
        tuner_only: None,
        min_tail_s: None,
    });
    let label = t!("telegram.server.keep_emu_label").to_string();
    let on_emulator = actions.emulator.clone();
    let emulator = crate::settings::common::checkbox_with_caption(
        "server-tape-emu",
        &label,
        MoonCheckbox::new("server-tape-emu")
            .checked(keep.record_emulator)
            .disabled(disabled)
            .label(label.clone())
            .on_change(move |on, window, app| on_emulator(*on, window, app)),
        t!("telegram.server.keep_emu_caption").to_string(),
        cx,
    );
    // Only a station that reports the choice shows it; an older one never receives the key.
    let tuner = keep.tuner_only.map(|on| {
        let label = t!("telegram.server.keep_tuner_label").to_string();
        let on_tuner = actions.tuner.clone();
        crate::settings::common::checkbox_with_caption(
            "server-tape-tuner",
            &label,
            MoonCheckbox::new("server-tape-tuner")
                .checked(on)
                .disabled(disabled)
                .label(label.clone())
                .on_change(move |on, window, app| on_tuner(*on, window, app)),
            t!("telegram.server.keep_tuner_caption").to_string(),
            cx,
        )
    });
    let keep_block = v_flex()
        .w_full()
        .min_w_0()
        .gap(design::ui_px(cx, 8.0))
        .debug_selector(|| "server-tape-keep".into())
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(t!("telegram.server.keep_title").to_string()),
        )
        .child(prose(
            t!("telegram.server.keep_hint").to_string(),
            p.text_muted,
            cx,
        ))
        .child(row(
            t!("telegram.server.keep_age_label").to_string(),
            keep_controls(ed, disabled, &actions, cx),
            None,
            cx,
        ))
        .child(row(
            t!("telegram.server.keep_emu").to_string(),
            emulator,
            None,
            cx,
        ))
        .children(tuner.map(|tuner| {
            row(
                t!("telegram.server.keep_tuner").to_string(),
                tuner,
                None,
                cx,
            )
        }))
        .when(matches!(state, KeepState::Old), |el| {
            el.child(
                prose(t!("telegram.server.keep_old").to_string(), p.amber, cx)
                    .debug_selector(|| "server-tape-keep-old".into()),
            )
        });
    // The age histogram cannot tell which trades lack millisecond stamps: no count then.
    let drops_untunable = ed.seen_keep.is_some_and(|seen| seen.drops_untunable(keep));
    let counts = c
        .and_then(|c| purge_counts(c, keep))
        .filter(|_| !drops_untunable);
    let confirm = ed.confirm.then(|| {
        let title = counts
            .as_ref()
            .map(|counts| {
                t!(
                    &plural_key("telegram.server.keep_confirm", counts.total),
                    n = counts.total
                )
                .to_string()
            })
            .unwrap_or_else(|| t!("telegram.server.keep_confirm_unknown").to_string());
        let yes = actions.confirm.clone();
        let cancel = actions.cancel.clone();
        v_flex()
            .w_full()
            .min_w_0()
            .gap(design::ui_px(cx, 6.0))
            .p(design::ui_px(cx, 10.0))
            .border_1()
            .border_color(rgba_from(p.red, 1.0))
            .bg(rgba_from(p.red, 0.08))
            .debug_selector(|| "server-tape-confirm".into())
            .child(prose(title, p.red, cx))
            .children(
                counts
                    .as_ref()
                    .filter(|counts| counts.older > 0)
                    .map(|counts| {
                        prose(
                            t!(
                                "telegram.server.keep_confirm_older",
                                n = counts.older,
                                period = period(keep.max_age_days)
                            )
                            .to_string(),
                            p.text,
                            cx,
                        )
                    }),
            )
            .children(
                counts
                    .as_ref()
                    .filter(|counts| counts.emulator > 0)
                    .map(|counts| {
                        prose(
                            t!(
                                &super::plural_key(
                                    "telegram.server.keep_confirm_emu",
                                    counts.emulator
                                ),
                                n = counts.emulator
                            )
                            .to_string(),
                            p.text,
                            cx,
                        )
                    }),
            )
            .children(drops_untunable.then(|| {
                prose(
                    t!("telegram.server.keep_confirm_tuner").to_string(),
                    p.text,
                    cx,
                )
                .debug_selector(|| "server-tape-confirm-tuner".into())
            }))
            .child(prose(
                t!("telegram.server.keep_confirm_note").to_string(),
                p.text_muted,
                cx,
            ))
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 8.0))
                    .child(
                        div()
                            .debug_selector(|| "server-tape-confirm-yes".into())
                            .child(
                                MoonButton::new("server-tape-confirm-yes")
                                    .variant(MoonButtonVariant::Red)
                                    .disabled(busy || matches!(state, KeepState::Purging { .. }))
                                    .label(t!("telegram.server.keep_confirm_yes").to_string())
                                    .on_click(move |_, window, app| yes(window, app))
                                    .render(),
                            ),
                    )
                    .child(
                        div()
                            .debug_selector(|| "server-tape-confirm-cancel".into())
                            .child(
                                MoonButton::new("server-tape-confirm-cancel")
                                    .ghost()
                                    .label(t!("common.cancel").to_string())
                                    .on_click(move |_, window, app| cancel(window, app))
                                    .render(),
                            ),
                    ),
            )
            .into_any_element()
    });
    v_flex()
        .w_full()
        .min_w_0()
        .gap(design::ui_px(cx, 10.0))
        .font_family(design::ui_font())
        .text_color(rgba_from(p.text, 1.0))
        .debug_selector(|| "server-tape-block".into())
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(t!("telegram.server.tape_title").to_string()),
        )
        .child(
            prose(
                t!("telegram.server.tape_hint").to_string(),
                p.text_muted,
                cx,
            )
            .flex_shrink_0()
            .mb(design::ui_px(cx, 8.0))
            .debug_selector(|| "server-tape-hint".into()),
        )
        .child(
            div()
                .w_full()
                .min_w_0()
                .debug_selector(|| "server-tape-margin-row".into())
                .child(row(
                    t!("storage.trades_margin").to_string(),
                    controls.margin,
                    Some(t!("telegram.server.tape_margin_hint").to_string()),
                    cx,
                )),
        )
        .child(row(
            t!("storage.trades_long_position").to_string(),
            controls.long,
            Some(t!("telegram.server.tape_long_hint").to_string()),
            cx,
        ))
        .child(keep_block)
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(t!("telegram.server.catchup_section").to_string()),
        )
        .child(tape_status_strip(
            kind,
            catchup_sentence(kind, c, crate::chartdx::axes::display_zone()),
            snapshot,
            cx,
        ))
        .children(match state {
            KeepState::Purging { frac } => Some(
                div()
                    .w_full()
                    .debug_selector(|| "server-tape-purge-progress".into())
                    .child(
                        MoonProgress::new("server-tape-purge-progress")
                            .tone(moon_ui::MoonTone::Info)
                            .value(frac * 100.0)
                            .height(design::ui_value(cx, 4.0))
                            .render(),
                    ),
            ),
            _ => None,
        })
        .when(!tiles.is_empty(), |el| {
            el.child(tape_tiles(&tiles, width, cx))
        })
        .children(
            c.and_then(catchup_breakdown)
                .map(|text| tape_breakdown(text, cx)),
        )
        .children(controls.switches)
        .child(controls.venues)
        .children(confirm)
        .child(prose(
            t!("telegram.server.tape_footnote").to_string(),
            p.text_muted,
            cx,
        ))
        .into_any_element()
}
