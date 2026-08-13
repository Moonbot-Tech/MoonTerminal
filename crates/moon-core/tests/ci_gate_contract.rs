//! Static contracts for the build and release workflows: CI must run the whole test suite on pull
//! requests, and releases must build one exact canonical tag and verify the published executable.
//!
//! This exists because the workflow built the binary and nothing else for the project's whole
//! history, so a test suite that did not even compile sat on `main` unnoticed. The guard is
//! deliberately textual, like `moonproto_feature_guard.rs` next to it: the workspace declares
//! no dev-dependencies, and a YAML parser is not worth becoming the first one.
//!
//! What it does NOT catch, stated plainly rather than implied:
//!
//! - **Deleting the job.** These assertions only run because CI runs the suite. Remove the job
//!   and they stop running in CI too — they would redden only for someone running `cargo test`
//!   locally. The same goes for any narrowing that drops `moon-core` from the run.
//! - **Merging past a red check.** `main` has no branch protection, so nothing mechanically
//!   stops a red merge. Closing both holes needs a required status check, which is a repository
//!   admin setting and cannot be expressed in this file.
//! - **A deliberately defeated command.** The command shape is checked, not its runtime
//!   behaviour; someone determined to neuter the gate can. The target is the plausible
//!   accident — shaving CI minutes, silencing a red job to land something else — not sabotage.
//! - **Narrowing the trigger rather than the job.** [`the_workflow_still_runs_on_pull_requests`]
//!   only checks that the trigger exists. Adding `paths:` or `branches:` filters under it would
//!   quietly exclude pull requests from CI while the assertion still passes.
//! - **A wholesale reindent of the YAML**, or renaming the job. Both fail loudly rather than
//!   silently, which is the safe direction for a gate — fix [`TEST_JOB`] and move on.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The job key this guard is pinned to.
///
/// Pinned by NAME rather than by "whichever job mentions `cargo test`": with the loose form, a
/// second test-running job added above this one in the file would silently capture every
/// assertion below, leaving the real gate unchecked while the suite still reported green.
const TEST_JOB: &str = "tests";

/// Every job that compiles the workspace and therefore runs the lockfile contract's three steps
/// (verify -> refresh MoonUI -> assert nothing else moved). `audit` is deliberately excluded: it
/// runs cargo-deny over the already-committed lock and never touches it.
const COMPILING_JOBS: [&str; 3] = ["windows", TEST_JOB, "macos-probe"];

/// The two compiling jobs that BLOCK a merge — the Windows `.exe` release gate and the `tests`
/// gate — and so must neither be silently skipped nor downgraded to a probe. `macos-probe` is
/// deliberately excluded here even though it shares the same `if:` condition: it already carries
/// `continue-on-error: true` and never blocks, so there is no gate left for a quiet skip to hide
/// behind, and it needs no guard of its own.
const GATING_JOBS: [&str; 2] = ["windows", TEST_JOB];

/// The only job-level `if:` a `GATING_JOBS` member may carry, normalized the same way
/// [`normalize_if_condition`] normalizes what it reads from `build.yml`. The weekly `schedule:`
/// trigger (`build.yml`'s `on:` header) exists for the `audit` job alone, so the two gates skip
/// it and nothing else.
const EXPECTED_SCHEDULE_EXCLUSION: &str = "github.event_name != 'schedule'";

/// Normalize a job-level `if:` line for comparison against [`EXPECTED_SCHEDULE_EXCLUSION`]:
/// strip the `if:` prefix, unwrap the optional `${{ ... }}` expression form (GitHub accepts a
/// bare condition and an explicitly-wrapped one equally), collapse internal whitespace, and fold
/// `'schedule'`/`"schedule"` to one quote style. A semantics-preserving YAML reformat (requoting,
/// re-wrapping, incidental spacing) must not redden the guard that checks this — only an actual
/// widening of the condition may.
fn normalize_if_condition(line: &str) -> String {
    let after_if = line
        .trim()
        .strip_prefix("if:")
        .unwrap_or_else(|| line.trim())
        .trim();
    let unwrapped = after_if
        .strip_prefix("${{")
        .and_then(|s| s.strip_suffix("}}"))
        .map(str::trim)
        .unwrap_or(after_if);
    unwrapped
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('"', "'")
}

/// The third blocking gate CONTRIBUTING.md promises, alongside `TEST_JOB` and the Windows `.exe`
/// job: cargo-deny over the committed lock. Runs on its own weekly `schedule:` trigger too, so an
/// advisory published against a pinned version doesn't sit unnoticed between PRs — the three
/// `COMPILING_JOBS` skip that trigger instead.
const AUDIT_JOB: &str = "audit";

fn workflow_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("moon-core must live under workspace/crates")
        .join(".github/workflows/build.yml")
}

fn workflow_text() -> String {
    let path = workflow_path();
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn release_workflow_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("moon-core must live under workspace/crates")
        .join(".github/workflows/release.yml")
}

fn release_workflow_text() -> String {
    let path = release_workflow_path();
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn release_verifier_text() -> String {
    let path = release_workflow_path()
        .parent()
        .expect("release workflow must have a parent")
        .parent()
        .expect("workflows must live under .github")
        .join("scripts/verify-release-assets.sh");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn release_validator_text() -> String {
    let path = release_workflow_path()
        .parent()
        .expect("release workflow must have a parent")
        .parent()
        .expect("workflows must live under .github")
        .join("scripts/validate-release-tag.sh");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn release_publisher_text() -> String {
    let path = release_workflow_path()
        .parent()
        .expect("release workflow must have a parent")
        .parent()
        .expect("workflows must live under .github")
        .join("scripts/publish-verified-release.sh");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Remove shell comment lines so disabled security checks cannot satisfy static contracts.
fn shell_code(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Body lines of the named job, comments stripped.
///
/// A job key is the only thing indented exactly two spaces inside the `jobs:` section, so the
/// split needs no YAML parser.
fn job_body<'a>(text: &'a str, want: &str) -> Option<Vec<&'a str>> {
    let after_jobs = text
        .split_once("\njobs:\n")
        .map(|(_, rest)| rest)
        .expect("build.yml must declare a top-level `jobs:` section");

    let is_comment = |line: &str| line.trim_start().starts_with('#');
    let is_job_key = |line: &str| {
        line.starts_with("  ")
            && !line.starts_with("   ")
            && line.trim_end().ends_with(':')
            && !is_comment(line)
    };

    let mut body: Option<Vec<&str>> = None;
    for line in after_jobs.lines() {
        if is_job_key(line) {
            if body.is_some() {
                break; // the next job starts here
            }
            if line.trim().trim_end_matches(':') == want {
                body = Some(Vec::new());
            }
        } else if let Some(body) = body.as_mut() {
            if !is_comment(line) {
                body.push(line);
            }
        }
    }
    body
}

/// The `cargo test` command a job body runs.
///
/// Anchored on `run:` rather than matched anywhere in the body, because a step's `name:` line
/// precedes its `run:` line — a step renamed to "Run cargo test suite" would otherwise be
/// picked up as the command itself. Both the inline and the block form (`run: |` with the
/// command on the following line) are accepted: a meaning-preserving reformat must not turn the
/// gate red.
fn run_command(body: &[&str]) -> Option<String> {
    for (i, line) in body.iter().enumerate() {
        let Some(rest) = line.trim().strip_prefix("run:") else {
            continue;
        };
        let rest = rest.trim();
        let cmd = if rest.is_empty() || rest.starts_with('|') || rest.starts_with('>') {
            body[i + 1..]
                .iter()
                .map(|l| l.trim())
                .find(|l| !l.is_empty())?
                .to_string()
        } else {
            rest.to_string()
        };
        if cmd.contains("cargo test") {
            return Some(cmd);
        }
    }
    None
}

fn test_command(text: &str) -> String {
    let body = job_body(text, TEST_JOB)
        .unwrap_or_else(|| panic!("build.yml must keep a `{TEST_JOB}:` job that runs the suite"));
    run_command(&body).unwrap_or_else(|| {
        panic!("job `{TEST_JOB}` must run `cargo test` — CI that only builds lets a broken suite reach main")
    })
}

/// Breakage guarded: narrowing the suite in `.github/workflows/build.yml` — the `tests` job's
/// command changed from `cargo test --workspace` to `cargo test --workspace --exclude
/// moon-ui-gpui` (or `-p moon-core`, or `--lib`, or `--no-run`) to shave CI minutes off the
/// heaviest crate. Test binaries would stop being compiled or run, and a break in them would
/// reach `main` silently — the exact failure this job was added to end.
#[test]
fn ci_runs_the_whole_workspace_test_suite() {
    let cmd = test_command(&workflow_text());

    assert!(
        cmd.starts_with("cargo test"),
        "job `{TEST_JOB}` must invoke cargo test directly, so its exit status gates the job: `{cmd}`"
    );

    // Chaining lets the suite fail while the step still succeeds (`cargo test ...; exit 0`).
    for sep in [";", "&&", "||", "|"] {
        assert!(
            !cmd.contains(sep),
            "job `{TEST_JOB}` must not chain anything onto the suite — `{sep}` can swallow a \
             failure: `{cmd}`"
        );
    }

    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    assert!(
        tokens.contains(&"--workspace"),
        "job `{TEST_JOB}` must test the WHOLE workspace, not one crate: `{cmd}`"
    );

    // Each of these keeps `--workspace` while silently dropping targets or tests from the run.
    // `--exclude` is matched in its `=` form too, and `--` because everything after it is a
    // filter handed to the test harness.
    for narrowing in [
        "--exclude",
        "--no-run",
        "--lib",
        "--bins",
        "--doc",
        "--skip",
        "--exact",
        "--",
    ] {
        let hit = tokens
            .iter()
            .find(|t| **t == narrowing || t.starts_with(&format!("{narrowing}=")));
        assert!(
            hit.is_none(),
            "job `{TEST_JOB}` must not narrow the suite with `{}`: `{cmd}`",
            hit.unwrap_or(&narrowing)
        );
    }
}

/// Breakage guarded: demoting a gate in `.github/workflows/build.yml` — `continue-on-error:
/// true` added to `windows` or `tests` to unblock an unrelated PR (matching what `macos-probe`
/// legitimately carries), or a job-level `if:` that quietly skips one of them on a trigger that
/// matters (`push`, `pull_request`, `workflow_dispatch`) — e.g. narrowing the Windows `.exe`
/// gate's condition to `github.event_name == 'push'` would silently skip the release-binary gate
/// on every PR with nothing reddening. The check would still be reported, and still look like a
/// check, while a red PR merged green.
///
/// The ONE narrowing either job accepts: [`EXPECTED_SCHEDULE_EXCLUSION`], compared after
/// [`normalize_if_condition`] — added because the weekly audit-only cron (`build.yml`'s `on:`
/// header) has no reason to also recompile the whole workspace. Anything else that happens to
/// exclude `schedule` too — but excludes something else alongside it — must still redden; only
/// that exact condition, in any equally-valid YAML spelling, is whitelisted.
#[test]
fn the_gating_jobs_stay_gates_not_probes() {
    let text = workflow_text();
    for job in GATING_JOBS {
        let body =
            job_body(&text, job).unwrap_or_else(|| panic!("build.yml must keep a `{job}:` job"));

        // `continue-on-error: false` is equivalent to omitting the key, so judge the value, not
        // the key's presence.
        for line in &body {
            let Some(value) = line.trim().strip_prefix("continue-on-error:") else {
                continue;
            };
            assert_eq!(
                value.trim(),
                "false",
                "job `{job}` blocks a merge, so a failure must fail the check: `{}`",
                line.trim()
            );
        }

        // A job-level key sits at four spaces; a step-level `if:` is nested deeper and is not
        // this test's business.
        let skipped = body.iter().find(|line| line.starts_with("    if:"));
        if let Some(line) = skipped {
            assert_eq!(
                normalize_if_condition(line),
                EXPECTED_SCHEDULE_EXCLUSION,
                "job `{job}` must run on every trigger that matters (push, pull_request, \
                 workflow_dispatch) — the only accepted narrowing skips the audit-only weekly \
                 schedule, nothing broader: `{}`",
                line.trim()
            );
        }
    }
}

/// Breakage guarded: dropping `pull_request` from `.github/workflows/build.yml`'s `on:`
/// triggers — narrowing CI to pushes to save minutes on branch churn. Every job, this one
/// included, would keep passing on `main` while pull requests stopped being checked at all,
/// so a broken suite would be discovered only after it had already landed.
#[test]
fn the_workflow_still_runs_on_pull_requests() {
    let text = workflow_text();
    let triggers = text
        .split_once("\non:\n")
        .and_then(|(_, rest)| rest.split_once("\njobs:\n"))
        .map(|(on, _)| on)
        .expect("build.yml must declare `on:` before `jobs:`");

    // Exact, not a prefix: `pull_request_target:` is a different trigger that runs against the
    // base ref, and must not satisfy this.
    assert!(
        triggers.lines().any(|line| line.trim() == "pull_request:"),
        "build.yml must stay triggered by pull_request, or nothing is checked before merge:\n{triggers}"
    );
}

/// Breakage guarded: `.github/workflows/build.yml`'s `audit` job — the third of the three
/// blocking gates CONTRIBUTING.md now promises — gets deleted, demoted with `continue-on-error:
/// true` (matching what `macos-probe` legitimately carries), or narrowed to drop the `sources`
/// (or `advisories`) check from cargo-deny's `command:`. Any of the three lets a supply-chain
/// regression — an advisory published against a pinned version, or a git source nobody approved
/// — merge silently while the job still looks green, or simply stops existing.
#[test]
fn the_audit_job_is_a_gate_that_actually_runs_cargo_deny() {
    let text = workflow_text();
    let body = job_body(&text, AUDIT_JOB).unwrap_or_else(|| {
        panic!("build.yml must keep an `{AUDIT_JOB}:` job — CONTRIBUTING.md promises three blocking gates")
    });

    // `continue-on-error: false` is equivalent to omitting the key, so judge the value, not the
    // key's presence — same convention as `the_gating_jobs_stay_gates_not_probes`.
    for line in &body {
        let Some(value) = line.trim().strip_prefix("continue-on-error:") else {
            continue;
        };
        assert_eq!(
            value.trim(),
            "false",
            "job `{AUDIT_JOB}` runs the supply-chain gate, so a failure must block: `{}`",
            line.trim()
        );
    }

    // A job-level key sits at four spaces, same as `the_gating_jobs_stay_gates_not_probes`.
    // Unlike `GATING_JOBS`, `audit` legitimately has no `if:` at all — the weekly `schedule:`
    // trigger exists FOR this job, so it must run on every trigger, not skip any of them.
    let skipped = body.iter().find(|line| line.starts_with("    if:"));
    assert!(
        skipped.is_none(),
        "job `{AUDIT_JOB}` must run unconditionally — the weekly schedule trigger exists for it: `{}`",
        skipped.map(|l| l.trim()).unwrap_or_default()
    );

    let command_line = body
        .iter()
        .find(|line| line.trim().starts_with("command:"))
        .unwrap_or_else(|| panic!("job `{AUDIT_JOB}` must invoke cargo-deny's `command:` input"));
    let command = command_line
        .trim()
        .strip_prefix("command:")
        .expect("matched by the find() above")
        .trim();

    for check in ["advisories", "sources"] {
        assert!(
            command.split_whitespace().any(|token| token == check),
            "job `{AUDIT_JOB}` must run cargo-deny's `{check}` check — dropping it lets exactly \
             the regression this gate exists for merge unnoticed: `{command}`"
        );
    }
}

/// Breakage guarded: a well-meaning reorder in `.github/workflows/build.yml`'s lockfile contract
/// (verify -> refresh MoonUI -> assert nothing else moved), in any compiling job. `cargo fetch
/// --locked` only proves the committed lock agrees with the manifests when it runs BEFORE
/// anything can rewrite the lock; run after `cargo update`, it only proves the lock agrees with
/// itself. `assert-only-moonui-moved.sh` only has a refreshed lock to inspect once `cargo
/// update` has actually run — moved ahead of it, or deleted outright, a MoonUI refresh that
/// drags an unaudited third-party version along goes unnoticed while the job stays green.
#[test]
fn the_lockfile_contracts_three_steps_run_in_order_in_every_compiling_job() {
    let text = workflow_text();
    for job in COMPILING_JOBS {
        let body =
            job_body(&text, job).unwrap_or_else(|| panic!("build.yml must keep a `{job}:` job"));

        let fetch_idx = body
            .iter()
            .position(|line| {
                line.trim_start().starts_with("run:") && line.contains("cargo fetch --locked")
            })
            .unwrap_or_else(|| {
                panic!("job `{job}` must verify the committed lockfile with `cargo fetch --locked`")
            });
        let update_idx = body
            .iter()
            .position(|line| line.trim_start().starts_with("run:") && line.contains("cargo update"))
            .unwrap_or_else(|| {
                panic!("job `{job}` must refresh MoonUI with a `cargo update` step")
            });
        let assert_idx = body
            .iter()
            .position(|line| {
                line.trim_start().starts_with("run:")
                    && line.contains("assert-only-moonui-moved.sh")
            })
            .unwrap_or_else(|| {
                panic!(
                    "job `{job}` must run `.github/scripts/assert-only-moonui-moved.sh` — without \
                     it, a MoonUI refresh that drags an unaudited third-party version along goes \
                     unnoticed"
                )
            });

        assert!(
            fetch_idx < update_idx,
            "job `{job}` must run `cargo fetch --locked` BEFORE `cargo update` — afterwards \
             `--locked` only proves the lock agrees with itself, not with the manifests"
        );
        assert!(
            update_idx < assert_idx,
            "job `{job}` must run `assert-only-moonui-moved.sh` AFTER `cargo update` — it \
             inspects the refresh's result, so run any earlier it has nothing yet to compare"
        );
    }
}

/// The three, and only three, crates the CI refresh is allowed to name — matching
/// `build.yml`'s own header ("refreshes the three Moonbot-owned MoonUI crates, and only those").
const MOONUI_REFRESH_CRATES: [&str; 3] = ["moon-gpui", "moon-gpui-platform", "moon-ui"];

/// The `-p <name>` package arguments a `cargo update ...` command line names, in the order they
/// appear. Anchored on the token immediately following each `-p`, so an unrelated flag elsewhere
/// on the line cannot be mistaken for a package name.
fn dash_p_packages(command: &str) -> Vec<&str> {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    tokens
        .windows(2)
        .filter(|pair| pair[0] == "-p")
        .map(|pair| pair[1])
        .collect()
}

/// Breakage guarded: a compiling job's MoonUI refresh drifts from EXACTLY `moon-gpui`,
/// `moon-gpui-platform` and `moon-ui` — a bare `cargo update` (moving every third-party version
/// the committed lock exists to freeze), a `-p moonproto` grown onto it (letting MoonProto move
/// on every CI run instead of by a deliberate, reviewed `cargo update -p moonproto` commit), or
/// one of the three swapped for something else entirely (e.g. `-p serde`, which would leave
/// MoonUI itself never refreshed while still looking like the documented step ran).
#[test]
fn the_moonui_refresh_stays_scoped_and_never_touches_moonproto() {
    let text = workflow_text();
    let mut expected = MOONUI_REFRESH_CRATES.to_vec();
    expected.sort_unstable();

    for job in COMPILING_JOBS {
        let body =
            job_body(&text, job).unwrap_or_else(|| panic!("build.yml must keep a `{job}:` job"));

        let update_lines: Vec<&str> = body
            .iter()
            .filter(|line| line.trim_start().starts_with("run:") && line.contains("cargo update"))
            .copied()
            .collect();

        assert!(
            !update_lines.is_empty(),
            "job `{job}` must refresh MoonUI with a `cargo update` step"
        );

        for line in update_lines {
            let mut named = dash_p_packages(line);
            named.sort_unstable();
            assert_eq!(
                named,
                expected,
                "job `{job}` must refresh exactly the three MoonUI crates \
                 (moon-gpui, moon-gpui-platform, moon-ui) and nothing else — a bare `cargo \
                 update`, a dropped crate, an added one, or `-p moonproto` all defeat the \
                 lockfile freeze: `{}`",
                line.trim()
            );
        }
    }
}

/// Breakage guarded: manual release dispatch checks out the default branch while publishing under
/// the requested tag, or a bare numeric/prerelease tag bypasses release validation. Either change
/// can put bytes from the wrong commit behind a version the updater trusts.
#[test]
fn releases_build_the_exact_canonical_tag() {
    let text = release_workflow_text();
    let validator = release_validator_text();
    let triggers = text
        .split_once("\non:\n")
        .and_then(|(_, rest)| rest.split_once("\npermissions:\n"))
        .map(|(triggers, _)| triggers)
        .expect("release.yml must declare triggers before permissions");

    assert!(
        triggers.contains("tags: [\"v*\"]") && !triggers.contains("[0-9]*"),
        "release.yml must trigger only on v-prefixed tags"
    );
    assert!(
        text.contains("validate-release-tag.sh \"$RELEASE_TAG\"")
            && validator.contains(
                "if [[ ! \"$release_tag\" =~ ^v(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)$ ]]"
            )
            && validator.contains("echo \"[FAIL] release tag must use canonical form"),
        "release.yml must reject non-canonical and prerelease tag forms"
    );
    assert!(
        text.contains("ref: ${{ inputs.tag || github.ref_name }}")
            && text
                .matches("ref: ${{ needs.validate.outputs.commit }}")
                .count()
                == 3
            && text.matches("fetch-depth: 0").count() == 4,
        "validation must resolve the tag and every release job must checkout the immutable commit with full tag history"
    );
    assert!(
        text.contains("group: release-${{ inputs.tag || github.ref_name }}")
            && text.contains("cancel-in-progress: false"),
        "release runs for the same canonical tag must not overlap"
    );
    assert!(
        validator.contains("git show-ref --verify --quiet \"$tag_ref\"")
            && validator.contains("git rev-list -n 1 \"$tag_ref\"")
            && validator.contains("git merge-base --is-ancestor \"$head_commit\" origin/main")
            && validator.contains("release tag aliases an existing stable version")
            && validator.contains("normalized_tag=\"${existing_tag}.0\"")
            && validator.contains("if [[ \"$release_tag\" != \"$latest_tag\" ]]")
            && validator.contains("echo \"[FAIL] only the greatest canonical stable tag"),
        "release validation must bind a real tag to HEAD on main and reject historical versions"
    );
}

/// Replacing semantic mixed-version selection with string-only validation would either reject
/// patch releases or allow `v0.21.0` to alias the historical `v0.21` release.
#[test]
fn release_validator_orders_patch_tags_and_rejects_legacy_aliases() {
    let root = std::env::temp_dir().join(format!(
        "moonterminal-release-policy-{}",
        std::process::id()
    ));
    if root.exists() {
        std::fs::remove_dir_all(&root).expect("remove stale release-policy fixture");
    }
    std::fs::create_dir(&root).expect("create release-policy fixture");
    std::fs::copy(
        release_workflow_path()
            .parent()
            .expect("release workflow has a parent")
            .parent()
            .expect("workflows directory has a parent")
            .join("scripts/validate-release-tag.sh"),
        root.join("validate-release-tag.sh"),
    )
    .expect("copy release validator into fixture");
    run_git(&root, &["init", "-b", "main"]);
    run_git(&root, &["config", "user.name", "MoonTerminal Test"]);
    run_git(
        &root,
        &["config", "user.email", "moonterminal@example.invalid"],
    );

    commit_fixture(&root, "legacy");
    run_git(&root, &["tag", "v0.21"]);
    run_git(&root, &["tag", "v0.21.0"]);
    commit_fixture(&root, "minor");
    run_git(&root, &["tag", "v0.24.0"]);
    run_git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let minor = run_release_validator(&root, "v0.24.0");
    assert!(
        minor.status.success(),
        "minor release rejected: status={} stdout={} stderr={}",
        minor.status,
        String::from_utf8_lossy(&minor.stdout),
        String::from_utf8_lossy(&minor.stderr)
    );

    commit_fixture(&root, "patch");
    run_git(&root, &["tag", "v0.24.1"]);
    run_git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let patch = run_release_validator(&root, "v0.24.1");
    assert!(
        patch.status.success(),
        "patch release rejected: status={} stdout={} stderr={}",
        patch.status,
        String::from_utf8_lossy(&patch.stdout),
        String::from_utf8_lossy(&patch.stderr)
    );
    assert!(!run_release_validator(&root, "v0.24").status.success());
    assert!(!run_release_validator(&root, "v18446744073709551616.0.0")
        .status
        .success());

    commit_fixture(&root, "patch-nine");
    run_git(&root, &["tag", "v0.24.9"]);
    commit_fixture(&root, "patch-ten");
    run_git(&root, &["tag", "v0.24.10"]);
    run_git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let patch_ten = run_release_validator(&root, "v0.24.10");
    assert!(
        patch_ten.status.success(),
        "multi-digit patch release rejected: status={} stdout={} stderr={}",
        patch_ten.status,
        String::from_utf8_lossy(&patch_ten.stdout),
        String::from_utf8_lossy(&patch_ten.stderr)
    );
    run_git(&root, &["checkout", "--detach", "v0.24.9"]);
    let patch_nine = run_release_validator(&root, "v0.24.9");
    assert!(!patch_nine.status.success());
    assert!(String::from_utf8_lossy(&patch_nine.stderr)
        .contains("only the greatest canonical stable tag"));

    run_git(&root, &["checkout", "--detach", "v0.21.0"]);
    let alias = run_release_validator(&root, "v0.21.0");
    assert!(!alias.status.success());
    assert!(String::from_utf8_lossy(&alias.stderr)
        .contains("release tag aliases an existing stable version"));
    std::fs::remove_dir_all(root).expect("remove release-policy fixture");
}

/// Run Git inside the isolated release-policy fixture and require success.
fn run_git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("run Git for release-policy fixture");
    assert!(
        output.status.success(),
        "Git fixture command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Add one fixture commit so release tags can express strict semantic ordering.
fn commit_fixture(root: &Path, label: &str) {
    std::fs::write(root.join("release.txt"), label).expect("write release-policy fixture");
    run_git(root, &["add", "release.txt"]);
    run_git(root, &["commit", "-m", label]);
}

/// Execute the repository's release validator against one isolated fixture tag.
fn run_release_validator(root: &Path, tag: &str) -> Output {
    assert!(
        tag.bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.'),
        "release-policy fixture tag must be shell-safe"
    );
    Command::new(release_script_bash())
        .arg("validate-release-tag.sh")
        .arg(tag)
        .env("GITHUB_OUTPUT", "github-output.txt")
        .current_dir(root)
        .output()
        .expect("run release validator fixture")
}

/// Resolve Git for Windows' Bash instead of the higher-priority WSL compatibility launcher.
#[cfg(windows)]
fn release_script_bash() -> PathBuf {
    let output = Command::new("where.exe")
        .arg("git.exe")
        .output()
        .expect("locate Git for Windows");
    assert!(
        output.status.success(),
        "Git for Windows lookup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("Git path must be UTF-8");
    let bash = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .flat_map(|git| Path::new(git).ancestors().skip(1).take(4))
        .map(|root| root.join("bin/bash.exe"))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| panic!("Git for Windows Bash is missing below: {stdout}"));
    bash
}

/// Resolve the ordinary Bash executable for release scripts on non-Windows CI hosts.
#[cfg(not(windows))]
fn release_script_bash() -> PathBuf {
    PathBuf::from("bash")
}

/// Breakage guarded: a draft-only release is looked up through the published-tag endpoint, or the
/// publisher mutates a tag instead of the exact verified release id. The release would either stay
/// permanently hidden after a 404 or publish different metadata than the updater was meant to trust.
#[test]
fn releases_verify_the_published_windows_digest() {
    let text = release_workflow_text();
    let publish = job_body(&text, "publish").expect("release.yml must keep the publish job");
    let upload = publish
        .iter()
        .position(|line| line.contains("softprops/action-gh-release"))
        .expect("publish must upload a draft release");
    let verify = publish
        .iter()
        .position(|line| line.contains("verify-release-assets.sh"))
        .expect("publish must verify the uploaded release");
    let finalize = publish
        .iter()
        .position(|line| line.contains("publish-verified-release.sh"))
        .expect("publish must finalize the verified draft");
    assert!(
        upload < verify && verify < finalize,
        "digest verification must happen between draft upload and publication"
    );
    assert!(
        publish[upload..verify]
            .iter()
            .any(|line| line.trim() == "draft: true")
            && publish[upload..verify]
                .iter()
                .any(|line| line.trim() == "make_latest: false"),
        "the uploaded release must remain a non-latest draft until verification"
    );

    let verifier = shell_code(&release_verifier_text());
    assert!(
        verifier.contains(
            "if [[ ! \"$release_tag\" =~ ^v(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)$ ]]"
        ),
        "release asset verification must accept only the three-component publication contract"
    );
    for required in [
        "max_release_pages=10",
        "/releases?per_page=${releases_per_page}&page=${page}",
        "select(.tag_name == $tag)",
        "release list must contain exactly one release tagged $release_tag",
        "release_by_id",
        "release_json=\"$(release_by_id \"$listed_release_id\")\"",
        "'[\"MoonTerminal.dmg\",\"MoonTerminal.exe\"]'",
        "remote_digest=",
        "local_digest=",
        "[[ \"${remote_digest,,}\" == \"$local_digest\" ]]",
        "remote_size=",
        "local_size=",
        "[[ \"$remote_size\" == \"$local_size\" ]]",
        "MoonTerminal.exe",
    ] {
        assert!(
            verifier.contains(required),
            "release verifier must check `{required}`"
        );
    }
    assert!(
        !verifier.contains("--argjson current") && !verifier.contains("--argjson found"),
        "release pages and accumulated matches must never be passed through process arguments"
    );

    let publisher = shell_code(&release_publisher_text());
    let preflight = publisher
        .find("/immutable-releases\" --silent")
        .expect("publisher must preflight immutable releases");
    assert!(
        publisher.contains("gh api \"${repository_api}/releases/${release_id}\""),
        "publication must PATCH the exact verified numeric release id"
    );
    let publish_draft = publisher
        .find("--method PATCH")
        .expect("publisher must patch the verified release id");
    let post_publish = publisher
        .find("published \"$release_id\"")
        .expect("publisher must verify the same public immutable release id");
    assert!(
        preflight < publish_draft && publish_draft < post_publish,
        "immutable-release preflight must run before publication and confirmation after it"
    );
    assert!(
        publisher.matches("require_remote_tag_commit \"").count() == 3,
        "publisher must check the remote tag at all three publication boundaries"
    );
    let verified_draft = publisher
        .find("release_id=\"$(bash .github/scripts/verify-release-assets.sh")
        .expect("publisher must capture the verified draft id");
    let pre_publish_tag = publisher
        .find("require_remote_tag_commit \"remote release tag moved before publication\"")
        .expect("publisher must recheck the remote tag before publication");
    assert!(
        verified_draft < pre_publish_tag && pre_publish_tag < publish_draft,
        "publisher must recheck the remote tag between draft verification and numeric-id PATCH"
    );
    assert!(
        !publisher.contains("gh release edit"),
        "draft publication must never resolve its mutation target by tag"
    );
    assert!(
        text.matches("secrets.RELEASE_ADMIN_TOKEN").count() == 1
            && publish
                .iter()
                .filter(|line| line.contains("secrets.RELEASE_ADMIN_TOKEN"))
                .count()
                == 1
            && text.contains("GH_TOKEN: ${{ secrets.RELEASE_ADMIN_TOKEN }}")
            && publisher.contains("RELEASE_ADMIN_TOKEN is required"),
        "only the publication step may use an explicit token with Administration read permission"
    );
}

/// Create an isolated tagged repository, exact release metadata, and a recording fake GitHub CLI.
///
/// `purpose` keeps fixture roots disjoint when Rust runs release-script tests in parallel.
///
/// Returns the fixture root and the tagged commit expected by both draft metadata and publication.
fn create_release_script_fixture(purpose: &str) -> (PathBuf, String) {
    let root = std::env::temp_dir().join(format!(
        "moonterminal-release-{purpose}-{}",
        std::process::id()
    ));
    if root.exists() {
        std::fs::remove_dir_all(&root).expect("remove stale release-publish fixture");
    }
    std::fs::create_dir_all(root.join(".github/scripts")).expect("create release script directory");
    std::fs::create_dir_all(root.join("bin")).expect("create fixture bin directory");
    std::fs::write(
        root.join(".github/scripts/verify-release-assets.sh"),
        release_verifier_text(),
    )
    .expect("write release verifier into fixture");
    std::fs::write(
        root.join(".github/scripts/publish-verified-release.sh"),
        release_publisher_text(),
    )
    .expect("write release publisher into fixture");
    std::fs::write(root.join("MoonTerminal.exe"), b"abc")
        .expect("write local Windows asset fixture");

    run_git(&root, &["init", "-b", "main"]);
    run_git(&root, &["config", "user.name", "MoonTerminal Test"]);
    run_git(
        &root,
        &["config", "user.email", "moonterminal@example.invalid"],
    );
    commit_fixture(&root, "release-script-fixture");
    run_git(&root, &["tag", "-a", "v0.24.2", "-m", "v0.24.2"]);
    let commit = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&root)
            .output()
            .expect("read fixture commit")
            .stdout,
    )
    .expect("fixture commit must be UTF-8")
    .trim()
    .to_owned();
    run_git(&root, &["init", "--bare", "remote.git"]);
    let remote = root.join("remote.git").to_string_lossy().into_owned();
    run_git(&root, &["remote", "add", "origin", &remote]);
    run_git(&root, &["push", "origin", "main", "--tags"]);

    let unrelated = (0..100)
        .map(|index| format!(r#"{{"id":{},"tag_name":"v0.0.{}"}}"#, 10_000 + index, index))
        .collect::<Vec<_>>()
        .join(",");
    std::fs::write(root.join("page-1.json"), format!("[{unrelated}]"))
        .expect("write full first release page");
    let draft = release_fixture_json(&commit, true, false);
    let published = release_fixture_json(&commit, false, true);
    std::fs::write(root.join("page-2.json"), format!("[{draft}]"))
        .expect("write second release page");
    std::fs::write(root.join("draft.json"), draft).expect("write draft release fixture");
    std::fs::write(root.join("published.json"), published)
        .expect("write published release fixture");
    std::fs::write(root.join("bin/gh"), fake_gh_script()).expect("write fake GitHub CLI");
    make_fixture_executable(&root.join("bin/gh"));
    (root, commit)
}

/// Render one independent GitHub release response for the three-byte `abc` Windows fixture.
fn release_fixture_json(commit: &str, draft: bool, immutable: bool) -> String {
    format!(
        r#"{{"id":4242,"tag_name":"v0.24.2","target_commitish":"{commit}","draft":{draft},"prerelease":false,"immutable":{immutable},"assets":[{{"name":"MoonTerminal.dmg","size":7,"digest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"}},{{"name":"MoonTerminal.exe","size":3,"digest":"sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"}}]}}"#
    )
}

/// Render a full release-list page whose ASCII representation exceeds one process argument.
fn oversized_release_page_json() -> String {
    let padding = "x".repeat(2_048);
    let releases = (0..100)
        .map(|index| {
            format!(
                r#"{{"id":{},"tag_name":"v0.0.{}","body":"{padding}"}}"#,
                10_000 + index,
                index
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("[{releases}]")
}

/// Return a fake `gh api` implementation that models draft 404 and records exact PATCH fields.
fn fake_gh_script() -> &'static str {
    r#"#!/usr/bin/env bash
set -euo pipefail
[[ "${1:-}" == "api" ]] || { echo "[FAIL] fake gh accepts only api" >&2; exit 1; }
shift
endpoint="${1:?endpoint is required}"
shift
printf '%s\t%s\n' "$endpoint" "$*" >> "$GH_FIXTURE_DIR/requests.log"

case "$endpoint" in
    repos/Moonbot-Tech/MoonTerminal/immutable-releases)
        exit 0
        ;;
    repos/Moonbot-Tech/MoonTerminal/releases\?per_page=100\&page=*)
        page="${endpoint##*page=}"
        file="$GH_FIXTURE_DIR/page-${page}.json"
        [[ -f "$file" ]] && cat "$file" || printf '[]\n'
        ;;
    repos/Moonbot-Tech/MoonTerminal/releases/4242)
        if [[ " $* " == *" --method PATCH "* ]]; then
            [[ " $* " == *" -F draft=false "* ]]
            [[ " $* " == *" -F prerelease=false "* ]]
            [[ " $* " == *" -f make_latest=true "* ]]
            [[ " $* " != *" -F make_latest=true "* ]]
            : > "$GH_FIXTURE_DIR/published.state"
            cat "$GH_FIXTURE_DIR/published.json"
        elif [[ -f "$GH_FIXTURE_DIR/published.state" ]]; then
            cat "$GH_FIXTURE_DIR/published.json"
        else
            cat "$GH_FIXTURE_DIR/draft.json"
        fi
        ;;
    repos/Moonbot-Tech/MoonTerminal/releases/tags/v0.24.2)
        if [[ -f "$GH_FIXTURE_DIR/published.state" ]]; then
            cat "$GH_FIXTURE_DIR/published.json"
        else
            echo "gh: Not Found (HTTP 404)" >&2
            exit 1
        fi
        ;;
    *)
        echo "[FAIL] unexpected fake gh endpoint: $endpoint" >&2
        exit 1
        ;;
esac
"#
}

/// Give a generated fixture command executable permissions where Unix requires them.
fn make_fixture_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = std::fs::metadata(path)
            .expect("read fixture command metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).expect("make fixture command executable");
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Run one release script with the recording fake GitHub CLI first on `PATH`.
fn run_release_script(root: &Path, script: &str, args: &[&str]) -> Output {
    let mut paths = vec![root.join("bin")];
    if let Some(current) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&current));
    }
    let fixture_path: OsString = std::env::join_paths(paths).expect("join fixture PATH");
    Command::new(release_script_bash())
        .arg(script)
        .args(args)
        .current_dir(root)
        .env("PATH", fixture_path)
        .env("GH_FIXTURE_DIR", root)
        .env("GITHUB_REPOSITORY", "Moonbot-Tech/MoonTerminal")
        .env("GH_TOKEN", "fixture-token")
        .output()
        .expect("run release script fixture")
}

/// Breakage guarded: replacing draft-list discovery with the tag endpoint reproduces GitHub's 404,
/// while publishing by a tag or wrong id can finalize metadata other than the verified draft.
#[test]
fn release_scripts_locate_and_publish_the_exact_draft_by_id() {
    let (root, commit) = create_release_script_fixture("publish");
    let verifier = ".github/scripts/verify-release-assets.sh";
    let publisher = ".github/scripts/publish-verified-release.sh";

    let draft = release_fixture_json(&commit, true, false);
    std::fs::write(root.join("page-2.json"), format!("[{draft}]"))
        .expect("restore exact draft release page");
    std::fs::write(root.join("draft.json"), &draft).expect("restore exact draft release by id");
    std::fs::write(root.join("requests.log"), "").expect("reset fake gh request log");
    let verified = run_release_script(&root, verifier, &["v0.24.2", "MoonTerminal.exe"]);
    assert!(
        verified.status.success() && String::from_utf8_lossy(&verified.stdout).trim() == "4242",
        "draft verification failed: status={} stdout={} stderr={}",
        verified.status,
        String::from_utf8_lossy(&verified.stdout),
        String::from_utf8_lossy(&verified.stderr)
    );
    let verify_calls =
        std::fs::read_to_string(root.join("requests.log")).expect("read draft verification calls");
    assert!(
        verify_calls.contains("releases?per_page=100&page=1")
            && verify_calls.contains("releases?per_page=100&page=2")
            && !verify_calls.contains("releases/tags/v0.24.2"),
        "draft verification did not use bounded list discovery: {verify_calls}"
    );

    let wrong_target = release_fixture_json(&"0".repeat(40), true, false);
    std::fs::write(root.join("page-2.json"), format!("[{wrong_target}]"))
        .expect("write wrong-target release page");
    std::fs::write(root.join("draft.json"), &wrong_target)
        .expect("write wrong-target release by id");
    let rejected = run_release_script(&root, verifier, &["v0.24.2", "MoonTerminal.exe"]);
    assert!(
        !rejected.status.success()
            && String::from_utf8_lossy(&rejected.stderr)
                .contains("release target does not match the tagged commit"),
        "wrong release target was not rejected: status={} stdout={} stderr={}",
        rejected.status,
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    std::fs::write(root.join("page-2.json"), format!("[{draft}]"))
        .expect("restore exact draft release page after rejection");
    std::fs::write(root.join("draft.json"), &draft)
        .expect("restore exact draft release by id after rejection");

    std::fs::write(root.join("requests.log"), "").expect("reset publication request log");
    let published = run_release_script(&root, publisher, &["v0.24.2", &commit, "MoonTerminal.exe"]);
    assert!(
        published.status.success(),
        "release publication failed: status={} stdout={} stderr={}",
        published.status,
        String::from_utf8_lossy(&published.stdout),
        String::from_utf8_lossy(&published.stderr)
    );
    let calls =
        std::fs::read_to_string(root.join("requests.log")).expect("read publication requests");
    let patch = calls
        .lines()
        .position(|line| {
            line.starts_with("repos/Moonbot-Tech/MoonTerminal/releases/4242\t")
                && line.contains("--method PATCH")
                && line.contains("-F draft=false")
                && line.contains("-F prerelease=false")
                && line.contains("-f make_latest=true")
                && !line.contains("-F make_latest=true")
        })
        .expect("publication must PATCH the verified id with correctly typed fields");
    let published_id_read = calls
        .lines()
        .enumerate()
        .find(|(index, line)| {
            *index > patch && *line == "repos/Moonbot-Tech/MoonTerminal/releases/4242\t"
        })
        .map(|(index, _)| index)
        .expect("publication must re-read the same release id");
    let published_tag_read = calls
        .lines()
        .position(|line| line == "repos/Moonbot-Tech/MoonTerminal/releases/tags/v0.24.2\t")
        .expect("publication must confirm public tag discovery");
    assert!(
        patch < published_id_read && published_id_read < published_tag_read,
        "publication did not preserve the verified release id through its postconditions: {calls}"
    );

    std::fs::remove_dir_all(root).expect("remove release-publish fixture");
}

/// Breakage guarded: passing a serialized release page through `--argjson` exceeds one argv
/// element, stranding a healthy draft before its digest can be verified and published.
#[test]
fn release_verifier_streams_release_page_larger_than_one_argument() {
    let (root, _) = create_release_script_fixture("large-page");
    let verifier = ".github/scripts/verify-release-assets.sh";
    let first_page = oversized_release_page_json();
    assert!(
        first_page.len() > 131_072,
        "oversized release page fixture must cross the observed single-argument boundary"
    );
    std::fs::write(root.join("page-1.json"), first_page)
        .expect("write oversized first release page");
    std::fs::write(root.join("requests.log"), "").expect("reset large-page request log");

    let verified = run_release_script(&root, verifier, &["v0.24.2", "MoonTerminal.exe"]);
    assert!(
        verified.status.success() && String::from_utf8_lossy(&verified.stdout).trim() == "4242",
        "large-page draft verification failed: status={} stdout={} stderr={}",
        verified.status,
        String::from_utf8_lossy(&verified.stdout),
        String::from_utf8_lossy(&verified.stderr)
    );
    let calls =
        std::fs::read_to_string(root.join("requests.log")).expect("read large-page requests");
    assert!(
        calls.contains("releases?per_page=100&page=1")
            && calls.contains("releases?per_page=100&page=2")
            && calls.contains("repos/Moonbot-Tech/MoonTerminal/releases/4242\t"),
        "large-page verification did not preserve bounded discovery and id re-read: {calls}"
    );

    std::fs::remove_dir_all(root).expect("remove large-page release fixture");
}
