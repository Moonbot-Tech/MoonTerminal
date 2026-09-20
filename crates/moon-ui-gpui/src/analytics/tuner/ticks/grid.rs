//! The parameter grid of the "Entry/Exit" axis: the two groups of `TICK_PARAMS`, each behind
//! its own caret, with the "now" value of the selected strategies beside every field.
//!
//! Phase 1 is read-only: the variant columns and the "vary" switches arrive with the search.
//! The Entry group is shown only when every kind in the scope has an entry model; otherwise it
//! folds to one line saying whose entry is taken from the fact.

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::super::super::AnalyticsView;
use super::super::shared::{card, collapse_caret};
use super::state::{NowValue, TicksData};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::tuner::ticks::params::{ParamGroup, TickParam, params_for};

impl AnalyticsView {
    /// The grid card: the assumptions line under the title, then the two groups.
    pub(in crate::analytics::tuner) fn ticks_grid(
        &self,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let data = self.ticks.data.data().cloned();
        let entry_open = self.ticks.entry_open;
        let exit_open = self.ticks.exit_open;
        let mut body = v_flex().w_full().flex_none();
        // What the model does not know, in one line — the spec's §4.4, kept where the user
        // reads the numbers the assumptions shape.
        body = body.child(
            div()
                .w_full()
                .px(design::ui_px(cx, 12.0))
                .pb(design::ui_px(cx, 6.0))
                .font_family(design::ui_font())
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(t!("analytics.ticks.assumptions").to_string()),
        );
        let entry_modelled = data.as_ref().is_some_and(|d| d.entry_modelled());
        let unmodelled: Vec<String> = data
            .as_ref()
            .map(|d| d.unmodelled_kinds().into_iter().map(String::from).collect())
            .unwrap_or_default();
        // Only a LOADED empty scope says so; a load in flight or a failed one has its own note.
        let no_deals = data.as_ref().is_some_and(|d| d.kinds.is_empty());
        let entry_note = if no_deals {
            Some(t!("analytics.ticks.no_deals").to_string())
        } else if entry_modelled {
            None
        } else {
            Some(
                t!(
                    "analytics.ticks.entry_from_fact",
                    kinds = unmodelled.join(", ")
                )
                .to_string(),
            )
        };
        body = body.child(self.ticks_group(
            "an-ticks-grp-entry",
            ParamGroup::Entry,
            t!("analytics.ticks.group_entry").to_string(),
            entry_note,
            entry_open && entry_modelled,
            entry_modelled,
            data.as_deref(),
            |this| this.ticks.entry_open = !this.ticks.entry_open,
            p,
            cx,
        ));
        body = body.child(self.ticks_group(
            "an-ticks-grp-exit",
            ParamGroup::Exit,
            t!("analytics.ticks.group_exit").to_string(),
            None,
            exit_open,
            true,
            data.as_deref(),
            |this| this.ticks.exit_open = !this.ticks.exit_open,
            p,
            cx,
        ));
        card(
            t!("analytics.ticks.params_title").to_string(),
            t!("analytics.ticks.params_sub").to_string(),
            body.into_any_element(),
            None,
            p,
            cx,
        )
    }

    /// One group: a header line with its caret, then a row per field when unfolded.
    #[allow(clippy::too_many_arguments)]
    fn ticks_group(
        &self,
        id: &'static str,
        group: ParamGroup,
        title: String,
        note: Option<String>,
        open: bool,
        enabled: bool,
        data: Option<&TicksData>,
        toggle: impl Fn(&mut Self) + 'static,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let caret = collapse_caret(
            id,
            !open,
            t!("analytics.ticks.group_collapse").to_string(),
            t!("analytics.ticks.group_expand").to_string(),
            p,
            cx.listener(move |this, _, _, cx| {
                toggle(this);
                cx.notify();
            }),
        );
        let mut head = h_flex()
            .w_full()
            .px(design::ui_px(cx, 12.0))
            .py(design::ui_px(cx, 4.0))
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .bg(moon(p.table_head))
            .font_family(design::ui_font())
            .child(
                div()
                    .flex_none()
                    .text_size(design::t_body(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if enabled {
                        moon(p.text)
                    } else {
                        moon(p.text_muted)
                    })
                    .child(title),
            );
        if let Some(note) = note {
            head = head.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .child(note),
            );
        } else {
            head = head.child(div().flex_1());
        }
        if enabled {
            head = head.child(div().flex_none().child(caret));
        }
        let mut out = v_flex().w_full().flex_none().child(head);
        if !(open && enabled) {
            return out.into_any_element();
        }
        // The fields every kind in the scope understands: the union over the kinds present,
        // in descriptor order.
        let kinds: Vec<&str> = data
            .map(|d| d.kinds.iter().map(String::as_str).collect())
            .unwrap_or_default();
        let fields: Vec<&'static TickParam> = moon_core::db::tuner::ticks::TICK_PARAMS
            .iter()
            .filter(|f| f.group == group)
            .filter(|f| {
                kinds
                    .iter()
                    .any(|k| params_for(group, k).any(|g| g.key == f.key))
            })
            .collect();
        out = out.child(
            h_flex()
                .w_full()
                .px(design::ui_px(cx, 12.0))
                .py(design::ui_px(cx, 2.0))
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_soft))
                .child(
                    div()
                        .flex_1()
                        .child(t!("analytics.tuner.field").to_string()),
                )
                .child(
                    div()
                        .w(design::font_w_px(cx, 96.0))
                        .flex_none()
                        .text_right()
                        .child(t!("analytics.ticks.now").to_string()),
                ),
        );
        for field in fields {
            let now = data.and_then(|d| d.now.get(field.key));
            let (text, muted) = match now {
                Some(NowValue::Same(v)) if !v.is_empty() => (v.clone(), false),
                Some(NowValue::Same(_)) | None => ("—".to_string(), true),
                Some(NowValue::Differs) => (t!("analytics.time.cur_varies").to_string(), true),
            };
            out = out.child(
                h_flex()
                    .w_full()
                    .px(design::ui_px(cx, 12.0))
                    .py(design::ui_px(cx, 2.0))
                    .items_center()
                    .border_t_1()
                    .border_color(moon_alpha(p.border, 0.5))
                    .text_size(design::t_body(cx))
                    .child(div().flex_1().min_w_0().truncate().child(field.key))
                    .child(
                        div()
                            .w(design::font_w_px(cx, 96.0))
                            .flex_none()
                            .text_right()
                            .text_color(if muted {
                                moon(p.text_muted)
                            } else {
                                moon(p.text)
                            })
                            .child(text),
                    ),
            );
        }
        out.into_any_element()
    }
}
