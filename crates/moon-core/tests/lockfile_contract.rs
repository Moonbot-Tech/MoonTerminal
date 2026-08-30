//! Static contract on the committed `Cargo.lock`: the frozen third-party dependency set CI
//! trusts, and cargo-deny's `sources` gate (`deny.toml`) scans, must actually be what it claims
//! to be — present, pinned to git (never a local path), and drawn only from reviewed hosts.
//!
//! Deliberately textual, like `moonproto_feature_guard.rs` and `ci_gate_contract.rs` next to it:
//! the workspace declares no dev-dependencies, and a TOML parser is not worth becoming the first
//! one. `Cargo.lock`'s `[[package]]` blocks are blank-line separated with unindented `key =
//! "value"` lines — a format stable enough to walk by hand.
//!
//! What it does NOT catch, stated plainly rather than implied:
//!
//! - **The pinned revision itself.** MoonUI's `#<rev>` legitimately moves on every CI-driven
//!   `cargo update -p moon-gpui -p moon-gpui-platform -p moon-ui` refresh; asserting a fixed
//!   revision would make this file redden on every ordinary MoonUI advance.
//! - **A drifted lock nobody committed.** These assertions only run against whatever
//!   `Cargo.lock` is on disk; they cannot tell a stale-but-honest lock from a stale-and-silently-
//!   diverged one. That is `cargo fetch --locked`'s job in `build.yml`, run before anything here.
//! - **The sibling MoonUI override workflow.** `docs/ARCHITECTURE.md` ("Локальная разработка")
//!   documents an ignored `.cargo/config.toml` at the workspace root that `[patch]`es the three
//!   MoonUI crates to a local checkout — which legitimately rewrites `Cargo.lock` with `path`
//!   entries for the duration of that work. CI never has that file, so the invariant below still
//!   holds there; a developer running with the override active would otherwise see this file
//!   redden for a reason that has nothing to do with a bad commit, so the lock-content checks
//!   return early when the override is active.

use std::path::PathBuf;

fn workspace_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("moon-core must live under workspace/crates")
        .to_path_buf()
}

fn lockfile_path() -> PathBuf {
    workspace_dir().join("Cargo.lock")
}

fn gitignore_path() -> PathBuf {
    workspace_dir().join(".gitignore")
}

fn lock_text() -> String {
    let path = lockfile_path();
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The documented local-override marker (`docs/ARCHITECTURE.md`, "Локальная разработка"). Its
/// presence means the on-disk `Cargo.lock` was deliberately rewritten with `path` entries for
/// the three MoonUI crates and no longer reflects what CI, which never has this file, compiles.
fn override_active() -> bool {
    workspace_dir().join(".cargo").join("config.toml").exists()
}

/// One `[[package]]` block: its name, and its raw `source` line's value (`None` for a workspace
/// member, which carries no `source` line at all).
struct Package {
    name: String,
    source: Option<String>,
}

/// Parse `Cargo.lock`'s `[[package]]` blocks the same way
/// `.github/scripts/assert-only-moonui-moved.sh`'s awk reader does: unindented `key = "value"`
/// lines, blocks separated by a blank line (or, for the final block, end of file).
fn parse_packages(text: &str) -> Vec<Package> {
    let mut packages = Vec::new();
    let mut name: Option<String> = None;
    let mut source: Option<String> = None;
    let mut in_package = false;

    // The trailing empty line flushes the final block whether or not the file itself ends in one.
    for line in text.lines().chain(std::iter::once("")) {
        if line == "[[package]]" {
            if let Some(n) = name.take() {
                packages.push(Package {
                    name: n,
                    source: source.take(),
                });
            }
            in_package = true;
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(rest) = line.strip_prefix("name = \"") {
            name = rest.strip_suffix('"').map(str::to_string);
        } else if let Some(rest) = line.strip_prefix("source = \"") {
            source = rest.strip_suffix('"').map(str::to_string);
        } else if line.is_empty() {
            if let Some(n) = name.take() {
                packages.push(Package {
                    name: n,
                    source: source.take(),
                });
            }
            in_package = false;
        }
    }
    packages
}

/// Reduce a raw `source = "..."` value to its bare repository URL: drop the `git+` prefix, the
/// `?branch=...`/`?rev=...` query and the `#<sha>` fragment, and a trailing `.git` — the lock
/// spells some of these forks with the suffix and some without (they are the same repository:
/// compare the `wgpu.git` and `font-kit` entries), and `deny.toml`'s `allow-git` list carries
/// neither form consistently. Returns `None` for a non-git source (a `registry+...` crates.io
/// entry, or a workspace member with no `source` line at all).
fn repo_url(source: &str) -> Option<String> {
    let rest = source.strip_prefix("git+")?;
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    Some(rest.strip_suffix(".git").unwrap_or(rest).to_string())
}

fn deny_toml_path() -> PathBuf {
    workspace_dir().join("deny.toml")
}

/// The `allow-git` list `deny.toml` declares, read the same textual way: everything quoted
/// between `allow-git = [` and the next `]`.
fn deny_allow_git() -> Vec<String> {
    let path = deny_toml_path();
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let start = text
        .find("allow-git = [")
        .expect("deny.toml must declare an `allow-git` allow-list under [sources]");
    let after = &text[start..];
    let end = after
        .find(']')
        .expect("deny.toml's `allow-git` list must be closed with `]`");
    after[..end]
        .lines()
        .filter_map(|line| {
            let line = line.trim().trim_end_matches(',');
            line.strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .map(str::to_string)
        })
        .collect()
}

/// The reviewed set of git hosts this workspace is allowed to depend on — the same seven this
/// task named, kept here as a literal so a drift in *either* `Cargo.lock` or `deny.toml` alone
/// (not both moving together) is what reddens this test.
///
/// A SLICE, not a fixed-size array (`[&str; N]`): the array form encodes its own length in the
/// type, so an eighth entry added to the literal without also bumping the count would fail to
/// compile with `mismatched types` rather than run and redden the named assertion below —
/// exactly the failure mode CONTRIBUTING.md's "the red check names the file" promises against.
const ALLOWED_GIT_REPOS: &[&str] = &[
    "https://github.com/Moonbot-Tech/MoonUI",
    "https://github.com/Moonbot-Tech/MoonProtoBeta",
    "https://github.com/zed-industries/wgpu",
    "https://github.com/zed-industries/async-process",
    "https://github.com/zed-industries/font-kit",
    "https://github.com/zed-industries/xim-rs",
    "https://github.com/smol-rs/async-task",
];

/// Breakage guarded: `Cargo.lock` stops being committed — either the file is deleted outright,
/// or (more likely to happen unnoticed) the `.gitignore` line that used to exclude it comes
/// back, silently un-tracking it on the next `git add`. CI's `cargo fetch --locked` step would
/// then fail on a fresh checkout with no lock to verify, before any of the other jobs in
/// `build.yml` even reach the compile step.
#[test]
fn cargo_lock_is_committed_at_the_workspace_root() {
    assert!(
        lockfile_path().is_file(),
        "Cargo.lock must exist on disk at the workspace root now that CI verifies it with \
         `cargo fetch --locked`: {}",
        lockfile_path().display()
    );

    let gitignore_path = gitignore_path();
    let gitignore = std::fs::read_to_string(&gitignore_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", gitignore_path.display()));
    let ignores_lock = gitignore
        .lines()
        .any(|line| matches!(line.trim(), "Cargo.lock" | "/Cargo.lock" | "**/Cargo.lock"));
    assert!(
        !ignores_lock,
        ".gitignore must not exclude Cargo.lock — third-party versions are frozen by committing \
         it, and an ignore rule would silently stop that from a checkout onward"
    );
}

/// Breakage guarded: a `Cargo.lock` generated while the sibling MoonUI override
/// (`.cargo/config.toml`) was active gets committed by mistake. `[patch]` resolves
/// `moon-gpui`/`moon-gpui-platform`/`moon-ui` (and, via the MoonProto patch, `moonproto`) to a
/// local checkout, so their `[[package]]` blocks lose their `source` line entirely (or, on some
/// cargo versions, carry `source = "path+file://..."`) — a lock that builds only on the machine
/// that committed it, and fails everywhere else, including all of CI.
#[test]
fn moonbot_owned_git_crates_still_pin_to_a_git_source() {
    if override_active() {
        eprintln!(
            "[skip] .cargo/config.toml override is active; the local lock legitimately carries \
             path entries for MoonUI right now, and CI (which never has this file) still enforces \
             the invariant"
        );
        return;
    }
    let packages = parse_packages(&lock_text());
    for name in ["moon-gpui", "moon-gpui-platform", "moon-ui", "moonproto"] {
        let pkg = packages
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("Cargo.lock must list a `{name}` package"));
        let source = pkg.source.as_deref().unwrap_or_else(|| {
            panic!(
                "`{name}` lost its `source` line — the lock was likely committed while the \
                 sibling MoonUI override was active; every other machine (and all of CI) has no \
                 such local path"
            )
        });
        assert!(
            source.starts_with("git+https://github.com/Moonbot-Tech/"),
            "`{name}` must stay pinned to a Moonbot-Tech git source, not `{source}` — a lock \
             committed while the sibling override was active carries a local `path` entry instead"
        );
    }
}

/// Breakage guarded: the set of git repositories `Cargo.lock` actually depends on drifts from
/// the reviewed allow-list — a new transitive git dependency pulled in by a MoonUI refresh, or a
/// fork moved to an unreviewed host — while `deny.toml`'s `allow-git` either silently blesses it
/// (defeating the supply-chain gate) or is edited to match without the corresponding review this
/// task's header describes ("adding a URL here is the deliberate act of approving a new source").
#[test]
fn git_dependency_hosts_match_the_reviewed_allow_list_and_deny_toml() {
    if override_active() {
        eprintln!(
            "[skip] .cargo/config.toml override is active; the local git source set is not the \
             one CI compiles"
        );
        return;
    }
    let packages = parse_packages(&lock_text());
    let mut lock_repos: Vec<String> = packages
        .iter()
        .filter_map(|p| p.source.as_deref())
        .filter_map(repo_url)
        .collect();
    lock_repos.sort();
    lock_repos.dedup();

    let mut expected: Vec<String> = ALLOWED_GIT_REPOS.iter().map(|s| s.to_string()).collect();
    expected.sort();

    assert_eq!(
        lock_repos, expected,
        "Cargo.lock's git dependency hosts drifted from the reviewed allow-list. If this is a \
         deliberate new dependency (CONTRIBUTING.md, \"New git dependency\"), approve its URL in \
         BOTH places in the SAME commit: `ALLOWED_GIT_REPOS` in \
         crates/moon-core/tests/lockfile_contract.rs, and `allow-git` in deny.toml"
    );

    let mut deny_repos = deny_allow_git();
    deny_repos.sort();
    deny_repos.dedup();

    assert_eq!(
        deny_repos, expected,
        "deny.toml's `[sources] allow-git` list drifted from the same allow-list this test \
         hardcodes in `ALLOWED_GIT_REPOS` (crates/moon-core/tests/lockfile_contract.rs). Edit \
         deny.toml's allow-git to match — both lists must name the same hosts in the same commit"
    );
}
