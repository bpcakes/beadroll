//! `cargo xtask diff`: beadroll-core against CPython (`docs/RUST-PORT.md` §4.2 item 3). Each case
//! goes to a stdlib-only Python driver and through the Rust core; the two answers must be the same
//! text.
//!
//! - `diff replay` replays journals: the fixtures the Python tests build (`xtask/fixtures/`), then
//!   generated histories, valid and hostile. Both sides print the same canonical report.
//! - `diff json` parses bytes and writes the values in every dialect.

mod cases;
mod generate;
mod json;
mod report;
mod run;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use beadroll_core::json::{Format, write};

use crate::workspace::{self, Failure};
use cases::Case;

/// How many differences are shown in full; the rest are counted.
const SHOWN: usize = 10;

struct Options {
    mode: String,
    cases: usize,
    seed: u64,
    jobs: usize,
    fixtures: Vec<PathBuf>,
    /// Where to write every case, to look at or replay.
    write_cases: Option<PathBuf>,
}

fn parse(args: Vec<OsString>) -> Result<Options, Failure> {
    let mut args = args.into_iter();
    let mode = args
        .next()
        .and_then(|m| m.into_string().ok())
        .unwrap_or_default();
    if !matches!(mode.as_str(), "replay" | "json") {
        return Err(Failure::Usage("diff takes replay or json".into()));
    }
    let jobs = std::thread::available_parallelism().map_or(4, usize::from);
    let mut options = Options {
        mode,
        cases: 1000,
        seed: 1,
        jobs,
        fixtures: Vec::new(),
        write_cases: None,
    };
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .and_then(|v| v.into_string().ok())
                .ok_or_else(|| Failure::Usage(format!("{name} needs a value")))
        };
        let number = |text: String, name: &str| {
            text.parse::<u64>()
                .map_err(|_| Failure::Usage(format!("{name} takes a number, not {text}")))
        };
        match arg.to_str() {
            Some("--cases") => {
                options.cases =
                    usize::try_from(number(value("--cases")?, "--cases")?).unwrap_or(usize::MAX)
            }
            Some("--seed") => options.seed = number(value("--seed")?, "--seed")?,
            Some("--jobs") => {
                options.jobs = usize::try_from(number(value("--jobs")?, "--jobs")?)
                    .unwrap_or(1)
                    .max(1)
            }
            Some("--fixtures") => options.fixtures.push(PathBuf::from(value("--fixtures")?)),
            Some("--write-cases") => {
                options.write_cases = Some(PathBuf::from(value("--write-cases")?))
            }
            _ => {
                return Err(Failure::Usage(format!(
                    "diff: unknown argument {}",
                    arg.to_string_lossy()
                )));
            }
        }
    }
    Ok(options)
}

pub(crate) fn main(args: Vec<OsString>) -> Result<ExitCode, Failure> {
    let options = parse(args)?;
    match options.mode.as_str() {
        "json" => json::main(options.cases, options.seed, options.jobs),
        _ => replay(&options),
    }
}

/// The fixture files `diff replay` reads by default: every `*.jsonl` in `xtask/fixtures/replay/`.
fn default_fixtures() -> Result<Vec<PathBuf>, Failure> {
    let dir = workspace::root().join("xtask/fixtures/replay");
    let Ok(listing) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut files = Vec::new();
    for entry in listing {
        let path = entry
            .map_err(|e| Failure::io("cannot list", &dir, &e))?
            .path();
        if path.extension().is_some_and(|ext| ext == "jsonl") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// The cases of the fixture files, line by line, as they are read.
fn read_fixtures(files: &[PathBuf]) -> impl Iterator<Item = Result<Case, Failure>> + '_ {
    files
        .iter()
        .flat_map(|path| -> Box<dyn Iterator<Item = Result<Case, Failure>>> {
            let file = match std::fs::File::open(path) {
                Ok(file) => file,
                Err(error) => {
                    return Box::new(std::iter::once(Err(Failure::io(
                        "cannot read",
                        path,
                        &error,
                    ))));
                }
            };
            let lines = std::io::BufRead::lines(std::io::BufReader::new(file)).enumerate();
            Box::new(lines.filter_map(move |(number, line)| match line {
                Err(error) => Some(Err(Failure::io("cannot read", path, &error))),
                Ok(line) if line.trim().is_empty() => None,
                Ok(line) => Some(Case::from_line(&line).map_err(|problem| {
                    Failure::Failed(format!(
                        "{}:{}: the case {problem}",
                        path.display(),
                        number + 1
                    ))
                })),
            }))
        })
}

/// Cases go to both sides this many at a time, so that a capture of gigabytes streams through.
const CHUNK: usize = 400;

fn replay(options: &Options) -> Result<ExitCode, Failure> {
    let files = if options.fixtures.is_empty() {
        default_fixtures()?
    } else {
        options.fixtures.clone()
    };
    let fixtures = read_fixtures(&files).map(|case| case.map(|case| (true, case)));
    let generated = generate::cases(options.cases, options.seed).map(|case| Ok((false, case)));
    let mut all = fixtures.chain(generated);
    let driver = workspace::root().join("xtask/python/replay_dump.py");
    let mut written = match &options.write_cases {
        Some(path) => {
            Some(std::fs::File::create(path).map_err(|e| Failure::io("cannot write", path, &e))?)
        }
        None => None,
    };
    let (mut total, mut from_fixtures, mut failures) = (0, 0, 0);
    loop {
        let chunk = all
            .by_ref()
            .take(CHUNK)
            .collect::<Result<Vec<(bool, Case)>, Failure>>()?;
        if chunk.is_empty() {
            break;
        }
        let cases: Vec<Case> = chunk.iter().map(|(_, case)| case.clone()).collect();
        from_fixtures += chunk.iter().filter(|(fixture, _)| *fixture).count();
        let lines = cases
            .iter()
            .map(Case::to_line)
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(file) = written.as_mut() {
            let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
            std::io::Write::write_all(file, text.as_bytes())
                .map_err(|e| Failure::Failed(format!("cannot write cases: {e}")))?;
        }
        let python = run::python(&driver, &lines, options.jobs)?;
        let rust = in_parallel(&cases, options.jobs, |case| {
            write(&report::report(&case.ops), Format::COMPACT)
                .unwrap_or_else(|error| format!("unwritable report: {error}"))
        });
        failures += compare(&cases, &python, &rust, "replay", total, failures)?;
        total += cases.len();
    }
    println!(
        "diff replay: {total} cases ({from_fixtures} fixtures, {} generated, seed {}), {failures} different",
        total - from_fixtures,
        options.seed
    );
    Ok(if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// `each` over `items` on `jobs` threads, the answers in order.
pub(crate) fn in_parallel<T: Sync, R: Send>(
    items: &[T],
    jobs: usize,
    each: impl Fn(&T) -> R + Sync,
) -> Vec<R> {
    let share = items.len().div_ceil(jobs.max(1)).max(1);
    std::thread::scope(|scope| {
        let each = &each;
        let running: Vec<_> = items
            .chunks(share)
            .map(|chunk| scope.spawn(move || chunk.iter().map(each).collect::<Vec<R>>()))
            .collect();
        running
            .into_iter()
            .flat_map(|handle| handle.join().unwrap_or_default())
            .collect()
    })
}

/// Counts the cases whose answers differ, shows the first few (`shown` so far), and keeps each case
/// it shows in `target/diff/` so that `--fixtures FILE` replays it alone. `offset` numbers the cases.
fn compare(
    cases: &[Case],
    python: &[String],
    rust: &[String],
    what: &str,
    offset: usize,
    shown: usize,
) -> Result<usize, Failure> {
    if rust.len() != cases.len() {
        return Err(Failure::Failed(format!(
            "rust answered {} of {} cases",
            rust.len(),
            cases.len()
        )));
    }
    let dir = workspace::output_dir("diff")?;
    let mut failures = 0;
    for (index, ((case, python), rust)) in cases.iter().zip(python).zip(rust).enumerate() {
        let Some(difference) = run::difference(python, rust) else {
            continue;
        };
        failures += 1;
        if shown + failures <= SHOWN {
            let index = offset + index;
            let path = dir.join(format!("{what}-{index}.jsonl"));
            let line = case.to_line()?;
            std::fs::write(&path, format!("{line}\n"))
                .map_err(|e| Failure::io("cannot write", &path, &e))?;
            println!(
                "case {index} ({}) differs at {difference}\n  kept in {}",
                case.name,
                path.display()
            );
        }
    }
    Ok(failures)
}
