//! `cargo xtask timing`: runs a command and records what it took as one JSON line, evidence for
//! bead's budgets (`docs/REVIEWING.md` §2). With `--replay N` it times
//! beadroll-core's replay of N events in-process instead ([`replay`]), and measures the peak memory
//! of a process that loads them from disk and replays them ([`memory`], `--load DIR`); with `--sync
//! N`, a publishing `bead sync` on a store of N events ([`sync`]); with `--read N`, the reading
//! commands there ([`read`]); with `--merge N`, a sync that merges what another machine wrote
//! after it ([`merge`]); with `--contend M`, M machines writing at once ([`contend`]); with
//! `--import N`, importing a generated export of N records ([`import`]); with `--plan N`, the
//! core calls an import of N records makes, on the dependency shape scanning the batch was worst
//! for ([`plan`]); with `--extend N`, events applied to a projection of N issues one at a time
//! and in one batch ([`extend`]); with `--dangling N`, replay of N creates whose imports name keys
//! no issue has yet ([`dangling`]); with `--ready N`, readiness judged on N issues in chains and a
//! lattice, for every issue at once and issue by issue ([`ready`]); with `--details N`, every issue
//! of N shown in full, its dependents found once for all and for each alone ([`details`]); with
//! `--renames N`, replay of N copies renamed, against the same creates renamed by nobody
//! ([`renames`]); with `--decode N`, a projection of N events decoded, its state part alone and
//! whole, and read back, extended by comments and encoded: after one, at the worst point between
//! two compactions and one comment past it ([`decode`]). A timing never decides whether anything
//! passes.

mod contend;
mod dangling;
mod decode;
mod details;
mod extend;
#[cfg(test)]
mod growth;
pub(crate) mod heap;
mod history;
mod import;
pub(crate) mod memory;
mod merge;
mod parse;
mod plan;
mod read;
mod ready;
mod renames;
mod replay;
mod sync;
mod world;

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::timing::memory::LoadMode;
use crate::timing::merge::MergeHistory;
use crate::workspace::{self, Failure, json_string};

struct Options {
    label: Option<String>,
    out: Option<PathBuf>,
    command: Vec<OsString>,
    /// Time replay of this many events instead of a command.
    replay: Option<usize>,
    /// Time a publishing sync on a store of this many events instead of a command.
    sync: Option<usize>,
    /// Time the reading commands on a store of this many events instead of a command.
    read: Option<usize>,
    /// Time a sync that merges another machine's events into a store of this many instead of a
    /// command.
    merge: Option<usize>,
    /// How many events each `--merge` brings.
    batch: Option<usize>,
    /// With `--merge`, the store writes too before each merge, so that what it merges was
    /// written concurrently with its own.
    concurrent: bool,
    /// Time this many machines writing one tracker at once instead of a command.
    contend: Option<usize>,
    /// Time importing a generated export of this many records instead of a command.
    import: Option<usize>,
    /// Time the core calls an import of this many records makes instead of a command.
    plan: Option<usize>,
    /// Time events applied to a projection of this many issues instead of a command.
    extend: Option<usize>,
    /// Time replay of this many creates naming keys no issue has yet instead of a command.
    dangling: Option<usize>,
    /// Time readiness judged on this many issues instead of a command.
    ready: Option<usize>,
    /// Time the details of this many issues instead of a command.
    details: Option<usize>,
    /// Time replay renaming this many copies instead of a command.
    renames: Option<usize>,
    /// Time decoding a projection of this many events instead of a command.
    decode: Option<usize>,
    /// Time core's JSON reader on `meta.json` of this many files, and on the lines of a history
    /// of as many events, parsed as values and read into a journal, instead of a command.
    parse: Option<usize>,
    /// How many writes each `--contend` machine makes.
    writes: Option<usize>,
    /// How many milliseconds each fetch and push of `--contend` waits.
    latency: Option<usize>,
    /// Where `--sync` rotates the journals it writes.
    rotate: Option<usize>,
    /// Which history `--replay` or `--sync` generates.
    shape: history::Shape,
    /// Load the journal written in this directory and replay it once: the process whose peak
    /// memory `--replay` measures.
    load: Option<PathBuf>,
    /// With `--load`, stop once the journal is loaded.
    no_replay: bool,
}

fn parse(args: Vec<OsString>) -> Result<Options, Failure> {
    let mut options = Options {
        label: None,
        out: None,
        command: Vec::new(),
        replay: None,
        sync: None,
        read: None,
        merge: None,
        batch: None,
        concurrent: false,
        contend: None,
        import: None,
        plan: None,
        extend: None,
        dangling: None,
        ready: None,
        details: None,
        renames: None,
        decode: None,
        parse: None,
        writes: None,
        latency: None,
        rotate: None,
        shape: history::Shape::Plain,
        load: None,
        no_replay: false,
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy().into_owned();
        let mut value = |name: &str| {
            args.next()
                .ok_or_else(|| Failure::Usage(format!("{name} needs a value")))
        };
        match text.as_str() {
            "--label" => options.label = Some(value("--label")?.to_string_lossy().into_owned()),
            "--out" => options.out = Some(PathBuf::from(value("--out")?)),
            "--replay" => options.replay = Some(number(&value("--replay")?, "--replay", "events")?),
            "--sync" => options.sync = Some(number(&value("--sync")?, "--sync", "events")?),
            "--read" => options.read = Some(number(&value("--read")?, "--read", "events")?),
            "--merge" => options.merge = Some(number(&value("--merge")?, "--merge", "events")?),
            "--batch" => options.batch = Some(number(&value("--batch")?, "--batch", "events")?),
            "--concurrent" => options.concurrent = true,
            "--import" => {
                options.import = Some(number(&value("--import")?, "--import", "records")?)
            }
            "--plan" => options.plan = Some(number(&value("--plan")?, "--plan", "records")?),
            "--extend" => {
                options.extend = Some(number(&value("--extend")?, "--extend", "issues")?);
            }
            "--dangling" => {
                options.dangling = Some(number(&value("--dangling")?, "--dangling", "issues")?);
            }
            "--ready" => options.ready = Some(number(&value("--ready")?, "--ready", "issues")?),
            "--details" => {
                options.details = Some(number(&value("--details")?, "--details", "issues")?);
            }
            "--renames" => {
                options.renames = Some(number(&value("--renames")?, "--renames", "copies")?);
            }
            "--decode" => options.decode = Some(number(&value("--decode")?, "--decode", "events")?),
            "--parse" => options.parse = Some(number(&value("--parse")?, "--parse", "files")?),
            "--contend" => {
                options.contend = Some(number(&value("--contend")?, "--contend", "machines")?);
            }
            "--writes" => options.writes = Some(number(&value("--writes")?, "--writes", "writes")?),
            "--latency" => {
                options.latency = Some(number(&value("--latency")?, "--latency", "milliseconds")?);
            }
            "--rotate" => options.rotate = Some(number(&value("--rotate")?, "--rotate", "bytes")?),
            "--ownership" => options.shape = history::Shape::Ownership,
            "--churn" => options.shape = history::Shape::Churn,
            "--text" => options.shape = history::Shape::Text,
            "--load" => options.load = Some(PathBuf::from(value("--load")?)),
            "--no-replay" => options.no_replay = true,
            "--" => {
                options.command = args.collect();
                break;
            }
            _ => {
                return Err(Failure::Usage(format!(
                    "unexpected {text}; the command follows --"
                )));
            }
        }
    }
    check(&options)?;
    Ok(options)
}

/// Why `options` do not go together, if they do not.
fn check(options: &Options) -> Result<(), Failure> {
    let modes = [
        options.replay,
        options.sync,
        options.read,
        options.merge,
        options.contend,
        options.import,
        options.plan,
        options.extend,
        options.dangling,
        options.ready,
        options.details,
        options.renames,
        options.decode,
        options.parse,
    ];
    let measured = modes.iter().flatten().count();
    if options.command.is_empty() && measured == 0 && options.load.is_none() {
        return Err(Failure::Usage(
            "timing needs --replay N, --sync N, --read N, --merge N, --contend M, --import N, \
             --plan N, --extend N, --dangling N, --ready N, --details N, --renames N, --decode N, \
             --parse N, or a command after --"
                .into(),
        ));
    }
    if measured > 1 {
        return Err(Failure::Usage(
            "one of --replay N, --sync N, --read N, --merge N, --contend M, --import N, \
             --plan N, --extend N, --dangling N, --ready N, --details N, --renames N, \
             --decode N and --parse N"
                .into(),
        ));
    }
    if options.concurrent && options.merge.is_none() {
        return Err(Failure::Usage("--concurrent goes with --merge N".into()));
    }
    if options.contend == Some(0)
        || (options.writes.is_some() || options.latency.is_some()) && options.contend.is_none()
    {
        return Err(Failure::Usage(
            "--writes W and --latency MS go with --contend M, of one machine or more".into(),
        ));
    }
    if options.shape != history::Shape::Plain && measured == 0 {
        return Err(Failure::Usage(
            "--ownership, --churn and --text shape the history of a measurement".into(),
        ));
    }
    if options.batch == Some(0) || options.batch.is_some() && options.merge.is_none() {
        return Err(Failure::Usage(
            "--batch M, more than none, goes with --merge N".into(),
        ));
    }
    if options.rotate == Some(0) || options.rotate.is_some() && options.sync.is_none() {
        return Err(Failure::Usage(
            "--rotate BYTES, more than none, goes with --sync N".into(),
        ));
    }
    if options.no_replay && options.load.is_none() {
        return Err(Failure::Usage("--no-replay goes with --load DIR".into()));
    }
    Ok(())
}

pub(crate) fn main(args: Vec<OsString>) -> Result<ExitCode, Failure> {
    let options = parse(args)?;
    if let Some(dir) = &options.load {
        let mode = if options.no_replay {
            LoadMode::Journal
        } else {
            LoadMode::Replay
        };
        return memory::main(dir, mode);
    }
    let out = match options.out {
        Some(out) => out,
        None => workspace::target_dir()?.join("timing.jsonl"),
    };
    let label = options.label.as_deref();
    let started = SystemTime::now();
    // one mode at most, as checked above, so the order of these is no precedence
    let measured = if let Some(count) = options.replay {
        Some(replay::measure(count, options.shape, label)?)
    } else if let Some(count) = options.sync {
        let rotate = options.rotate.unwrap_or(sync::ROTATE_BYTES);
        Some(sync::measure(count, options.shape, rotate, label)?)
    } else if let Some(count) = options.read {
        Some(read::measure(count, options.shape, label)?)
    } else if let Some(count) = options.merge {
        let batch = options.batch.unwrap_or(merge::BATCH);
        let history = if options.concurrent {
            MergeHistory::Concurrent
        } else {
            MergeHistory::Sequential
        };
        Some(merge::measure(count, options.shape, batch, history, label)?)
    } else if let Some(machines) = options.contend {
        let writes = options.writes.unwrap_or(contend::WRITES);
        let latency = options.latency.map_or(contend::LATENCY, |ms| ms as u64);
        Some(contend::measure(machines, writes, latency, label)?)
    } else if let Some(records) = options.import {
        Some(import::measure(records, label)?)
    } else if let Some(records) = options.plan {
        Some(plan::measure(records, label)?)
    } else if let Some(issues) = options.extend {
        Some(extend::measure(issues, label)?)
    } else if let Some(issues) = options.dangling {
        Some(dangling::measure(issues, label)?)
    } else if let Some(issues) = options.ready {
        Some(ready::measure(issues, label)?)
    } else if let Some(issues) = options.details {
        Some(details::measure(issues, label)?)
    } else if let Some(copies) = options.renames {
        Some(renames::measure(copies, label)?)
    } else if let Some(count) = options.decode {
        Some(decode::measure(count, options.shape, label)?)
    } else if let Some(count) = options.parse {
        Some(parse::measure(count, options.shape, label)?)
    } else {
        None
    };
    if let Some(measured) = measured {
        let line = format!("{measured}{}", context(started));
        append(&out, &line)?;
        println!("timing: {line}\nrecorded in {}", out.display());
        return Ok(ExitCode::SUCCESS);
    }
    let (program, args) = options
        .command
        .split_first()
        .ok_or_else(|| Failure::Usage("timing needs a command after --".into()))?;
    let started = SystemTime::now();
    let clock = Instant::now();
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|error| Failure::Failed(format!("cannot run {program:?}: {error}")))?;
    let seconds = clock.elapsed().as_secs_f64();
    let record = Record {
        label: options.label.as_deref(),
        command: &options.command,
        status,
        seconds,
        started,
        commit: git(&["rev-parse", "HEAD"]).map(|out| out.trim().to_owned()),
        dirty: git(&["status", "--porcelain"]).map(|out| !out.trim().is_empty()),
    };
    append(&out, &record.to_json())?;
    println!("timing: {seconds:.3} s, recorded in {}", out.display());
    Ok(if status.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// A count given to `option`, of `what`.
/// `name` under the target's `timing/` directory, emptied of whatever an earlier run left there and
/// not yet made: where a timing writes its stores and histories.
pub(crate) fn scratch(name: &str) -> Result<std::path::PathBuf, Failure> {
    let dir = workspace::output_dir("timing")?.join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|error| Failure::io("cannot remove", &dir, &error))?;
    }
    Ok(dir)
}

/// The fastest of `runs` runs (one at least) of `run` on what `given` makes for each, made before the
/// clock starts, and what the last run made: the shape every in-process timing takes. What a run
/// made is let go of after its clock stops, never while it runs.
pub(crate) fn fastest_of<G, T, E>(
    runs: usize,
    mut given: impl FnMut() -> G,
    mut run: impl FnMut(G) -> Result<T, E>,
) -> Result<(f64, T), E> {
    let mut timed = || -> Result<(f64, T), E> {
        let input = given();
        let clock = Instant::now();
        let made = run(input)?;
        Ok((clock.elapsed().as_secs_f64(), made))
    };
    let (mut best, mut last) = timed()?;
    for _ in 1..runs {
        let (seconds, made) = timed()?;
        best = best.min(seconds);
        last = made;
    }
    Ok((best, last))
}

fn number(text: &std::ffi::OsStr, option: &str, what: &str) -> Result<usize, Failure> {
    let text = text.to_string_lossy();
    text.parse()
        .map_err(|_| Failure::Usage(format!("{option} takes a number of {what}, not {text}")))
}

/// git's output in the repository, or `None` if git did not answer.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace::root())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn append(path: &Path, line: &str) -> Result<(), Failure> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| Failure::io("cannot open", path, &error))?;
    writeln!(file, "{line}").map_err(|error| Failure::io("cannot write", path, &error))
}

/// What one timed run leaves as evidence: no host name or user, only what the machine is.
struct Record<'a> {
    label: Option<&'a str>,
    command: &'a [OsString],
    status: ExitStatus,
    seconds: f64,
    started: SystemTime,
    commit: Option<String>,
    dirty: Option<bool>,
}

impl Record<'_> {
    fn to_json(&self) -> String {
        let text = |value: Option<&str>| value.map_or_else(|| "null".to_owned(), json_string);
        let command: Vec<String> = self
            .command
            .iter()
            .map(|arg| json_string(&arg.to_string_lossy()))
            .collect();
        let status = match (self.status.code(), self.status.signal()) {
            (Some(code), _) => code.to_string(),
            (None, Some(signal)) => (-signal).to_string(),
            (None, None) => "null".to_owned(),
        };
        let cpus = std::thread::available_parallelism().map_or(0, usize::from);
        format!(
            "{{\"label\": {}, \"command\": [{}], \"status\": {status}, \"seconds\": {:.3}, \"started\": {}, \
             \"commit\": {}, \"dirty\": {}, \"os\": {}, \"arch\": {}, \"cpus\": {cpus}}}",
            text(self.label),
            command.join(", "),
            self.seconds,
            json_string(&utc(self.started)),
            text(self.commit.as_deref()),
            self.dirty
                .map_or("null", |dirty| if dirty { "true" } else { "false" }),
            json_string(std::env::consts::OS),
            json_string(std::env::consts::ARCH),
        )
    }
}

/// The rest of a record: when, at which commit, and on what machine, closing the object.
fn context(started: SystemTime) -> String {
    let commit = git(&["rev-parse", "HEAD"]).map(|out| out.trim().to_owned());
    let dirty = git(&["status", "--porcelain"]).map(|out| !out.trim().is_empty());
    let cpus = std::thread::available_parallelism().map_or(0, usize::from);
    format!(
        "\"started\": {}, \"commit\": {}, \"dirty\": {}, \"os\": {}, \"arch\": {}, \"cpus\": {cpus}}}",
        json_string(&utc(started)),
        commit
            .as_deref()
            .map_or_else(|| "null".to_owned(), json_string),
        dirty.map_or("null", |dirty| if dirty { "true" } else { "false" }),
        json_string(std::env::consts::OS),
        json_string(std::env::consts::ARCH),
    )
}

/// `2026-10-09T15:04:05Z`, in UTC, to the second, as beadroll-core stamps a sync.
fn utc(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    beadroll_core::time::utc_stamp(secs)
}

#[cfg(test)]
mod tests;
