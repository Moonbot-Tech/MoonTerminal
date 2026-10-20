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

/// A short label beside a sibling in a wrapping row: it never shrinks, so a narrow row wraps
/// the label to the next line instead of folding its text into one glyph per line.
fn label(text: String, color: u32, cx: &App) -> Div {
    prose(text, color, cx)
        .w_auto()
        .min_w_auto()
        .flex_none()
        .whitespace_nowrap()
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

/// Widths of the profit block's fact, arrow and V1 columns, shared by its head and every row.
#[derive(Clone, Copy)]
struct ProfitCols {
    fact: Pixels,
    /// `None` on a narrow host: the arrow column drops and the other three stay aligned.
    arrow: Option<Pixels>,
    v1: Pixels,
}

impl ProfitCols {
    /// Size each figure column to the widest of its head and its figures, so heads sit over them.
    fn measure(rows: &[&ProfitRow], arrow: bool, cx: &App) -> Self {
        let width = |head: &str, value: fn(&ProfitRow) -> f64| {
            let normal = FontWeight::NORMAL.0;
            let widest = rows
                .iter()
                .map(|row| design::mono_body_text_width(cx, &fmt_signed(value(row)), normal))
                .fold(design::ui_caption_text_width(cx, head, normal), f32::max);
            // The estimate sums advances without kerning; one pixel keeps a rounded glyph whole.
            px(widest.ceil() + 1.0)
        };
        Self {
            fact: width(&t!("analytics.ticks.sum.col_fact"), |row| row.fact),
            arrow: arrow.then(|| {
                px(design::mono_body_text_width(cx, "→", FontWeight::NORMAL.0).ceil() + 1.0)
            }),
            v1: width(&t!("analytics.ticks.sum.col_v1"), |row| row.variant),
        }
    }

    /// Lay a label column and the figure cells out as one grid row, each cell's content
    /// right-aligned inside its fixed column.
    fn row(self, label: Div, cells: [Div; 3], cx: &App) -> Div {
        let [fact, arrow, v1] = cells;
        let cell =
            |content: Div, w: Pixels| div().flex_none().w(w).flex().justify_end().child(content);
        h_flex()
            .w_full()
            .items_start()
            .gap(design::ui_px(cx, 12.0))
            .child(label.flex_1().min_w_0())
            .child(cell(fact, self.fact))
            .children(self.arrow.map(|w| cell(arrow, w).justify_center()))
            .child(cell(v1, self.v1))
    }
}

/// Column heads share the figure columns, right-aligned over the numbers they name.
fn profit_head(head: String, cols: ProfitCols, p: MoonPalette, cx: &App) -> Div {
    let caption = |key: &str, sel: &'static str| {
        div()
            .debug_selector(move || sel.to_string())
            .font_family(design::ui_font())
            .text_size(design::t_caption(cx))
            .text_color(moon(p.text_muted))
            .whitespace_nowrap()
            .child(t!(key).to_string())
    };
    cols.row(
        div().child(label(head, p.text_muted, cx)),
        [
            caption("analytics.ticks.sum.col_fact", "an-ticks-sum-col-fact"),
            div(),
            caption("analytics.ticks.sum.col_v1", "an-ticks-sum-col-v1"),
        ],
        cx,
    )
}

/// Figures remain separate mono cells from the human label and its slice description.
fn profit_row(
    row: &ProfitRow,
    id: &'static str,
    cols: ProfitCols,
    p: MoonPalette,
    cx: &App,
) -> Div {
    let figure = |value: f64, sel: String| {
        div()
            .debug_selector(move || sel.clone())
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .whitespace_nowrap()
            .text_color(moon(sign_color(p, value)))
            .child(fmt_signed(value))
    };
    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 4.0))
        .child(
            cols.row(
                v_flex()
                    .font_family(design::ui_font())
                    .text_size(design::t_body(cx))
                    .child(row.label.clone())
                    .child(
                        prose(row.sub.clone(), p.text_muted, cx)
                            .id(id)
                            .tooltip(crate::panels::common::text_tooltip(row.n.to_string())),
                    ),
                [
                    figure(row.fact, format!("{id}-fact")),
                    div()
                        .font_family(design::mono())
                        .text_size(design::t_body(cx))
                        .text_color(moon(p.text_muted))
                        .child("→"),
                    figure(row.variant, format!("{id}-v1")),
                ],
                cx,
            ),
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
///     arrow: Whether the host is wide enough for the profit block's arrow column; the same
///         width decision as the grid's range columns.
///     p: Active palette for advice and signed profit colours.
///     cx: Application context providing scale and localized captions.
///
/// Returns:
///     A naturally growing card; its caller owns the only vertical scroll container.
pub(in crate::analytics::tuner) fn summary_card_el(
    m: &SummaryModel,
    acts: &SummaryActs,
    arrow: bool,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let (tag, color) = match &m.verdict {
        Verdict::Apply => (MoonTag::positive(), p.green),
        Verdict::DontApply(_) => (MoonTag::warning(), p.amber),
        Verdict::Cannot(_) => (MoonTag::danger(), p.red),
        Verdict::Idle | Verdict::Loading | Verdict::Running { .. } | Verdict::Unchecked(_) => {
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
        stopping,
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
                    .child(label(
                        progress_text(*done, *total, *points),
                        p.text_muted,
                        cx,
                    ))
                    .children(left.map(|time| {
                        label(
                            t!("analytics.ticks.sum.left", time = duration_text(time)).to_string(),
                            p.text_muted,
                            cx,
                        )
                    })),
            )
            // A sentence, not a short label: it wraps instead of pushing past a narrow card.
            .children((*stopping).then(|| {
                prose(
                    t!("analytics.ticks.sum.stopping").to_string(),
                    p.text_muted,
                    cx,
                )
            }))
            .child(h_flex().child(button(
                "an-ticks-sum-stop",
                "analytics.ticks.sum.stop",
                MoonButtonVariant::Soft,
                !*stopping,
                acts.stop.clone(),
                cx,
            )));
    } else if m.verdict != Verdict::Loading {
        if m.train.is_some() || m.holdout.is_some() {
            let unit = crate::analytics::pnl_unit_label();
            let head = if unit.is_empty() {
                t!("analytics.ticks.sum.profit_head").to_string()
            } else {
                format!("{}, {unit}", t!("analytics.ticks.sum.profit_head"))
            };
            let rows: Vec<&ProfitRow> = m.train.iter().chain(&m.holdout).collect();
            let cols = ProfitCols::measure(&rows, arrow, cx);
            body = body.child(profit_head(head, cols, p, cx));
            if let Some(row) = &m.train {
                body = body.child(profit_row(row, "an-ticks-sum-train-count", cols, p, cx));
            }
            if let Some(row) = &m.holdout {
                body = body.child(profit_row(row, "an-ticks-sum-hold-count", cols, p, cx));
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
                                .child(
                                    div()
                                        .debug_selector({
                                            let key = row.key.clone();
                                            move || format!("an-ticks-change-old-{key}")
                                        })
                                        .text_color(moon(p.text_muted))
                                        .child(row.old.clone()),
                                )
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
            let actions = h_flex().w_full().flex_wrap().gap(design::ui_px(cx, 6.0));
            let actions = actions.child(button(
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
        .font_family(design::ui_font())
        .text_size(design::t_body(cx))
        .text_color(moon(p.text))
        .child(body)
        .into_any_element()
}

/// Put Copy and Save in the shared header without changing their guarded consent callbacks.
pub(in crate::analytics::tuner) fn parameters_header(
    m: &SummaryModel,
    acts: &SummaryActs,
    cx: &App,
) -> AnyElement {
    let handlers = crate::analytics::tuner::shell::ToolbarActs {
        on_copy: acts.copy.clone(),
        on_save: acts.write.clone(),
    };
    let write = crate::analytics::tuner::shell::ToolbarWrite {
        enabled: m.write_enabled
            && m.can_reset
            && !matches!(m.verdict, Verdict::Running { .. } | Verdict::Loading),
        copy: m.copy_visible,
        dirty: m.can_reset,
    };
    div()
        .id("tuner-ticks-parameters-header")
        .tooltip(crate::panels::common::text_tooltip(format!(
            "{} · {}",
            m.title, m.scope_label
        )))
        .child(crate::analytics::tuner::shell::toolbar_el(
            crate::analytics::tuner::shared::TunerKind::Ticks,
            t!("analytics.tuner.fields_title").to_string(),
            Some(&write),
            &handlers,
            cx,
        ))
        .into_any_element()
}
