//! Per-kind chart preferences and default inheritance.

use super::*;

impl WindowLayout {
    /// The candle settings a tab of this kind opens with.
    pub fn candle_view_for(
        &self,
        kind: super::chart_defaults::ChartTabKind,
    ) -> crate::market::candles::CandleViewCfg {
        self.kind_defaults(kind)
            .and_then(|d| d.candle_view)
            .unwrap_or(self.candle_view)
    }

    /// The chart graphics a tab of this kind opens with.
    pub fn chart_graphics_for(
        &self,
        kind: super::chart_defaults::ChartTabKind,
    ) -> ChartGraphicsCfg {
        self.kind_defaults(kind)
            .and_then(|d| d.chart_graphics)
            .unwrap_or(self.chart_graphics)
    }

    /// The captions a tab of this kind opens with.
    pub fn chart_labels_for(
        &self,
        kind: super::chart_defaults::ChartTabKind,
    ) -> &super::chart_labels::ChartLabelsCfg {
        // Stored, else the kind's own shipped set, else Main's — one chain, with "a kind may ship
        // its own captions" living on the KIND rather than as a branch here. The shipped set is
        // built once and shared: this is read on every settings comparison, and a fresh clone per
        // read would make a panel's signature differ from itself.
        self.kind_defaults(kind)
            .and_then(|d| d.chart_labels.as_ref())
            .or_else(|| kind.builtin_labels())
            .unwrap_or(&self.chart_labels)
    }

    /// The captions one kind holds OF ITS OWN, or `None` while it follows something else.
    ///
    /// A narrower question than [`Self::chart_labels_for`], and the one a migration has to ask: that
    /// one resolves the chain and always answers, so a caller using it could not tell a kind that
    /// stores a set from one that is merely following Main — and would freeze the follower on a
    /// copy by writing the resolved value back.
    pub fn stored_chart_labels(
        &self,
        kind: super::chart_defaults::ChartTabKind,
    ) -> Option<&super::chart_labels::ChartLabelsCfg> {
        self.kind_defaults(kind)?.chart_labels.as_ref()
    }

    /// Every chart-graphics default this layout STORES, to be edited in place: the base one, which
    /// is Main's, and then each kind that holds a value of its own. A kind that follows Main is not
    /// visited — it has nothing of its own, and the base already speaks for it.
    ///
    /// For a pass that must reach whatever a tab of ANY kind would open with, without splitting the
    /// kinds apart the way [`Self::set_chart_graphics_default`] does: writing through that one would
    /// freeze every following kind on a copy of Main.
    pub fn stored_chart_graphics_mut(&mut self) -> impl Iterator<Item = &mut ChartGraphicsCfg> {
        std::iter::once(&mut self.chart_graphics)
            .chain(self.chart_defaults_addto.chart_graphics.as_mut())
            .chain(self.chart_defaults_compare.chart_graphics.as_mut())
            .chain(self.chart_defaults_trade.chart_graphics.as_mut())
    }

    /// Store the candle default for one kind, reporting whether it actually moved.
    pub fn set_candle_view_default(
        &mut self,
        kind: super::chart_defaults::ChartTabKind,
        value: crate::market::candles::CandleViewCfg,
    ) -> bool {
        // No kind ships its own candles: every one of them follows Main until it is given a value.
        let split = self.split_defaults(
            |d| &mut d.candle_view,
            |l, k| l.candle_view_for(k),
            |_| false,
        );
        let moved = match self.kind_defaults_mut(kind) {
            Some(d) => d.candle_view.replace(value) != Some(value),
            None => std::mem::replace(&mut self.candle_view, value) != value,
        };
        split || moved
    }

    /// Store the graphics default for one kind, reporting whether it actually moved.
    pub fn set_chart_graphics_default(
        &mut self,
        kind: super::chart_defaults::ChartTabKind,
        value: ChartGraphicsCfg,
    ) -> bool {
        let split = self.split_defaults(
            |d| &mut d.chart_graphics,
            |l, k| l.chart_graphics_for(k),
            |_| false,
        );
        let moved = match self.kind_defaults_mut(kind) {
            Some(d) => d.chart_graphics.replace(value) != Some(value),
            None => std::mem::replace(&mut self.chart_graphics, value) != value,
        };
        split || moved
    }

    /// Store the caption default for one kind, reporting whether it actually moved.
    pub fn set_chart_labels_default(
        &mut self,
        kind: super::chart_defaults::ChartTabKind,
        value: super::chart_labels::ChartLabelsCfg,
    ) -> bool {
        let split = self.split_defaults(
            |d| &mut d.chart_labels,
            |l, k| l.chart_labels_for(k).clone(),
            // The trade window and a comparison both ship their own captions; see the guard inside.
            |k| k.builtin_labels().is_some(),
        );
        // `|` rather than `||`: the store must run even when the separation already reported a
        // change, or the pressed value would never be written.
        split | self.store_chart_labels(kind, value)
    }

    /// Store one kind's candles and NOTHING else, reporting whether they moved.
    ///
    /// The candle twin of [`Self::store_chart_labels`], for the same caller: a row inside the
    /// trade window records what THAT kind shows and makes no statement about the tab kinds, so
    /// the ⧉ press's separation pass must not run under it.
    ///
    /// Args:
    ///     kind: The kind whose candles are being stored.
    ///     value: The set to store.
    ///
    /// Returns:
    ///     Whether the stored value actually changed.
    pub fn store_candle_view(
        &mut self,
        kind: super::chart_defaults::ChartTabKind,
        value: crate::market::candles::CandleViewCfg,
    ) -> bool {
        match self.kind_defaults_mut(kind) {
            Some(d) => d.candle_view.replace(value) != Some(value),
            None => std::mem::replace(&mut self.candle_view, value) != value,
        }
    }

    /// Store one kind's graphics and NOTHING else; see [`Self::store_candle_view`].
    pub fn store_chart_graphics(
        &mut self,
        kind: super::chart_defaults::ChartTabKind,
        value: ChartGraphicsCfg,
    ) -> bool {
        match self.kind_defaults_mut(kind) {
            Some(d) => d.chart_graphics.replace(value) != Some(value),
            None => std::mem::replace(&mut self.chart_graphics, value) != value,
        }
    }

    /// Store one kind's captions and NOTHING else, reporting whether they moved.
    ///
    /// No separation of the other kinds: the caller is recording what ONE view is showing, not
    /// making a statement about the others. Separating them is a deliberate press with its own
    /// wording in the popup — a right-click toggle inside a window must not perform it silently.
    ///
    /// Args:
    ///     kind: The kind whose captions are being stored.
    ///     value: The set to store.
    ///
    /// Returns:
    ///     Whether the stored value actually changed.
    pub fn store_chart_labels(
        &mut self,
        kind: super::chart_defaults::ChartTabKind,
        value: super::chart_labels::ChartLabelsCfg,
    ) -> bool {
        match self.kind_defaults_mut(kind) {
            Some(d) => d.chart_labels.replace(value.clone()) != Some(value),
            None => std::mem::replace(&mut self.chart_labels, value.clone()) != value,
        }
    }

    /// Put one kind's candle default back to what the terminal ships, reporting whether it moved.
    ///
    /// Two different acts under one word, because "back to the shipped set" means two different
    /// things depending on where the kind's default lives:
    ///
    /// - a non-Main kind's slot is EMPTIED, so it follows again whatever it followed before anyone
    ///   pressed anything — its own built-in set where it has one, Main where it does not. Storing
    ///   today's shipped value instead would freeze the kind on a copy, and a later build that
    ///   improved that set would never reach the reader.
    /// - Main's default IS the base field and cannot be emptied, so it takes the shipped value —
    ///   through the setter, whose separation pass keeps the other kinds where they are. Without
    ///   that, resetting the main chart would drag every kind still following it along, which is
    ///   the one thing a per-kind reset must not do. The cost is deliberate and worth naming: a
    ///   kind that held nothing is left holding a copy of what it was showing, so it is now
    ///   detached from Main. Moving it instead — silently redressing tabs the reader did not tick
    ///   — is the worse of the two.
    ///
    /// "The shipped set" therefore means what the kind ships FOR THIS SETTING, and only the
    /// captions have any: a comparison and the trade window answer
    /// [`super::chart_defaults::ChartTabKind::builtin_labels`], while candles and graphics ship
    /// none at all. For everything else — and for `AddTo` even in the captions — an emptied slot
    /// means "follow Main again" rather than a set of its own.
    pub fn reset_candle_view_default(&mut self, kind: super::chart_defaults::ChartTabKind) -> bool {
        match self.kind_defaults_mut(kind) {
            Some(d) => d.candle_view.take().is_some(),
            None => {
                self.set_candle_view_default(kind, crate::market::candles::CandleViewCfg::default())
            }
        }
    }

    /// Put one kind's graphics default back to the shipped set; see
    /// [`Self::reset_candle_view_default`].
    pub fn reset_chart_graphics_default(
        &mut self,
        kind: super::chart_defaults::ChartTabKind,
    ) -> bool {
        match self.kind_defaults_mut(kind) {
            Some(d) => d.chart_graphics.take().is_some(),
            None => self.set_chart_graphics_default(kind, ChartGraphicsCfg::default()),
        }
    }

    /// Put one kind's caption default back to the shipped set; see
    /// [`Self::reset_candle_view_default`].
    ///
    /// Emptying the slot is what hands a comparison and the trade window their OWN shipped
    /// captions back, rather than Main's: both kinds answer
    /// [`super::chart_defaults::ChartTabKind::builtin_labels`].
    pub fn reset_chart_labels_default(
        &mut self,
        kind: super::chart_defaults::ChartTabKind,
    ) -> bool {
        match self.kind_defaults_mut(kind) {
            Some(d) => d.chart_labels.take().is_some(),
            None => {
                self.set_chart_labels_default(kind, super::chart_labels::ChartLabelsCfg::default())
            }
        }
    }

    /// This kind's own defaults, or `None` for Main, whose defaults are the base fields.
    pub(super) fn kind_defaults(
        &self,
        kind: super::chart_defaults::ChartTabKind,
    ) -> Option<&super::chart_defaults::ChartTabDefaults> {
        match kind {
            super::chart_defaults::ChartTabKind::Main => None,
            super::chart_defaults::ChartTabKind::AddTo => Some(&self.chart_defaults_addto),
            super::chart_defaults::ChartTabKind::Compare => Some(&self.chart_defaults_compare),
            super::chart_defaults::ChartTabKind::Trade => Some(&self.chart_defaults_trade),
        }
    }

    pub(super) fn kind_defaults_mut(
        &mut self,
        kind: super::chart_defaults::ChartTabKind,
    ) -> Option<&mut super::chart_defaults::ChartTabDefaults> {
        match kind {
            super::chart_defaults::ChartTabKind::Main => None,
            super::chart_defaults::ChartTabKind::AddTo => Some(&mut self.chart_defaults_addto),
            super::chart_defaults::ChartTabKind::Compare => Some(&mut self.chart_defaults_compare),
            super::chart_defaults::ChartTabKind::Trade => Some(&mut self.chart_defaults_trade),
        }
    }

    /// Freeze the kinds this write is NOT addressing at what they currently show.
    ///
    /// Until the first press the two non-Main kinds hold nothing and follow Main, which is what a
    /// profile that never used the feature wants. The moment one kind is given its own value that
    /// stops being true: without this, setting the Main default would still drag the other two
    /// along, and the reader who just separated them would watch them move together anyway.
    ///
    /// Per SETTING, not per kind — separating the captions must not freeze the candles as well —
    /// and only where the value is still absent, so it can never overwrite a stored default.
    ///
    /// Returns whether it wrote anything, and the caller must fold that into its own "changed"
    /// answer: a press that stores a value already in the file still SPLIT the kinds apart, and
    /// reporting "nothing moved" would leave that split in memory only, to be lost on restart.
    fn split_defaults<T: Clone>(
        &mut self,
        slot: impl Fn(&mut super::chart_defaults::ChartTabDefaults) -> &mut Option<T>,
        base: impl Fn(&Self, super::chart_defaults::ChartTabKind) -> T,
        ships_builtin: impl Fn(super::chart_defaults::ChartTabKind) -> bool,
    ) -> bool {
        let mut wrote = false;
        // Every kind: `kind_defaults_mut` answers `None` for Main, whose defaults ARE the base
        // fields, so this needs no hand-kept list of "the others" — a new kind is covered by
        // adding it to `ALL` and nowhere else.
        for kind in super::chart_defaults::ChartTabKind::ALL {
            // A kind that SHIPS its own set does not follow Main and has nothing to be separated
            // from: freezing it here would copy today's built-in value into the profile, and the
            // reader would then be stuck on it — a later build could improve that set and never
            // reach them, because their file now holds a copy made the first time they pressed
            // "make default" on some entirely different kind.
            if ships_builtin(kind) {
                continue;
            }
            // Asked and released BEFORE the value is built, so the two borrows never overlap and
            // the value — a whole caption configuration — is only cloned for a slot that will
            // actually take it.
            let empty = match self.kind_defaults_mut(kind) {
                Some(defaults) => slot(defaults).is_none(),
                None => false,
            };
            if !empty {
                continue;
            }
            // Read per KIND rather than one Main value copied to all: the trade window's captions
            // do not follow Main, so freezing it at Main's set would replace the view the reader is
            // looking at with a different one the first time any OTHER kind's default was set.
            let value = base(self, kind);
            if let Some(defaults) = self.kind_defaults_mut(kind) {
                *slot(defaults) = Some(value);
                wrote = true;
            }
        }
        wrote
    }
}
