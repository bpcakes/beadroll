//! CI coverage: every push tests Linux and macOS, and nightly runs include the oldest supported
//! git built from source, the growth guards timed in release, the coverage report and the fuzz
//! targets. These checks keep the workflow matrix aligned with supported environments.

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

mod coverage_report;
mod fuzzing;
mod growth_guards;
mod supported_environments;
