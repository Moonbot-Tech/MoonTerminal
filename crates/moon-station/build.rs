// The release baseline and revision, by the terminal's own rules: the station's version is the
// terminal's release tag (STATION.md §6), and its updater compares it with the latest release.
#[path = "../build-support/git_meta.rs"]
mod git_meta;

/// Embed the release baseline and the revision the station reports and updates from.
fn main() {
    let manifest = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set"),
    );
    let workspace = manifest
        .parent()
        .and_then(std::path::Path::parent)
        .expect("moon-station must live under crates/");
    git_meta::emit_release_metadata(workspace);
}
