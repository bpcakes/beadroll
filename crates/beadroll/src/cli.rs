//! The command line (Python's `cli.py`): the only code that reads the process's environment,
//! working directory and arguments; it finds the workspace, runs a command, publishes what it
//! wrote, and renders the result, as JSON with `--json` or as lines for a human.

mod args;
mod output;
mod texts;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStrExt as _;
use std::process::ExitCode;
use std::sync::Arc;

use beadroll_core::code::Code;
use beadroll_core::event::FORMAT;
use beadroll_core::json::{self, Format, Int, Object, Value};
use beadroll_store::{Settings, detect_actor, find_workspace, nearest_harness};
use beadroll_sync::{publish_soon, unpublished_count};

use crate::commands::{self, Context, Failure, Outcome};
use args::{Command, Parsed};
use output::{Stream, Unwritten, emit, visible};

/// The version this binary reports.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Runs `bead` with the process's arguments; the exit status is 0, 1 for an error, 2 for a usage
/// error.
pub(crate) fn main() -> ExitCode {
    let argv: Vec<OsString> = std::env::args_os().skip(1).collect();
    ExitCode::from(run(argv))
}

/// What the command line asked for once `--json`, `--actor` and `--` are read: everything after
/// `--` is data, never an option; `--json` counts anywhere before it, `--actor` once.
struct Request {
    as_json: bool,
    name: Option<OsString>,
    args: Vec<OsString>,
}

fn read_request(argv: Vec<OsString>) -> Request {
    let cut = argv.iter().position(|a| a == "--").unwrap_or(argv.len());
    let (head, tail) = argv.split_at(cut);
    let as_json = head.iter().any(|a| a == "--json");
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
    head.extend(tail.iter().cloned());
    Request {
        as_json,
        name,
        args: head,
    }
}

/// The settings of this process.
fn settings() -> Result<Settings, Failure> {
    let vars: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    let user_home = match vars.get(std::ffi::OsStr::new("HOME")) {
        Some(home) => home.clone(),
        None => std::env::home_dir().map(OsString::from).unwrap_or_default(),
    };
    let cwd = std::env::current_dir().ok();
    Ok(Settings::from_process(
        vars,
        cwd,
        user_home,
        std::env::temp_dir(),
        &home_of,
    )?)
}

/// The home directory of the user `name`, from the password database (Python's `pwd.getpwnam`),
/// for `~NAME` in `BEAD_HOME`.
fn home_of(name: &std::ffi::OsStr) -> Option<OsString> {
    let name = name.to_str()?;
    let user = nix::unistd::User::from_name(name).ok()??;
    Some(user.dir.into_os_string())
}

fn run(argv: Vec<OsString>) -> u8 {
    let request = read_request(argv);
    let as_json = request.as_json;
    match execute(&request) {
        Ok(None) => 0,
        Ok(Some(rendered)) => {
            match emit(Some(&rendered), Stream::Stdout) {
                Ok(()) | Err(Unwritten::Gone) => 0,
                // the answer could not be delivered: say so where it can still be read
                Err(Unwritten::Failed(error)) => report(
                    &Failure::new(Code::Io, beadroll_git::IoError::bare(&error).to_string()),
                    as_json,
                ),
            }
        }
        Err(failure) => report(&failure, as_json),
    }
}

/// Runs the request: the text for standard output, `None` when nothing (more) is to be printed.
fn execute(request: &Request) -> Result<Option<String>, Failure> {
    let settings = Arc::new(settings()?);
    let args = &request.args;
    if args.len() == 1 && (args[0] == "--version" || args[0] == "-V") {
        return Ok(Some(if request.as_json {
            let mut out = Object::new();
            out.insert("version".into(), Value::from(VERSION));
            out.insert("format".into(), Value::from(FORMAT));
            write(&Value::Object(out), Format::ASCII)?
        } else {
            format!("bead {VERSION}")
        }));
    }
    if args.is_empty() || (args.len() == 1 && (args[0] == "-h" || args[0] == "--help")) {
        return Ok(Some(if request.as_json {
            let mut out = Object::new();
            out.insert("help".into(), Value::from(texts::OVERVIEW));
            write(&Value::Object(out), Format::ASCII)?
        } else {
            texts::OVERVIEW.trim_end_matches('\n').to_owned()
        }));
    }
    let command = match args::parse(args) {
        Parsed::Command(command) => command,
        Parsed::Help(page) => {
            // `COMMAND --help`: the page, delivered like any output
            return match output::emit_raw(page.as_bytes()) {
                Ok(()) | Err(Unwritten::Gone) => Ok(None),
                Err(Unwritten::Failed(error)) => Err(beadroll_git::IoError::bare(&error).into()),
            };
        }
        Parsed::Usage(message) => return Err(Failure::new(Code::Usage, message)),
    };
    let ws = find_workspace(&settings)?;
    let name = request
        .name
        .as_ref()
        .map(|n| n.to_string_lossy().into_owned());
    let actor = detect_actor(&settings, name.as_deref(), &|| {
        nearest_harness(std::os::unix::process::parent_id(), &run_ps)
    });
    let ctx = Context {
        ws: &ws,
        actor: &actor,
    };
    let (outcome, offline, remote) = dispatch(&ctx, *command)?;
    let Outcome {
        mut result,
        mut lines,
    } = outcome;
    if ws.wrote()
        && !offline
        && let Some(warning) = publish_soon(&ws, offline, remote.as_deref())
    {
        // a local write is published at once, as a close is
        if let Some(lines) = &mut lines {
            lines.push(warning);
        }
    }
    let behind = unpublished_count(&ws)?.filter(|n| *n != Int::from(0));
    if let Some(behind) = behind {
        // after --offline, a remote that did not answer, or on a store that has never published
        if let Some(Value::Object(result)) = &mut result {
            result.insert("unpublished".into(), Value::Int(behind.clone()));
        }
        if let Some(lines) = &mut lines {
            let plural = if behind == Int::from(1) { "" } else { "s" };
            lines.push(format!(
                "{behind} event{plural} not yet published: bead sync"
            ));
        }
    }
    Ok(match (result, lines) {
        (None, None) => None,
        (result, _) if request.as_json => {
            Some(write(&result.unwrap_or(Value::Null), Format::SPACED)?)
        }
        (_, lines) => Some(
            lines
                .unwrap_or_default()
                .iter()
                .map(|line| visible(line))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
    })
}

/// Runs the command; with what it says about publishing: whether it was `--offline`, and the
/// remote it named.
fn dispatch(
    ctx: &Context<'_>,
    command: Command,
) -> Result<(Outcome, bool, Option<OsString>), Failure> {
    Ok(match command {
        Command::Init(a) => {
            let (offline, remote) = (a.offline, a.remote.clone());
            let args = commands::InitArgs {
                prefix: a.prefix,
                import_path: a.import_path,
                tracked: a.tracked,
                remote: a.remote,
                offline: a.offline,
            };
            (commands::init(ctx, args)?, offline, remote)
        }
        Command::Create(a) => {
            let args = commands::CreateArgs {
                title_pos: a.title_pos,
                title: a.title,
                description: a.description,
                description_file: a.description_file,
                acceptance_criteria: a.acceptance_criteria,
                notes: a.notes,
                design: a.design,
                priority: a.priority,
                issue_type: a.issue_type,
                status: a.status,
                assignee: a.assignee,
                estimate: a.estimate,
                external_ref: a.external_ref,
                parent: a.parent,
                labels: a.labels,
                deps: a.deps,
                silent: a.silent,
                offline: a.offline,
            };
            (commands::create(ctx, &args)?, args.offline, None)
        }
        Command::Show(a) => (commands::show(ctx, &a.ids)?, false, None),
        Command::List(a) => {
            let args = commands::ListArgs {
                status: a.status,
                all: a.all,
                deferred: a.deferred,
                title_contains: a.title_contains,
                id: a.id,
                parent: a.parent.into_iter().next(),
                filters: filters(a.filters, a.limit),
            };
            (commands::list(ctx, &args)?, false, None)
        }
        Command::Ready(a) => {
            let args = commands::ReadyArgs {
                root: a.root,
                epic: a.epic.into_iter().next(),
                parent: a.parent.into_iter().next(),
                filters: filters(a.filters, a.limit),
            };
            (commands::ready(ctx, &args)?, false, None)
        }
        Command::Sync(a) => (commands::sync(ctx, a.remote.as_ref())?, false, a.remote),
    })
}

fn filters(given: args::Filters, limit: Int) -> commands::FilterArgs {
    commands::FilterArgs {
        issue_type: given.issue_type,
        label: given.label,
        priority: given.priority,
        assignee: given.assignee,
        limit,
    }
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

fn write(value: &Value, format: Format) -> Result<String, Failure> {
    json::write(value, format).map_err(|e| Failure::new(Code::Validation, e.to_string()))
}

/// Reports `failure` on standard error: `{"error": {"code", "message"}}` with `--json`, else
/// `Error: message`. The status is the command's (2 for `USAGE`, else 1), whatever stderr does.
fn report(failure: &Failure, as_json: bool) -> u8 {
    let text = if as_json {
        let mut error = Object::new();
        error.insert("code".into(), Value::from(failure.code.as_str()));
        error.insert("message".into(), Value::from(failure.message.as_str()));
        let mut body = Object::new();
        body.insert("error".into(), Value::Object(error));
        json::write(&Value::Object(body), Format::ASCII).unwrap_or_default()
    } else {
        format!("Error: {}", visible(&failure.message))
    };
    let _ = emit(Some(&text), Stream::Stderr);
    if failure.code == Code::Usage { 2 } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn another_users_home_comes_from_the_password_database() {
        let me = nix::unistd::User::from_uid(nix::unistd::getuid())
            .unwrap()
            .unwrap();
        assert_eq!(
            home_of(std::ffi::OsStr::new(&me.name)),
            Some(me.dir.into_os_string())
        );
        assert_eq!(
            home_of(std::ffi::OsStr::new("no-such-user-bead-test")),
            None
        );
    }
}
