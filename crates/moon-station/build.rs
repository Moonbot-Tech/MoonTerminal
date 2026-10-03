// The release baseline and revision, by the terminal's own rules: the station's version is the
// terminal's release tag (STATION.md §6), and its updater compares it with the latest release.
#[path = "../build-support/git_meta.rs"]
mod git_meta;

/// Embed station release metadata and reserve an 8 MiB main stack for MSVC binaries.
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
    // (`auto_update.rs`), and only from a clean tree: edited sources on a tagged commit are not
    // the release. The rerun triggers above cover a tag added or moved and every tracked file.
    let clean = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .current_dir(workspace)
        .output()
        .is_ok_and(|out| out.status.success() && out.stdout.is_empty());
    let exact = std::process::Command::new("git")
        .args(["tag", "--points-at", "HEAD", "--list", "v*"])
        .current_dir(workspace)
        .output()
        .ok()
        .filter(|out| clean && out.status.success())
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    println!("cargo:rustc-env=MOONSTATION_EXACT_TAGS={exact}");
    // Give station binaries an 8 MiB main stack on MSVC, whose default is 1 MiB.
    // moon_tg::warm_locales() initializes its dictionary on a separate 8 MiB thread.
    if std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|env| env == "msvc") {
        println!("cargo:rustc-link-arg-bins=/STACK:8388608");
    }
}
