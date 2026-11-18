//! The fuzz targets (roll-k2wy.14.4): every target fuzz/Cargo.toml declares runs nightly, in a step
//! of its own after the corpus is seeded, for a budget the job's timeout leaves room for, on the
//! nightly fuzz/ pins for itself alone; the input of a crash is kept, and so is the corpus; and
//! every push checks, lints and runs cargo-deny on the targets.

use super::read;

/// A step of a job: its lines, comments left out.
type Step<'w> = Vec<&'w str>;

/// The lines of job `job` in `workflow` before its steps, and its steps, comments left out.
fn job<'w>(workflow: &'w str, job: &str) -> (Vec<&'w str>, Vec<Step<'w>>) {
    let header = format!("  {job}:");
    let lines: Vec<&str> = (workflow.lines())
        .skip_while(|line| *line != header)
        .skip(1)
        .take_while(|line| line.is_empty() || line.starts_with("   ") || line.starts_with("  #"))
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect();
    assert!(!lines.is_empty(), "no job {job}");
    let (mut head, mut steps) = (Vec::new(), Vec::<Step<'w>>::new());
    for line in lines {
        if line.starts_with("      - ") {
            steps.push(vec![line]);
        } else if let Some(step) = steps.last_mut() {
            step.push(line);
        } else {
            head.push(line);
        }
    }
    (head, steps)
}

/// What `name: ...` gives in `lines`.
fn field<'w>(lines: &[&'w str], name: &str) -> &'w str {
    let prefix = format!("{name}: ");
    (lines.iter())
        .find_map(|line| line.trim().strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("no {name} in {lines:?}"))
}

/// The number `name: N` gives in `lines`.
fn number(lines: &[&str], name: &str) -> u64 {
    (field(lines, name).trim_matches('"').parse()).unwrap_or_else(|error| panic!("{name}: {error}"))
}

/// The index of the one step of `steps` with a line holding every one of `parts`.
fn step_with(steps: &[Step<'_>], parts: &[&str]) -> usize {
    let found: Vec<usize> = (0..steps.len())
        .filter(|at| (steps[*at].iter()).any(|line| parts.iter().all(|part| line.contains(part))))
        .collect();
    assert_eq!(found.len(), 1, "steps with {parts:?}: {found:?}");
    found[0]
}

fn targets(manifest: &str) -> Vec<&str> {
    (manifest.lines())
        .filter_map(|line| line.strip_prefix("name = \""))
        .filter_map(|rest| rest.strip_suffix('"'))
        .filter(|name| *name != "beadroll-fuzz")
        .collect()
}

#[test]
fn the_nightly_fuzzes_every_target_for_a_budget_and_keeps_what_crashed() {
    let nightly = read(".github/workflows/rust-nightly.yml");
    let manifest = read("fuzz/Cargo.toml");
    let targets = targets(&manifest);
    assert!(targets.len() >= 2, "fuzz/Cargo.toml declares {targets:?}");
    let (head, steps) = job(&nightly, "fuzz");
    let timeout = number(&head, "timeout-minutes");
    let seconds = number(&head, "FUZZ_SECONDS");
    // libFuzzer takes a budget of 0 for none: the job would run to its timeout and be
    // cancelled, which keeps nothing. Building, seeding and minimizing get ten minutes.
    assert!(
        seconds > 0,
        "FUZZ_SECONDS is 0, which libFuzzer reads as no limit"
    );
    let budget = targets.len() as u64 * seconds + 600;
    assert!(
        budget <= timeout * 60,
        "{} targets of {seconds} s leave no room in {timeout} minutes",
        targets.len()
    );
    let seeded = step_with(&steps, &["run: fuzz/seed.sh "]);
    let restored = step_with(&steps, &["uses: actions/cache/restore@"]);
    assert!(
        restored < seeded,
        "the corpus is seeded before it is restored"
    );
    let mut runs = Vec::new();
    for target in &targets {
        let run = format!("run: cargo fuzz run {target} ");
        let passed = format!("&& touch \"$RUNNER_TEMP/passed-{target}\"");
        let at = step_with(
            &steps,
            &[&run, "-max_total_time=\"$FUZZ_SECONDS\"", &passed],
        );
        assert!(
            seeded < at,
            "{target} is fuzzed before the corpus is seeded"
        );
        let others = (targets.iter()).filter(|other| *other != target);
        for other in others {
            let theirs = format!("cargo fuzz run {other} ");
            assert!(
                !steps[at].iter().any(|line| line.contains(&theirs)),
                "{target} and {other} share a step"
            );
        }
        runs.push(at);
    }
    // A crash in one target leaves the others to run; each runs whatever the one before did.
    let first = runs.iter().min().copied().unwrap_or_default();
    for at in runs.iter().filter(|at| **at != first) {
        assert!(steps[*at].contains(&"        if: ${{ !cancelled() }}"));
    }
    let kept = step_with(&steps, &["path: fuzz/artifacts/"]);
    assert!(steps[kept].contains(&"        if: ${{ failure() }}"));
    assert!(
        steps[kept]
            .iter()
            .any(|line| line.contains("actions/upload-artifact@"))
    );
    // The nightly is pinned for fuzz/ alone: the main workspace keeps its own toolchain.
    let pinned = read("fuzz/rust-toolchain.toml");
    assert!(
        pinned.contains("channel = \"nightly-20"),
        "fuzz/ pins no dated nightly"
    );
    assert!(!read("rust-toolchain.toml").contains("nightly"));
    assert!(read("Cargo.toml").contains("# fuzz/ is its own workspace"));
}

#[test]
fn the_nightly_keeps_the_corpus_it_grew_and_what_failed() {
    let nightly = read(".github/workflows/rust-nightly.yml");
    let (_, steps) = job(&nightly, "fuzz");
    let restored = step_with(&steps, &["uses: actions/cache/restore@"]);
    let runs: Vec<usize> = (0..steps.len())
        .filter(|at| (steps[*at].iter()).any(|line| line.contains("-max_total_time=")))
        .collect();
    // Only a target that ran clean is merged: the merge passes over an input that crashes,
    // which the corpus keeps, with what failed, until a fix makes it pass.
    let merged = step_with(&steps, &["cargo fuzz run \"$target\"", "-merge=1"]);
    let minimizing = steps[merged].join("\n");
    // every target fuzz/Cargo.toml declares, as it has a run step of its own: one the loop missed
    // would be fuzzed each night and its corpus never minimized, nor what failed kept in it
    let manifest = read("fuzz/Cargo.toml");
    let looped = format!("for target in {}; do", targets(&manifest).join(" "));
    assert!(
        minimizing.contains(&looped),
        "the minimizing loop is not `{looped}`"
    );
    assert!(minimizing.contains("if [ ! -e \"$RUNNER_TEMP/passed-$target\" ]; then"));
    assert!(minimizing.contains("find \"artifacts/$target\" -type f -exec cp {} \"$corpus/\""));
    assert!(steps[merged].contains(&"        if: ${{ !cancelled() }}"));
    let saved = step_with(&steps, &["uses: actions/cache/save@"]);
    assert!(
        runs.iter().all(|at| *at < merged) && merged < saved,
        "the corpus is kept before it grows, or minimized after it is kept"
    );
    assert!(steps[saved].contains(&"        if: ${{ !cancelled() }}"));
    // Each night's corpus under a key of its own, the newest of them restored: a constant key
    // would be saved once and never again, and the corpus would stop growing.
    let key = field(&steps[saved], "key");
    assert_eq!(field(&steps[restored], "key"), key);
    assert_eq!(
        field(&steps[restored], "path"),
        field(&steps[saved], "path")
    );
    let prefix = field(&steps[restored], "restore-keys");
    let rest = key
        .strip_prefix(prefix)
        .unwrap_or_else(|| panic!("restore-keys {prefix} is no prefix of the corpus's key {key}"));
    assert!(
        rest.starts_with("${{ github.run_id }}"),
        "the corpus's key {key} is not the run's own"
    );
}

/// The `[[package]]` entry `lock` holds for `name`.
fn locked<'l>(lock: &'l str, name: &str) -> &'l str {
    let head = format!("[[package]]\nname = \"{name}\"\n");
    let at = lock
        .find(&head)
        .unwrap_or_else(|| panic!("no {name} locked"));
    let entry = &lock[at + head.len()..];
    entry.split("\n\n").next().unwrap_or(entry)
}

#[test]
fn the_fuzz_lock_holds_beadroll_core_as_the_root_lock_does() {
    // CI checks fuzz/ --locked: a release's version bump, or a dependency beadroll-core gains,
    // that only the root's lock took would turn every push red. fuzz/ builds none of
    // beadroll-core's dev-dependencies.
    let manifest = read("crates/beadroll-core/Cargo.toml");
    let dev: Vec<&str> = (manifest
        .split("[dev-dependencies]\n")
        .nth(1)
        .unwrap_or_default())
    .lines()
    .take_while(|line| !line.starts_with('['))
    .filter_map(|line| line.split(['.', ' ', '=']).next())
    .filter(|name| !name.is_empty())
    .collect();
    let (root, fuzz) = (read("Cargo.lock"), read("fuzz/Cargo.lock"));
    let built = (locked(&root, "beadroll-core").lines())
        .filter(|line| {
            let name = line.trim().trim_start_matches('"');
            !dev.iter().any(|dev| {
                name.strip_prefix(dev)
                    .is_some_and(|rest| rest.starts_with(['"', ' ']))
            })
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        locked(&fuzz, "beadroll-core"),
        built,
        "run cargo update --workspace --manifest-path fuzz/Cargo.toml"
    );
}

#[test]
fn every_push_checks_lints_and_denies_the_fuzz_targets() {
    let every_push = read(".github/workflows/rust.yml");
    let (_, clippy) = job(&every_push, "clippy");
    step_with(
        &clippy,
        &[
            "run: cargo clippy --manifest-path fuzz/Cargo.toml",
            "-D warnings",
        ],
    );
    let (_, deny) = job(&every_push, "deny");
    step_with(
        &deny,
        &["run: cargo deny --manifest-path fuzz/Cargo.toml", "check"],
    );
    let (_, fmt) = job(&every_push, "fmt");
    step_with(&fmt, &["run: rustfmt --check", "fuzz/fuzz_targets/"]);
}
