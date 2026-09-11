//! The click that deletes a drawn figure.
//!
//! WHICH click is a setting — `hotkeys.fig_delete_click`, the mouse row beside the `fig_delete` key
//! in Settings → Hotkeys → Drawing, defaulting to the middle button as Moonbot does. Every button
//! routes here, so the setting can name a left, middle or right gesture and be obeyed. Two of the
//! values it offers cannot reach a figure, and that is a property of the layers above rather than
//! of this one: `Ctrl+Left` is taken by the drawing layer before the press arrives (on macOS the
//! secondary modifier is Command, so there it does arrive), and a right DOUBLE click is taken by
//! the figure menu, which opens on press one and whose overlay eats press two.

use gpui::{Context, Modifiers};

use super::super::trade::TradeMouseButton;
use super::ChartPanel;

impl ChartPanel {
    /// Whether this press is the one the delete gesture is bound to.
    ///
    /// Split from the act so the caller can tell "this press is mine, there was simply nothing to
    /// delete" from "this press is not mine at all": the first still owns the press — the series it
    /// belongs to was claimed — while the second must go on to trading and the menus untouched.
    ///
    /// Read from the settings PREVIEW when one is open, like every other live hotkey read: the
    /// gesture then answers to the row being edited, before it is saved.
    pub(in crate::panels::chart) fn fig_delete_gesture(
        &self,
        button: TradeMouseButton,
        modifiers: Modifiers,
        click_count: usize,
        cx: &Context<Self>,
    ) -> bool {
        let b = self.backend.read(cx);
        let cfg = b.preview.as_ref().unwrap_or(&b.config);
        // The same matcher the trading gestures use, so one press cannot mean this on one button
        // and something else by a different rule on another.
        Self::gesture_matches(cfg.hotkeys.fig_delete_click, button, modifiers, click_count)
    }

    /// Delete the figure under the cursor for the configured mouse gesture.
    ///
    /// Hit tested at the CURSOR, exactly like the right-click menu, and NOT against `fig_selected`:
    /// pointing at the figure is the whole gesture. Requiring a selection first would make this a
    /// two-step act, which is what the keyboard route already is — `HotkeyAction::FigDelete` has no
    /// cursor position and can only work off a selection.
    ///
    /// Deletes only a figure this chart DREW. Two neighbours look identical on screen and are not:
    /// a figure another core shares onto this market lives there, and deleting it destroys the
    /// original for every core; a `from_server` figure is Moonbot's own chart object, and removing
    /// it fires `chart_alert_delete` at the core. Both are reachable — deliberately, and only —
    /// through the right-click menu, where the act is a named item that can be read before it is
    /// clicked, and `undo_last_figure` refuses them outright for the same reason. A click that
    /// says nothing does neither.
    ///
    /// A press landing on such a figure is still CONSUMED. Falling through would send the same
    /// gesture, aimed at a same-looking line, to order placement — delete on one, a live order on
    /// the other. Over a figure this button trades on nothing.
    ///
    /// Returns whether the press was consumed.
    pub(in crate::panels::chart) fn try_fig_delete_click(
        &mut self,
        button: TradeMouseButton,
        modifiers: Modifiers,
        click_count: usize,
        pos: (f32, f32),
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.fig_delete_gesture(button, modifiers, click_count, cx) {
            return false;
        }
        // Mid-draft and mid-drag the press belongs to the figure being placed or moved, not to an
        // old one lying under the cursor — and deleting the dragged figure would strand `fig_drag`
        // on a dead id, still published as `dragging` and re-upserted on release.
        //
        // The historical viewer is NOT excluded: the figures it draws over a replayed market are
        // that market's live ones, which drawing, dragging, the menu's Delete and the Delete key
        // all edit there. Only trading refuses in that window, and this is not trading.
        if self.fig_draft.is_some() || self.fig_drag.is_some() {
            return false;
        }
        // While Sells-to-zone is armed the LEFT button is a drawing surface — `try_fig_click`
        // refuses to grab there for the same reason — but the mode has no claim on the others: a
        // middle-bound delete going silently dead the moment a band is being aimed reads as a bug,
        // not as a mode.
        if button == TradeMouseButton::Left && self.sells_zone_armed(cx) {
            return false;
        }
        let Some((core, market, id)) = self.fig_hit_key_at(pos, cx) else {
            return false;
        };
        // The store borrow is dropped before `remove_figure` takes the same `RefCell` mutably: a
        // `Ref` still alive across that call panics inside the frame loop.
        let ours = {
            let b = self.backend.read(cx);
            let store = b.figures.borrow();
            store.is_local(core, &market, id)
        };
        if !ours {
            return true;
        }
        // Cleared BEFORE the removal, so the republish that the notify below triggers already sees
        // it gone. Nothing moves the pointer after a click, so a hover still naming the deleted
        // figure would keep its highlight and readout on screen — and the probe position goes with
        // it, or a figure lying under the deleted one would wait for the cursor to travel before it
        // lights up.
        if self.fig_hover == Some(id) {
            self.fig_hover = None;
            self.fig_hover_probe = None;
        }
        self.backend.update(cx, |b, bcx| {
            b.remove_figure(core, &market, id);
            // The repaint rides this notify, and only it: the observer republishes the figure
            // layer in THIS window and in every other one showing this market, and closes a style
            // panel left open on the figure. A `sync_fig_visual` here would publish a third time on
            // the same tick and, on the panel that did the delete, still not be the one that
            // reaches the screen.
            bcx.notify();
        });
        true
    }
}
