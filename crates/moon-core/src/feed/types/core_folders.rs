//! The core's own folder tree — including the folders that hold no strategy at all.
//!
//! Until moonproto `55f78d0` a folder existed only as a prefix of some strategy's path, so an empty
//! one could not be represented on the wire and the terminal kept its own local list instead. The
//! core now maintains a VERSIONED folder tree and reports every folder in it, empty ones included
//! (`docs/strats.md`, "Folders, Including Empty Folders"). That makes "new folder" a real edit the
//! core can hold, rather than a mark that lives until the window closes.
//!
//! Whether a given core can do that is not a question this terminal gets to ask directly. The
//! answer is [`CoreFolders::supported`], and it means exactly one thing: a versioned tree has
//! arrived. A core too old to send one leaves it false forever, and every folder path here is then
//! whatever the strategies themselves imply.

/// Folders one core currently holds, as it reports them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoreFolders {
    /// Whether this core has published a versioned folder tree.
    ///
    /// False covers two situations the terminal cannot tell apart and does not need to: a core
    /// whose build predates folder synchronization, and one whose first tree has not arrived yet.
    /// In both, an empty folder cannot be sent, so the caller keeps its local fallback and does not
    /// promise the operator that a new folder will survive.
    ///
    /// Deliberately NOT latched across connections: a replacement feed can point at a different
    /// MoonBot, and a core downgraded below the extension has to be able to go back to "not known".
    pub supported: bool,
    /// Whether this terminal may EDIT that tree.
    ///
    /// Stricter than [`Self::supported`], and the difference is a real account rather than a
    /// hypothetical one. A folder edit is submitted as the complete desired tree, and moonproto
    /// validates every path in it — splitting on each `/` and refusing a segment with surrounding
    /// whitespace. MoonBot, meanwhile, allows a `/` INSIDE a folder name, and for a strategy in one
    /// of those moonproto's own state adds the split halves as parent folders. So a core holding a
    /// folder called `"EMA / ORGANIC"` reports a tree containing `"EMA "`, which nothing can send
    /// back: every folder edit on that core would be refused whole.
    ///
    /// Rather than discover that per command, the terminal asks once. False here means the window
    /// keeps its local marks and promises the operator nothing, exactly as for a core too old to
    /// synchronize folders at all — while [`Self::supported`] still lets it DRAW what the core
    /// reports.
    pub editable: bool,
    /// Every confirmed folder the core reports, parents included, sorted.
    ///
    /// SORTED here rather than kept in whatever order the core hands them over: the protocol calls
    /// that order unspecified, moonproto iterates a map to produce it, and a set that reshuffles
    /// itself would republish as a change on every rehash.
    ///
    /// A DISPLAY projection: paths are cleaned of control characters and clamped, so this is not
    /// the list an edit is built from — the feed builds those from the core's own untouched paths.
    /// Nothing here splits them either. Which slashes separate folders is a question this terminal
    /// answers in ONE place, the Strategies window's own path module, and a second opinion formed
    /// at the boundary would invent folders that exist nowhere.
    pub paths: Vec<String>,
}
