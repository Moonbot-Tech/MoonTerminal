//! Which userdata buffers changed, and whether that change lives in the baked base.
#![cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
//!
//! Zones are drawn inside the base cache (under the candles). Lines, segments and markers
//! are drawn over the blit on every frame, so a line-only or marker-only replacement must
//! upload those buffers and leave the base image alone.

use bytemuck::Pod;

/// Which of the four userdata buffers a replacement actually rewrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UserdataDirty {
    /// Filled bands. These are baked into the base image.
    pub zones: bool,
    /// Horizontal lines. Drawn over the blit.
    pub hlines: bool,
    /// Segments. Drawn over the blit.
    pub segs: bool,
    /// Markers, including trade arrows. Drawn over the blit.
    pub markers: bool,
}

impl UserdataDirty {
    /// Whether the baked base image is stale.
    ///
    /// Only a zone change rebakes it. Rebaking on a line or marker change redraws candles,
    /// the book and the grid for pixels that did not move.
    pub(crate) fn base_stale(self) -> bool {
        self.zones
    }
}

/// Replace `slot` with `next` when the bytes differ.
///
/// Args:
///     slot: The buffer the backend will upload.
///     next: The freshly mapped instances.
///
/// Returns:
///     `true` when `slot` was replaced.
pub(crate) fn replace_pod<T: Pod>(slot: &mut Vec<T>, next: Vec<T>) -> bool {
    if same_pod(slot, &next) {
        return false;
    }
    *slot = next;
    true
}

/// Byte equality of two pod slices, including length.
///
/// One slice compare, so the runtime `memcmp` runs the scan. A hand-rolled byte loop in this
/// crate is opt-level 0 and loses to that on a full order union.
pub(crate) fn same_pod<T: Pod>(left: &[T], right: &[T]) -> bool {
    bytemuck::cast_slice::<T, u8>(left) == bytemuck::cast_slice::<T, u8>(right)
}

/// Index of the first element that differs.
///
/// `left` and `right` have the same length and are not byte-equal. The search compares
/// sub-slices, so each step is a `memcmp`, then returns the element that contains the first
/// different byte.
fn first_diff_elem<T: Pod>(left: &[T], right: &[T]) -> usize {
    let elem = std::mem::size_of::<T>();
    debug_assert!(elem > 0 && left.len() == right.len());
    let lb = bytemuck::cast_slice::<T, u8>(left);
    let rb = bytemuck::cast_slice::<T, u8>(right);
    let mut lo = 0;
    let mut hi = lb.len();
    while lo + 1 < hi {
        let mid = lo + (hi - lo) / 2;
        if lb[lo..mid] == rb[lo..mid] {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo / elem
}

/// How many trailing elements are byte-identical, not overlapping `prefix`.
fn shared_suffix<T: Pod>(left: &[T], right: &[T], prefix: usize) -> usize {
    let elem = std::mem::size_of::<T>();
    let max_elems = left.len().min(right.len()).saturating_sub(prefix);
    if elem == 0 || max_elems == 0 {
        return max_elems;
    }
    let lb = bytemuck::cast_slice::<T, u8>(left);
    let rb = bytemuck::cast_slice::<T, u8>(right);
    let max_bytes = max_elems * elem;
    let mut lo = 0;
    let mut hi = max_bytes;
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if lb[lb.len() - mid..] == rb[rb.len() - mid..] {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo / elem
}

/// Map `src` into `gpu`, rewriting only the elements that differ from `retained`.
///
/// A matching buffer is left untouched after one byte compare. A same-length edit rewrites the
/// middle between the common head and tail. An append or a dropped tail extends or truncates.
/// Anything else, including a `gpu` buffer that is not the image of `retained`, is mapped
/// from scratch.
///
/// Args:
///     src: Instances the caller holds now.
///     retained: Instances last mapped into `gpu`. Replaced with `src` when `gpu` changes.
///     gpu: Mapped buffer.
///     map: One instance to its GPU struct.
///
/// Returns:
///     `true` when `gpu` was rewritten.
pub(crate) fn resync_mapped<T, G, F>(
    src: &[T],
    retained: &mut Vec<T>,
    gpu: &mut Vec<G>,
    mut map: F,
) -> bool
where
    T: Pod,
    G: Copy,
    F: FnMut(&T) -> G,
{
    if gpu.len() != retained.len() {
        gpu.clear();
        gpu.extend(src.iter().map(&mut map));
        retained.clear();
        retained.extend_from_slice(src);
        return true;
    }
    // A zero-sized element has no bytes to compare. Only its length can change.
    if std::mem::size_of::<T>() == 0 {
        if retained.len() == src.len() {
            return false;
        }
        if src.len() > retained.len() {
            gpu.extend(src[retained.len()..].iter().map(&mut map));
            retained.extend_from_slice(&src[retained.len()..]);
        } else {
            gpu.truncate(src.len());
            retained.truncate(src.len());
        }
        return true;
    }
    if retained.len() == src.len() {
        if same_pod(retained, src) {
            return false;
        }
        let prefix = first_diff_elem(retained, src);
        let suffix = shared_suffix(retained, src, prefix).min(src.len() - prefix);
        let end = src.len() - suffix;
        for (slot, item) in gpu[prefix..end].iter_mut().zip(&src[prefix..end]) {
            *slot = map(item);
        }
        retained[prefix..end].copy_from_slice(&src[prefix..end]);
        return true;
    }
    if src.len() > retained.len() && same_pod(retained, &src[..retained.len()]) {
        gpu.extend(src[retained.len()..].iter().map(&mut map));
        retained.extend_from_slice(&src[retained.len()..]);
        return true;
    }
    if src.len() < retained.len() && same_pod(src, &retained[..src.len()]) {
        gpu.truncate(src.len());
        retained.truncate(src.len());
        return true;
    }
    gpu.clear();
    gpu.extend(src.iter().map(map));
    retained.clear();
    retained.extend_from_slice(src);
    true
}

#[cfg(test)]
mod tests;
