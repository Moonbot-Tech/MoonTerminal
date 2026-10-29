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
use std::time::Instant;

use super::param_entries::{FlatParams, ParamEntry};
use super::*;

/// One full-mode scroll the next render should apply.
pub(super) enum ParamScroll {
    /// Schema section heading, placed at the top of the list.
    Section(usize),
    /// Flat entry index of a field row, placed in the center.
    Field(usize),
}

/// A field reveal the parameter pane is showing, optionally carrying a blacklist coin.
///
/// The outline fades. The token highlight and the remove button stay until the coin is gone
/// from the field, the removal is accepted, or another strategy is selected. An empty coin
/// requests the outline alone.
#[derive(Clone)]
pub(super) struct FieldFlash {
    pub(super) key: String,
    pub(super) coin: String,
    /// When the outline started. The alpha is derived from this, not from a frame counter.
    pub(super) started: Instant,
    /// Strategy that was selected when the reveal landed. A different selection drops the flash.
    pub(super) strategy: Option<Key>,
    /// Refusal text shown beside the remove button. `None` after a clean frame.
    pub(super) error: Option<String>,
}

/// A field reveal waiting until the selected strategy's schema can name the field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PendingFieldReveal {
    /// Schema field and coin token to land on.
    pub field: super::RevealField,
    /// Strategy the reveal named. A different selection drops it.
    pub strategy: Key,
}

/// What to do with a pending field reveal given the strategy now selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FieldRevealDecision {
    /// The selected strategy is the one the reveal named.
    Consume,
    /// The user selected a different strategy. Drop the request.
    Drop,
    /// Nothing is selected yet. Leave the request pending.
    Wait,
}

/// Match a pending field reveal against the strategy on screen.
///
/// Args:
///     selected: Strategy the params pane is showing, if one is selected.
///     target: Strategy the reveal was stored for.
///
/// Returns:
///     Consume when they are the same strategy, drop when the user picked another, wait when
///     the pane has no selection yet.
pub(super) fn field_reveal_decision(selected: Option<Key>, target: Key) -> FieldRevealDecision {
    match selected {
        Some(key) if key == target => FieldRevealDecision::Consume,
        Some(_) => FieldRevealDecision::Drop,
        None => FieldRevealDecision::Wait,
    }
}

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

/// Find a field row in a full-mode flatten.
///
/// The first matching name wins, which is the same rule `flatten_params` uses when a name occurs
/// in two sections. An orphan row, present only on a version diff, comes back with `section: None`.
///
/// Args:
///     flat: Flattened headings and field rows.
///     key: Schema field name, compared exactly.
///
/// Returns:
///     The owning section and the entry index of that field row. `None` when the name is absent.
pub(super) fn resolve_field_target(flat: &FlatParams, key: &str) -> Option<(Option<usize>, usize)> {
    flat.entries.iter().enumerate().find_map(|(ix, entry)| {
        let ParamEntry::Field { section, field } = entry else {
            return None;
        };
        (field.name == key).then_some((*section, ix))
    })
}

/// Section index of a field, read from the schema itself.
///
/// Per-section mode uses this instead of a flatten. A changed-only filter can hide the field
/// from that flatten while the section that declares it is still in the schema. The first
/// section that contains the name wins.
///
/// Args:
///     sections: Schema sections of the selected strategy kind.
///     key: Schema field name, compared exactly.
///
/// Returns:
///     The index of the first section that contains `key`, or `None` when none does.
pub(super) fn resolve_owning_section(
    sections: &[moon_core::feed::SchemaSection],
    key: &str,
) -> Option<usize> {
    sections
        .iter()
        .position(|section| section.fields.iter().any(|field| field.name == key))
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
        self.pending_param_scroll = Some(ParamScroll::Section(section));
        cx.notify();
    }

    /// Drop a flash whose strategy is no longer the one on screen.
    fn retire_stale_flash(&mut self) {
        let current = selected_key(self);
        if self
            .field_flash
            .as_ref()
            .is_some_and(|flash| flash.strategy != current)
        {
            self.field_flash = None;
        }
    }

    /// Turn a pending field reveal into a scroll plus a flash, once the schema can name the field.
    ///
    /// A missing schema leaves the request pending so the next render can try again. A schema that
    /// has arrived and does not contain the key drops the request. Per-section mode selects the
    /// owning section by field name and ignores the changed-only filter. Full mode flattens with
    /// that filter and queues a centered scroll to the field's entry.
    ///
    /// Args:
    ///     store: Live store that holds the selected strategy's schema.
    ///
    /// Returns:
    ///     Whether the selected section changed and should be persisted. Persistence is the
    ///     caller's job: it writes the backend, and this method still holds a store borrow.
    pub(super) fn prepare_field_reveal(&mut self, store: &CoreStore) -> bool {
        self.retire_stale_flash();
        let Some(pending) = self.pending_field_reveal.clone() else {
            return false;
        };
        match field_reveal_decision(selected_key(self), pending.strategy) {
            FieldRevealDecision::Wait => return false,
            FieldRevealDecision::Drop => {
                self.pending_field_reveal = None;
                return false;
            }
            FieldRevealDecision::Consume => {}
        }
        let reveal = pending.field;
        let Some(sections) = selected_sections(self, store) else {
            return false;
        };
        let mut section_changed = false;
        if self.prefs.params_full {
            let multi = multi_row_pairs(self, store).len() > 1;
            let common = common_fields(self, store);
            let differ = kinds_differ(self, store);
            let orphans = t!("strat.params_other_fields").to_string();
            let flat = param_entries::flatten_params(
                sections,
                self.version_changed_filter(),
                multi,
                common.as_ref(),
                differ,
                param_entries::ParamLabels {
                    orphans: &orphans,
                    section_title: &|raw| {
                        super::section_display_title(raw, self.prefs.human_labels)
                    },
                },
            );
            let Some((_, ix)) = resolve_field_target(&flat, reveal.key) else {
                self.pending_field_reveal = None;
                return false;
            };
            self.pending_param_scroll = Some(ParamScroll::Field(ix));
        } else {
            let Some(section) = resolve_owning_section(sections, reveal.key) else {
                self.pending_field_reveal = None;
                return false;
            };
            if self.selected_section != section {
                self.selected_section = section;
                section_changed = true;
            }
            self.pending_section_scroll = Some(reveal.key.to_string());
        }
        self.field_flash = Some(FieldFlash {
            key: reveal.key.to_string(),
            coin: reveal.coin,
            started: Instant::now(),
            strategy: selected_key(self),
            error: None,
        });
        self.pending_field_reveal = None;
        section_changed
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
        row_pairs: Vec<(Key, Arc<StrategyRow>)>,
        pending: HashMap<Key, StrategyEditRow>,
        // Unused: `MoonVirtualList` gives its `render_item` closure a fresh `Window` per call,
        // which is what compact rows actually build against (they never create a retained editor
        // state, so they never need this one). Kept in the signature to match the pane-method
        // shape every other body-dispatch branch shares.
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.pending_param_scroll.take() {
            Some(ParamScroll::Section(section)) => {
                if let Some(ix) = resolve_scroll_target(&flat, section) {
                    self.params_scroll
                        .scroll_to_item_strict(ix, ScrollStrategy::Top);
                }
            }
            Some(ParamScroll::Field(ix)) => {
                self.params_scroll
                    .scroll_to_item_strict(ix, ScrollStrategy::Center);
            }
            None => {}
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

#[cfg(test)]
mod tests;
