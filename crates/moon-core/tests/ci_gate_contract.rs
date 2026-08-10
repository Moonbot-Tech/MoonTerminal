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
    let after_if = line.trim().strip_prefix("if:").unwrap_or_else(|| line.trim()).trim();
    let unwrapped = after_if
        .strip_prefix("${{")
        .and_then(|s| s.strip_suffix("}}"))
        .map(str::trim)
        .unwrap_or(after_if);
    unwrapped.split_whitespace().collect::<Vec<_>>().join(" ").replace('"', "'")
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
        let body = job_body(&text, job)
            .unwrap_or_else(|| panic!("build.yml must keep a `{job}:` job"));

        let fetch_idx = body
            .iter()
            .position(|line| line.trim_start().starts_with("run:") && line.contains("cargo fetch --locked"))
            .unwrap_or_else(|| {
                panic!("job `{job}` must verify the committed lockfile with `cargo fetch --locked`")
            });
        let update_idx = body
            .iter()
            .position(|line| line.trim_start().starts_with("run:") && line.contains("cargo update"))
            .unwrap_or_else(|| panic!("job `{job}` must refresh MoonUI with a `cargo update` step"));
        let assert_idx = body
            .iter()
            .position(|line| {
                line.trim_start().starts_with("run:") && line.contains("assert-only-moonui-moved.sh")
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
        let body = job_body(&text, job)
            .unwrap_or_else(|| panic!("build.yml must keep a `{job}:` job"));

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
                named, expected,
                "job `{job}` must refresh exactly the three MoonUI crates \
                 (moon-gpui, moon-gpui-platform, moon-ui) and nothing else — a bare `cargo \
                 update`, a dropped crate, an added one, or `-p moonproto` all defeat the \
                 lockfile freeze: `{}`",
                line.trim()
            );
        }
    }
}
