//! Splitting a strategy's flat `folder_path` into folder segments, and joining them back.
//!
//! A folder exists only as a prefix of strategy paths; the data model has no empty folders. That
//! prefix arrives as ONE flat, unescaped string, so where its segment boundaries fall is decided in
//! exactly one place, [`path_segments`]. The desktop Strategies window and the Telegram Mini App
//! both group strategies by folder, so both split through this module.

/// Iterates nonempty path segments without allocating. This is the one source of path splitting
/// for trees, counts, expansion, operations, and the Mini App's folder groups.
///
/// A slash separates two folders only when NEITHER neighbouring character is whitespace. MoonProto
/// delivers a strategy's placement as ONE flat `StrategySnapshot::path` joined with `/` and with no
/// escaping, while MoonBot allows a `/` INSIDE a folder name — `"EMA / ORGANIC WAVE STRUCTURE
/// STRATEGIES LLM"` is a single folder there. Splitting on every slash therefore invents folders
/// that exist nowhere, and the invented ones are recognisable: they carry a leading or trailing
/// space. This rule is what keeps the window's tree agreeing with MoonBot's.
///
/// The edges follow the same rule, the missing neighbour counting as non-whitespace: `"/a"` splits
/// and its empty leading segment is filtered, while `"/ a"` stays one segment named `"/ a"`.
///
/// [`join_path`] inverts this only for canonical paths: nonempty segments joined with `/`.
/// It does not restore doubled, leading, or trailing separators or replace an input `\`.
pub fn path_segments(path: &str) -> impl Iterator<Item = &str> {
    let mut start = 0usize;
    path.match_indices(['/', '\\'])
        .filter(|&(i, _)| {
            !path[..i]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace)
                && !path[i + 1..]
                    .chars()
                    .next()
                    .is_some_and(char::is_whitespace)
        })
        .map(|(i, _)| i)
        // The tail after the last separator, which has no separator to announce it.
        .chain(std::iter::once(path.len()))
        .map(move |i| {
            let seg = &path[start..i];
            start = i + 1;
            seg
        })
        .filter(|s| !s.is_empty())
}

/// Splits a folder path into owned segments through [`path_segments`].
pub fn split_path(path: &str) -> Vec<String> {
    path_segments(path).map(str::to_string).collect()
}

/// Joins path segments with `/`; for nonempty segments, this inverts [`path_segments`] on
/// canonical paths.
pub fn join_path(parts: &[String]) -> String {
    parts.join("/")
}

#[cfg(test)]
mod tests;
