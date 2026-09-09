//! What moved since the last time a table was drawn, and how far along it is.
//!
//! A table of figures that simply redraws is a table nobody watches: the numbers change and the
//! eye is already elsewhere. What makes a board readable is the MOVEMENT — a row climbing past
//! another, a row dropping off the end, a row lighting up because something just happened on it.
//! None of that can be worked out from the rows alone, because a row does not know where it was.
//! So the view keeps one of these per table and hands it each new board.
//!
//! **The screen keeps its own clock.** Every effect here is a pure function of how long ago it
//! started, and the view asks for the state at a moment of ITS choosing — which is what lets the
//! statistics screen redraw at its own rate rather than the monitor's. Handed to the framework's
//! animations instead, this cost sixty frames a second and six percent of a core the moment the
//! market got busy: measured on a storm, and the reason none of this is an `Animation`.
//!
//! **The key is not the place.** A row is named by something that survives a reshuffle — the
//! ticker for a coin, the account number for a trader — because "third row" is exactly the thing
//! that changes when a board moves. It is also why the trader board needed the service's `id`:
//! half its rows are anonymous and share the same handle, so there was nothing else to name them
//! by.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long a row takes to slide from where it was to where it now belongs.
///
/// Fast enough not to be waited for, slow enough to be followed: the point of the movement is
/// that the eye can see WHICH row went where, and a jump cut says nothing at all.
pub const SLIDE: Duration = Duration::from_millis(420);
/// How long a row that has dropped off the board stays on screen on its way out.
pub const FAREWELL: Duration = Duration::from_millis(800);
/// How long a row stays lit after its figures move.
pub const GLOW: Duration = Duration::from_millis(600);

/// What one row is doing right now, and how far through it is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Move {
    /// Where it was standing before this move, while it is still on its way from there.
    pub from: Option<usize>,
    /// How far along that move it is: `0` at the old place, `1` at the new one.
    pub slid: f32,
    /// How far along its way off the board it is, for a row that is leaving.
    pub leaving: Option<f32>,
    /// How brightly it is still lit: `1` the moment its figures moved, `0` once the glow is over.
    pub glow: f32,
    /// Which way those figures went — what colour that glow is.
    pub gain: bool,
}

impl Move {
    /// Whether anything about this row is MOVING.
    ///
    /// The whole cost model rests on this: a row with nothing to say is drawn where it stands,
    /// and a board where every row is still asks for no frames at all.
    pub fn still(&self) -> bool {
        self.from.is_none() && self.leaving.is_none() && self.glow <= 0.0
    }
}

/// A row on its way off the board, and the row it was.
///
/// The row itself is kept because a departure has to be DRAWN, and by the time it is drawn the
/// board it was on is gone. A dimmed name with no figures beside it reads as a broken row, not as
/// a row leaving.
struct Gone<T> {
    key: String,
    row: T,
    /// The place it was standing in when it left.
    place: usize,
    left_at: Instant,
}

/// Where a row is sliding from, and when it set off.
#[derive(Clone, Copy)]
struct Slide {
    from: usize,
    at: Instant,
}

/// The movement of one table, from one board to the next.
pub struct Motion<T> {
    /// Where each key is drawn now.
    places: HashMap<String, usize>,
    /// The row each key last had, for as long as it might still have to be drawn.
    last: HashMap<String, T>,
    /// Where each key is sliding from, for the keys that are still sliding.
    slides: HashMap<String, Slide>,
    /// When each key's figures last moved, and which way they went.
    glow: HashMap<String, (Instant, bool)>,
    /// Rows that have left and are still being shown.
    gone: Vec<Gone<T>>,
}

impl<T> Default for Motion<T> {
    fn default() -> Self {
        Self {
            places: HashMap::new(),
            last: HashMap::new(),
            slides: HashMap::new(),
            glow: HashMap::new(),
            gone: Vec::new(),
        }
    }
}

impl<T: Clone> Motion<T> {
    /// Take the new order of the board and work out what moved.
    ///
    /// A key that appears twice is taken once, at its first place. Everything on the screen is
    /// keyed by it, so two rows with one name would slide together, glow together and leave
    /// together — one row wearing two sets of figures.
    ///
    /// Args:
    ///     rows: The board, in the order it will be drawn: what names each row, and the row.
    ///     now: The clock every effect is measured against.
    pub fn settle(&mut self, rows: &[(String, T)], now: Instant) {
        // Whoever is no longer on the board is on their way out, from wherever they were standing.
        let departed: Vec<Gone<T>> = self
            .places
            .iter()
            .filter(|(key, _)| !rows.iter().any(|(live, _)| live == *key))
            .filter_map(|(key, place)| {
                Some(Gone {
                    key: key.clone(),
                    row: self.last.get(key)?.clone(),
                    place: *place,
                    left_at: now,
                })
            })
            .collect();
        self.gone.extend(departed);
        self.gone
            .retain(|gone| now.duration_since(gone.left_at) < FAREWELL);
        // A row that has come BACK is not also leaving.
        self.gone
            .retain(|gone| !rows.iter().any(|(live, _)| *live == gone.key));
        self.glow
            .retain(|_, (at, _)| now.duration_since(*at) < GLOW);
        self.slides
            .retain(|_, slide| now.duration_since(slide.at) < SLIDE);

        let mut places: HashMap<String, usize> = HashMap::with_capacity(rows.len());
        for (place, (key, _)) in rows.iter().enumerate() {
            if places.contains_key(key) {
                continue;
            }
            // It sets off from where it was STANDING, not from where a slide already in flight
            // began: a row overtaken twice starts again from where the eye last saw it.
            if let Some(was) = self.places.get(key).copied().filter(|was| *was != place) {
                self.slides
                    .insert(key.clone(), Slide { from: was, at: now });
            }
            places.insert(key.clone(), place);
        }
        self.places = places;
        // Only what is on the board or on its way off it: a row nobody has seen for an hour is
        // not something this has any business remembering.
        let mut last: HashMap<String, T> = HashMap::with_capacity(rows.len() + self.gone.len());
        for (key, row) in rows {
            last.entry(key.clone()).or_insert_with(|| row.clone());
        }
        for gone in &self.gone {
            last.insert(gone.key.clone(), gone.row.clone());
        }
        self.last = last;
    }

    /// Say that this row's figures have just moved, so it lights up.
    ///
    /// Args:
    ///     key: Which row.
    ///     gain: Whether the money went UP — which is the colour it glows in.
    ///     now: The clock.
    pub fn beat(&mut self, key: &str, gain: bool, now: Instant) {
        self.glow.insert(key.to_string(), (now, gain));
    }

    /// How this row is standing at this moment.
    pub fn of(&self, key: &str, now: Instant) -> Move {
        let slide = self
            .slides
            .get(key)
            .map(|slide| (slide.from, share(now, slide.at, SLIDE)))
            .filter(|(_, along)| *along < 1.0);
        let (glow, gain) = self
            .glow
            .get(key)
            .map(|(at, gain)| (1.0 - share(now, *at, GLOW), *gain))
            .unwrap_or((0.0, false));
        Move {
            from: slide.map(|(from, _)| from),
            slid: slide.map_or(1.0, |(_, along)| along),
            leaving: None,
            glow,
            gain,
        }
    }

    /// The rows that have left and are still on their way out: the key, the row it was, the place
    /// it was standing in, and how far through leaving it is.
    pub fn leaving(&self, now: Instant) -> impl Iterator<Item = (&str, &T, usize, Move)> {
        self.gone
            .iter()
            .filter(move |gone| now.duration_since(gone.left_at) < FAREWELL)
            .map(move |gone| {
                (
                    gone.key.as_str(),
                    &gone.row,
                    gone.place,
                    Move {
                        from: None,
                        slid: 1.0,
                        leaving: Some(share(now, gone.left_at, FAREWELL)),
                        glow: 0.0,
                        gain: false,
                    },
                )
            })
    }

    /// Whether anything is still moving, so the view knows whether to ask for another frame.
    pub fn moving(&self, now: Instant) -> bool {
        self.gone
            .iter()
            .any(|gone| now.duration_since(gone.left_at) < FAREWELL)
            || self
                .slides
                .values()
                .any(|slide| now.duration_since(slide.at) < SLIDE)
            || self
                .glow
                .values()
                .any(|(at, _)| now.duration_since(*at) < GLOW)
    }
}

/// How far through `span` the moment `at` is, from `0` to `1`.
fn share(now: Instant, at: Instant, span: Duration) -> f32 {
    if span.is_zero() {
        return 1.0;
    }
    (now.duration_since(at).as_secs_f32() / span.as_secs_f32()).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests;
