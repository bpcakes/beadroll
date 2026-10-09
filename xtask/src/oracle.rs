//! `cargo xtask oracle`: the Python test suite against any `bead` binary (`docs/RUST-PORT.md` §4.2).
//!
//! Each module runs in its own Python process (`xtask/python/oracle.py`) with `BEAD_UNDER_TEST`
//! naming the binary, a fresh scratch `BEAD_HOME`, and the tests of `xtask/oracle-skip.txt` left
//! out: those that cannot target a binary (drivers that runpy the launcher, in-process tests, CLI
//! tests whose verdict comes from patching in-process Python) and those that run no bead at all.
//! While the binary is not Python's own launcher, [`REFUSALS`] names a file, and Python bead
//! refuses to start and records the attempt there: a test that reaches it by any path other than
//! `BEAD_UNDER_TEST` fails instead of counting a Python run as a pass.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::{Mutex, PoisonError};

use crate::workspace::{self, Failure};

/// `check.sh`'s fast modules, in its order.
pub(crate) const FAST: [&str; 8] = [
    "test_hygiene",
    "test_check",
    "test_replay",
    "test_storage",
    "test_compat",
    "test_commands",
    "test_protocol",
    "test_hardening",
];

/// The slow modules `check.sh --stress` adds.
pub(crate) const SLOW: [&str; 2] = ["test_stress", "test_sweep"];

/// Names the file in which Python bead records each start it refuses; set, it refuses to start
/// (`beadroll/cli.py`).
pub(crate) const REFUSALS: &str = "BEADROLL_ORACLE_REFUSALS";

struct Options {
    bead: Option<PathBuf>,
    report: bool,
    jobs: usize,
    modules: Vec<String>,
}

fn parse(args: Vec<OsString>) -> Result<Options, Failure> {
    let mut options = Options {
        bead: None,
        report: false,
        jobs: 1,
        modules: Vec::new(),
    };
    let mut stress = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy().into_owned();
        let (flag, inline) = match text.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => {
                (flag.to_owned(), Some(value.to_owned()))
            }
            _ => (text.clone(), None),
        };
        let mut value = |name: &str| match inline.clone() {
            Some(value) => Ok(OsString::from(value)),
            None => args
                .next()
                .ok_or_else(|| Failure::Usage(format!("{name} needs a value"))),
        };
        match flag.as_str() {
            "--bead" => options.bead = Some(PathBuf::from(value("--bead")?)),
            "--report" => options.report = true,
            "--stress" => stress = true,
            "--jobs" => {
                let jobs = value("--jobs")?;
                options.jobs = jobs
                    .to_str()
                    .and_then(|jobs| jobs.parse().ok())
                    .filter(|jobs| *jobs > 0)
                    .ok_or_else(|| Failure::Usage(format!("--jobs takes a count, not {jobs:?}")))?;
            }
            _ if flag.starts_with('-') => {
                return Err(Failure::Usage(format!("unknown option {flag}")));
            }
            _ => options.modules.push(text),
        }
    }
    if options.modules.is_empty() {
        let slow: &[&str] = if stress { &SLOW } else { &[] };
        options.modules = FAST.iter().chain(slow).map(|m| (*m).to_owned()).collect();
    } else if stress {
        return Err(Failure::Usage(
            "--stress chooses modules; name them or give --stress".into(),
        ));
    }
    Ok(options)
}

pub(crate) fn main(args: Vec<OsString>) -> Result<ExitCode, Failure> {
    let options = parse(args)?;
    let root = workspace::root();
    // A module the lists do not know would never run, and the suite would pass without it.
    let unknown = unlisted_modules(&root)?;
    if !unknown.is_empty() {
        return Err(Failure::Failed(format!(
            "add {} to FAST or SLOW in xtask/src/oracle.rs",
            unknown.join(", ")
        )));
    }
    for module in &options.modules {
        if !root.join("tests").join(format!("{module}.py")).is_file() {
            return Err(Failure::Usage(format!("tests/{module}.py does not exist")));
        }
    }
    let bead = match &options.bead {
        Some(path) => std::path::absolute(path)
            .map_err(|error| Failure::io("cannot resolve", path, &error))?,
        None => build(&root)?,
    };
    if !bead.is_file() {
        return Err(Failure::Failed(format!("{} is not a file", bead.display())));
    }
    let python = is_python_launcher(&root, &bead);
    let suite = Suite {
        dir: root.clone(),
        runner: root.join("xtask").join("python").join("oracle.py"),
        skip: root.join("xtask").join("oracle-skip.txt"),
        guard: !python,
        out: workspace::output_dir("oracle")?,
        bead,
    };
    println!(
        "oracle: the Python suite against {}{}; logs in {}",
        suite.bead.display(),
        if python {
            " (Python's own launcher)"
        } else {
            ""
        },
        suite.out.display()
    );
    let results = run_all(&suite, &options.modules, options.jobs);
    println!("\n{}", table(&results));
    let broken: Vec<_> = results.iter().filter(|r| r.summary.is_none()).collect();
    let failing: Vec<_> = results.iter().filter(|r| r.status != Some(0)).collect();
    for result in &failing {
        println!("{}: see {}", result.module, result.log.display());
    }
    if !broken.is_empty() || (!options.report && !failing.is_empty()) {
        return Ok(ExitCode::FAILURE);
    }
    Ok(ExitCode::SUCCESS)
}

/// The test modules in `tests/` that neither [`FAST`] nor [`SLOW`] names.
pub(crate) fn unlisted_modules(root: &Path) -> Result<Vec<String>, Failure> {
    let modules = workspace::test_modules(root)?;
    Ok(modules
        .into_iter()
        .filter(|m| !FAST.contains(&m.as_str()) && !SLOW.contains(&m.as_str()))
        .collect())
}

/// Builds `bead` in the profile xtask itself was built in and returns the executable Cargo
/// reports: the target directory, a build target and the profile can each move it, so its path is
/// never guessed.
fn build(root: &Path) -> Result<PathBuf, Failure> {
    let profile = workspace::profile_dir()?;
    let output = build_command(root, &profile)?
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| Failure::Failed(format!("cannot run cargo: {error}")))?;
    if !output.status.success() {
        return Err(Failure::Failed(format!(
            "building bead failed ({})",
            output.status
        )));
    }
    built_executable(&String::from_utf8_lossy(&output.stdout))
}

/// The cargo command that builds `bead` in the profile of `profile`, the directory xtask was built
/// in, and reports its artifacts as JSON on standard output.
fn build_command(root: &Path, profile: &Path) -> Result<Command, Failure> {
    let Some(name) = profile.file_name().and_then(|name| name.to_str()) else {
        return Err(Failure::Failed(format!(
            "no profile in {}",
            profile.display()
        )));
    };
    let mut command = Command::new(workspace::cargo());
    command
        .args([
            "build",
            "--locked",
            "--package",
            "beadroll",
            "--bin",
            "bead",
        ])
        .arg("--message-format=json-render-diagnostics")
        .current_dir(root);
    // Cargo's dev profile builds into `debug`; every other profile into its own name.
    if name != "debug" {
        command.args(["--profile", name]);
    }
    Ok(command)
}

/// The executable Cargo reported for the `bead` binary among its artifact messages.
fn built_executable(messages: &str) -> Result<PathBuf, Failure> {
    let found: Vec<String> = messages
        .lines()
        .filter(|line| line.contains(r#""reason":"compiler-artifact""#))
        .filter(|line| line.contains(r#""kind":["bin"]"#))
        .filter(|line| workspace::json_field(line, "name").as_deref() == Some("bead"))
        .filter_map(|line| workspace::json_field(line, "executable"))
        .collect();
    match found.as_slice() {
        [executable] => Ok(PathBuf::from(executable)),
        _ => Err(Failure::Failed(format!(
            "cargo reported {} executables for bead, not one",
            found.len()
        ))),
    }
}

/// Whether `bead` is this checkout's Python launcher, the one binary the guard must not stop.
fn is_python_launcher(root: &Path, bead: &Path) -> bool {
    match (bead.canonicalize(), root.join("bead").canonicalize()) {
        (Ok(bead), Ok(launcher)) => bead == launcher,
        _ => false,
    }
}

/// Where and how the modules run.
pub(crate) struct Suite {
    /// The directory that holds the `tests` package, where Python runs.
    pub(crate) dir: PathBuf,
    /// `xtask/python/oracle.py`.
    pub(crate) runner: PathBuf,
    /// The skip list.
    pub(crate) skip: PathBuf,
    /// The binary under test, an absolute path.
    pub(crate) bead: PathBuf,
    /// Whether Python bead must refuse to start ([`REFUSALS`]).
    pub(crate) guard: bool,
    /// Where logs, scratch homes and refusal files go.
    pub(crate) out: PathBuf,
}

/// One module's summary line, as the runner prints it.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Summary {
    pub(crate) ran: u64,
    pub(crate) passed: u64,
    pub(crate) failed: u64,
    pub(crate) errors: u64,
    pub(crate) skipped: u64,
    pub(crate) excluded: u64,
    pub(crate) fixture_errors: u64,
    pub(crate) seconds: u64,
}

impl Summary {
    /// Reads `MODULE ran=N passed=N ...`; `None` if the line is not one.
    pub(crate) fn parse(module: &str, line: &str) -> Option<Summary> {
        let mut words = line.split_whitespace();
        if words.next() != Some(module) {
            return None;
        }
        let mut summary = Summary::default();
        let mut seen = 0;
        for word in words {
            let (key, value) = word.split_once('=')?;
            let value = value.parse().ok()?;
            let field = match key {
                "ran" => &mut summary.ran,
                "passed" => &mut summary.passed,
                "failed" => &mut summary.failed,
                "errors" => &mut summary.errors,
                "skipped" => &mut summary.skipped,
                "excluded" => &mut summary.excluded,
                "fixture_errors" => &mut summary.fixture_errors,
                "seconds" => &mut summary.seconds,
                _ => return None,
            };
            *field = value;
            seen += 1;
        }
        (seen == 8).then_some(summary)
    }
}

/// What one module's run gave.
pub(crate) struct ModuleResult {
    pub(crate) module: String,
    /// The runner's exit status: 0 when every test passed.
    pub(crate) status: Option<i32>,
    /// Its summary, if it got as far as one.
    pub(crate) summary: Option<Summary>,
    /// Its full report.
    pub(crate) log: PathBuf,
}

/// Runs one module and reads its summary; the runner's own report goes to the module's log.
pub(crate) fn run_module(suite: &Suite, module: &str) -> ModuleResult {
    let log = suite.out.join(format!("{module}.log"));
    let mut result = ModuleResult {
        module: module.to_owned(),
        status: None,
        summary: None,
        log: log.clone(),
    };
    let home = suite.out.join(format!("{module}.home"));
    let refusals = suite.out.join(format!("{module}.refusals"));
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_file(&refusals);
    let report = match std::fs::create_dir_all(&home).and_then(|()| File::create(&log)) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("{module}: cannot prepare {}: {error}", suite.out.display());
            return result;
        }
    };
    let mut command = Command::new(workspace::python());
    command
        .arg(&suite.runner)
        .arg("--skip")
        .arg(&suite.skip)
        .arg(module)
        .current_dir(&suite.dir)
        .env("BEAD_UNDER_TEST", &suite.bead)
        .env("BEAD_HOME", &home) // never the real store, whatever this shell has
        .env_remove(REFUSALS)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(report);
    if suite.guard {
        command.env(REFUSALS, &refusals);
    }
    match command.output() {
        Ok(output) => {
            result.status = output.status.code();
            let stdout = String::from_utf8_lossy(&output.stdout);
            result.summary = stdout
                .lines()
                .rev()
                .find_map(|line| Summary::parse(module, line));
        }
        Err(error) => eprintln!("{module}: cannot run {:?}: {error}", workspace::python()),
    }
    result
}

/// Runs the modules, `jobs` at a time, and returns their results in the order given.
fn run_all(suite: &Suite, modules: &[String], jobs: usize) -> Vec<ModuleResult> {
    let queue = Mutex::new(modules.iter().enumerate().collect::<VecDeque<_>>());
    let done = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..jobs.min(modules.len()) {
            scope.spawn(|| {
                loop {
                    let next = queue
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .pop_front();
                    let Some((index, module)) = next else { break };
                    let result = run_module(suite, module);
                    println!("{}", line(&result));
                    done.lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push((index, result));
                }
            });
        }
    });
    let mut done = done.into_inner().unwrap_or_else(PoisonError::into_inner);
    done.sort_by_key(|(index, _)| *index);
    done.into_iter().map(|(_, result)| result).collect()
}

const HEADER: &str =
    "module               ran  passed  failed  errors skipped excluded fixtures seconds";

fn row(name: &str, s: &Summary) -> String {
    format!(
        "{name:<16} {:>7} {:>7} {:>7} {:>7} {:>7} {:>8} {:>8} {:>7}",
        s.ran, s.passed, s.failed, s.errors, s.skipped, s.excluded, s.fixture_errors, s.seconds
    )
}

fn line(result: &ModuleResult) -> String {
    match &result.summary {
        Some(summary) => row(&result.module, summary),
        None => format!(
            "{:<16} did not finish (runner exit status {:?}): see {}",
            result.module,
            result.status,
            result.log.display()
        ),
    }
}

/// The per-module table, with a total row.
fn table(results: &[ModuleResult]) -> String {
    let mut total = Summary::default();
    let mut lines = vec![HEADER.to_owned()];
    for result in results {
        lines.push(line(result));
        if let Some(s) = &result.summary {
            total.ran += s.ran;
            total.passed += s.passed;
            total.failed += s.failed;
            total.errors += s.errors;
            total.skipped += s.skipped;
            total.excluded += s.excluded;
            total.fixture_errors += s.fixture_errors;
            total.seconds += s.seconds;
        }
    }
    lines.push(row("all", &total));
    lines.join("\n")
}

#[cfg(test)]
mod tests;
