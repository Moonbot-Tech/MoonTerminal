//! The build's Git identity, shared by the build scripts of every binary the release ships — the
//! terminal (`moon-ui-gpui`) and the station (`moon-station`) — through `#[path]`, so both embed
//! the same release baseline by the same rules: the updater of each compares it with the release.

/// Emit `MOONTERMINAL_GIT_REV` and `MOONTERMINAL_RELEASE_BASE` for the repository at `workspace`,
/// with the Git paths that change them as rerun triggers.
pub fn emit_release_metadata(workspace: &std::path::Path) {
    emit_git_rerun_paths(workspace);
    println!(
        "cargo:rustc-env=MOONTERMINAL_GIT_REV={}",
        git_rev(workspace).unwrap_or_else(|| "unknown".to_string())
    );
    println!(
        "cargo:rustc-env=MOONTERMINAL_RELEASE_BASE={}",
        git_release_base(workspace).unwrap_or_else(|| "unknown".to_string())
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

/// Return the short revision of `repo` with a dirty-tree suffix when applicable.
pub fn git_rev(repo: &std::path::Path) -> Option<String> {
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
