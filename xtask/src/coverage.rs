//! `cargo xtask coverage [--stress] [--sweep]`: what the suites run of the product's code, as an
//! HTML report, an `lcov.info` and a `summary.json` under `target/coverage/`, made by
//! cargo-llvm-cov, and the ownership protocol's lines that no test reaches, on standard output.
//!
//! - the fast suite always: the workspace's tests through nextest, as `check` runs them, with
//!   the sweep's thirteen-schedule slice among them. The black-box tests run `bead` as a child
//!   process, and the children inherit the profile file's name from the test, so what they run
//!   counts too ([`CHILD_ONLY`] shows it, and the command fails when it does not);
//! - with `--stress`, the ignored stress models; with `--sweep`, all 660 schedules of the
//!   ownership sweep (`BEAD_SWEEP=full`), which the nightly workflow does;
//! - not the doctests, of which the workspace has none and which cargo-llvm-cov covers only on a
//!   nightly toolchain, and not the growth guards, whose timings an instrumented build would
//!   spoil;
//! - always the `bead` this build makes, whatever `BEAD_UNDER_TEST` names in the shell: another
//!   `bead` has no counters.
//!
//! The profiles of the children are merged into a few files per binary ([`PROFILE`]) rather than
//! written one to a process: the suite starts `bead` tens of thousands of times, half a megabyte
//! of counters each. A `bead` that dies by SIGKILL (the sweep's `sigkill` schedules) or runs under
//! a resource limit (a test that does takes `UNPROFILED`, which keeps the profile out of the limit)
//! writes no profile, so a line only such a process runs counts as not run.
//!
//! cargo-llvm-cov and the toolchain's `llvm-tools` are not in `rust-toolchain.toml`: the command
//! checks that they are installed and says how to install them.

mod lcov;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, ExitCode};

use crate::check::{self, Kind, Setup, Step};
use crate::workspace::{self, Failure};

/// The cargo-llvm-cov the nightly workflow installs (`ci::coverage_report` keeps them alike): the
/// version this command was tried with. Another one runs, and is named in the report.
pub(crate) const CARGO_LLVM_COV: &str = "0.8.7";

/// Source files that are not the product's: the tooling and the test scaffolding, which tests
/// run and nothing tests.
const IGNORED: &str = r"(^|/)(xtask|beadroll-test-support)/|/testing\.rs$";

/// The name of the profile files, in the directory cargo-llvm-cov keeps them in: processes of one
/// binary merge into one of sixteen (`%16m`), which keeps a run of tens of thousands of `bead`
/// processes to a few hundred megabytes. cargo-llvm-cov's own name has the process in it, and
/// would leave a file of half a megabyte for each.
const PROFILE: &str = "beadroll-%16m.profraw";

/// A file only a `bead` process runs: the executable's `main`. In-process tests cannot reach it,
/// so lines of it that ran were counted from a child, and a report where none did lost the
/// children.
const CHILD_ONLY: &str = "crates/beadroll/src/main.rs";

/// The ownership protocol's source, by path prefix: `beadroll-sync` (synchronise, coordinated,
/// the settling of receipts and fences, merge, the claims transaction), the replay of claims and
/// takeovers, and the commands that claim, take over, release and close.
const PROTOCOL: [&str; 7] = [
    "crates/beadroll-sync/src/",
    "crates/beadroll-core/src/replay/apply/claims.rs",
    "crates/beadroll-core/src/state/claim.rs",
    "crates/beadroll/src/commands/ownership.rs",
    "crates/beadroll/src/commands/transition.rs",
    "crates/beadroll/src/commands/update.rs",
    "crates/beadroll/src/commands/claims.rs",
];

/// What the command line asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Options {
    stress: bool,
    sweep: bool,
}

fn parse(args: Vec<OsString>) -> Result<Options, Failure> {
    let mut options = Options::default();
    for arg in args {
        match arg.to_str() {
            Some("--stress") => options.stress = true,
            Some("--sweep") => options.sweep = true,
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
    let tools = Tools {
        cargo: workspace::cargo(),
        rustc: std::env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc")),
        llvm_from_env: ["LLVM_COV", "LLVM_PROFDATA"]
            .iter()
            .all(|name| std::env::var_os(name).is_some()),
    };
    let (root, out) = (workspace::root(), workspace::output_dir("coverage")?);
    let (mut stdout, mut stderr) = (io::stdout(), io::stderr());
    let passed = make(&tools, &root, &out, options, &mut stdout, &mut stderr)?;
    Ok(exit_code(passed))
}

/// The exit status of a run that `passed`, or did not.
fn exit_code(passed: bool) -> ExitCode {
    if passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Runs the suites `options` select under `tools`, writes the report under `out` and tells what it
/// shows: whether every step ran, no profile was left out of the report and it counts the
/// children.
fn make(
    tools: &Tools,
    root: &Path,
    out: &Path,
    options: Options,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<bool, Failure> {
    let cannot_report = |error: io::Error| Failure::Failed(format!("cannot report: {error}"));
    let installed = tools.require(root)?;
    writeln!(stdout, "{:<14} {installed}", "tools").map_err(cannot_report)?;
    // A report is of this run alone: nothing of an earlier one stays in its place.
    for stale in ["html", "lcov.info", "summary.json", "home", "logs"] {
        let path = out.join(stale);
        let _ = std::fs::remove_dir_all(&path).or_else(|_| std::fs::remove_file(&path));
    }
    let (home, logs) = (out.join("home"), out.join("logs"));
    for dir in [&home, &logs] {
        std::fs::create_dir_all(dir).map_err(|error| Failure::io("cannot create", dir, &error))?;
    }
    let setup = Setup {
        root: root.to_path_buf(),
        cargo: tools.cargo.clone(),
        python: workspace::python(),
        home,
        logs,
        bead: None,
    };
    let steps = steps(&setup, out, options);
    let mut passed = check::run(&steps, &setup, stdout, stderr).map_err(cannot_report)?;
    // With `--failure-mode all` the reports are made of the profiles that merge and only warn of
    // the rest, in logs nobody reads: a profile left out is counts lost, which would read as code
    // no test reaches, so it fails the run and is said here.
    let skipped = unmerged_profiles(&setup.logs)?;
    if let Some(first) = skipped.first() {
        let these = match skipped.len() {
            1 => "1 profile".to_owned(),
            n => format!("{n} profiles"),
        };
        let logs = setup.logs.strip_prefix(root).unwrap_or(&setup.logs);
        writeln!(
            stdout,
            "{:<14} {these} could not be merged and the counts in them are missing from this \
             report, the first {first} ({}/report-*.log name them all)",
            "profiles",
            logs.display()
        )
        .map_err(cannot_report)?;
        passed = false;
    }
    let lcov = out.join("lcov.info");
    let text = std::fs::read_to_string(&lcov).map_err(|error| {
        Failure::Failed(format!(
            "no report was made ({error}: {}); every step's output is in {}",
            lcov.display(),
            setup.logs.display()
        ))
    })?;
    let files = lcov::parse(&text, root)
        .map_err(|error| Failure::Failed(format!("{} does not read, {error}", lcov.display())))?;
    passed &= summarize(&files, stdout).map_err(cannot_report)?;
    let shown = out
        .strip_prefix(root)
        .unwrap_or(out)
        .join("html")
        .join("index.html");
    // The HTML index and summary.json add up the lines of each function, so that a line two
    // functions share (a closure on the line of the call that takes it) counts twice.
    let kept = format!(
        "{}, lcov.info, summary.json (the HTML index and summary.json count per function: more \
         lines than above)",
        shown.display()
    );
    writeln!(stdout, "{:<14} {kept}", "report").map_err(cannot_report)?;
    Ok(passed)
}

/// The profiles `llvm-profdata` left out of the reports, from the logs of the report steps: it
/// warns `warning: PATH.profraw: invalid instrumentation profile data (...)`, once for each report.
/// A log is read as bytes, since a path with one that is not UTF-8 (the checkout's, in the line
/// that says where the report was saved) is in all of them; one that cannot be read at all is an
/// error, not a log with nothing in it.
fn unmerged_profiles(logs: &Path) -> Result<Vec<String>, Failure> {
    let mut skipped = BTreeSet::new();
    let names =
        std::fs::read_dir(logs).map_err(|error| Failure::io("cannot read", logs, &error))?;
    for entry in names {
        let entry = entry.map_err(|error| Failure::io("cannot read", logs, &error))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("report-") && name.ends_with(".log") {
            let path = entry.path();
            let bytes =
                std::fs::read(&path).map_err(|error| Failure::io("cannot read", &path, &error))?;
            skipped.extend(skipped_in(&String::from_utf8_lossy(&bytes)));
        }
    }
    Ok(skipped.into_iter().collect())
}

/// The profiles `text`, a report step's output, says were left out.
fn skipped_in(text: &str) -> Vec<String> {
    const EXTENSION: &str = ".profraw";
    text.lines()
        .filter_map(|line| line.strip_prefix("warning: "))
        .filter_map(|warning| {
            let at = warning.find(&format!("{EXTENSION}: "))? + EXTENSION.len();
            warning.get(..at).map(str::to_owned)
        })
        .collect()
}

/// The tools the report is made with.
struct Tools {
    cargo: OsString,
    rustc: OsString,
    /// `LLVM_COV` and `LLVM_PROFDATA` name the LLVM tools, which cargo-llvm-cov then uses in
    /// place of the toolchain's.
    llvm_from_env: bool,
}

impl Tools {
    /// What is installed, in a line for the report, or why the report cannot be made: each tool
    /// that is missing, and the command that installs it.
    fn require(&self, root: &Path) -> Result<String, Failure> {
        let mut missing = Vec::new();
        let asked = Command::new(&self.cargo)
            .args(["llvm-cov", "--version"])
            .current_dir(root)
            .output()
            .map_err(|error| Failure::Failed(format!("cannot run cargo: {error}")))?;
        // Cargo ran, and says it has no such subcommand when it is not installed.
        let version = (asked.status.success())
            .then(|| String::from_utf8_lossy(&asked.stdout).trim().to_owned());
        if version.is_none() {
            missing.push(format!(
                "cargo-llvm-cov is not installed: cargo install --locked cargo-llvm-cov --version {CARGO_LLVM_COV}"
            ));
        }
        if !self.llvm_from_env && !self.llvm_tools_installed(root)? {
            missing.push(
                "llvm-tools is not installed for this toolchain: rustup component add llvm-tools"
                    .to_owned(),
            );
        }
        match version {
            Some(version) if missing.is_empty() => Ok(if version == pinned() {
                format!("{version}, llvm-tools")
            } else {
                format!("{version} (the nightly job pins {CARGO_LLVM_COV}), llvm-tools")
            }),
            _ => Err(Failure::Failed(missing.join("\n"))),
        }
    }

    /// Whether the toolchain `rustc` is has `llvm-profdata` and `llvm-cov` beside its libraries,
    /// where `rustup component add llvm-tools` puts them.
    fn llvm_tools_installed(&self, root: &Path) -> Result<bool, Failure> {
        let output = Command::new(&self.rustc)
            .args(["--print", "target-libdir"])
            .current_dir(root)
            .output()
            .map_err(|error| Failure::Failed(format!("cannot run rustc: {error}")))?;
        let libdir = String::from_utf8_lossy(&output.stdout);
        let bin = Path::new(libdir.trim()).parent().map(|dir| dir.join("bin"));
        Ok(output.status.success()
            && bin.is_some_and(|bin| {
                ["llvm-profdata", "llvm-cov"]
                    .iter()
                    .all(|tool| bin.join(tool).is_file())
            }))
    }
}

/// What `cargo llvm-cov --version` says of the version the nightly job pins.
fn pinned() -> String {
    format!("cargo-llvm-cov {CARGO_LLVM_COV}")
}

/// The steps `options` select: the old profiles removed, the suites, and the three reports made
/// of what they left.
fn steps(setup: &Setup, out: &Path, options: Options) -> Vec<Step> {
    let cargo = &setup.cargo;
    let mut steps = vec![Step::new(
        "clean",
        Kind::Plain,
        cargo,
        ["llvm-cov", "clean", "--workspace"],
    )];
    let nextest = ["llvm-cov", "nextest", "--locked", "--no-report"];
    let mut suite = |name: &'static str, selection: &[&str], env: Option<(&'static str, &str)>| {
        let mut step = Step::new(name, Kind::Nextest, cargo, nextest.iter().chain(selection));
        // The bead that is counted is the one this build makes, whatever the shell names.
        step.env.push(("BEAD_UNDER_TEST", OsString::new()));
        step.env.push(("LLVM_PROFILE_FILE_NAME", PROFILE.into()));
        step.env
            .extend(env.map(|(name, value)| (name, value.into())));
        steps.push(step);
    };
    suite("tests", &["--workspace"], None);
    if options.stress {
        let selection = [
            "--workspace",
            "--run-ignored",
            "only",
            "-E",
            "test(/^stress::/)",
        ];
        suite("stress", &selection, None);
    }
    if options.sweep {
        let selection = ["--package", "beadroll", "-E", "test(=sweep::sweep::sweep)"];
        suite("sweep", &selection, Some(("BEAD_SWEEP", "full")));
    }
    let to = |flags: &[&str], target: &OsStr| -> Vec<OsString> {
        let mut args: Vec<OsString> = ["llvm-cov", "report", "--failure-mode", "all"]
            .iter()
            .map(OsString::from)
            .collect();
        args.extend(["--ignore-filename-regex", IGNORED].map(OsString::from));
        args.extend(flags.iter().map(OsString::from));
        args.push(target.to_owned());
        args
    };
    let reports = [
        (
            "report html",
            to(&["--html", "--output-dir"], out.as_os_str()),
        ),
        (
            "report lcov",
            to(
                &["--lcov", "--output-path"],
                out.join("lcov.info").as_os_str(),
            ),
        ),
        (
            "report json",
            to(
                &["--json", "--summary-only", "--output-path"],
                out.join("summary.json").as_os_str(),
            ),
        ),
    ];
    for (name, args) in reports {
        steps.push(Step::new(name, Kind::Plain, cargo, args));
    }
    steps
}

/// The lines the suites ran, by crate, then the ownership protocol's files with the lines that
/// never ran, then whether the children's are in the report. `false` when they are not.
fn summarize(files: &[lcov::File], out: &mut dyn Write) -> io::Result<bool> {
    let share = |files: &[&lcov::File]| {
        let covered: usize = files.iter().map(|file| file.covered()).sum();
        let total: usize = files.iter().map(|file| file.total()).sum();
        format!("{:>6} ({covered} of {total})", percent(covered, total))
    };
    let all: Vec<&lcov::File> = files.iter().collect();
    writeln!(out, "{:<14} {}", "lines", share(&all))?;
    let crates: BTreeSet<&str> = files.iter().map(|file| crate_of(&file.path)).collect();
    for name in crates {
        let of: Vec<&lcov::File> = files
            .iter()
            .filter(|file| crate_of(&file.path) == name)
            .collect();
        writeln!(out, "  {name:<24} {}", share(&of))?;
    }
    let protocol: Vec<&lcov::File> = files
        .iter()
        .filter(|file| PROTOCOL.iter().any(|prefix| file.path.starts_with(prefix)))
        .collect();
    writeln!(
        out,
        "{:<14} {}; lines that never ran:",
        "protocol",
        share(&protocol)
    )?;
    let width = protocol
        .iter()
        .map(|file| file.path.len())
        .max()
        .unwrap_or(0);
    for file in protocol.iter().copied() {
        let ranges: Vec<String> = file
            .unreached()
            .into_iter()
            .map(|(first, last)| {
                if first == last {
                    first.to_string()
                } else {
                    format!("{first}-{last}")
                }
            })
            .collect();
        writeln!(out, "  {:<width$} {}", file.path, share(&[file]))?;
        if !ranges.is_empty() {
            writeln!(out, "      {}", ranges.join(", "))?;
        }
    }
    let child = files.iter().find(|file| file.path == CHILD_ONLY);
    match child.filter(|file| file.covered() > 0) {
        Some(file) => writeln!(
            out,
            "{:<14} counted: {CHILD_ONLY} ran {} of {} lines, which only a bead process runs",
            "children",
            file.covered(),
            file.total()
        )
        .map(|()| true),
        None => writeln!(
            out,
            "{:<14} NOT counted: no line of {CHILD_ONLY} ran, and only a bead process runs it; \
             the children of the black-box tests are missing from this report (was LLVM_PROFILE_FILE \
             removed from the environment bead runs in, beadroll-test-support's SCRUBBED, or a bead \
             that is not instrumented named in the machines?)",
            "children"
        )
        .map(|()| false),
    }
}

/// The crate a file belongs to: `crates/NAME/...` is NAME, any other path its first part.
fn crate_of(path: &str) -> &str {
    let mut parts = path.split('/');
    match (parts.next(), parts.next()) {
        (Some("crates"), Some(name)) => name,
        (Some(first), _) => first,
        (None, _) => path,
    }
}

/// `covered` of `total` as a percentage to a tenth, rounded to nearest; `-` when nothing has code.
fn percent(covered: usize, total: usize) -> String {
    if total == 0 {
        return "-".to_owned();
    }
    let tenths = (covered * 1000 + total / 2) / total;
    format!("{}.{}%", tenths / 10, tenths % 10)
}
