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
    // The tags on the built commit itself: only a build of a release tag updates itself
    // (`auto_update.rs`). The rerun triggers above cover a tag added or moved.
    let exact = std::process::Command::new("git")
        .args(["tag", "--points-at", "HEAD", "--list", "v*"])
        .current_dir(workspace)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    println!("cargo:rustc-env=MOONSTATION_EXACT_TAGS={exact}");
}
