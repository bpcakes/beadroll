//! CI coverage: every push tests Linux and macOS, and nightly runs include the oldest supported
//! git built from source, the growth guards timed in release, the coverage report and the fuzz
//! targets; every push builds the docs with warnings denied. These checks keep the workflow
//! matrix aligned with supported environments.

/// The text of the file at `path` from the repository's root.
fn read(path: &str) -> String {
    let file = crate::workspace::root().join(path);
    std::fs::read_to_string(&file).unwrap_or_else(|error| panic!("{path}: {error}"))
}

/// The workflow at `path` from the repository's root, without its comments: what every check of a
/// workflow reads, so that a comment saying what a job should do never passes for the job doing it.
fn workflow(path: &str) -> String {
    uncommented(&read(path))
}

/// `text` without its YAML comments: a line that starts with `#`, and what follows ` #` in
/// the others. The job is judged by what it does, not by what it says it does.
fn uncommented(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(|line| {
            let code = if line.trim_start().starts_with('#') {
                ""
            } else {
                line.split(" #").next().unwrap_or(line)
            };
            code.trim_end()
        })
        .collect();
    lines.join("\n") + "\n"
}

/// A step of a job: its lines, comments left out.
pub(super) type Step<'w> = Vec<&'w str>;

/// The lines of job `job` in `workflow`, read without its comments, before its steps, and its steps.
pub(super) fn job<'w>(workflow: &'w str, job: &str) -> (Vec<&'w str>, Vec<Step<'w>>) {
    let header = format!("  {job}:");
    let lines: Vec<&str> = (workflow.lines())
        .skip_while(|line| *line != header)
        .skip(1)
        .take_while(|line| line.is_empty() || line.starts_with("   "))
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

/// The index of the one step of `steps` with a line holding every one of `parts`.
pub(super) fn step_with(steps: &[Step<'_>], parts: &[&str]) -> usize {
    let found: Vec<usize> = (0..steps.len())
        .filter(|at| (steps[*at].iter()).any(|line| parts.iter().all(|part| line.contains(part))))
        .collect();
    assert_eq!(found.len(), 1, "steps with {parts:?}: {found:?}");
    found[0]
}

mod coverage_report;
mod documentation;
mod fuzzing;
mod growth_guards;
mod supported_environments;
