//! CI coverage: every push tests Linux and macOS, and nightly runs include the oldest supported
//! git built from source, the growth guards timed in release, the coverage report and the fuzz
//! targets. These checks keep the workflow matrix aligned with supported environments.

/// The text of the file at `path` from the repository's root.
fn read(path: &str) -> String {
    let file = crate::workspace::root().join(path);
    std::fs::read_to_string(&file).unwrap_or_else(|error| panic!("{path}: {error}"))
}

mod coverage_report;
mod fuzzing;
mod growth_guards;
mod supported_environments;
