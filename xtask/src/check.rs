//! `cargo xtask check [--stress] [--real]`: the workspace's baseline before a commit, which
//! `./check.sh` runs. Every step runs with a scratch `BEAD_HOME`
//! and gets one line; a failed step fails the check, and the steps after it still run.
//!
//! - `tests`: the workspace's tests through nextest, the sweep's slice among them;
//!   `BEAD_UNDER_TEST` and `BEAD_SWEEP` reach them as they are set;
//! - `doctests`: `cargo test --doc`, which nextest does not run;
//! - `infra`: the Python tests of the `infra/` drivers, beside them;
//! - with `--stress`, `stress`: the ignored stress models;
//! - with `--real`, `compat real` and then `roundtrip`: the real-export compatibility test, and
//!   `infra/roundtrip.py` over this machine's Beads exports, against `BEAD_UNDER_TEST` or else the
//!   bead Cargo builds. A round trip that passes shows only its summary, the last line; one that
//!   fails shows everything it printed.
//!
//! A failed step shows what it printed but nextest's progress lines; `target/check/` keeps each
//! step's whole output.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Output, Stdio};
use std::time::Instant;

use crate::workspace::{self, Failure};

/// The real-export compatibility test, `#[ignore]`d until a check asks for it.
const COMPAT_REAL: &str = "test(=compat::compat_audit_real_compatibility::all_available_real_exports_and_br_bv_consumers)";

/// What the command line asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Options {
    stress: bool,
    real: bool,
}

fn parse(args: Vec<OsString>) -> Result<Options, Failure> {
    let mut options = Options::default();
    for arg in args {
        match arg.to_str() {
            Some("--stress") => options.stress = true,
            Some("--real") => options.real = true,
            _ => {
                let shown = arg.to_string_lossy();
                return Err(Failure::Usage(format!("unknown option {shown}")));
            }
        }
    }
    Ok(options)
}

pub(crate) fn main(args: Vec<OsString>) -> Result<ExitCode, Failure> {
    let options = parse(args)?;
    let root = workspace::root();
    let out = workspace::output_dir("check")?;
    let home = out.join("home");
    // A fresh scratch store for every check, never the real one, whatever this shell has.
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).map_err(|error| Failure::io("cannot create", &home, &error))?;
    let bead = if options.real {
        Some(
            match std::env::var_os("BEAD_UNDER_TEST").filter(|b| !b.is_empty()) {
                Some(bead) => std::path::absolute(&bead)
                    .map_err(|error| Failure::io("cannot resolve", Path::new(&bead), &error))?,
                None => build(&root)?,
            },
        )
    } else {
        None
    };
    let setup = Setup {
        root,
        cargo: workspace::cargo(),
        python: workspace::python(),
        home,
        logs: out,
        bead,
    };
    let passed = run(
        &steps(&setup, options),
        &setup,
        &mut io::stdout(),
        &mut io::stderr(),
    )
    .map_err(|error| Failure::Failed(format!("cannot report: {error}")))?;
    Ok(if passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Builds `bead` in the profile xtask itself was built in and returns the executable Cargo
/// reports: the target directory, a build target and the profile can each move it, so its path is
/// never guessed.
pub(crate) fn build(root: &Path) -> Result<PathBuf, Failure> {
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

/// Where a check runs and with what. `cargo xtask coverage` runs its steps the same way
/// ([`run`]), with no `bead` of its own.
pub(crate) struct Setup {
    pub(crate) root: PathBuf,
    pub(crate) cargo: OsString,
    pub(crate) python: OsString,
    /// The scratch `BEAD_HOME` of every step.
    pub(crate) home: PathBuf,
    /// Where each step's whole output is kept.
    pub(crate) logs: PathBuf,
    /// The bead `--real` runs.
    pub(crate) bead: Option<PathBuf>,
}

/// How a step's output is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Nextest,
    Doctests,
    Unittest,
    /// `infra/roundtrip.py`, reported by its own summary line ([`report_roundtrip`]).
    Roundtrip,
    /// A command with nothing to sum up, reported as `done`: it printed what its log keeps.
    Plain,
}

/// One step of a check: its name, how to read it, and the command (program and arguments).
pub(crate) struct Step {
    pub(crate) name: &'static str,
    pub(crate) kind: Kind,
    pub(crate) program: OsString,
    pub(crate) args: Vec<OsString>,
    /// Variables the step sets besides `BEAD_HOME`.
    pub(crate) env: Vec<(&'static str, OsString)>,
}

impl Step {
    pub(crate) fn new<I: IntoIterator<Item = S>, S: Into<OsString>>(
        name: &'static str,
        kind: Kind,
        program: &OsString,
        args: I,
    ) -> Step {
        Step {
            name,
            kind,
            program: program.clone(),
            args: args.into_iter().map(Into::into).collect(),
            env: Vec::new(),
        }
    }
}

/// The steps `options` select, in the order they run.
fn steps(setup: &Setup, options: Options) -> Vec<Step> {
    let (cargo, python) = (&setup.cargo, &setup.python);
    let mut steps = vec![
        Step::new(
            "tests",
            Kind::Nextest,
            cargo,
            ["nextest", "run", "--workspace", "--locked"],
        ),
        Step::new(
            "doctests",
            Kind::Doctests,
            cargo,
            ["test", "--workspace", "--doc", "--locked"],
        ),
        Step::new(
            "infra",
            Kind::Unittest,
            python,
            [
                "-m",
                "unittest",
                "discover",
                "-s",
                "infra",
                "-p",
                "test_*.py",
            ],
        ),
    ];
    if options.stress {
        let stress = [
            "nextest",
            "run",
            "--workspace",
            "--locked",
            "--run-ignored",
            "only",
        ];
        let filter = ["-E", "test(/^stress::/)"];
        steps.push(Step::new(
            "stress",
            Kind::Nextest,
            cargo,
            stress.into_iter().chain(filter),
        ));
    }
    if let (true, Some(bead)) = (options.real, &setup.bead) {
        let compat = ["nextest", "run", "--package", "beadroll", "--locked"];
        let ignored = ["--run-ignored", "only", "-E", COMPAT_REAL];
        let mut compat = Step::new(
            "compat real",
            Kind::Nextest,
            cargo,
            compat.into_iter().chain(ignored),
        );
        compat
            .env
            .push(("BEAD_UNDER_TEST", bead.clone().into_os_string()));
        steps.push(compat);
        let script = setup.root.join("infra").join("roundtrip.py");
        let mut roundtrip = Step::new("roundtrip", Kind::Roundtrip, python, [script]);
        roundtrip
            .env
            .push(("BEAD_UNDER_TEST", bead.clone().into_os_string()));
        roundtrip
            .env
            .push(("RT_HOME", setup.home.join("roundtrip").into_os_string()));
        steps.push(roundtrip);
    }
    steps
}

/// Runs every step, reporting each on `out` (and a round trip's standard error on `err`): whether
/// all of them passed.
pub(crate) fn run(
    steps: &[Step],
    setup: &Setup,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> io::Result<bool> {
    let mut passed = true;
    for step in steps {
        let started = Instant::now();
        let output = Command::new(&step.program)
            .args(&step.args)
            .current_dir(&setup.root)
            .env("BEAD_HOME", &setup.home)
            .envs(step.env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::null())
            .output();
        let seconds = started.elapsed().as_secs();
        let output = match output {
            Ok(output) => output,
            Err(error) => {
                passed = false;
                let program = step.program.to_string_lossy();
                writeln!(out, "{:<14} cannot run {program}: {error}", step.name)?;
                continue;
            }
        };
        let ok = output.status.success();
        passed &= ok;
        let log = setup
            .logs
            .join(format!("{}.log", step.name.replace(' ', "-")));
        let kept = std::fs::write(&log, [&output.stdout[..], &output.stderr[..]].concat()).is_ok();
        if step.kind == Kind::Roundtrip {
            report_roundtrip(&output, out, err)?;
            continue;
        }
        let summary = summary(step.kind, &output).unwrap_or_else(|| status(&output));
        writeln!(out, "{:<14} {summary} ({seconds}s)", step.name)?;
        if !ok {
            for line in diagnostics(step.kind, &output) {
                writeln!(out, "    {line}")?;
            }
            if kept {
                writeln!(out, "    the whole output: {}", log.display())?;
            }
        }
    }
    Ok(passed)
}

/// A round trip: when it passed, only the last line it printed, its summary; when it failed,
/// everything it printed, so that a failure keeps its whole report (H02 in `docs/FINDINGS.md`).
/// What it said on standard error goes to `err`.
fn report_roundtrip(output: &Output, out: &mut dyn Write, err: &mut dyn Write) -> io::Result<()> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    if output.status.success() {
        if let Some(last) = stdout.lines().last() {
            writeln!(out, "{last}")?;
        }
    } else {
        out.write_all(stdout.as_bytes())?;
        if !stdout.ends_with('\n') && !stdout.is_empty() {
            writeln!(out)?;
        }
    }
    err.write_all(&output.stderr)
}

/// The line that sums up a step's run, when its output has one: nextest's `Summary`, the test
/// counts of every doctest binary, unittest's `Ran` and verdict.
fn summary(kind: Kind, output: &Output) -> Option<String> {
    let text = [&output.stdout[..], &output.stderr[..]].concat();
    let text = String::from_utf8_lossy(&text);
    match kind {
        Kind::Nextest => text
            .lines()
            .rev()
            .find_map(|line| line.trim().strip_prefix("Summary ["))
            .and_then(|rest| rest.split_once("] "))
            .map(|(_, counts)| counts.to_owned()),
        Kind::Doctests => {
            let results: Vec<&str> = text
                .lines()
                .filter_map(|line| line.trim().strip_prefix("test result: "))
                .collect();
            (!results.is_empty()).then(|| {
                let count = |word: &str| -> u64 {
                    results
                        .iter()
                        .filter_map(|result| counted(result, word))
                        .sum()
                };
                format!("{} passed, {} failed", count("passed"), count("failed"))
            })
        }
        Kind::Unittest => {
            let lines: Vec<&str> = text
                .lines()
                .filter(|l| l.starts_with("Ran ") || l.starts_with("OK") || l.starts_with("FAILED"))
                .collect();
            (!lines.is_empty()).then(|| lines.join(" "))
        }
        Kind::Roundtrip => None,
        Kind::Plain => Some("done".to_owned()),
    }
}

/// The number before ` WORD` in a `test result:` line (`ok. 3 passed; 0 failed; ...`).
fn counted(result: &str, word: &str) -> Option<u64> {
    let suffix = format!(" {word}");
    let part = result
        .split(';')
        .find(|part| part.trim_end().ends_with(&suffix))?;
    part.trim_end()
        .strip_suffix(&suffix)?
        .rsplit(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// How a step ended, when its output sums up nothing (it did not build, or it was killed).
fn status(output: &Output) -> String {
    match output.status.code() {
        Some(code) => format!("exit {code}, no summary"),
        None => format!("{}, no summary", output.status),
    }
}

/// What a failed step printed, but nextest's lines for tests that started, passed or were slow.
fn diagnostics(kind: Kind, output: &Output) -> Vec<String> {
    let text = [&output.stdout[..], &output.stderr[..]].concat();
    let progress = ["PASS [", "START ", "SLOW [", "LEAK [", "SETUP ["];
    String::from_utf8_lossy(&text)
        .lines()
        .filter(|line| {
            kind != Kind::Nextest || !progress.iter().any(|p| line.trim_start().starts_with(p))
        })
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod check_script;
#[cfg(test)]
mod tests;
