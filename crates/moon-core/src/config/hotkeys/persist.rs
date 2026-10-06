//! Hotkey config loading, migrations, saving and share strings.

use super::*;

impl HotkeysConfig {
    /// Return the primary and secondary move gestures for one order line.
    ///
    /// `entry` selects the Buy leg against the exit legs (sell, stop, trailing, take profit), and
    /// `short` the position direction — Moonbot's four buckets. This is the ONE place that reads
    /// `same_hotkeys_for_move`: the settings panel also mirrors long values into the short fields as
    /// they are edited, but a shared or hand-edited file can carry the flag with stale short values,
    /// and the flag is what the user sees.
    pub fn move_gestures(&self, entry: bool, short: bool) -> [MouseGestureBinding; 2] {
        let short = short && !self.same_hotkeys_for_move;
        [false, true].map(|second| self.gesture(MoveKindSlot::row(entry, second).half(short)))
    }

    /// What one recognised move gesture has to send.
    ///
    /// Args:
    ///     matches: Whether a press being examined satisfies one binding. The caller owns the
    ///         platform's modifier type, so the comparison stays in the UI and only the ANSWER
    ///         comes back here.
    ///
    /// Returns:
    ///     The side of the book to move, the layout to move it into and the position side it
    ///     addresses, or `None` when no slot claims the press — including a slot whose kind is
    ///     `None`, which is Moonbot's way of leaving a bound gesture inert. Rows are examined in
    ///     [`MoveKindSlot::ALL`]'s order — the order the settings page lists them — so a gesture
    ///     the user put on two of them resolves the same way twice rather than by whichever
    ///     branch happened to run first.
    pub fn resolve_move_gesture(
        &self,
        matches: impl Fn(MouseGestureBinding) -> bool,
    ) -> Option<MoveGestureCommand> {
        for row in MoveKindSlot::ALL {
            let entry = row.entry();
            let kind = self.move_kind(row);
            // Both sides come from `move_gestures`, which is the one place that reads
            // `same_hotkeys_for_move`: with the mirror on it hands back the long gesture for the
            // short side too, so one press claims both and the core is told `Both`.
            let ix = usize::from(row.second());
            let long = self.move_gestures(entry, false)[ix];
            let short = self.move_gestures(entry, true)[ix];
            let hit_long = long != MouseGestureBinding::None && matches(long);
            let hit_short = short != MouseGestureBinding::None && matches(short);
            let side = match (hit_long, hit_short) {
                (true, true) => MoveSide::Both,
                (true, false) => MoveSide::Long,
                (false, true) => MoveSide::Short,
                (false, false) => continue,
            };
            // Moonbot's way of switching one gesture off without clearing its binding. `continue`
            // rather than `return`: another slot may hold the same binding WITH a kind, and giving
            // up here would let a disabled row silence a working one.
            if kind == MoveKind::None {
                continue;
            }
            return Some(MoveGestureCommand {
                sell: !entry,
                kind,
                side,
            });
        }
        None
    }

    /// Part count for the `Split N` action, clamped to [`SPLIT_PARTS_MIN`]..=[`SPLIT_PARTS_MAX`].
    ///
    /// Callers use this instead of the raw field: the value reaches a live trading command, and
    /// both a hand-edited `hotkeys.toml` and a Moonbot import can carry anything a `u8` holds.
    pub fn split_n_parts(&self) -> i32 {
        i32::from(self.split_parts.clamp(SPLIT_PARTS_MIN, SPLIT_PARTS_MAX))
    }

    /// Reads `hotkeys.toml`. `None` means the file does not exist yet (first launch after moving
    /// hotkeys out of settings.toml; the caller migrates the legacy section and writes the file).
    /// A corrupt file yields the default (and logs internally), NOT `None`; otherwise the corrupt
    /// file would be silently overwritten by the stale legacy copy from settings.toml.
    pub fn load() -> Option<Self> {
        let path = paths::hotkeys_path();
        if !path.exists() {
            return None;
        }
        let mut cfg: Self = super::toml_io::load_or_default(&path, "hotkeys.toml", |_| {});
        // Persist the stamp right here, or "runs once" is a promise the next launch breaks: the
        // generation would live in memory until some unrelated settings save happened to write it,
        // and until then every launch would refill a slot the user cleared.
        if cfg.fill_unbound_slots() {
            if let Err(error) = cfg.save() {
                log::warn!("hotkeys.toml migration not persisted: {error:#}");
            }
        }
        Some(cfg)
    }

    /// Brings a file written by an older build up to [`SCHEMA`], one generation at a time.
    ///
    /// Generation 0 → 1: the actions Moonbot binds by default shipped here UNBOUND, so a file from
    /// that build has empty strings where a new install now has Moonbot's key. Those empties are
    /// filled ONCE. It has to be once: a user is free to clear a hotkey, and a fill that ran on
    /// every load would hand it back on the next launch. Only empty slots are touched, so a key the
    /// user chose is never overwritten — and a shipped key ALREADY IN USE elsewhere in this file is
    /// skipped rather than duplicated, because a duplicate resolves by branch order in the
    /// dispatcher and would silently turn, say, a manual-strategy Alt+1 into a live long order.
    ///
    /// Generation 1 → 2: clear `chart_shot` where Ctrl+F10 was already the user's key for something
    /// else. A NEW field never needs backfilling — serde's default fills it — but it does need that
    /// collision check, and running it must NOT drag generation 1 along behind it, which is why
    /// each arm is gated on its own predecessor rather than on the aggregate.
    ///
    /// Generation 2 → 3: the same check for `fig_undo`, which arrives on Ctrl+Z the same way.
    ///
    /// Generation 3 → 4: the first arm about a GESTURE rather than a key, and about meaning rather
    /// than collision — the two pending-order gestures went from saved-but-inert to placing a live
    /// order, so a value chosen while the row said it did nothing is cleared.
    ///
    /// Generation 4 → 5: the collision check generations 2 and 3 run for an arriving key, run for
    /// an arriving gesture — `fig_delete_click` yields its Middle default where Middle already
    /// trades. Its own generation rather than a widening of 4, because files stamped 4 exist.
    ///
    /// Generation 5 → 6: the key check once more, for `center_chart` arriving on Ctrl+Right.
    ///
    /// Generation 6 → 7: the same check for `toggle_live` arriving on Space.
    ///
    /// Returns whether anything changed, so the caller can persist the stamp.
    pub(in crate::config) fn fill_unbound_slots(&mut self) -> bool {
        if self.schema >= SCHEMA {
            return false;
        }
        // Each generation is gated on its OWN predecessor, never on `schema < SCHEMA` as a whole:
        // a file already at generation 1 must NOT have the empty-slot backfill run over it again,
        // or every key its owner has deliberately cleared since comes back on the next launch.
        if self.schema < 1 {
            self.fill_generation_1();
        }
        if self.schema < 2 {
            self.clear_generation_2_collisions();
        }
        if self.schema < 3 {
            self.clear_generation_3_collisions();
        }
        if self.schema < 4 {
            self.clear_generation_4_pending_gestures();
        }
        if self.schema < 5 {
            self.clear_generation_5_figure_gesture();
        }
        if self.schema < 6 {
            self.clear_generation_6_collisions();
        }
        if self.schema < 7 {
            self.clear_generation_7_collisions();
        }
        self.schema = SCHEMA;
        true
    }

    /// Generation 0 -> 1: backfill the slots that shipped unbound.
    ///
    /// Returns:
    ///     Nothing; updates only slots that were empty in a generation-0 file.
    fn fill_generation_1(&mut self) {
        let defaults = Self::default();
        let taken = self.bound_keys();
        // `count` is how many slots already hold this key. A candidate for an EMPTY slot may hold
        // none; a slot serde has already filled from a NEW field's default holds one — its own —
        // and anything above that is a real collision.
        let occurrences = |key: &str| taken.iter().filter(|held| held.as_str() == key).count();
        clear_if_duplicate(&taken, &mut self.sells_to_rect, "Sells to rectangle");
        // ONLY the slots that shipped unbound. A slot that always had a key (panic_sell_one,
        // cancel_all_buys, switch_figure) is empty for exactly one reason — the user cleared it —
        // and filling it would take that choice back. Those keep their old value; the Moonbot key
        // is what a fresh install gets.
        for (slot, shipped) in [
            (&mut self.cancel_buy, defaults.cancel_buy),
            (&mut self.panic_sell, defaults.panic_sell),
            (&mut self.join_sells, defaults.join_sells),
            (&mut self.switch_charts, defaults.switch_charts),
            (&mut self.new_long, defaults.new_long),
            (&mut self.new_short, defaults.new_short),
            (&mut self.split_order, defaults.split_order),
            (&mut self.split_order_x, defaults.split_order_x),
            (&mut self.sells_to_rect, defaults.sells_to_rect),
            (&mut self.shift_buy_up, defaults.shift_buy_up),
            (&mut self.shift_buy_down, defaults.shift_buy_down),
            (&mut self.shift_sell_up, defaults.shift_sell_up),
            (&mut self.shift_sell_down, defaults.shift_sell_down),
        ] {
            if slot.trim().is_empty() && occurrences(&shipped) == 0 {
                *slot = shipped;
            }
        }
    }

    /// Generation 1 -> 2: `chart_shot` arrives pre-filled by its serde default, so it never reaches
    /// generation 1's empty-slot loop and would keep Ctrl+F10 even where the user had already given
    /// that keystroke to another action.
    ///
    /// The duplicate is not harmless: `resolve_binding` answers the FIRST matching branch, and the
    /// chart shot is resolved above every trading action, so the shipped default would quietly take
    /// a key that used to send an order. Clearing the NEW slot rather than the old one keeps the
    /// user's own choice, which is the same trade generation 1 made for `sells_to_rect`.
    ///
    /// Returns:
    ///     Nothing; clears only the new chart-shot slot when its default collides.
    pub(super) fn clear_generation_2_collisions(&mut self) {
        // Recomputed rather than reused: generation 1 may have just filled slots above.
        let taken = self.bound_keys();
        clear_if_duplicate(&taken, &mut self.chart_shot, "Make Shot");
    }

    /// Generation 2 -> 3: `fig_undo` ships on Ctrl+Z through its serde default, so it reaches an
    /// existing file already filled and never passes through generation 1's empty-slot loop.
    ///
    /// Ctrl+Z is free on every default we and Moonbot ship, but nothing stops a user from having
    /// given it to another action — and the figure layer resolves ABOVE the trading actions, so the
    /// arriving default would quietly take a key that used to send an order. The NEW slot is the
    /// one cleared, keeping the user's own choice, exactly as generations 1 and 2 did.
    ///
    /// Returns:
    ///     Nothing; clears only the new figure-undo slot when its default collides.
    pub(super) fn clear_generation_3_collisions(&mut self) {
        // Recomputed rather than reused: the generations above may have just changed slots.
        let taken = self.bound_keys();
        clear_if_duplicate(&taken, &mut self.fig_undo, "Delete last figure");
    }

    /// Generation 3 -> 4: the two pending-order gestures start FIRING.
    ///
    /// They shipped editable and saved, with the row itself saying the terminal did not send them
    /// yet — so anyone who set one was told, on that screen, that it did nothing. It now places a
    /// live pending order on the selected core. A value chosen under that promise is not a choice to
    /// trade, and the two are cleared once rather than waking up as a trading gesture; both ship
    /// unset, so this touches nobody who did not deliberately set one.
    ///
    /// Returns:
    ///     Nothing; clears only the two pending gesture slots.
    fn clear_generation_4_pending_gestures(&mut self) {
        for (slot, label) in [
            (&mut self.pending_long_click, "Pending Long"),
            (&mut self.pending_short_click, "Pending Short"),
        ] {
            if *slot != MouseGestureBinding::None {
                log::warn!(
                    "hotkeys.toml: {label} now places a real pending order; its gesture \
                     {slot:?} was cleared, set it again if that is what you want"
                );
                *slot = MouseGestureBinding::None;
            }
        }
    }

    /// Generation 4 -> 5: the arriving figure-delete gesture yields to a trading gesture the file
    /// already fires on the same button.
    ///
    /// `fig_delete_click` is the gesture counterpart of `chart_shot` and `fig_undo` in generations 2
    /// and 3: it reaches an existing file already filled by its serde default (Middle), and the
    /// figure layer is offered a press ABOVE every trading layer on that button. A user whose file
    /// already moves or places orders on Middle would find those presses deleting figures instead
    /// wherever one sits under the pointer. The NEW slot yields, exactly as the keys do, through
    /// [`Self::bound_gestures`] — the mirror of what generations 2 and 3 do through `bound_keys`.
    ///
    /// Returns:
    ///     Nothing; clears only the figure gesture, and only on a collision.
    fn clear_generation_5_figure_gesture(&mut self) {
        // Recomputed here rather than reused, like every key generation recomputes `bound_keys`:
        // generation 4 may have just cleared slots above.
        let taken = self.bound_gestures();
        let arriving = self.fig_delete_click;
        if arriving != MouseGestureBinding::None
            && taken.iter().filter(|held| **held == arriving).count() > 1
        {
            log::warn!(
                "hotkeys.toml: {arriving:?} is already a trading gesture, \
                 Delete figure was left without one"
            );
            self.fig_delete_click = MouseGestureBinding::None;
        }
    }

    /// Generation 5 -> 6: `center_chart` ships on Ctrl+Right through its serde default, so it
    /// reaches an existing file already filled, exactly as `chart_shot` and `fig_undo` did.
    ///
    /// Ctrl+Right is free on every default we and Moonbot ship, and the slot resolves among the
    /// chart keys ABOVE the trading actions — so a user who had given it to an order action would
    /// find that order replaced by a recentred chart. The NEW slot yields, as in generations 2 and 3.
    ///
    /// Returns:
    ///     Nothing; clears only the new centre-chart slot when its default collides.
    pub(super) fn clear_generation_6_collisions(&mut self) {
        // Recomputed rather than reused: the generations above may have just changed slots.
        let taken = self.bound_keys();
        clear_if_duplicate(&taken, &mut self.center_chart, "Center chart");
    }

    /// Generation 6 -> 7: `toggle_live` ships on Space through its serde default, so it reaches
    /// an existing file already filled, exactly as `center_chart` did.
    ///
    /// Space is free on every default we and Moonbot ship, but nothing stops a user from having
    /// given it to another action — and the chart keys resolve ABOVE the trading actions, so the
    /// arriving default would quietly take a key that used to send an order. The NEW slot yields.
    ///
    /// Returns:
    ///     Nothing; clears only the new toggle-live slot when its default collides.
    pub(super) fn clear_generation_7_collisions(&mut self) {
        let taken = self.bound_keys();
        clear_if_duplicate(&taken, &mut self.toggle_live, "Live / Pause");
    }

    /// The gesture one slot actually FIRES on, or `None` for a slot that dispatches nothing.
    ///
    /// Two ways to hold no press, and both read the same here: an unset slot, and a move row whose
    /// kind is `None`, which [`Self::resolve_move_gesture`] steps past on purpose. Mirror-aware
    /// through [`Self::gesture_in_effect`]. The one reading the migration, the settings page's
    /// clash index and its captions all take, so none of them can call a row live that another
    /// calls inert.
    pub fn firing_gesture(&self, slot: GestureSlot) -> Option<MouseGestureBinding> {
        let gesture = self.gesture_in_effect(slot);
        let inert = slot
            .move_half()
            .is_some_and(|half| self.move_kind(half.row) == MoveKind::None);
        (gesture != MouseGestureBinding::None && !inert).then_some(gesture)
    }

    /// Every gesture this file fires, for collision checks — the counterpart of
    /// [`Self::bound_keys`].
    ///
    /// Through [`Self::firing_gesture`], so a mirrored-away short field and an inert move row are
    /// left out: counting either would report a collision nobody can press. A gesture held by two
    /// slots appears twice, which is what makes a duplicate visible to the caller. Unlike the keys,
    /// these compare exactly — a gesture is an enum, not a string with spellings.
    pub fn bound_gestures(&self) -> Vec<MouseGestureBinding> {
        GestureSlot::all()
            .into_iter()
            .filter_map(|slot| self.firing_gesture(slot))
            .collect()
    }

    /// Every keystroke this file already binds, for collision checks.
    ///
    /// Includes the preset and manual-strategy arrays: those are exactly where a user's own
    /// `alt-1` is most likely to sit. A key held by two slots appears twice, which is what makes a
    /// duplicate visible to the caller.
    ///
    /// Raw strings, exactly as stored — this crate cannot parse them and deliberately does not try.
    /// A caller that must decide whether two of these are the SAME PRESS compares them through
    /// `crate::hotkeys::binding_id` on the UI side, as the core pull's conflict gate does; a caller
    /// that compares them literally, as [`clear_if_duplicate`] does, misses a differently-spelled
    /// duplicate and says so where it is written.
    pub fn bound_keys(&self) -> Vec<String> {
        KeySlot::all()
            .into_iter()
            .map(|slot| self.key(slot).trim().to_string())
            .filter(|key| !key.is_empty())
            .collect()
    }

    /// Writes `hotkeys.toml` (open, human-readable TOML that can be shared).
    pub fn save(&self) -> anyhow::Result<()> {
        super::toml_io::save(&paths::hotkeys_path(), self, "hotkeys.toml")
    }

    /// Text in hotkeys.toml format for "Copy" in Settings (= file contents).
    pub fn to_share_string(&self) -> Option<String> {
        toml::to_string_pretty(self).ok()
    }

    /// Parses hotkeys.toml text (clipboard paste / file contents). Validates using distinctive
    /// keys; serde ignores unknown fields and would silently produce the default for a foreign file.
    /// `None` means the text is not a hotkey configuration.
    pub fn parse_share(text: &str) -> Option<Self> {
        const KEYS: [&str; 4] = ["order_size", "sell_preset", "buy_set_click", "draw_hline"];
        let v: toml::Value = toml::from_str(text).ok()?;
        if v.as_table()
            .is_some_and(|t| KEYS.iter().any(|k| t.contains_key(*k)))
        {
            // Pasted text is a file like any other and gets the same one-time fills: a set shared
            // from an older build otherwise arrives with its new slots unbound.
            let mut cfg: Self = toml::from_str(text).ok()?;
            cfg.fill_unbound_slots();
            Some(cfg)
        } else {
            None
        }
    }
}

/// Clear `field` when the keystroke it holds is ALREADY bound elsewhere in this file.
///
/// Every generation of [`HotkeysConfig::fill_unbound_slots`] needs this and for the same reason: a
/// field ADDED in that generation arrives pre-filled by its serde default, so it never reaches the
/// empty-slot loop and would keep a keystroke its owner has given to something else. A duplicate
/// resolves by branch order in the dispatcher, so the shipped default would silently shadow the
/// user's own binding — which is why the NEW slot is the one that yields, never the old one.
///
/// `taken` is a [`HotkeysConfig::bound_keys`] snapshot, in which the field's OWN key already counts
/// once; anything above one occurrence is a real collision. An empty field binds nothing and is
/// left alone.
///
/// STILL NOT REDUNDANT, now that the settings page captions duplicates by name. The two answer
/// different questions and only one of them is about the user: the page reports a duplicate the USER
/// created and leaves it alone, because their binding is their business; this clears a default WE
/// ship into a slot that did not exist in their file yet, which they never chose and would only
/// discover by noticing an old key had stopped working. A caption cannot serve the second case — it
/// is only read by someone who opens the page, and by then the key is already stolen.
///
/// LIMIT, and it is a real one: this compares the STRINGS, while the dispatcher compares the press
/// (`crate::hotkeys::binding_id` in the UI crate is the definition). `Keystroke::parse` is
/// case-insensitive, takes the modifiers in any order, and reads `cmd`/`super`/`win` as one
/// modifier, so a file whose key is spelled `Ctrl-F10` or `ctrl-alt-win-shift-k` hides a real
/// collision from this check and the shipped default takes the key after all. Not fixed here on
/// purpose: this crate has no keystroke parser, and writing a second one to compare with would
/// invite exactly the drift it is meant to catch. The honest fix is to hand the comparison in from
/// the crate that owns the parser; `docs-internal/HOTKEYS_UNIFIED_PLAN.md` carries it as debt.
///
/// Args:
///     taken: Snapshot of all already-bound, non-empty keystrokes.
///     field: Newly introduced binding that yields to an existing collision.
///     label: User-facing name included in the collision warning.
///
/// Returns:
///     Nothing; clears `field` only when its key occurs more than once in `taken`.
fn clear_if_duplicate(taken: &[String], field: &mut String, label: &str) {
    let key = field.trim();
    if key.is_empty() || taken.iter().filter(|held| held.as_str() == key).count() <= 1 {
        return;
    }
    log::warn!(
        "hotkeys.toml: {} is already taken, {} was left without a key",
        field,
        label
    );
    field.clear();
}
