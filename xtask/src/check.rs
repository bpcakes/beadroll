//! `cargo xtask check [--stress] [--real]`: the Rust workspace's baseline before a commit, as
//! `./check.sh` is the Python package's (`docs/RUST-PORT.md` §4.3, §4.9); at the cutover
//! `./check.sh` becomes a thin wrapper around it. Every step runs with a scratch `BEAD_HOME` and
//! gets one line; a failed step fails the check, and the steps after it still run.
//!
//! - `tests`: the workspace's tests through nextest, the sweep's slice among them;
//!   `BEAD_UNDER_TEST` and `BEAD_SWEEP` reach them as they are set;
//! - `doctests`: `cargo test --doc`, which nextest does not run;
//! - `infra`: the Python tests of the `infra/` drivers, beside them;
//! - with `--stress`, `stress`: the ignored stress models;
//! - with `--real`, `compat real` and then `roundtrip`: the real-export compatibility test, and
//!   `infra/roundtrip.py` over this machine's Beads exports, against `BEAD_UNDER_TEST` or else the
//!   bead Cargo builds. As in `check.sh`, a round trip that passes shows only its summary, the
//!   last line; one that fails shows everything it printed.
//!
//! A failed step shows what it printed but nextest's progress lines; `target/check/` keeps each
//! step's whole output.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Output, Stdio};
use std::time::Instant;

use crate::oracle;
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
                None => oracle::build(&root)?,
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

/// Where a check runs and with what.
struct Setup {
    root: PathBuf,
    cargo: OsString,
    python: OsString,
    /// The scratch `BEAD_HOME` of every step.
    home: PathBuf,
    /// Where each step's whole output is kept.
    logs: PathBuf,
    /// The bead `--real` runs.
    bead: Option<PathBuf>,
}

/// How a step's output is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Nextest,
    Doctests,
    Unittest,
    /// `infra/roundtrip.py`, reported as `check.sh` reports it.
    Roundtrip,
}

/// One step of a check: its name, how to read it, and the command (program and arguments).
struct Step {
    name: &'static str,
    kind: Kind,
    program: OsString,
    args: Vec<OsString>,
    /// Variables the step sets besides `BEAD_HOME`.
    env: Vec<(&'static str, OsString)>,
}

impl Step {
    fn new<I: IntoIterator<Item = S>, S: Into<OsString>>(
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
fn run(
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

/// A round trip as `check.sh` reports it: when it passed, only the last line it printed, its
/// summary; when it failed, everything it printed. What it said on standard error goes to `err`.
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
mod tests {
    use super::*;

    #[test]
    fn options_are_stress_and_real_and_nothing_else() {
        let args = |list: &[&str]| list.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(parse(args(&[])).ok(), Some(Options::default()));
        let both = Options {
            stress: true,
            real: true,
        };
        assert_eq!(parse(args(&["--real", "--stress"])).ok(), Some(both));
        assert!(matches!(
            parse(args(&["--full"])),
            Err(Failure::Usage(message)) if message == "unknown option --full"
        ));
    }

    #[test]
    fn summaries_are_read_from_each_runners_output() {
        let output = |stdout: &str, stderr: &str| Output {
            status: std::process::ExitStatus::default(),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        };
        let nextest = output(
            "",
            "        PASS [   0.1s] (1/2) x\n     Summary [ 232.489s] 603 tests run: 603 passed (2 leaky), 2 skipped\n",
        );
        assert_eq!(
            summary(Kind::Nextest, &nextest).as_deref(),
            Some("603 tests run: 603 passed (2 leaky), 2 skipped")
        );
        let doctests = output(
            "test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n\
             test result: FAILED. 1 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out\n",
            "",
        );
        let summed = summary(Kind::Doctests, &doctests);
        assert_eq!(summed.as_deref(), Some("3 passed, 3 failed"));
        let unittest = output("", "...\n------\nRan 3 tests in 0.012s\n\nOK\n");
        assert_eq!(
            summary(Kind::Unittest, &unittest).as_deref(),
            Some("Ran 3 tests in 0.012s OK")
        );
        assert_eq!(
            summary(Kind::Nextest, &output("error: could not compile", "")),
            None
        );
    }

    #[test]
    fn a_failed_nextest_run_shows_what_failed_without_the_progress() {
        let failed = Output {
            status: std::process::ExitStatus::default(),
            stdout: Vec::new(),
            stderr: b"        PASS [   0.1s] (1/2) a\n        FAIL [   0.2s] (2/2) b\n  thread 'b' panicked\n     Summary [   0.3s] 2 tests run: 1 passed, 1 failed\n".to_vec(),
        };
        let shown = diagnostics(Kind::Nextest, &failed);
        assert_eq!(shown.len(), 3, "{shown:?}");
        assert!(shown[0].contains("FAIL") && shown[1].contains("panicked"));
    }
}

/// `test_check.CheckScript`, ported (`docs/RUST-PORT.md` §4.6): the check must fail when a selected
/// step fails, shown with synthetic results only. A fake `cargo` and a fake `python3` stand in for
/// the real ones, as Python's test put a fake `python3` on `PATH`: every test step exits with the
/// module status, and the round trip prints its details and summary, and a diagnostic on standard
/// error when it fails.
#[cfg(test)]
mod check_script {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    /// A check with `--real` whose test steps exit with `module_status` and whose round trip
    /// exits with `real_status`: whether it passed, and what it printed on each stream.
    fn check(module_status: i32, real_status: i32) -> (bool, String, String) {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let verdict = if module_status == 0 { "OK" } else { "FAILED" };
        let cargo = script(
            dir.path(),
            "cargo",
            &format!(
                "echo '     Summary [   0.001s] 1 test run: 1 passed'\n\
                 echo 'test result: ok. 1 passed; 0 failed'\n\
                 exit {module_status}\n"
            ),
        );
        let python = script(
            dir.path(),
            "python3",
            &format!(
                "if [ \"$1\" = \"-m\" ]; then\n\
                 echo 'Ran 1 test in 0.001s'\necho {verdict}\nexit {module_status}\nfi\n\
                 echo 'roundtrip details'\necho 'roundtrip summary'\n\
                 if [ {real_status} -ne 0 ]; then echo 'roundtrip diagnostic' >&2; fi\n\
                 exit {real_status}\n"
            ),
        );
        let setup = Setup {
            root: dir.path().to_path_buf(),
            cargo,
            python,
            home: dir.path().join("home"),
            logs: dir.path().to_path_buf(),
            bead: Some(dir.path().join("bead")),
        };
        let options = Options {
            stress: false,
            real: true,
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let passed = run(&steps(&setup, options), &setup, &mut out, &mut err).expect("a report");
        let text = |bytes: Vec<u8>| String::from_utf8(bytes).expect("UTF-8");
        (passed, text(out), text(err))
    }

    /// An executable shell script `name` in `dir` with `body`: its path.
    fn script(dir: &Path, name: &str, body: &str) -> OsString {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}")).expect("the fake tool");
        let executable = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&path, executable).expect("an executable fake");
        path.into_os_string()
    }

    #[test]
    fn success_keeps_the_roundtrip_summary() {
        let (passed, out, err) = check(0, 0);
        assert!(passed, "{out}{err}");
        assert_eq!(out.lines().last(), Some("roundtrip summary"), "{out}");
        assert!(!out.contains("roundtrip details"), "{out}");
    }

    #[test]
    fn roundtrip_failure_fails_the_check_and_keeps_diagnostics() {
        let (passed, out, err) = check(0, 17);
        assert!(!passed, "{out}");
        assert!(err.contains("roundtrip diagnostic"), "{err}");
        assert!(out.contains("roundtrip details"), "{out}");
    }

    #[test]
    fn successful_roundtrip_does_not_hide_a_failed_module() {
        let (passed, out, _) = check(19, 0);
        assert!(!passed, "{out}");
        assert!(out.contains("roundtrip summary"), "{out}");
    }
}
