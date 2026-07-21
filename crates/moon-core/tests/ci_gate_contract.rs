//! Static contract on `.github/workflows/build.yml`: CI must actually RUN the whole test
//! suite on pull requests, and must treat a failure as blocking.
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

use std::path::PathBuf;

/// The job key this guard is pinned to.
///
/// Pinned by NAME rather than by "whichever job mentions `cargo test`": with the loose form, a
/// second test-running job added above this one in the file would silently capture every
/// assertion below, leaving the real gate unchecked while the suite still reported green.
const TEST_JOB: &str = "tests";

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

/// Breakage guarded: demoting the gate in `.github/workflows/build.yml` — `continue-on-error:
/// true` added to the `tests` job to unblock an unrelated PR (matching what `macos-probe`
/// legitimately carries), or a job-level `if:` that quietly skips it. The check would still be
/// reported, and still look like a check, while a red suite merged green.
#[test]
fn the_test_job_is_a_gate_not_a_probe() {
    let text = workflow_text();
    let body = job_body(&text, TEST_JOB)
        .unwrap_or_else(|| panic!("build.yml must keep a `{TEST_JOB}:` job"));

    // `continue-on-error: false` is equivalent to omitting the key, so judge the value, not the
    // key's presence.
    for line in &body {
        let Some(value) = line.trim().strip_prefix("continue-on-error:") else {
            continue;
        };
        assert_eq!(
            value.trim(),
            "false",
            "job `{TEST_JOB}` runs the suite, so a failure must block: `{}`",
            line.trim()
        );
    }

    // A job-level key sits at four spaces; a step-level `if:` is nested deeper and is not this
    // test's business.
    let skipped = body.iter().find(|line| line.starts_with("    if:"));
    assert!(
        skipped.is_none(),
        "job `{TEST_JOB}` must run unconditionally: `{}`",
        skipped.map(|l| l.trim()).unwrap_or_default()
    );
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
