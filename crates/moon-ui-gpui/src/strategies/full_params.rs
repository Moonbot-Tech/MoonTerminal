//! Virtualized renderer for the full-mode parameters pane.
//!
//! The pitch is uniform because `MoonVirtualList` is `uniform_list`-backed and clips each item to
//! `item_height`: a variable-height row (a memo/formula field, or a version's "was:"/"current:"
//! notes) would either be clipped or leave dead space under an ordinary row, so full mode renders a
//! COMPACT row variant (see `field_row`'s `compact` parameter) sized to one uniform pitch instead.
//!
//! The row factory reaches the view through a WEAK handle (`cx.entity().downgrade()`), never a
//! strong one, because `MoonVirtualList` stores the closure inside the rendered element: a strong
//! `View -> element -> closure -> View` cycle would keep the window alive forever and its
//! `on_release` would never run (see `settings/connections/tab.rs`'s `range_weak` for the same
//! shape). For the same reason this never uses `cx.processor`, which captures `self.entity()`
//! strongly.

use std::rc::Rc;

use super::param_entries::{FlatParams, ParamEntry};
use super::*;

/// Return the uniform row pitch used by the full-mode virtual list.
///
/// It mirrors `field_row`'s ordinary-row `min_h` and vertical padding. Frozen version mode also
/// reserves space for both note lines and the copy-to-current control when a field exposes them,
/// so every virtualized item remains within the same clipped pitch.
///
/// Args:
///     cx: Application context that resolves the current design scale.
///     frozen: Whether the pane displays a persisted version.
///
/// Returns:
///     The fixed height that every full-mode list item receives.
pub(super) fn full_row_h_value(cx: &App, frozen: bool) -> f32 {
    design::fit_h_value(cx, 30.0, 14.0, 8.0)
        + 2.0 * design::ui_value(cx, 4.0)
        + if frozen {
            2.0 * design::line_value(cx, 12.0) + design::micro_control_h_value(cx) + 4.0
        } else {
            0.0
        }
}

/// Resolve a table-of-contents scroll request against a flatten that may have dropped the
/// requested section (every one of its fields filtered out by `flatten_params`).
///
/// Args:
///     flat: The current full-mode flatten.
///     section: Schema section index the sections panel asked to jump to.
///
/// Returns:
///     The heading's own entry index when the section survived, otherwise the heading of the
///     first FOLLOWING section that does, otherwise the last entry. `None` only for an empty list.
fn resolve_scroll_target(flat: &FlatParams, section: usize) -> Option<usize> {
    if let Some(ix) = flat.heading_at.get(&section) {
        return Some(*ix);
    }
    flat.heading_at
        .iter()
        .filter(|(sec, _)| **sec > section)
        .min_by_key(|(sec, _)| **sec)
        .map(|(_, ix)| *ix)
        .or_else(|| flat.entries.len().checked_sub(1))
}

impl StrategiesView {
    /// Queue a full-mode scroll to a schema section for the next render.
    ///
    /// A per-section pane already changes body when a section is selected, so it needs no scroll
    /// request. The request stays one-shot rather than comparing selections, allowing a click on
    /// the visible section to place its heading at the top again.
    ///
    /// Args:
    ///     section: Schema section index selected in the sections panel.
    ///     cx: View context repainted to consume the request.
    ///
    /// Returns:
    ///     Nothing; ignored outside full mode and otherwise consumed by [`Self::full_params_list`].
    pub(super) fn request_param_scroll(&mut self, section: usize, cx: &mut Context<Self>) {
        if !self.prefs.params_full {
            return;
        }
        self.pending_param_scroll = Some(section);
        cx.notify();
    }

    /// Render every surviving schema section as one virtualized full-mode parameter list.
    ///
    /// The factory owns the frame's model inputs because `MoonVirtualList` retains a `'static`
    /// closure. It reaches the view through a weak entity handle, preserving the window's drop
    /// path while off-screen items are rebuilt on demand.
    ///
    /// Args:
    ///     flat: Flattened section headings and field rows for the current frame.
    ///     keys: Effective selected keys used for row identity and staging.
    ///     values: Selected strategy dependency values used to enable fields.
    ///     row_pairs: Effective selected rows used to merge field values.
    ///     pending: Open edits used for each row's pending-state badge.
    ///     _window: Retained for the shared pane-dispatch signature; virtual rows use their callback window.
    ///     cx: View context used to resolve scroll requests and construct controls.
    ///
    /// Returns:
    ///     The full-height virtualized list element for the parameter-pane body.
    pub(super) fn full_params_list(
        &mut self,
        flat: Rc<FlatParams>,
        keys: &[Key],
        // Taken BY VALUE: only one body-dispatch arm runs, `params_panel` owns these and does not
        // read them after the match, and the row factory has to own them anyway
        // (`MoonVirtualList` requires a `'static` closure). Borrowing here would clone each of
        // them a second time on every frame of a monitor-rate repaint.
        values: Values,
        row_pairs: Vec<(Key, StrategyRow)>,
        pending: HashMap<Key, StrategyEditRow>,
        // Unused: `MoonVirtualList` gives its `render_item` closure a fresh `Window` per call,
        // which is what compact rows actually build against (they never create a retained editor
        // state, so they never need this one). Kept in the signature to match the pane-method
        // shape every other body-dispatch branch shares.
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(section) = self.pending_param_scroll.take() {
            if let Some(ix) = resolve_scroll_target(&flat, section) {
                self.params_scroll
                    .scroll_to_item_strict(ix, ScrollStrategy::Top);
            }
        }

        let frozen = self.viewing_version();
        let row_h = full_row_h_value(cx, frozen);
        let entry_count = flat.entries.len();

        let factory_flat = flat;
        let factory_values = Rc::new(values);
        let factory_row_pairs = Rc::new(row_pairs);
        let factory_pending = Rc::new(pending);
        let factory_keys = Rc::new(keys.to_vec());
        let weak = cx.entity().downgrade();

        let list = MoonVirtualList::new(
            "strat-params-full",
            entry_count,
            row_h,
            move |ix, window, app| {
                let Some(entry) = factory_flat.entries.get(ix) else {
                    return div().into_any_element();
                };
                match entry {
                    ParamEntry::SectionHeader {
                        section,
                        title,
                        field_count,
                    } => {
                        let p = MoonPalette::active(app);
                        // Keyed on the schema section rather than the entry index so the heading's
                        // identity survives an unrelated section appearing or disappearing above it.
                        let id = match section {
                            Some(s) => format!("param-sec-{s}"),
                            None => "param-sec-orphan".to_string(),
                        };
                        h_flex()
                            .id(SharedString::from(id))
                            .w_full()
                            .items_center()
                            .justify_between()
                            .pb(design::ui_px(app, 2.0))
                            .pr(design::ui_px(app, design::MOON_SCROLLBAR_OVERLAY_W))
                            .border_b_1()
                            .border_color(moon(p.border))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(moon(p.text))
                                    .child(title.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(design::t_caption(app))
                                    .text_color(moon(p.text_muted))
                                    .child(t!("strat.fields_count", n = *field_count).to_string()),
                            )
                            .into_any_element()
                    }
                    ParamEntry::Field { section, field } => {
                        let field = field.clone();
                        let section = *section;
                        let values = factory_values.clone();
                        let row_pairs = factory_row_pairs.clone();
                        let pending = factory_pending.clone();
                        let keys = factory_keys.clone();
                        weak.update(app, |this, cx| {
                            let active = this.rules.field_active(&field.name, &values);
                            let merged = merged_value_for_owned(this, &row_pairs, &field, &pending);
                            let phase = field_pending_phase(&row_pairs, &pending, &field);
                            this.field_row(
                                &field,
                                &keys,
                                merged,
                                active,
                                phase,
                                Some(section),
                                window,
                                cx,
                            )
                        })
                        .unwrap_or_else(|_| div().into_any_element())
                    }
                }
            },
        )
        // Nothing in a param row is CONTROLLED by view state -- `MoonDropdown`'s menu is
        // uncontrolled and keyed on element id -- so an off-screen row simply is not built, and
        // there is no popup-eviction policy to wire (unlike the Connections list). This is a
        // deliberate omission, not a missing `on_visible_range`.
        .track_scroll(&self.params_scroll)
        .surface(false)
        .border(false)
        .radius(0.0)
        .padding(0.0)
        .scrollbar_visibility(MoonScrollbarVisibility::Always);

        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(list)
            .into_any_element()
    }
}
