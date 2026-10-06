//! Typed slot access on the hotkey config.

use super::*;

impl HotkeysConfig {
    /// The keystroke stored for one slot, or `""` for a slot that binds nothing.
    ///
    /// An index outside its family answers `""` rather than panicking: the families are arrays, and
    /// this is reached from a page that builds indices in loops and from a config file a user can
    /// hand-edit.
    pub fn key(&self, slot: KeySlot) -> &str {
        match slot {
            KeySlot::OrderSize(i) => self.order_size.get(i).map_or("", String::as_str),
            KeySlot::SellPreset(i) => self.sell_preset.get(i).map_or("", String::as_str),
            KeySlot::ManualStrategy(i) => self.manual_strategy.get(i).map_or("", String::as_str),
            KeySlot::CancelBuy => &self.cancel_buy,
            KeySlot::PanicSell => &self.panic_sell,
            KeySlot::PanicSellOne => &self.panic_sell_one,
            KeySlot::CancelAllBuys => &self.cancel_all_buys,
            KeySlot::CancelAllBuysAllCores => &self.cancel_all_buys_all_cores,
            KeySlot::JoinSells => &self.join_sells,
            KeySlot::SwitchCharts => &self.switch_charts,
            KeySlot::NewLong => &self.new_long,
            KeySlot::NewShort => &self.new_short,
            KeySlot::SplitOrder => &self.split_order,
            KeySlot::SplitOrderX => &self.split_order_x,
            KeySlot::SellsToRect => &self.sells_to_rect,
            KeySlot::ShiftBuyUp => &self.shift_buy_up,
            KeySlot::ShiftBuyDown => &self.shift_buy_down,
            KeySlot::ShiftSellUp => &self.shift_sell_up,
            KeySlot::ShiftSellDown => &self.shift_sell_down,
            KeySlot::ScalePlus => &self.scale_plus,
            KeySlot::ScaleMinus => &self.scale_minus,
            KeySlot::SuperZoomIn => &self.super_zoom_in,
            KeySlot::SuperZoomOut => &self.super_zoom_out,
            KeySlot::CenterChart => &self.center_chart,
            KeySlot::ToggleLive => &self.toggle_live,
            KeySlot::SwitchFigure => &self.switch_figure,
            KeySlot::ChartShot => &self.chart_shot,
            KeySlot::DrawHline => &self.draw_hline,
            KeySlot::DrawHorizontalRay => &self.draw_horizontal_ray,
            KeySlot::DrawSegment => &self.draw_segment,
            KeySlot::DrawTriangle => &self.draw_triangle,
            KeySlot::DrawChannel => &self.draw_channel,
            KeySlot::FigDelete => &self.fig_delete,
            KeySlot::FigAlert => &self.fig_alert,
            KeySlot::FigUndo => &self.fig_undo,
        }
    }

    /// Write one slot's keystroke, answering whether anything actually changed.
    ///
    /// The change answer is what keeps a settings render from marking the config dirty on every
    /// keystroke that re-selects what was already there. An index outside its family writes nothing
    /// and reports no change, matching [`Self::key`]'s reading of the same index.
    pub fn set_key(&mut self, slot: KeySlot, value: String) -> bool {
        match self.key_mut(slot) {
            Some(target) => assign_if_changed(target, value),
            None => false,
        }
    }

    /// The stored field for one slot, or `None` for an index outside its family.
    fn key_mut(&mut self, slot: KeySlot) -> Option<&mut String> {
        Some(match slot {
            KeySlot::OrderSize(i) => self.order_size.get_mut(i)?,
            KeySlot::SellPreset(i) => self.sell_preset.get_mut(i)?,
            KeySlot::ManualStrategy(i) => self.manual_strategy.get_mut(i)?,
            KeySlot::CancelBuy => &mut self.cancel_buy,
            KeySlot::PanicSell => &mut self.panic_sell,
            KeySlot::PanicSellOne => &mut self.panic_sell_one,
            KeySlot::CancelAllBuys => &mut self.cancel_all_buys,
            KeySlot::CancelAllBuysAllCores => &mut self.cancel_all_buys_all_cores,
            KeySlot::JoinSells => &mut self.join_sells,
            KeySlot::SwitchCharts => &mut self.switch_charts,
            KeySlot::NewLong => &mut self.new_long,
            KeySlot::NewShort => &mut self.new_short,
            KeySlot::SplitOrder => &mut self.split_order,
            KeySlot::SplitOrderX => &mut self.split_order_x,
            KeySlot::SellsToRect => &mut self.sells_to_rect,
            KeySlot::ShiftBuyUp => &mut self.shift_buy_up,
            KeySlot::ShiftBuyDown => &mut self.shift_buy_down,
            KeySlot::ShiftSellUp => &mut self.shift_sell_up,
            KeySlot::ShiftSellDown => &mut self.shift_sell_down,
            KeySlot::ScalePlus => &mut self.scale_plus,
            KeySlot::ScaleMinus => &mut self.scale_minus,
            KeySlot::SuperZoomIn => &mut self.super_zoom_in,
            KeySlot::SuperZoomOut => &mut self.super_zoom_out,
            KeySlot::CenterChart => &mut self.center_chart,
            KeySlot::ToggleLive => &mut self.toggle_live,
            KeySlot::SwitchFigure => &mut self.switch_figure,
            KeySlot::ChartShot => &mut self.chart_shot,
            KeySlot::DrawHline => &mut self.draw_hline,
            KeySlot::DrawHorizontalRay => &mut self.draw_horizontal_ray,
            KeySlot::DrawSegment => &mut self.draw_segment,
            KeySlot::DrawTriangle => &mut self.draw_triangle,
            KeySlot::DrawChannel => &mut self.draw_channel,
            KeySlot::FigDelete => &mut self.fig_delete,
            KeySlot::FigAlert => &mut self.fig_alert,
            KeySlot::FigUndo => &mut self.fig_undo,
        })
    }

    /// The gesture stored for one slot — `None` for a key half with no entry.
    pub fn gesture(&self, slot: GestureSlot) -> MouseGestureBinding {
        match slot {
            // A named slot's stem IS its key, so the twenty-six of them are read with no
            // allocation; only a family member has to spell its index.
            GestureSlot::ForKey(key) => match key.index() {
                None => self.action_clicks.get(key.stem()),
                Some(_) => self.action_clicks.get(key.name().as_str()),
            }
            .copied()
            .unwrap_or(MouseGestureBinding::None),
            GestureSlot::BuySet => self.buy_set_click,
            GestureSlot::ShortSet => self.short_set_click,
            GestureSlot::PendingLong => self.pending_long_click,
            GestureSlot::PendingShort => self.pending_short_click,
            GestureSlot::BuyMove => self.buy_move_click,
            GestureSlot::SellMove => self.sell_move_click,
            GestureSlot::BuyMove2 => self.buy_move_click2,
            GestureSlot::SellMove2 => self.sell_move_click2,
            GestureSlot::ShortBuyMove => self.short_buy_move_click,
            GestureSlot::ShortSellMove => self.short_sell_move_click,
            GestureSlot::ShortBuyMove2 => self.short_buy_move_click2,
            GestureSlot::ShortSellMove2 => self.short_sell_move_click2,
            GestureSlot::FigDelete => self.fig_delete_click,
        }
    }

    /// Write one slot's gesture and NOTHING else, answering whether anything changed.
    ///
    /// No mirroring, whatever `same_hotkeys_for_move` says: the settings editor carries the mirror
    /// itself, and a layout transfer must not — with the flag on locally and off at the core, a
    /// mirrored write of the long row would overwrite a short value the transfer had decided to
    /// leave alone.
    pub fn set_gesture(&mut self, slot: GestureSlot, value: MouseGestureBinding) -> bool {
        match slot {
            // Unset is ABSENT, not stored: the table stays empty for a file that never used one.
            GestureSlot::ForKey(key) => {
                let name = key.name();
                if value == MouseGestureBinding::None {
                    self.action_clicks.remove(&name).is_some()
                } else {
                    self.action_clicks.insert(name, value) != Some(value)
                }
            }
            own => self
                .gesture_mut(own)
                .is_some_and(|field| assign_if_changed(field, value)),
        }
    }

    /// The field of one gesture that has a field, or `None` for a key half, which lives in the
    /// table. Exhaustive, so a new variant has to say which of the two it is.
    fn gesture_mut(&mut self, slot: GestureSlot) -> Option<&mut MouseGestureBinding> {
        Some(match slot {
            GestureSlot::ForKey(_) => return None,
            GestureSlot::BuySet => &mut self.buy_set_click,
            GestureSlot::ShortSet => &mut self.short_set_click,
            GestureSlot::PendingLong => &mut self.pending_long_click,
            GestureSlot::PendingShort => &mut self.pending_short_click,
            GestureSlot::BuyMove => &mut self.buy_move_click,
            GestureSlot::SellMove => &mut self.sell_move_click,
            GestureSlot::BuyMove2 => &mut self.buy_move_click2,
            GestureSlot::SellMove2 => &mut self.sell_move_click2,
            GestureSlot::ShortBuyMove => &mut self.short_buy_move_click,
            GestureSlot::ShortSellMove => &mut self.short_sell_move_click,
            GestureSlot::ShortBuyMove2 => &mut self.short_buy_move_click2,
            GestureSlot::ShortSellMove2 => &mut self.short_sell_move_click2,
            GestureSlot::FigDelete => &mut self.fig_delete_click,
        })
    }

    /// The gesture the terminal actually FIRES for one slot.
    ///
    /// A move row resolves through [`Self::move_gestures`], the one reader of
    /// `same_hotkeys_for_move`: with the mirror set the short field is not what fires, and showing
    /// or indexing it would put a binding nothing executes into a preview or a clash caption. The
    /// placement and figure rows have no mirror, so the field is what fires.
    pub fn gesture_in_effect(&self, slot: GestureSlot) -> MouseGestureBinding {
        match slot.move_half() {
            Some(half) => {
                self.move_gestures(half.row.entry(), half.short)[usize::from(half.row.second())]
            }
            None => self.gesture(slot),
        }
    }

    /// The keyboard slot whose click half claims a press, or `None` for a press no half is bound
    /// to.
    ///
    /// Args:
    ///     matches: Whether a press being examined satisfies one binding — the caller owns the
    ///         platform's modifier type, exactly as in [`Self::resolve_move_gesture`].
    ///
    /// The ordered walk the sibling dispatchers make — `resolve_move_gesture` over
    /// `MoveKindSlot::ALL`, `placement_intent` over `GestureSlot::OWN` — so two halves bound to one
    /// press resolve in `KeySlot::all` order, the order the settings page lists them and captions
    /// the loser by, rather than by the table's own alphabetical order. A press on a terminal with
    /// nothing bound costs one emptiness check. Only halves `KeySlot::mouse_half` makes are read,
    /// so a hand-edited `fig_delete = "middle"` in the table cannot fire a slot no row shows, no
    /// caption counts and no collision check sees.
    pub fn action_for_gesture(
        &self,
        matches: impl Fn(MouseGestureBinding) -> bool,
    ) -> Option<KeySlot> {
        if self.action_clicks.is_empty() {
            return None;
        }
        KeySlot::all()
            .into_iter()
            .filter_map(KeySlot::mouse_half)
            .find(|half| matches(self.gesture(*half)))
            .and_then(|half| match half {
                GestureSlot::ForKey(key) => Some(key),
                _ => None,
            })
    }

    /// The "move kind" of one move row.
    pub fn move_kind(&self, row: MoveKindSlot) -> MoveKind {
        match row {
            MoveKindSlot::BuyMove => self.buy_move_kind,
            MoveKindSlot::SellMove => self.sell_move_kind,
            MoveKindSlot::BuyMove2 => self.buy_move_kind2,
            MoveKindSlot::SellMove2 => self.sell_move_kind2,
        }
    }

    /// Write one row's move kind, answering whether anything changed.
    pub fn set_move_kind(&mut self, row: MoveKindSlot, value: MoveKind) -> bool {
        let target = match row {
            MoveKindSlot::BuyMove => &mut self.buy_move_kind,
            MoveKindSlot::SellMove => &mut self.sell_move_kind,
            MoveKindSlot::BuyMove2 => &mut self.buy_move_kind2,
            MoveKindSlot::SellMove2 => &mut self.sell_move_kind2,
        };
        assign_if_changed(target, value)
    }
}

/// Writes `value` into `target` and answers whether that was a change.
///
/// The change answer is what keeps a settings render from marking the config dirty on every
/// keystroke that re-selects what was already there; every slot setter answers through this one.
fn assign_if_changed<T: PartialEq>(target: &mut T, value: T) -> bool {
    if *target == value {
        return false;
    }
    *target = value;
    true
}
