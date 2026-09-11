//! The left column of the expert window: the core list.
//!
//! The list is the same shape every core menu in the terminal draws — venue sections in canonical
//! order, `MoonListItem` rows — with the selection gestures of the Core Status panel: a plain
//! click picks one core, Ctrl toggles, Shift ranges. Rows are drawn from [`super::cores`]'s
//! roster, which the sync rebuilds and compares, so this file holds no rule about which core is
//! which.
//!
//! A venue heads its cores — a dot in the accent, the venue's name, and the cores indented beneath,
//! the shape the Connections tab gives its sections — so the eye reads each core as belonging to
//! the venue above it rather than as a flat list interrupted by captions.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonListItem, MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use crate::design::{self, moon, moon_alpha};

use super::cores::RosterRow;
use super::{CoreExpertView, widgets};

/// Design-reference width of the column.
const SIDEBAR_W: f32 = 236.0;
/// Design-reference indent of a core row under its venue heading, past the heading's own inset.
const ROW_INDENT: f32 = 14.0;
/// Design-reference diameter of the dot before a venue heading — the status dot's own size.
const VENUE_DOT: f32 = 6.0;

impl CoreExpertView {
    /// Build the column: a caption with the selection count, then the list.
    pub(super) fn sidebar(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let selected = self.selection.len();
        v_flex()
            .flex_none()
            .h_full()
            .w(design::ui_px(cx, SIDEBAR_W))
            .min_h_0()
            .border_r(px(1.0))
            .border_color(moon_alpha(p.border, 1.0))
            .bg(moon(p.shell_high))
            .child(
                h_flex()
                    .flex_none()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .px(design::ui_px(cx, design::HEADER_PAD_X))
                    .py(design::ui_px(cx, 6.0))
                    .child(widgets::text_line(
                        t!("core_expert.cores_title").to_string(),
                        p.text,
                        true,
                        cx,
                    ))
                    .child(widgets::hint(
                        t!("core_expert.cores_selected", n = selected.to_string()).to_string(),
                        p,
                        cx,
                    )),
            )
            .child(self.core_list(p, cx))
    }

    /// The scrolling list of cores, in venue sections.
    fn core_list(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        // Resolved once per frame, not once per row without a page.
        let no_page: SharedString = t!("core_expert.core_no_page").to_string().into();
        let mut list = v_flex()
            .id("core-expert-core-list")
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_y_scroll()
            .pb(design::ui_px(cx, 4.0));
        for section in &self.roster.sections {
            list = list.child(venue_heading(section.label.clone(), p, cx));
            for row in &section.rows {
                list = list.child(self.core_row(row, &no_page, &view, p, cx));
            }
        }
        if self.roster.sections.is_empty() {
            list = list.child(
                div()
                    .px(design::ui_px(cx, design::HEADER_PAD_X))
                    .py(design::ui_px(cx, 6.0))
                    .child(widgets::text_block(
                        t!("core_expert.cores_none").to_string(),
                        p.text_muted,
                        false,
                        cx,
                    )),
            );
        }
        list
    }

    /// One core's row: its name, and a note when it has no page for OK to reach.
    fn core_row(
        &self,
        row: &RosterRow,
        no_page: &SharedString,
        view: &Entity<CoreExpertView>,
        p: MoonPalette,
        cx: &App,
    ) -> impl IntoElement {
        let core = row.core;
        let view = view.clone();
        let is_anchor = self.anchor == Some(core);
        // Derived from the core, never from a row index: a shared literal id would migrate GPUI
        // hover and press state between rows as the list re-sorts.
        MoonListItem::new(("core-expert-core", core))
            .selected(self.selection.contains(Some(core)))
            // Under the heading, not level with it: the indent is what says "this venue's".
            .pl(design::ui_px(cx, design::HEADER_PAD_X + ROW_INDENT))
            .on_click(move |event, _window, app| {
                let modifiers = event.modifiers();
                view.update(app, |this, cx| this.select_core_row(core, modifiers, cx));
            })
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap(design::ui_px(cx, 6.0))
                    .child(div().flex_1().min_w_0().child(widgets::text_line(
                        row.name.clone(),
                        if row.has_page { p.text } else { p.text_muted },
                        is_anchor,
                        cx,
                    )))
                    .when(!row.has_page, |this| {
                        this.child(widgets::hint(no_page.clone(), p, cx))
                    }),
            )
    }
}

/// A venue's heading: the accent dot and the venue's name.
fn venue_heading(label: SharedString, p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .px(design::ui_px(cx, design::HEADER_PAD_X))
        .pt(design::ui_px(cx, 8.0))
        .pb(design::ui_px(cx, 2.0))
        .child(design::status_dot_sized(p.accent, VENUE_DOT, cx))
        .child(widgets::text_line(label, p.text_soft, true, cx))
}
