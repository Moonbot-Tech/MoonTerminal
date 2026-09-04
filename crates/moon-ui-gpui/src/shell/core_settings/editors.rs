//! Retained editors and sliders for a staged [`CoreConfig`] page, shared by both faces of the
//! core-settings gear.
//!
//! Two surfaces stage that page — the compact popup in [`crate::shell::core_settings_popup`] and
//! the expert window in [`crate::core_expert`] — and both need the same two things from a control:
//! it must be CREATED once, on the first render of its row, and it must take the core's values back
//! exactly when the page is re-seeded rather than on every repaint. Getting either wrong is
//! visible: building every editor up front costs each session that never opens the surface, and
//! re-synchronizing on each repaint rewrites `5.5` as `5.50` mid-word and moves the caret while the
//! user types.
//!
//! The host owns the store and says how a change reaches its draft; everything else lives here, so
//! the two surfaces cannot drift apart on the rule that decides when a control follows the core.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use gpui::*;
use moon_ui::{MoonInputEvent, MoonInputState, MoonSliderEvent, MoonSliderState};

use moon_core::feed::CoreConfig;

/// Editors and sliders one surface has built so far.
#[derive(Default)]
pub(crate) struct EditorStore {
    /// Numeric and text editors by row id. Each entry remembers the [`Self::generation`] it last
    /// took a value from, so a re-seed reaches it exactly once instead of on every repaint.
    inputs: HashMap<&'static str, (u64, Entity<MoonInputState>)>,
    /// Sliders by row id. Unlike the editors they follow the draft on every render, so they need no
    /// generation of their own: a slider has no half-typed state to protect.
    sliders: HashMap<&'static str, Entity<MoonSliderState>>,
    /// Advances whenever the surface's draft is seeded from a core.
    generation: u64,
}

impl EditorStore {
    /// Note that the draft was (re-)seeded, which is what tells the retained editors to take the
    /// core's values back.
    pub(crate) fn reseeded(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// Drop every control.
    ///
    /// Called with the draft: an editor retained past its draft would seed the NEXT core's row with
    /// the previous core's text on its first frame, before the generation check had a value to
    /// correct it with.
    pub(crate) fn clear(&mut self) {
        self.inputs.clear();
        self.sliders.clear();
        // The generation moves with the drop: a rebuilt editor must never match a generation an
        // earlier one had already seen, whatever order the host clears and re-seeds in.
        self.reseeded();
    }

    /// The editor built for one row, if that row has rendered.
    pub(crate) fn input(&self, id: &str) -> Option<Entity<MoonInputState>> {
        self.inputs.get(id).map(|(_, state)| state.clone())
    }

    /// Whether any control has been built. A store that never held one cannot have held focus.
    pub(crate) fn is_empty(&self) -> bool {
        self.inputs.is_empty() && self.sliders.is_empty()
    }

    /// The slider built for one row, if that row has rendered.
    ///
    /// A row asking for a control that was never declared renders WITHOUT it rather than panicking
    /// mid-frame: the mismatch is a page listing a row in its body but not in its specs.
    pub(crate) fn slider(&self, id: &str) -> Option<Entity<MoonSliderState>> {
        self.sliders.get(id).cloned()
    }
}

/// A view that stages one `CoreConfig` page and owns the controls editing it.
pub(crate) trait CoreDraftHost: Sized + 'static {
    /// The controls this surface has built.
    fn editors(&mut self) -> &mut EditorStore;

    /// Apply one staged change to the surface's draft. A change arriving without a draft is dropped
    /// rather than creating one.
    fn stage_draft(&mut self, apply: impl FnOnce(&mut CoreConfig), cx: &mut Context<Self>);

    /// The window this surface draws in, used to write a field from a slider drag:
    /// `MoonInputState::set_value` needs a `&mut Window`, which an event handler does not have.
    fn editor_window(&self) -> AnyWindowHandle;
}

/// Retained editor for one numeric or text field, created on first render of that field.
///
/// An existing editor is written to ONLY when the draft has been re-seeded since it last saw one —
/// Cancel, a core switch, or the core's first configuration arriving. It deliberately does not
/// follow the draft the way `strategies::StrategiesView::field_input_state` follows its staged
/// value: there the staged text IS what the user typed, so the round trip is an identity, while
/// here the draft holds a PARSED value that formats back differently. Re-synchronizing from it on
/// every repaint would rewrite "5.5" as "5.50" mid-word, refill a field the user just cleared, and
/// — since `sync_value` collapses the selection to the end — move the caret while they type.
///
/// Args:
///     view: Surface that owns the store and the draft.
///     id: Stable row id the control is remembered under.
///     value: The draft's current value, formatted for display.
///     stage: How a typed value reaches the draft.
///     window: Window used to build the editor.
///     cx: View context.
///
/// Returns:
///     The editor for that row.
pub(crate) fn input_state<V: CoreDraftHost>(
    view: &mut V,
    id: &'static str,
    value: String,
    stage: fn(&mut CoreConfig, &str),
    window: &mut Window,
    cx: &mut Context<V>,
) -> Entity<MoonInputState> {
    let generation = view.editors().generation;
    if let Some((seen, state)) = view.editors().inputs.get_mut(id) {
        let state = state.clone();
        let stale = *seen != generation;
        *seen = generation;
        if stale && state.read(cx).value() != value {
            state.update(cx, |s, c| s.sync_value(value, c));
        }
        return state;
    }
    let state = cx.new(|c| MoonInputState::new(window, c).default_value(value));
    cx.subscribe(
        &state,
        move |this: &mut V, state, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                let text = state.read(cx).value().to_string();
                this.stage_draft(|draft| stage(draft, &text), cx);
            }
        },
    )
    .detach();
    view.editors()
        .inputs
        .insert(id, (generation, state.clone()));
    state
}

/// Retained slider for one numeric field, created on first render of that row.
///
/// Unlike the editors, sliders DO follow the draft on every render (skipped mid-drag), because a
/// slider has no partially typed state to protect and its thumb would otherwise ignore a value
/// changed elsewhere — by Cancel, by a re-seed, or by the field beside it.
/// `MoonSliderState::set_value` emits no Change, so this cannot loop back through the staging
/// subscription below.
///
/// Args:
///     view: Surface that owns the store and the draft.
///     id: Stable row id the control is remembered under.
///     bounds: `(minimum, maximum, step)` of the CONTROL — nothing clamps what the core is sent
///         beyond what the slider itself can reach.
///     value: The draft's current value.
///     stage: How a dragged value reaches the draft.
///     mirror: Row id of the numeric editor showing the same value, if the row has one.
///     window: Window used to build the slider.
///     cx: View context.
///
/// Returns:
///     The slider for that row.
#[allow(clippy::too_many_arguments)]
pub(crate) fn slider_state<V: CoreDraftHost>(
    view: &mut V,
    id: &'static str,
    bounds: (f32, f32, f32),
    value: f32,
    stage: fn(&mut CoreConfig, f32),
    mirror: Option<&'static str>,
    window: &mut Window,
    cx: &mut Context<V>,
) -> Entity<MoonSliderState> {
    let (min, max, step) = bounds;
    // A value the wire made non-finite is DISPLAYED as the point closest to zero the control can
    // reach — the least eventful reading of "no value" on a range that may run either way. Clamping
    // alone cannot do it: `f32::clamp` returns NaN for a NaN input, `NaN != NaN` is true, and the
    // render-time sync below would then fire — with its repaint — on every frame for the life of
    // the surface.
    //
    // Display only: the draft keeps whatever the core sent, so OK returns that value rather than
    // this substitute. Deciding what a non-finite number MEANS belongs to the projection, not to a
    // slider.
    let value = if value.is_finite() {
        value.clamp(min, max)
    } else {
        0.0f32.clamp(min, max)
    };
    match view.editors().sliders.entry(id) {
        Entry::Occupied(slot) => {
            let state = slot.get().clone();
            // `MoonSlider` drags through GPUI's drag payload (`on_drag(DragThumb…)`), so this guard
            // is live: it stops the render-time sync from writing over the value a drag is
            // producing. It is app-WIDE — any drag anywhere suppresses it — which is harmless
            // because the sync only ever restores what the draft already holds.
            if !cx.has_active_drag() && state.read(cx).value().end() != value {
                state.update(cx, |s, c| s.set_value(value, window, c));
            }
            state
        }
        Entry::Vacant(slot) => {
            let state = cx.new(|_| {
                MoonSliderState::new()
                    .min(min)
                    .max(max)
                    .step(step)
                    .default_value(value)
            });
            slot.insert(state.clone());
            cx.subscribe(
                &state,
                move |this: &mut V, _state, ev: &MoonSliderEvent, cx| {
                    if let MoonSliderEvent::Change(v) = ev {
                        let v = v.end();
                        this.stage_draft(|draft| stage(draft, v), cx);
                        // A row that also shows the value in an editor writes it there directly:
                        // the editor only re-reads the draft on a re-seed (so typing survives), and
                        // without this the number would contradict the thumb being dragged. Every
                        // such pair is a whole count — an error level, a ping in milliseconds — so a
                        // rounded integer is the whole formatting rule.
                        let Some(field) = mirror.and_then(|m| this.editors().input(m)) else {
                            return;
                        };
                        let text = format!("{}", v.round() as i64);
                        // Deferred through the window handle because `MoonInputState::set_value`
                        // requires a `&mut Window`, which this handler does not have.
                        let handle = this.editor_window();
                        cx.defer(move |app| {
                            let _ = handle.update(app, move |_, window, app| {
                                field.update(app, |st, c| st.set_value(text, window, c));
                            });
                        });
                    }
                },
            )
            .detach();
            state
        }
    }
}
