//! The command line: the only code that reads the process environment, working directory and
//! arguments. It finds the workspace, runs a command, publishes what it wrote and renders the
//! result as JSON with `--json` or as human-readable lines.

mod args;
#[cfg(test)]
mod ending;
mod free;
mod help;
mod layout;
mod output;
mod paint;
mod parse;
mod recap;
mod report;
mod spec;
mod tempdir;
mod texts;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStrExt as _;
use std::process::ExitCode;
use std::sync::Arc;

use args::{
    comment_args, create_args, init_args, list_args, network, ownership_args, ready_args, remote,
    transition_args, update_args,
};
use beadroll_core::code::Code;
use beadroll_core::event::FORMAT;
use beadroll_core::json::{self, Format, Object, Value};
use beadroll_core::query::Edges;
use beadroll_store::{Clock, Settings, detect_actor, find_workspace, nearest_harness};
use beadroll_sync::{NetworkMode, publish_soon_at};
use output::{Stream, Unwritten, emit};
use paint::{Colors, When};
use parse::{Namespace, Page, Parsed};
use report::{report, warn};
use spec::Scope;

use crate::cli::paint::Color;
use crate::commands::{
    self, Context, Delivered, Failure, Held, ImportMode, Line, Outcome, SearchScope, Tone,
    Transition, VERSION,
};

/// Whether another state will be read to publish the command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AfterCommand {
    /// Nothing follows: the answer is returned as the command made it.
    Return,
    /// What the command wrote is published, which reads a state of its own.
    Publish,
}

/// Whether the process ends with the command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunLifetime {
    /// The process goes on after the run (a test in process): nothing the run made is kept.
    Embedded,
    /// The process exits after the run, so what the command held need not be freed.
    Process,
}

/// The representation requested for command output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputMode {
    /// Lines for a reader, painted on a terminal.
    Human,
    /// `--json`: one JSON document on standard output, errors as JSON on standard error.
    Json,
}

#[cfg(test)]
mod tests;

/// Runs `bead` with the process's arguments; the exit status is 0, 1 for an error, 2 for a usage
/// error. The process ends with the run, so what a command held when it answered, the state it
/// answered from or the transaction it wrote in, is never freed ([`Outcome::answered_from`],
/// [`Outcome::wrote_in`]).
pub(crate) fn main() -> ExitCode {
    ExitCode::from(run(
        std::env::args_os().skip(1).collect(),
        RunLifetime::Process,
    ))
}

/// Runs `bead` with `argv`, the arguments after the program's name, as [`main`] runs it, in a
/// process that goes on afterwards: nothing the run made is kept, so that a caller that runs bead
/// again (a test in process) does not pile one command's state on another's.
pub(crate) fn main_with(argv: Vec<OsString>) -> ExitCode {
    ExitCode::from(run(argv, RunLifetime::Embedded))
}

/// What the command line asked for once `--json`, `--actor`, `--color` and `--` are read:
/// everything after `--` is data, never an option; `--json` counts anywhere before it, `--actor`
/// once, the last `--color` (`None` for one without its value).
struct Request {
    output: OutputMode,
    name: Option<OsString>,
    color: Option<Option<OsString>>,
    args: Vec<OsString>,
}

fn read_request(argv: Vec<OsString>) -> Request {
    let cut = argv.iter().position(|a| a == "--").unwrap_or(argv.len());
    let (head, tail) = argv.split_at(cut);
    let output = if head.iter().any(|a| a == "--json") {
        OutputMode::Json
    } else {
        OutputMode::Human
    };
    let mut head: Vec<OsString> = head.iter().filter(|a| *a != "--json").cloned().collect();
    let mut name = None;
    for index in 0..head.len() {
        if head[index] == "--actor" && index + 1 < head.len() {
            name = Some(head[index + 1].clone());
            head.drain(index..index + 2);
            break;
        }
        if let Some(value) = head[index].as_bytes().strip_prefix(b"--actor=") {
            name = Some(OsString::from(std::ffi::OsStr::from_bytes(value)));
            head.remove(index);
            break;
        }
    }
    let mut color = None;
    let mut index = 0;
    while index < head.len() {
        if head[index] == "--color" {
            color = Some(head.get(index + 1).cloned());
            head.drain(index..(index + 2).min(head.len()));
        } else if let Some(value) = head[index].as_bytes().strip_prefix(b"--color=") {
            color = Some(Some(OsString::from(std::ffi::OsStr::from_bytes(value))));
            head.remove(index);
        } else {
            index += 1;
        }
    }
    head.extend(tail.iter().cloned());
    Request {
        output,
        name,
        color,
        args: head,
    }
}

/// Which streams show colour: none with `--json`, else as `--color` and the environment say
/// ([`paint::colors`]); a `--color` that names no choice is a usage error.
fn colors_for(request: &Request) -> Result<Colors, Failure> {
    let when = match &request.color {
        None => When::Auto,
        Some(given) => given.as_deref().and_then(When::named).ok_or_else(|| {
            let shown = given
                .as_deref()
                .map_or_else(String::new, |g| g.to_string_lossy().into_owned());
            Failure::usage(
                format!(
                    "invalid value '{shown}' for '--color <WHEN>' [possible values: auto, \
                     always, never]"
                ),
                "",
            )
        })?,
    };
    if request.output == OutputMode::Json {
        return Ok(Colors::default());
    }
    Ok(paint::colors(when, &|key| std::env::var_os(key)))
}

/// The settings of this process.
fn settings() -> Result<Settings, Failure> {
    let vars: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    let user_home = match vars.get(std::ffi::OsStr::new("HOME")) {
        Some(home) => home.clone(),
        None => std::env::home_dir().map(OsString::from).unwrap_or_default(),
    };
    let cwd = std::env::current_dir().ok();
    let mut settings =
        Settings::from_process(vars, cwd, user_home, std::env::temp_dir(), &home_of)?;
    settings.build = build();
    Ok(settings)
}

/// Which build of bead this is: its version and its executable's identity on disk, which every
/// build and every install changes. `None` when the executable cannot be found: bead then keeps
/// no projection.
fn build() -> Option<String> {
    use std::os::unix::fs::MetadataExt as _;
    let meta = std::fs::metadata(std::env::current_exe().ok()?).ok()?;
    Some(format!(
        "{} {}:{} {} {}.{:09}",
        VERSION,
        meta.dev(),
        meta.ino(),
        meta.len(),
        meta.mtime(),
        meta.mtime_nsec()
    ))
}

/// The home directory of user `name` from the password database, for `~NAME` in `BEAD_HOME`.
fn home_of(name: &std::ffi::OsStr) -> Option<OsString> {
    let name = name.to_str()?;
    let user = nix::unistd::User::from_name(name).ok()??;
    Some(user.dir.into_os_string())
}

fn run(argv: Vec<OsString>, run_lifetime: RunLifetime) -> u8 {
    let request = read_request(argv);
    let output = request.output;
    let colors = match colors_for(&request) {
        Ok(colors) => colors,
        Err(failure) => {
            return report(&failure, output, Color::Plain);
        }
    };
    match execute(&request, colors, run_lifetime) {
        Ok(None) => 0,
        Ok(Some(printed)) => match emit(Some(&printed.text), Stream::Stdout) {
            // written: then what follows it (recap marks what it showed as seen), whose failure is
            // reported where it can still be read
            Ok(()) => match printed.delivered.map_or(Ok(()), Delivered::done) {
                Ok(()) => 0,
                Err(failure) => report(&failure, output, colors.stderr),
            },
            Err(Unwritten::Gone) => 0,
            // the answer could not be delivered: say so where it can still be read
            Err(Unwritten::Failed(error)) => report(
                &Failure::new(Code::Io, beadroll_git::IoError::bare(&error).to_string()),
                output,
                colors.stderr,
            ),
        },
        Err(failure) => report(&failure, output, colors.stderr),
    }
}

/// What a run prints on standard output, and what it does once that is written.
struct Printed {
    text: String,
    delivered: Option<Delivered>,
}

impl From<String> for Printed {
    fn from(text: String) -> Printed {
        Printed {
            text,
            delivered: None,
        }
    }
}

/// Runs the request: what standard output is to show, `None` when nothing (more) is to be printed.
/// Standard output shows colour when `colors` says so; `run_lifetime` says whether the process
/// ends with the run (`Process`) or goes on after it (`Embedded`).
fn execute(
    request: &Request,
    colors: Colors,
    run_lifetime: RunLifetime,
) -> Result<Option<Printed>, Failure> {
    let output_mode = request.output;
    let mut settings = settings()?;
    let args = &request.args;
    if args.len() == 1 && (args[0] == "--version" || args[0] == "-V") {
        return Ok(Some(Printed::from(if request.output == OutputMode::Json {
            let mut out = Object::new();
            out.insert("version".into(), Value::from(VERSION));
            out.insert("format".into(), Value::from(FORMAT));
            json::write(&Value::from(out), Format::ASCII)?
        } else {
            format!("bead {VERSION}")
        })));
    }
    let overview = || -> Result<Option<Printed>, Failure> {
        Ok(Some(Printed::from(if request.output == OutputMode::Json {
            // laid out for no width: the reader wraps it
            let mut out = Object::new();
            let overview = layout::lay_out(texts::OVERVIEW, None);
            out.insert("help".into(), Value::from(overview));
            json::write(&Value::from(out), Format::ASCII)?
        } else {
            let width = layout::width(settings.var("COLUMNS"));
            let overview = layout::lay_out_lines(texts::OVERVIEW, width);
            paint::render_all(&overview, colors.stdout)
        })))
    };
    if args.is_empty() || (args.len() == 1 && (args[0] == "-h" || args[0] == "--help")) {
        return overview();
    }
    let ns = match parse::parse(args) {
        Parsed::Command(ns) => ns,
        Parsed::Help(Page::Top) => return overview(),
        Parsed::Help(Page::Command(spec)) => {
            // `COMMAND --help`: the page, delivered like any output
            let page = help::page(spec, layout::width(settings.var("COLUMNS")), colors.stdout);
            return match output::emit_raw(page.as_bytes()) {
                Ok(()) | Err(Unwritten::Gone) => Ok(None),
                Err(Unwritten::Failed(error)) => Err(beadroll_git::IoError::bare(&error).into()),
            };
        }
        Parsed::Usage(usage) => return Err(Failure::usage(usage.message, usage.usage)),
    };
    if ns.spec.scope == Scope::Home {
        // every tracker of this machine's and no checkout: none to find, and no git to start
        let args = recap::args(&ns, output_mode, &settings.clock)?;
        let (outcome, delivered) = commands::recap(&Arc::new(settings), &args)?;
        let text = rendered((outcome.result, outcome.lines), output_mode, colors)?;
        return Ok(text.map(|text| Printed { text, delivered }));
    }
    let free = ns.spec.scope == Scope::Free;
    // Select a directory for output files before starting the first git process.
    let tmp = tempdir::temporary_directory(settings.git.vars(), Some(settings.cwd.as_path()))?;
    settings.git = settings.git.with_tmp(tmp);
    let settings = Arc::new(settings);
    let ws = match find_workspace(&settings) {
        Ok(ws) => Some(ws),
        // help, prime and onboard work anywhere: they pass over bead's own errors, not the
        // system's
        Err(error) if free && error.code() != Code::Io => None,
        Err(error) => return Err(error.into()),
    };
    let actor = detect_actor(&settings, request.name.as_deref(), &|| {
        nearest_harness(std::os::unix::process::parent_id(), &run_ps)
    });
    let (outcome, network, remote) = match &ws {
        Some(ws) if !free => dispatch(&Context { ws, actor: &actor }, &ns)?,
        _ => (
            free::run(&ns, ws.as_ref(), &actor, &settings, colors.stdout)?,
            NetworkMode::Online,
            None,
        ),
    };
    let finished = finish(
        outcome,
        ws.as_ref().filter(|_| !free),
        network,
        &remote,
        run_lifetime,
        |warning| warn(&warning, output_mode, colors.stderr),
    )?;
    let Some(answer) = finished else {
        return Ok(None);
    };
    Ok(rendered(answer, output_mode, colors)?.map(Printed::from))
}

/// A command's answer: the result `--json` prints and the lines a human reads.
type Answer = (Option<Value>, Option<Vec<Line>>);

/// `answer` as standard output shows it: its result as JSON with `--json`, else its lines, painted
/// when `colors` says so; `None` when it has neither.
fn rendered(
    (result, lines): Answer,
    output: OutputMode,
    colors: Colors,
) -> Result<Option<String>, Failure> {
    Ok(match (result, lines) {
        (None, None) => None,
        (result, _) if output == OutputMode::Json => {
            Some(json::write(&result.unwrap_or(Value::Null), Format::SPACED)?)
        }
        (_, lines) => Some(paint::render_all(&lines.unwrap_or_default(), colors.stdout)),
    })
}

/// What follows a command: its own output written (`export`), what it wrote published unless it
/// was `--offline` or the tracker has no remote, and the count of events not yet published added;
/// the result and lines then, or `None` when a reader went away. What the command held goes first
/// ([`ended_with`]) when what it wrote is to reach a remote, and is kept otherwise.
fn finish(
    outcome: Outcome,
    ws: Option<&beadroll_store::Workspace>,
    network_mode: NetworkMode,
    remote: &Option<OsString>,
    run_lifetime: RunLifetime,
    mut warning: impl FnMut(Failure),
) -> Result<Option<Answer>, Failure> {
    let Outcome {
        mut result,
        mut lines,
        raw,
        settled,
        held,
        count_warning,
    } = outcome;
    let ws = ws.filter(|_| !settled);
    // where what the command wrote goes, chosen before what it held is let go of: a write with no
    // remote to reach keeps it, as an offline write does (roll-lqjj.50)
    let destination = ws
        .filter(|ws| ws.wrote())
        .and_then(|ws| beadroll_sync::destination(ws, network_mode, remote.as_deref()));
    ended_with(
        held,
        if matches!(destination, Some(Ok(_))) {
            AfterCommand::Publish
        } else {
            AfterCommand::Return
        },
        run_lifetime,
    );
    if let Some(count_warning) = count_warning {
        warning(count_warning);
    }
    if let Some(raw) = raw {
        // the command's own output (`export`): written as the command runs, so a reader that went
        // away ends the run there, and a failed write is the command's IO error
        match output::emit_raw(raw.as_bytes()) {
            Ok(()) => {}
            Err(Unwritten::Gone) => return Ok(None),
            Err(Unwritten::Failed(error)) => {
                return Err(beadroll_git::IoError::bare(&error).into());
            }
        }
    }
    let Some(ws) = ws else {
        return Ok(Some((result, lines)));
    };
    if let Some(warning) = publish_soon_at(ws, destination) {
        // a local write is published at once, as a close is
        if let Some(lines) = &mut lines {
            lines.push(Line::toned(Tone::Warning, warning));
        }
    }
    if let Some(count_warning) = commands::note_unpublished(ws, &mut result, &mut lines)? {
        warning(count_warning);
    }
    Ok(Some((result, lines)))
}

/// What becomes of what the command held (the state it answered from, or the transaction a
/// write was made in), once the command is done: freed, or kept until the process exits when
/// the run's `lifetime` is the process's, which is whether it was kept. A command answers from
/// its state, or writes, and the process then ends, so freeing each of its issues first is work
/// nobody waits for, a quarter of a reading command's time on a large tracker (roll-lqjj.32),
/// and about 83 ms of a write's at 200,000 events (roll-lqjj.42). Freed instead when what
/// follows the command (`after`) publishes what it wrote, which reads a state of its own, so
/// that none is held beside another.
fn ended_with(held: Option<Box<dyn Held>>, after: AfterCommand, lifetime: RunLifetime) -> bool {
    match (held, after, lifetime) {
        (Some(held), AfterCommand::Return, RunLifetime::Process) => {
            // never freed: the process ends once the answer is written
            std::mem::forget(held);
            true
        }
        _ => false,
    }
}

/// Runs the command; with what it says about publishing: whether it may reach the remote (not
/// with `--offline`), and the remote it named.
fn dispatch(
    ctx: &Context<'_>,
    ns: &Namespace,
) -> Result<(Outcome, NetworkMode, Option<OsString>), Failure> {
    let network = network(ns);
    let remote = remote(ns);
    let outcome = match ns.spec.name {
        "init" => commands::init(ctx, init_args(ns))?,
        "import" => commands::import(
            ctx,
            ns.text("path").unwrap_or_default(),
            if ns.flag("dry_run") {
                ImportMode::DryRun
            } else {
                ImportMode::Apply
            },
        )?,
        "export" => commands::export(ctx, ns.text("output"))?,
        "create" => commands::create(ctx, &create_args(ns))?,
        "update" => commands::update(ctx, &update_args(ns))?,
        "close" => commands::transition(ctx, Transition::Close, &transition_args(ns))?,
        "reopen" => commands::transition(ctx, Transition::Reopen, &transition_args(ns))?,
        "delete" => commands::transition(ctx, Transition::Delete, &transition_args(ns))?,
        "show" => commands::show(ctx, ns.texts("ids"))?,
        "list" => commands::list(ctx, &list_args(ns))?,
        "ready" => commands::ready(ctx, &ready_args(ns))?,
        "blocked" => commands::blocked(ctx)?,
        "search" => commands::search(
            ctx,
            ns.text("query").unwrap_or_default(),
            if ns.flag("all") {
                SearchScope::All
            } else {
                SearchScope::Open
            },
        )?,
        "dep" => commands::dep(
            ctx,
            ns.texts("words"),
            ns.text("type"),
            if ns.flag("blocking_only") {
                Edges::Blocking
            } else {
                Edges::WaitsFor
            },
        )?,
        "comments" => commands::comments(ctx, &comment_args(ns))?,
        "lint" => commands::lint(ctx)?,
        "sync" => commands::sync(ctx, remote.as_ref())?,
        "log" => commands::log(ctx, ns.text("key"), ns.int("limit"))?,
        "info" => commands::info(ctx)?,
        "machines" => commands::machines(ctx, ns.text("name"))?,
        "claims" => commands::claims(ctx, real_seconds(&ctx.ws.settings.clock))?,
        "claim" => commands::claim(ctx, &ownership_args(ns))?,
        "takeover" => commands::takeover(ctx, &ownership_args(ns))?,
        "release" => commands::release(ctx, &ownership_args(ns))?,
        // the parser takes only the commands of the spec, each dispatched above
        other => return Err(Failure::new(Code::Usage, format!("no command {other}"))),
    };
    Ok((outcome, network, remote))
}

/// Seconds since the epoch on the real clock ([`Clock::real_ns`]), which the `clock` failpoint
/// does not shift: how long a claim has been held.
fn real_seconds(clock: &Clock) -> f64 {
    clock.real_ns() as f64 / 1e9
}

/// `ps -o ppid=,args= -p PID` from `PATH`: its exit status and output, or `None` when it cannot run.
fn run_ps(pid: u32) -> Option<(i32, String)> {
    let output = std::process::Command::new("ps")
        .args(["-o", "ppid=,args=", "-p", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    let status = output.status.code().unwrap_or(-1);
    Some((status, String::from_utf8_lossy(&output.stdout).into_owned()))
}
