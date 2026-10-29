//! FIFO notification rows with direct id lookup and one head per chat.

use std::collections::HashMap;
use std::ops::{Index, IndexMut};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::Pending;

/// Process-only identities distinguish replaced queues as well as newly appended rows.
static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);

/// A row's neighbours in file order and in its chat's send order.
#[derive(Clone, Debug)]
struct Row {
    value: Pending,
    previous: Option<u64>,
    next: Option<u64>,
    previous_chat: Option<u64>,
    next_chat: Option<u64>,
    sequence: u64,
}

/// An unbounded outbox, serialized as the original JSON array in original file order.
/// Removal and lookup by id do not walk the remaining cards.
#[derive(Clone, Debug, Default)]
pub struct Outbox {
    rows: HashMap<u64, Row>,
    ids: HashMap<u64, Vec<u64>>,
    first: Option<u64>,
    last: Option<u64>,
    heads: HashMap<i64, u64>,
    tails: HashMap<i64, u64>,
    photos: HashMap<String, usize>,
    sequence: u64,
    revision: u64,
}

impl Outbox {
    /// Append a row at the end of both send orders, preserving even legacy duplicate ids.
    pub fn push(&mut self, value: Pending) {
        self.revision = NEXT_REVISION.fetch_add(1, Ordering::Relaxed);
        let id = self.sequence;
        self.ids.entry(value.id).or_default().push(id);
        let chat = value.chat;
        let previous_chat = self.tails.insert(chat, id);
        if let Some(previous) = self.last {
            self.rows.get_mut(&previous).expect("tail").next = Some(id);
        } else {
            self.first = Some(id);
        }
        if let Some(previous) = previous_chat {
            self.rows.get_mut(&previous).expect("chat tail").next_chat = Some(id);
        } else {
            self.heads.insert(chat, id);
        }
        if let Some(photo) = &value.photo {
            *self.photos.entry(photo.clone()).or_default() += 1;
        }
        self.rows.insert(
            id,
            Row {
                value,
                previous: self.last,
                next: None,
                previous_chat,
                next_chat: None,
                sequence: self.sequence,
            },
        );
        self.sequence = self.sequence.wrapping_add(1);
        self.last = Some(id);
    }

    /// Remove the same id set the old Vec retain removed, keeping all survivors in order.
    pub fn remove_id(&mut self, id: u64) -> Option<Pending> {
        let slots = self.ids.remove(&id)?;
        let mut first = None;
        for slot in slots {
            let removed = self.remove_slot(slot);
            if first.is_none() {
                first = removed;
            }
        }
        first
    }

    /// Unlink one live slot from both orders and its payload indexes.
    fn remove_slot(&mut self, id: u64) -> Option<Pending> {
        let row = self.rows.remove(&id)?;
        if let Some(slots) = self.ids.get_mut(&row.value.id) {
            slots.retain(|slot| *slot != id);
            if slots.is_empty() {
                self.ids.remove(&row.value.id);
            }
        }
        if let Some(previous) = row.previous {
            self.rows.get_mut(&previous).expect("previous").next = row.next;
        } else {
            self.first = row.next;
        }
        if let Some(next) = row.next {
            self.rows.get_mut(&next).expect("next").previous = row.previous;
        } else {
            self.last = row.previous;
        }
        if let Some(previous) = row.previous_chat {
            self.rows
                .get_mut(&previous)
                .expect("previous chat")
                .next_chat = row.next_chat;
        } else if let Some(next) = row.next_chat {
            self.heads.insert(row.value.chat, next);
        } else {
            self.heads.remove(&row.value.chat);
        }
        if let Some(next) = row.next_chat {
            self.rows.get_mut(&next).expect("next chat").previous_chat = row.previous_chat;
        } else if let Some(previous) = row.previous_chat {
            self.tails.insert(row.value.chat, previous);
        } else {
            self.tails.remove(&row.value.chat);
        }
        if let Some(photo) = &row.value.photo {
            let count = self.photos.get_mut(photo).expect("photo count");
            *count -= 1;
            if *count == 0 {
                self.photos.remove(photo);
            }
        }
        Some(row.value)
    }

    /// Look up an allocated id without scanning queued cards.
    pub fn by_id(&self, id: u64) -> Option<&Pending> {
        self.rows
            .get(self.ids.get(&id)?.first()?)
            .map(|row| &row.value)
    }

    /// First queued row of each chat, in file order; blocked chats cannot hide other chats.
    pub fn heads(&self) -> Vec<&Pending> {
        let mut rows: Vec<&Row> = self.heads.values().map(|id| &self.rows[id]).collect();
        rows.sort_unstable_by_key(|row| row.sequence);
        rows.into_iter().map(|row| &row.value).collect()
    }

    /// Whether any queued row still owns this photo.
    pub fn names_photo(&self, photo: &str) -> bool {
        self.photos.contains_key(photo)
    }

    /// Changes on append, replacement or payload mutation; removing an allowed row cannot
    /// introduce a revoked disclosure and does not invalidate the revocation scan.
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    /// Number of live rows, independently of their allocated ids.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether the sender has drained every row.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Visit live rows in file order.
    pub fn iter(&self) -> Iter<'_> {
        Iter {
            outbox: self,
            next: self.first,
        }
    }

    /// Last appended row, whose optional send payload the caller is still filling in.
    /// Allocated ids, destination chats and photo ownership remain fixed at append.
    pub fn last_mut(&mut self) -> Option<&mut Pending> {
        self.revision = NEXT_REVISION.fetch_add(1, Ordering::Relaxed);
        self.rows.get_mut(&self.last?).map(|row| &mut row.value)
    }

    /// Last live row in file order.
    pub fn last(&self) -> Option<&Pending> {
        self.rows.get(&self.last?).map(|row| &row.value)
    }

    /// Keep exactly the rows the predicate accepts, without reordering survivors.
    pub fn retain(&mut self, mut keep: impl FnMut(&Pending) -> bool) {
        let mut removed = Vec::new();
        let mut slot = self.first;
        while let Some(id) = slot {
            let row = &self.rows[&id];
            slot = row.next;
            if !keep(&row.value) {
                removed.push(id);
            }
        }
        for id in removed {
            self.remove_slot(id);
        }
    }

    /// Forget every queued row and its send indexes.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Keep the first `len` rows in file order.
    pub fn truncate(&mut self, len: usize) {
        let mut index = 0;
        self.retain(|_| {
            index += 1;
            index <= len
        });
    }
}

/// A borrowing iterator that follows live neighbours rather than tombstones.
pub struct Iter<'a> {
    outbox: &'a Outbox,
    next: Option<u64>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a Pending;
    /// Advance to the next live row in file order.
    fn next(&mut self) -> Option<Self::Item> {
        let row = self.outbox.rows.get(&self.next?)?;
        self.next = row.next;
        Some(&row.value)
    }
}

impl PartialEq for Outbox {
    /// Allocation and hash order are process details; only the ordered rows are observable.
    fn eq(&self, other: &Self) -> bool {
        self.iter().eq(other.iter())
    }
}
impl Eq for Outbox {}

impl From<Vec<Pending>> for Outbox {
    /// Index a loaded or synthetic file without changing its row order.
    fn from(rows: Vec<Pending>) -> Self {
        let mut outbox = Self::default();
        for row in rows {
            outbox.push(row);
        }
        outbox
    }
}

impl<'a> IntoIterator for &'a Outbox {
    type Item = &'a Pending;
    type IntoIter = Iter<'a>;
    /// Borrow the live FIFO rows.
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl IntoIterator for Outbox {
    type Item = Pending;
    type IntoIter = std::vec::IntoIter<Pending>;
    /// Take the rows in FIFO order, independently of the hash table's layout.
    fn into_iter(mut self) -> Self::IntoIter {
        let mut rows = Vec::with_capacity(self.len());
        while let Some(id) = self.first {
            rows.push(self.remove_slot(id).expect("head"));
        }
        rows.into_iter()
    }
}

impl Index<usize> for Outbox {
    type Output = Pending;
    /// Positional inspection for callers outside the id-based sender.
    fn index(&self, index: usize) -> &Self::Output {
        self.iter().nth(index).expect("outbox index")
    }
}

impl IndexMut<usize> for Outbox {
    /// Edit a row's payload without changing its allocated id, destination chat or photo ownership.
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        self.revision = NEXT_REVISION.fetch_add(1, Ordering::Relaxed);
        let mut slot = self.first.expect("outbox index");
        for _ in 0..index {
            slot = self.rows[&slot].next.expect("outbox index");
        }
        &mut self.rows.get_mut(&slot).expect("outbox index").value
    }
}

impl Serialize for Outbox {
    /// Preserve the original JSON array contract, including file-order ties.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}

impl<'de> Deserialize<'de> for Outbox {
    /// Restore send indexes from the stored FIFO array.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::<Pending>::deserialize(deserializer).map(Self::from)
    }
}

#[cfg(test)]
mod tests;
