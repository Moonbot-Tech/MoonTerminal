//! Row publications share small chunks so a fetch edit cannot copy a whole background snapshot.

use std::ops::{Index, IndexMut, Range};
use std::sync::Arc;

/// Bound one copy-on-write edit independently of the number of deals in the scope.
const CHUNK_ROWS: usize = 64;

/// Ordered rows sharing all chunks until a particular row changes.
#[derive(Debug)]
pub(in crate::analytics::tuner) struct SnapshotRows<T> {
    chunks: Vec<Arc<Vec<T>>>,
    len: usize,
}

impl<T> Default for SnapshotRows<T> {
    /// An empty publication allocates no chunks.
    fn default() -> Self {
        Self {
            chunks: Vec::new(),
            len: 0,
        }
    }
}

impl<T> Clone for SnapshotRows<T> {
    /// Copy chunk handles, never the rows inside them.
    fn clone(&self) -> Self {
        Self {
            chunks: self.chunks.clone(),
            len: self.len,
        }
    }
}

impl<T> FromIterator<T> for SnapshotRows<T> {
    /// Preserve input order, filling every chunk except the last.
    fn from_iter<I: IntoIterator<Item = T>>(rows: I) -> Self {
        let mut rows = rows.into_iter().peekable();
        let mut out = Self::default();
        while rows.peek().is_some() {
            let chunk: Vec<_> = rows.by_ref().take(CHUNK_ROWS).collect();
            out.len += chunk.len();
            out.chunks.push(Arc::new(chunk));
        }
        out
    }
}

impl<T> From<Vec<T>> for SnapshotRows<T> {
    /// Move existing deals into their publication without cloning them.
    fn from(rows: Vec<T>) -> Self {
        rows.into_iter().collect()
    }
}

impl<T> SnapshotRows<T> {
    /// Number of rows, including the partial final chunk.
    pub fn len(&self) -> usize {
        self.len
    }
    /// Whether the publication contains no deals.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Read by the stable index the virtual table uses.
    pub fn get(&self, index: usize) -> Option<&T> {
        self.chunks.get(index / CHUNK_ROWS)?.get(index % CHUNK_ROWS)
    }
    /// Read in publication order with an exact size hint for row maps and permutations.
    pub fn iter(&self) -> RowsIter<'_, T> {
        RowsIter {
            rows: self,
            range: 0..self.len,
        }
    }
}

impl<T: Clone> SnapshotRows<T> {
    /// Copy only the affected shared chunk before editing a row.
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        if index >= self.len {
            return None;
        }
        Arc::make_mut(self.chunks.get_mut(index / CHUNK_ROWS)?).get_mut(index % CHUNK_ROWS)
    }
}

impl<T> Index<usize> for SnapshotRows<T> {
    type Output = T;
    /// Indexed readers do not depend on chunk boundaries.
    fn index(&self, index: usize) -> &T {
        &self.chunks[index / CHUNK_ROWS][index % CHUNK_ROWS]
    }
}

impl<T: Clone> IndexMut<usize> for SnapshotRows<T> {
    /// Indexed edits have the same bounded copy behavior as `get_mut`.
    fn index_mut(&mut self, index: usize) -> &mut T {
        self.get_mut(index).expect("row index in publication")
    }
}

/// A bidirectional, exactly sized cursor over a row publication.
pub(in crate::analytics::tuner) struct RowsIter<'a, T> {
    rows: &'a SnapshotRows<T>,
    range: Range<usize>,
}

impl<T> Clone for RowsIter<'_, T> {
    /// Copy only the cursor of a read pass.
    fn clone(&self) -> Self {
        Self {
            rows: self.rows,
            range: self.range.clone(),
        }
    }
}

impl<'a, T> Iterator for RowsIter<'a, T> {
    type Item = &'a T;
    /// Advance across chunk boundaries without flattening the table.
    fn next(&mut self) -> Option<Self::Item> {
        self.range.next().map(|index| &self.rows[index])
    }
    /// Reserve exactly enough space when collecting a pass.
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.range.size_hint()
    }
}

impl<T> DoubleEndedIterator for RowsIter<'_, T> {
    /// Read newest rows first for the retained-tape cap.
    fn next_back(&mut self) -> Option<Self::Item> {
        self.range.next_back().map(|index| &self.rows[index])
    }
}

impl<T> ExactSizeIterator for RowsIter<'_, T> {}

impl<'a, T> IntoIterator for &'a SnapshotRows<T> {
    type Item = &'a T;
    type IntoIter = RowsIter<'a, T>;
    /// Borrow a publication for a complete read pass.
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests;
