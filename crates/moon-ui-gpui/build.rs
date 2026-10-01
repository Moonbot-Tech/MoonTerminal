// The release baseline and revision, shared with the station's build script.
#[path = "../build-support/git_meta.rs"]
mod git_meta;

/// Emit build inputs, embed resources, and publish compile-time metadata.
fn main() {
    println!("cargo:rerun-if-changed=../../assets/icons");
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    println!("cargo:rustc-check-cfg=cfg(moon_profile_debug)");
    println!("cargo:rustc-check-cfg=cfg(uidoc)");
    emit_uidoc_overlay();
    emit_build_metadata();
    if std::env::var("PROFILE").is_ok_and(|profile| profile == "debug") {
        println!("cargo:rustc-cfg=moon_profile_debug");
    }
    // MSVC gives the main thread 1 MiB, and the unoptimized translation table that `i18n!`
    // builds on the first `t!` outgrows it as locales grow; match the 8 MiB of macOS and Linux.
    if std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|env| env == "msvc") {
        println!("cargo:rustc-link-arg-bins=/STACK:8388608");
    }

    // Embed every numerically named group icon (assets/icons/<id>.png) in the executable,
    // avoiding runtime disk paths in development and deployment. Codegen: GROUP_ICONS[id] = Option<&[u8]>.
    if let Err(err) = embed_group_icons() {
        println!("cargo:warning=failed to embed group icons: {err}");
    }

    #[cfg(windows)]
    if let Err(err) = embed_exe_icon() {
        println!("cargo:warning=failed to embed MoonTerminal exe icon: {err}");
    }
}

/// Define `uidoc` when the private UI-atlas overlay is present, and say nothing when it is not.
///
/// The atlas is not part of this repository: it lives in `private/uidoc`, which is excluded
/// locally, and `main.rs` reaches it through a `#[path]` guarded by this cfg. Presence on disk is
/// the switch rather than a Cargo feature, because a feature published here would name files a
/// clone does not have and fail to build the moment anyone enabled it.
///
/// Both the `ui-inspector` feature and the directory are required; the doc comment on the
/// function body says why one of them cannot stand in for the other.
///
/// The rerun path is emitted ONLY when the overlay exists. A `rerun-if-changed` naming a missing
/// file makes Cargo re-run this script - and therefore recompile the whole crate - on every single
/// build, which is a heavy price for a directory that appears once. The consequence is the one
/// documented in `private/README.md`: after creating the overlay for the first time, touch this
/// file (or any git ref, which is already a rerun path) so the cfg is picked up.
fn emit_uidoc_overlay() {
    // Two conditions, because neither alone is enough. The feature says the build was asked for;
    // it cannot say the code is there. The overlay says the code is there; it cannot say the
    // inspector APIs the code calls were compiled, since those hang on `debug-assertions` raised
    // per package by `--profile inspector` and a build script cannot see another package's
    // profile. The feature is the developer's half of that statement.
    if std::env::var_os("CARGO_FEATURE_UI_INSPECTOR").is_none() {
        return;
    }
    let entry = std::path::Path::new("../../private/uidoc/mod.rs");
    if !entry.is_file() {
        println!(
            "cargo:warning=ui-inspector is on but private/uidoc is missing: the atlas is a local \
             overlay and this build has none, so --ui-atlas will do nothing"
        );
        return;
    }
    println!("cargo:rerun-if-changed=../../private/uidoc/mod.rs");
    println!("cargo:rustc-cfg=uidoc");
}

/// Generates `GROUP_ICONS: &[Option<&[u8]>]` (index = icon ID) from `assets/icons/<id>.png`
/// via `include_bytes!` (absolute paths from `CARGO_MANIFEST_DIR`) for inclusion by windowing.rs.
/// Returns an I/O error if the input directory cannot be read or the output cannot be created or written.
fn embed_group_icons() -> std::io::Result<()> {
    use std::io::Write;
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let dir = std::path::Path::new("../../assets/icons");
    let mut max_id = 0usize;
    let mut ids: Vec<usize> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) == Some("png")
            && let Some(id) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.parse::<usize>().ok())
        {
            ids.push(id);
            max_id = max_id.max(id);
        }
    }
    let mut present = vec![false; max_id + 1];
    for id in ids {
        present[id] = true;
    }
    let out =
        std::path::Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("group_icons.rs");
    let mut f = std::fs::File::create(out)?;
    writeln!(f, "pub static GROUP_ICONS: &[Option<&[u8]>] = &[")?;
    for (id, present) in present.iter().enumerate() {
        if *present {
            let path = format!("{manifest}/../../assets/icons/{id}.png").replace('\\', "/");
            writeln!(f, "    Some(include_bytes!(\"{path}\")),")?;
        } else {
            writeln!(f, "    None,")?;
        }
    }
    writeln!(f, "];")?;
    Ok(())
}

/// Emit MoonTerminal and MoonUI revisions plus the stable update baseline.
fn emit_build_metadata() {
    let manifest = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set"),
    );
    let workspace = manifest
        .parent()
        .and_then(std::path::Path::parent)
        .expect("moon-ui-gpui must live under crates/");

    git_meta::emit_release_metadata(workspace);
    println!(
        "cargo:rustc-env=MOONUI_GIT_REV={}",
        moonui_rev(workspace).unwrap_or_else(|| "unknown".to_string())
    );
}

fn moonui_rev(workspace: &std::path::Path) -> Option<String> {
    moonui_rev_from_lock(&workspace.join("Cargo.lock")).or_else(|| {
        let moonui = workspace.parent()?.join("MoonUI");
        if moonui.is_dir() {
            println!(
                "cargo:rerun-if-changed={}",
                moonui.join(".git").join("HEAD").display()
            );
        }
        git_meta::git_rev(&moonui).map(|rev| format!("local:{rev}"))
    })
}

fn moonui_rev_from_lock(lock_path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(lock_path).ok()?;
    let mut in_moon_gpui = false;
    for line in text.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            in_moon_gpui = false;
            continue;
        }
        if line == "name = \"moon-gpui\"" {
            in_moon_gpui = true;
            continue;
        }
        if in_moon_gpui && line.starts_with("source = \"git+https://github.com/Moonbot-Tech/MoonUI")
        {
            return line
                .rsplit_once('#')
                .map(|(_, rev)| rev.trim_end_matches('"').to_string());
        }
    }
    None
}

#[cfg(windows)]
fn embed_exe_icon() -> std::io::Result<()> {
    use std::fs::File;
    use std::path::Path;

    let png = File::open("../../assets/icons/0.png")?;
    let image = ico::IconImage::read_png(png)?;

    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    dir.add_entry(ico::IconDirEntry::encode(&image)?);

    let out = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("moon-terminal.ico");
    dir.write(File::create(&out)?)?;

    let mut res = winresource::WindowsResource::new();
    // ID must be exactly "1": MoonUI loads the window icon as LoadImageW(module, MAKEINTRESOURCE(1)).
    // winresource also defaults to ID 1; any different explicit ID would break the window/taskbar icon lookup.
    res.set_icon_with_id(out.to_str().expect("icon path must be valid UTF-8"), "1");
    res.compile()
}
