//! View-free summary card: advice, slice profits, changed fields and one inline consent.

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonPalette, MoonProgress, MoonTag, h_flex,
    v_flex,
};
use rust_i18n::t;

use super::super::estimate::duration_text;
use super::{ConfirmModel, ProfitRow, SummaryModel, Verdict, progress_text};
use crate::analytics::summary::{fmt_signed, sign_color};
use crate::analytics::tuner::shared::{collapse_caret, tagged};
use crate::design;
use crate::design::{moon, moon_alpha};

/// A summary action updates its owning entity; the card never reads an entity itself.
type Act = Rc<dyn Fn(&mut Window, &mut App)>;

/// Callbacks supplied by the production view or by a layout probe.
pub(in crate::analytics::tuner) struct SummaryActs {
    pub write: Act,
    pub confirm: Act,
    pub cancel: Act,
    pub copy: Act,
    pub reset: Act,
    pub stop: Act,
    pub details: Act,
}

/// A wrapping prose line has no fixed height, so translations can grow vertically.
fn prose(text: String, color: u32, cx: &App) -> Div {
    let selector = format!("an-ticks-prose-{text}");
    let content = format!("an-ticks-prose-content-{text}");
    div()
        .debug_selector(move || selector.clone())
        .w_full()
        .min_w_0()
        .font_family(design::ui_font())
        .text_size(design::t_caption(cx))
        .text_color(moon(color))
        .child(div().debug_selector(move || content.clone()).child(text))
}

/// Give the layout probe a content-sized button using the active density tier.
fn button(
    id: &'static str,
    key: &'static str,
    variant: MoonButtonVariant,
    enabled: bool,
    act: Act,
    cx: &App,
) -> Div {
    tagged(
        id,
        MoonButton::new(id)
            .variant(variant)
            .mono(false)
            .size(MoonButtonSize::density(cx))
            .label(t!(key).to_string())
            .disabled(!enabled)
            .on_click(move |_, window, app| act(window, app))
            .render(),
    )
}

/// Keep a warning bullet outside its wrapping text, including in a narrow side dock.
fn warning(text: String, p: MoonPalette, cx: &App) -> Div {
    h_flex()
        .w_full()
        .items_start()
        .gap(design::ui_px(cx, 6.0))
        .child(
            div()
                .flex_none()
                .font_family(design::ui_font())
                .text_color(moon(p.amber))
                .child("•"),
        )
        .child(prose(text, p.text_soft, cx).flex_1())
}

/// Figures remain a separate mono segment from the human label and its slice description.
fn profit_row(row: &ProfitRow, id: &'static str, p: MoonPalette, cx: &App) -> Div {
    let figures = h_flex()
        .flex_none()
        .gap(design::ui_px(cx, 8.0))
        .font_family(design::mono())
        .text_size(design::t_body(cx))
        .child(
            div()
                .text_color(moon(sign_color(p, row.fact)))
                .child(fmt_signed(row.fact)),
        )
        .child(div().text_color(moon(p.text_muted)).child("→"))
        .child(
            div()
                .text_color(moon(sign_color(p, row.variant)))
                .child(fmt_signed(row.variant)),
        );
    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 4.0))
        .child(
            h_flex()
                .w_full()
                .flex_wrap()
                .items_start()
                .gap(design::ui_px(cx, 8.0))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .font_family(design::ui_font())
                        .text_size(design::t_body(cx))
                        .child(row.label.clone())
                        .child(
                            prose(row.sub.clone(), p.text_muted, cx)
                                .id(id)
                                .tooltip(crate::panels::common::text_tooltip(row.n.to_string())),
                        ),
                )
                .child(figures),
        )
        .children(row.open_line.clone().map(|line| warning(line, p, cx)))
}

/// One inline question holds every warning computed at preparation, without another modal.
fn confirmation(
    model: &ConfirmModel,
    acts: &SummaryActs,
    enabled: bool,
    p: MoonPalette,
    cx: &App,
) -> Div {
    let mut body = v_flex()
        .w_full()
        .min_w_0()
        .gap(design::ui_px(cx, 8.0))
        .p(design::ui_px(cx, 10.0))
        .rounded(design::r_container(cx))
        .bg(moon_alpha(p.blue, 0.1))
        .border_1()
        .border_color(moon(p.blue))
        .child(prose(model.title.clone(), p.text, cx));
    if let Some(reason) = &model.reason {
        body = body.child(
            prose(reason.clone(), p.amber, cx)
                .when(model.bold_reason, |line| line.font_weight(FontWeight::BOLD)),
        );
    }
    for warn in &model.warns {
        body = body.child(warning(warn.clone(), p, cx));
    }
    body.child(
        h_flex()
            .w_full()
            .flex_wrap()
            .gap(design::ui_px(cx, 6.0))
            .child(button(
                "an-ticks-sum-confirm-yes",
                "analytics.ticks.sum.confirm_yes",
                MoonButtonVariant::Blue,
                enabled,
                acts.confirm.clone(),
                cx,
            ))
            .child(button(
                "an-ticks-sum-confirm-no",
                "analytics.ticks.sum.confirm_no",
                MoonButtonVariant::Ghost,
                true,
                acts.cancel.clone(),
                cx,
            )),
    )
}

/// Render a summary using only owned model data and callbacks.
///
/// Args:
///     m: The current displayed variant's advice and inline-confirmation snapshot.
///     acts: Actions supplied without a live view dependency.
///     p: Active palette for advice and signed profit colours.
///     cx: Application context providing scale and localized captions.
///
/// Returns:
///     A naturally growing card; its caller owns the only vertical scroll container.
pub(in crate::analytics::tuner) fn summary_card_el(
    m: &SummaryModel,
    acts: &SummaryActs,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let (tag, color) = match &m.verdict {
        Verdict::Apply => (MoonTag::positive(), p.green),
        Verdict::DontApply(_) => (MoonTag::warning(), p.amber),
        Verdict::Cannot(_) => (MoonTag::danger(), p.red),
        Verdict::Idle | Verdict::Running { .. } | Verdict::Unchecked(_) => {
            (MoonTag::new(), p.text_muted)
        }
    };
    let tag = tagged("an-ticks-sum-tag", tag.mono(false).child(m.tag.clone()));
    let mut body = v_flex()
        .w_full()
        .flex_none()
        .min_w_0()
        .gap(design::ui_px(cx, 10.0))
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 10.0))
        .child(
            h_flex()
                .w_full()
                .flex_wrap()
                .items_start()
                .gap(design::ui_px(cx, 10.0))
                .p(design::ui_px(cx, 10.0))
                .rounded(design::r_container(cx))
                .bg(moon_alpha(color, 0.1))
                .child(tag)
                .child(prose(m.sentence.clone(), p.text, cx).flex_1()),
        );
    if let Verdict::Running {
        done,
        total,
        left,
        points,
    } = &m.verdict
    {
        let pct = if *total == 0 {
            0.0
        } else {
            (*done as f32 / *total as f32 * 100.0).clamp(0.0, 100.0)
        };
        body = body
            .child(
                MoonProgress::new("an-ticks-sum-progress")
                    .value(pct)
                    .color(p.blue),
            )
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .justify_between()
                    .gap(design::ui_px(cx, 8.0))
                    .child(prose(progress_text(*done, *total, *points), p.text_muted, cx).w_auto())
                    .children(left.map(|time| {
                        prose(
                            t!("analytics.ticks.sum.left", time = duration_text(time)).to_string(),
                            p.text_muted,
                            cx,
                        )
                        .w_auto()
                    })),
            )
            .child(h_flex().child(button(
                "an-ticks-sum-stop",
                "analytics.ticks.sum.stop",
                MoonButtonVariant::Soft,
                true,
                acts.stop.clone(),
                cx,
            )));
    } else {
        if m.train.is_some() || m.holdout.is_some() {
            let unit = crate::analytics::pnl_unit_label();
            let head = if unit.is_empty() {
                t!("analytics.ticks.sum.profit_head").to_string()
            } else {
                format!("{}, {unit}", t!("analytics.ticks.sum.profit_head"))
            };
            body = body.child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .justify_between()
                    .gap(design::ui_px(cx, 8.0))
                    .child(prose(head, p.text_muted, cx).w_auto())
                    .child(
                        h_flex()
                            .gap(design::ui_px(cx, 16.0))
                            .font_family(design::ui_font())
                            .text_color(moon(p.text_muted))
                            .text_size(design::t_caption(cx))
                            .child(t!("analytics.ticks.sum.col_fact").to_string())
                            .child(t!("analytics.ticks.sum.col_v1").to_string()),
                    ),
            );
            if let Some(row) = &m.train {
                body = body.child(profit_row(row, "an-ticks-sum-train-count", p, cx));
            }
            if let Some(row) = &m.holdout {
                body = body.child(profit_row(row, "an-ticks-sum-hold-count", p, cx));
            }
        }
        if !m.changes.is_empty() {
            let mut changed = v_flex()
                .w_full()
                .gap(design::ui_px(cx, 5.0))
                .pt(design::ui_px(cx, 8.0))
                .border_t_1()
                .border_color(moon(p.border))
                .child(
                    prose(m.changed_header.clone(), p.text_muted, cx)
                        .font_weight(FontWeight::SEMIBOLD),
                );
            for row in &m.changes {
                changed = changed.child(
                    h_flex()
                        .w_full()
                        .flex_wrap()
                        .items_start()
                        .gap(design::ui_px(cx, 8.0))
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .flex_wrap()
                                .gap(design::ui_px(cx, 4.0))
                                .font_family(design::ui_font())
                                .text_size(design::t_body(cx))
                                .child(div().child(row.key.clone()))
                                .child(
                                    div()
                                        .text_size(design::t_caption(cx))
                                        .text_color(moon(p.text_muted))
                                        .child(row.section.clone()),
                                ),
                        )
                        .child(
                            h_flex()
                                .flex_none()
                                .flex_wrap()
                                .gap(design::ui_px(cx, 8.0))
                                .font_family(design::mono())
                                .text_size(design::t_body(cx))
                                .child(div().text_color(moon(p.text_muted)).child(row.old.clone()))
                                .child(div().text_color(moon(p.text_muted)).child("→"))
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(row.new.clone()),
                                ),
                        ),
                );
            }
            body = body.child(changed);
        }
        if (m.can_reset || m.train.is_some()) && !m.warnings.is_empty() {
            let mut warns = v_flex().w_full().gap(design::ui_px(cx, 6.0));
            for warn in &m.warnings {
                warns = warns.child(warning(warn.clone(), p, cx));
            }
            body = body.child(warns);
        }
        if m.can_reset {
            let mut actions = h_flex()
                .w_full()
                .flex_wrap()
                .gap(design::ui_px(cx, 6.0))
                .child(button(
                    "an-ticks-sum-write",
                    "analytics.ticks.sum.write",
                    if m.verdict == Verdict::Apply {
                        MoonButtonVariant::Blue
                    } else {
                        MoonButtonVariant::Soft
                    },
                    m.write_enabled,
                    acts.write.clone(),
                    cx,
                ));
            if m.copy_visible {
                actions = actions.child(button(
                    "an-ticks-sum-copy",
                    "analytics.ticks.sum.copy",
                    MoonButtonVariant::Soft,
                    m.write_enabled,
                    acts.copy.clone(),
                    cx,
                ));
            }
            actions = actions.child(button(
                "an-ticks-sum-reset",
                "analytics.ticks.sum.reset",
                MoonButtonVariant::Ghost,
                true,
                acts.reset.clone(),
                cx,
            ));
            body = body.child(actions).child(prose(
                t!("analytics.ticks.sum.write_note").to_string(),
                p.text_muted,
                cx,
            ));
        }
        if let Some(confirm) = &m.confirm {
            body = body.child(confirmation(confirm, acts, m.write_enabled, p, cx));
        }
        if let Some(error) = &m.write_error {
            body = body.child(prose(error.clone(), p.red, cx));
        }
        if !m.details.is_empty() {
            let act = acts.details.clone();
            let caret = collapse_caret(
                "an-ticks-sum-details",
                !m.details_open,
                t!("analytics.ticks.sum.details").to_string(),
                t!("analytics.ticks.sum.details").to_string(),
                p,
                move |_, window, app| act(window, app),
            );
            let mut details = v_flex()
                .w_full()
                .gap(design::ui_px(cx, 6.0))
                .pt(design::ui_px(cx, 8.0))
                .border_t_1()
                .border_color(moon(p.border))
                .child(
                    h_flex()
                        .w_full()
                        .gap(design::ui_px(cx, 6.0))
                        .child(tagged("an-ticks-sum-details", caret))
                        .child(prose(
                            t!("analytics.ticks.sum.details").to_string(),
                            p.text_muted,
                            cx,
                        )),
                );
            if m.details_open {
                for (label, value) in &m.details {
                    details = details.child(
                        h_flex()
                            .w_full()
                            .flex_wrap()
                            .gap(design::ui_px(cx, 8.0))
                            .child(prose(label.clone(), p.text_muted, cx).flex_1())
                            .child(
                                div()
                                    .flex_none()
                                    .font_family(design::mono())
                                    .text_size(design::t_caption(cx))
                                    .text_color(moon(p.text))
                                    .child(value.clone()),
                            ),
                    );
                }
            }
            body = body.child(details);
        }
    }
    v_flex()
        .id("an-ticks-sum")
        .debug_selector(|| "an-ticks-sum".to_string())
        .w_full()
        .flex_none()
        .min_w_0()
        .rounded(design::r_container(cx))
        .bg(moon(p.panel))
        .border_1()
        .border_color(moon(p.border))
        .font_family(design::ui_font())
        .text_size(design::t_body(cx))
        .text_color(moon(p.text))
        .child(
            h_flex()
                .w_full()
                .flex_wrap()
                .items_start()
                .gap(design::ui_px(cx, 6.0))
                .px(design::ui_px(cx, 12.0))
                .pt(design::ui_px(cx, 10.0))
                .child(
                    div()
                        .font_family(design::ui_font())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(design::t_title(cx))
                        .child(m.title.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_family(design::mono())
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_muted))
                        .child(m.scope_label.clone()),
                ),
        )
        .child(body)
        .into_any_element()
}
