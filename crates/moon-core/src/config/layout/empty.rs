//! Where each block of the empty Main screen sits, and what an unreadable answer falls back to.
//!
//! The empty screen is five independent blocks — the brand, the line under it, and the three crowd
//! tables — and until now each one was nailed to one corner by the code that drew it. This module
//! is the whole of the placement model: nine anchors, one saved anchor per block, and the rule for
//! reading a file that says something the terminal does not recognise.
//!
//! **Nine anchors, not free pixels.** A window is resized, split and moved between monitors, and a
//! block remembered at an absolute point is a block that ends up half off the screen on the next
//! machine. An anchor reflows: whatever the panel is, "bottom right" is still bottom right, and a
//! narrow panel can degrade the whole arrangement to one readable column without any saved value
//! becoming wrong.
//!
//! **Two blocks may share an anchor, and that is not a clash.** They stack in the order of
//! [`EmptyBlock::ALL`], which is why the shipped arrangement puts the brand and its hint in the
//! same middle cell and gets exactly the screen the terminal has always had. Nothing here rejects,
//! renames or moves a block to keep anchors unique — the drawing side stacks, so there is no
//! invalid combination to repair.
//!
//! **An unreadable value is not an error.** Every key is read leniently by the layout that owns it
//! (`de_lenient`), so a hand edit or a value written by a newer build arrives as `None` and takes
//! the block's own default. That is the whole of the sanitization: a placement can never cost the
//! window layout it is stored beside.

use serde::{Deserialize, Serialize};

use super::WindowLayout;

/// One of the nine anchors a block of the empty screen can be placed in.
///
/// Named by row and by side rather than by compass point, and by SIDE rather than by hand:
/// "start" and "end" are what the row's own flow calls its two ends, which is the same vocabulary
/// the drawing code lays them out with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EmptySlot {
    TopStart,
    TopCenter,
    TopEnd,
    MiddleStart,
    MiddleCenter,
    MiddleEnd,
    BottomStart,
    BottomCenter,
    BottomEnd,
}

impl EmptySlot {
    /// Every anchor, in READING order: the top row left to right, then the middle, then the bottom.
    ///
    /// The order is not decoration. A panel too narrow for three columns is drawn as one column,
    /// and this is the order that column is read in — so the arrangement a person chose still says
    /// the same thing about what comes first.
    pub const ALL: [Self; 9] = [
        Self::TopStart,
        Self::TopCenter,
        Self::TopEnd,
        Self::MiddleStart,
        Self::MiddleCenter,
        Self::MiddleEnd,
        Self::BottomStart,
        Self::BottomCenter,
        Self::BottomEnd,
    ];

    /// Which of the three rows this anchor is in, from the top.
    pub fn row(self) -> u8 {
        match self {
            Self::TopStart | Self::TopCenter | Self::TopEnd => 0,
            Self::MiddleStart | Self::MiddleCenter | Self::MiddleEnd => 1,
            Self::BottomStart | Self::BottomCenter | Self::BottomEnd => 2,
        }
    }

    /// Which of the three columns it is in, from the row's start.
    pub fn column(self) -> u8 {
        match self {
            Self::TopStart | Self::MiddleStart | Self::BottomStart => 0,
            Self::TopCenter | Self::MiddleCenter | Self::BottomCenter => 1,
            Self::TopEnd | Self::MiddleEnd | Self::BottomEnd => 2,
        }
    }

    /// The anchor at that row and column, or `None` when either is past the third.
    ///
    /// Args:
    ///     row: Row from the top, from zero.
    ///     column: Column from the row's start, from zero.
    pub fn at(row: u8, column: u8) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|slot| slot.row() == row && slot.column() == column)
    }
}

/// One movable block of the empty screen.
///
/// The brand and its hint are two blocks rather than one because they are two switches: somebody
/// who wants the line without the mark, or the mark somewhere other than the middle, is asking a
/// question a single block could not answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EmptyBlock {
    /// The Moonbot mark.
    Logo,
    /// The one line naming the gesture that opens a chart.
    Hint,
    /// The rolling minute of the crowd's trades.
    Minute,
    /// The service's trader board for the day.
    Traders,
    /// The service's coin board for the day.
    Coins,
}

impl EmptyBlock {
    /// How many movable blocks the empty screen has.
    pub const COUNT: usize = 5;

    /// Every block, in the order two of them stack when they share an anchor.
    ///
    /// The brand before its hint, and the tables after both: the shipped arrangement puts the first
    /// two in the middle cell, and this order is what makes that read as a mark with a line under
    /// it rather than as a line with a mark under it.
    pub const ALL: [Self; Self::COUNT] = [
        Self::Logo,
        Self::Hint,
        Self::Minute,
        Self::Traders,
        Self::Coins,
    ];

    /// Where this block sits on a profile that has never chosen.
    ///
    /// Between them these five reproduce the screen the terminal shipped with, anchor for anchor:
    /// the brand and its line in the middle, the minute top right, the trader board top left and
    /// the coin board bottom right. A default that did not would silently rearrange every existing
    /// profile on its next launch.
    pub fn default_slot(self) -> EmptySlot {
        match self {
            Self::Logo | Self::Hint => EmptySlot::MiddleCenter,
            Self::Minute => EmptySlot::TopEnd,
            Self::Traders => EmptySlot::TopStart,
            Self::Coins => EmptySlot::BottomEnd,
        }
    }

    /// This block's saved anchor, or `None` when nobody has chosen one.
    ///
    /// Args:
    ///     layout: Persisted window layout.
    pub fn saved(self, layout: &WindowLayout) -> Option<EmptySlot> {
        match self {
            Self::Logo => layout.main_empty_place_logo,
            Self::Hint => layout.main_empty_place_hint,
            Self::Minute => layout.main_empty_place_minute,
            Self::Traders => layout.main_empty_place_traders,
            Self::Coins => layout.main_empty_place_coins,
        }
    }

    /// Write this block's anchor, leaving every other block's key exactly as it stands.
    ///
    /// Only the EDITED key is ever written, for the same reason the screen's switches do it that
    /// way: stamping the others would turn "never chosen" into an explicit value for blocks nobody
    /// touched, and a later change of default could then never reach them.
    ///
    /// Args:
    ///     layout: Persisted window layout, edited in place.
    ///     slot: Where the block now sits, or `None` to forget the choice.
    pub fn store(self, layout: &mut WindowLayout, slot: Option<EmptySlot>) {
        match self {
            Self::Logo => layout.main_empty_place_logo = slot,
            Self::Hint => layout.main_empty_place_hint = slot,
            Self::Minute => layout.main_empty_place_minute = slot,
            Self::Traders => layout.main_empty_place_traders = slot,
            Self::Coins => layout.main_empty_place_coins = slot,
        }
    }
}

/// Where all five blocks sit right now.
///
/// Resolved once per frame and passed to the drawing rather than asked per block, so that one
/// frame cannot draw two blocks from two readings of the same file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmptyPlaces {
    /// One anchor per block, in [`EmptyBlock::ALL`] order.
    slots: [EmptySlot; EmptyBlock::COUNT],
}

impl Default for EmptyPlaces {
    /// The screen the terminal has always had.
    fn default() -> Self {
        let mut slots = [EmptySlot::MiddleCenter; EmptyBlock::COUNT];
        for (index, block) in EmptyBlock::ALL.into_iter().enumerate() {
            slots[index] = block.default_slot();
        }
        Self { slots }
    }
}

impl EmptyPlaces {
    /// Read the arrangement out of the saved layout.
    ///
    /// A block with no saved anchor — never chosen, or a value this build does not recognise, which
    /// the lenient reader has already turned into `None` — takes its own default. Nothing else can
    /// happen here: two blocks sharing an anchor stack, so there is no clash to repair.
    ///
    /// Args:
    ///     layout: Persisted window layout.
    pub fn restore(layout: &WindowLayout) -> Self {
        let mut places = Self::default();
        for (index, block) in EmptyBlock::ALL.into_iter().enumerate() {
            places.slots[index] = block.saved(layout).unwrap_or_else(|| block.default_slot());
        }
        places
    }

    /// Where one block sits.
    ///
    /// Args:
    ///     block: The block asked about.
    pub fn slot(self, block: EmptyBlock) -> EmptySlot {
        let index = EmptyBlock::ALL
            .into_iter()
            .position(|candidate| candidate == block)
            .unwrap_or(0);
        self.slots[index]
    }

    /// Which blocks are anchored here, in the order they stack.
    ///
    /// Args:
    ///     slot: The anchor asked about.
    pub fn blocks_in(self, slot: EmptySlot) -> impl Iterator<Item = EmptyBlock> {
        EmptyBlock::ALL
            .into_iter()
            .filter(move |block| self.slot(*block) == slot)
    }

    /// Forget every choice, so the screen comes back to the arrangement it shipped with.
    ///
    /// The keys are CLEARED rather than written with today's defaults: a profile that has been
    /// reset is a profile that has never chosen, which is what lets a later change of default
    /// reach it.
    ///
    /// Args:
    ///     layout: Persisted window layout, edited in place.
    pub fn reset(layout: &mut WindowLayout) {
        for block in EmptyBlock::ALL {
            block.store(layout, None);
        }
    }
}

#[cfg(test)]
mod tests;
