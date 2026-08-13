/// Emit build inputs, embed resources, and publish compile-time metadata.
fn main() {
    println!("cargo:rerun-if-changed=../../assets/icons");
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    println!("cargo:rustc-check-cfg=cfg(moon_profile_debug)");
    emit_build_metadata();
    if std::env::var("PROFILE").is_ok_and(|profile| profile == "debug") {
        println!("cargo:rustc-cfg=moon_profile_debug");
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
        if path.extension().and_then(|s| s.to_str()) == Some("png") {
            if let Some(id) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.parse::<usize>().ok())
            {
                ids.push(id);
                max_id = max_id.max(id);
            }
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

    emit_git_rerun_paths(workspace);

    println!(
        "cargo:rustc-env=MOONTERMINAL_GIT_REV={}",
        git_rev(workspace).unwrap_or_else(|| "unknown".to_string())
    );
    println!(
        "cargo:rustc-env=MOONTERMINAL_RELEASE_BASE={}",
        git_release_base(workspace).unwrap_or_else(|| "unknown".to_string())
    );
    println!(
        "cargo:rustc-env=MOONUI_GIT_REV={}",
        moonui_rev(workspace).unwrap_or_else(|| "unknown".to_string())
    );
}

/// Tell Cargo which Git metadata can change the embedded revision and release baseline.
fn emit_git_rerun_paths(repo: &std::path::Path) {
    if let Some(paths) = git_stdout(
        repo,
        [
            "rev-parse",
            "--git-path",
            "HEAD",
            "--git-path",
            "index",
            "--git-path",
            "packed-refs",
            "--git-path",
            "refs/heads",
            "--git-path",
            "refs/tags",
        ],
    ) {
        for path in paths.lines().map(std::path::PathBuf::from) {
            let path = if path.is_absolute() {
                path
            } else {
                repo.join(path)
            };
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    if let Some(paths) = git_stdout(
        repo,
        ["ls-files", "--cached", "--others", "--exclude-standard"],
    ) {
        for path in paths.lines() {
            println!("cargo:rerun-if-changed={}", repo.join(path).display());
        }
    }
}

/// Return the greatest historical or canonical stable tag reachable from the built commit.
///
/// The value is a release baseline for local pre-commit builds, not proof that their bytes equal
/// the tagged release. The release workflow separately enforces exact tag-to-commit equality.
///
/// Args:
///     repo: Repository whose reachable tags define the build baseline.
///
/// Returns:
///     The exact greatest stable Git tag, or `None` for missing, malformed, or ambiguous history.
fn git_release_base(repo: &std::path::Path) -> Option<String> {
    if !repo.is_dir() {
        return None;
    }
    let tags = git_stdout(
        repo,
        [
            "tag",
            "--merged",
            "HEAD",
            "--sort=-version:refname",
            "--list",
            "v*",
        ],
    )?;
    let mut greatest: Option<((u64, u64, u64), &str)> = None;
    let mut seen = Vec::new();
    for tag in tags.lines() {
        let Some(version) = parse_release_tag(tag) else {
            continue;
        };
        if seen
            .iter()
            .any(|(seen_version, seen_tag)| *seen_version == version && *seen_tag != tag)
        {
            return None;
        }
        seen.push((version, tag));
        match greatest {
            Some((current, _)) if version <= current => {}
            _ => greatest = Some((version, tag)),
        }
    }
    greatest.map(|(_, tag)| tag.to_owned())
}

/// Parse a historical two-component or canonical three-component stable release tag.
///
/// Args:
///     tag: Candidate Git tag.
///
/// Returns:
///     A normalized major/minor/patch tuple, using patch zero for a historical tag.
fn parse_release_tag(tag: &str) -> Option<(u64, u64, u64)> {
    let mut components = tag.strip_prefix('v')?.split('.');
    let major = components.next()?;
    let minor = components.next()?;
    let patch = components.next().unwrap_or("0");
    if components.next().is_some()
        || !valid_release_component(major)
        || !valid_release_component(minor)
        || !valid_release_component(patch)
    {
        return None;
    }
    Some((
        major.parse().ok()?,
        minor.parse().ok()?,
        patch.parse().ok()?,
    ))
}

/// Accept one decimal release component without signs or redundant leading zeroes.
fn valid_release_component(component: &str) -> bool {
    !component.is_empty()
        && component.bytes().all(|byte| byte.is_ascii_digit())
        && (component == "0" || !component.starts_with('0'))
}

/// Run Git in one repository and return trimmed UTF-8 stdout on success.
fn git_stdout<const N: usize>(repo: &std::path::Path, args: [&str; N]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_string())
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
        git_rev(&moonui).map(|rev| format!("local:{rev}"))
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

/// Return the short MoonTerminal revision with a dirty-tree suffix when applicable.
fn git_rev(repo: &std::path::Path) -> Option<String> {
    if !repo.is_dir() {
        return None;
    }
    let mut rev = git_stdout(repo, ["rev-parse", "--short=12", "HEAD"])?;
    let dirty = std::process::Command::new("git")
        .args(["-C", repo.to_str()?, "status", "--porcelain"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .is_some_and(|out| !out.stdout.is_empty());
    if dirty {
        rev.push_str("+dirty");
    }
    Some(rev)
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
