//! The command line: the only code that reads the process environment, working directory and
//! arguments. It finds the workspace, runs a command, publishes what it wrote and renders the
//! result as JSON with `--json` or as human-readable lines.

mod free;
mod help;
mod layout;
mod output;
mod paint;
mod parse;
mod spec;
mod tempdir;
mod texts;

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt as _;
use std::process::ExitCode;
use std::sync::Arc;

use beadroll_core::code::Code;
use beadroll_core::event::FORMAT;
use beadroll_core::json::{self, Format, Int, Object, Value};
use beadroll_core::text::json_ascii_bytes;
use beadroll_store::{Settings, detect_actor, find_workspace, nearest_harness};
use beadroll_sync::publish_soon;

use crate::commands::{self, Context, Failure, Line, Outcome, Tone};
use output::{Stream, Unwritten, emit};
use paint::{Colors, When};
use parse::{Namespace, Page, Parsed};

/// The version this binary reports.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Runs `bead` with the process's arguments; the exit status is 0, 1 for an error, 2 for a usage
/// error.
pub(crate) fn main() -> ExitCode {
    main_with(std::env::args_os().skip(1).collect())
}

/// Runs `bead` with `argv`, the arguments after the program's name, as [`main`] runs it.
pub(crate) fn main_with(argv: Vec<OsString>) -> ExitCode {
    ExitCode::from(run(argv))
}

/// What the command line asked for once `--json`, `--actor`, `--color` and `--` are read:
/// everything after `--` is data, never an option; `--json` counts anywhere before it, `--actor`
/// once, the last `--color` (`None` for one without its value).
struct Request {
    as_json: bool,
    name: Option<OsString>,
    color: Option<Option<OsString>>,
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
        as_json,
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
    if request.as_json {
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
        env!("CARGO_PKG_VERSION"),
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

fn run(argv: Vec<OsString>) -> u8 {
    let request = read_request(argv);
    let as_json = request.as_json;
    let colors = match colors_for(&request) {
        Ok(colors) => colors,
        Err(failure) => return report(&failure, as_json, false),
    };
    match execute(&request, colors) {
        Ok(None) => 0,
        Ok(Some(rendered)) => {
            match emit(Some(&rendered), Stream::Stdout) {
                Ok(()) | Err(Unwritten::Gone) => 0,
                // the answer could not be delivered: say so where it can still be read
                Err(Unwritten::Failed(error)) => report(
                    &Failure::new(Code::Io, beadroll_git::IoError::bare(&error).to_string()),
                    as_json,
                    colors.stderr,
                ),
            }
        }
        Err(failure) => report(&failure, as_json, colors.stderr),
    }
}

/// Runs the request: the text for standard output, `None` when nothing (more) is to be printed.
/// Standard output shows colour when `colors` says so.
fn execute(request: &Request, colors: Colors) -> Result<Option<String>, Failure> {
    let mut settings = settings()?;
    let args = &request.args;
    if args.len() == 1 && (args[0] == "--version" || args[0] == "-V") {
        return Ok(Some(if request.as_json {
            let mut out = Object::new();
            out.insert("version".into(), Value::from(VERSION));
            out.insert("format".into(), Value::from(FORMAT));
            write(&Value::from(out), Format::ASCII)?
        } else {
            format!("bead {VERSION}")
        }));
    }
    let overview = || -> Result<Option<String>, Failure> {
        Ok(Some(if request.as_json {
            // laid out for no width: the reader wraps it
            let mut out = Object::new();
            let overview = layout::lay_out(texts::OVERVIEW, None);
            out.insert("help".into(), Value::from(overview));
            write(&Value::from(out), Format::ASCII)?
        } else {
            let width = layout::width(settings.var("COLUMNS"));
            let overview = layout::lay_out_lines(texts::OVERVIEW, width);
            paint::render_all(&overview, colors.stdout)
        }))
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
    let free = ns.spec.workspace_free;
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
    let (outcome, offline, remote) = match &ws {
        Some(ws) if !free => dispatch(&Context { ws, actor: &actor }, &ns)?,
        _ => (
            free::run(&ns, ws.as_ref(), &actor, &settings, colors.stdout)?,
            false,
            None,
        ),
    };
    let Some((result, lines)) = finish(outcome, ws.as_ref().filter(|_| !free), offline, &remote)?
    else {
        return Ok(None);
    };
    Ok(match (result, lines) {
        (None, None) => None,
        (result, _) if request.as_json => {
            Some(write(&result.unwrap_or(Value::Null), Format::SPACED)?)
        }
        (_, lines) => Some(paint::render_all(&lines.unwrap_or_default(), colors.stdout)),
    })
}

/// A command's answer: the result `--json` prints and the lines a human reads.
type Answer = (Option<Value>, Option<Vec<Line>>);

/// What follows a command: its own output written (`export`), what it wrote published unless it
/// was `--offline`, and the count of events not yet published added; the result and lines then,
/// or `None` when a reader went away.
fn finish(
    outcome: Outcome,
    ws: Option<&beadroll_store::Workspace>,
    offline: bool,
    remote: &Option<OsString>,
) -> Result<Option<Answer>, Failure> {
    let Outcome {
        mut result,
        mut lines,
        raw,
        settled,
    } = outcome;
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
    let Some(ws) = ws.filter(|_| !settled) else {
        return Ok(Some((result, lines)));
    };
    if ws.wrote()
        && !offline
        && let Some(warning) = publish_soon(ws, offline, remote.as_deref())
    {
        // a local write is published at once, as a close is
        if let Some(lines) = &mut lines {
            lines.push(Line::toned(Tone::Warning, warning));
        }
    }
    commands::note_unpublished(ws, &mut result, &mut lines)?;
    Ok(Some((result, lines)))
}

/// Runs the command; with what it says about publishing: whether it was `--offline`, and the
/// remote it named.
fn dispatch(
    ctx: &Context<'_>,
    ns: &Namespace,
) -> Result<(Outcome, bool, Option<OsString>), Failure> {
    let offline = ns.flag("offline");
    let remote = ns.text("remote").map(OsStr::to_os_string);
    let outcome = match ns.spec.name {
        "init" => commands::init(
            ctx,
            commands::InitArgs {
                prefix: owned(ns, "prefix"),
                import_path: owned(ns, "import_path"),
                tracked: ns.flag("tracked"),
                remote: remote.clone(),
                offline,
            },
        )?,
        "import" => commands::import(ctx, ns.text("path").unwrap_or_default())?,
        "export" => commands::export(ctx, ns.text("output"))?,
        "create" => commands::create(ctx, &create_args(ns))?,
        "update" => commands::update(ctx, &update_args(ns))?,
        name @ ("close" | "reopen" | "delete") => commands::transition(
            ctx,
            name,
            &commands::TransitionArgs {
                ids: ns.texts("ids").to_vec(),
                reason: owned(ns, "reason"),
                force: ns.flag("force"),
                offline,
                remote: remote.clone(),
            },
        )?,
        "show" => commands::show(ctx, ns.texts("ids"))?,
        "list" => commands::list(
            ctx,
            &commands::ListArgs {
                status: ns.texts("status").to_vec(),
                all: ns.flag("all"),
                deferred: ns.flag("deferred"),
                title_contains: owned(ns, "title_contains"),
                id: ns.texts("id").to_vec(),
                parent: owned(ns, "parent"),
                filters: filters(ns),
            },
        )?,
        "ready" => commands::ready(
            ctx,
            &commands::ReadyArgs {
                root: owned(ns, "root"),
                epic: owned(ns, "epic"),
                parent: owned(ns, "parent"),
                filters: filters(ns),
            },
        )?,
        "blocked" => commands::blocked(ctx)?,
        "search" => commands::search(ctx, ns.text("query").unwrap_or_default(), ns.flag("all"))?,
        "dep" => commands::dep(
            ctx,
            ns.texts("words"),
            ns.text("type"),
            ns.flag("blocking_only"),
        )?,
        "comments" => commands::comments(
            ctx,
            &commands::CommentArgs {
                words: ns.texts("words").to_vec(),
                message: owned(ns, "message"),
                file: owned(ns, "file"),
                author: owned(ns, "author"),
            },
        )?,
        "lint" => commands::lint(ctx)?,
        "sync" => commands::sync(ctx, remote.as_ref())?,
        "log" => commands::log(ctx, ns.text("key"), ns.int("limit"))?,
        "info" => commands::info(ctx)?,
        "machines" => commands::machines(ctx, ns.text("name"))?,
        "claims" => commands::claims(ctx, real_time())?,
        name => {
            let args = commands::OwnershipArgs {
                ids: ns.texts("ids").to_vec(),
                reason: owned(ns, "reason"),
                offline,
                remote: remote.clone(),
                asked: ns.asked(),
            };
            match name {
                "claim" => commands::claim(ctx, &args)?,
                "takeover" => commands::takeover(ctx, &args)?,
                _ => commands::release(ctx, &args)?,
            }
        }
    };
    Ok((outcome, offline, remote))
}

/// Seconds since the epoch on the real clock. The `clock` failpoint shifts only the nanosecond
/// clock used for event ids, not this reading.
fn real_time() -> f64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(since) => since.as_nanos() as f64 / 1e9,
        Err(before) => -(before.duration().as_nanos() as f64) / 1e9,
    }
}

/// `create`'s arguments.
fn create_args(ns: &Namespace) -> commands::CreateArgs {
    commands::CreateArgs {
        title_pos: owned(ns, "title_pos"),
        title: owned(ns, "title"),
        fields: content(ns),
        labels: ns.texts("labels").to_vec(),
        deps: ns.texts("deps").to_vec(),
        silent: ns.flag("silent"),
    }
}

/// `update`'s arguments, with the command line as its receipt names it.
fn update_args(ns: &Namespace) -> commands::UpdateArgs {
    commands::UpdateArgs {
        ids: ns.texts("ids").to_vec(),
        title: owned(ns, "title"),
        fields: content(ns),
        claim: ns.flag("claim"),
        force: ns.flag("force"),
        add_label: ns.texts("add_label").to_vec(),
        remove_label: ns.texts("remove_label").to_vec(),
        set_labels: owned(ns, "set_labels"),
        offline: ns.flag("offline"),
        remote: owned(ns, "remote"),
        asked: ns.asked(),
    }
}

/// A text argument, owned.
fn owned(ns: &Namespace, dest: &str) -> Option<OsString> {
    ns.text(dest).map(OsStr::to_os_string)
}

/// The fields `create` and `update` both set.
fn content(ns: &Namespace) -> commands::ContentArgs {
    commands::ContentArgs {
        description: owned(ns, "description"),
        description_file: owned(ns, "description_file"),
        acceptance_criteria: owned(ns, "acceptance_criteria"),
        notes: owned(ns, "notes"),
        design: owned(ns, "design"),
        priority: owned(ns, "priority"),
        issue_type: owned(ns, "type"),
        status: owned(ns, "status"),
        assignee: owned(ns, "assignee"),
        estimate: ns.int("estimate").cloned(),
        external_ref: owned(ns, "external_ref"),
        parent: owned(ns, "parent"),
    }
}

/// The filters `list` and `ready` share.
fn filters(ns: &Namespace) -> commands::FilterArgs {
    commands::FilterArgs {
        issue_type: ns.texts("type").to_vec(),
        label: ns.texts("label").to_vec(),
        priority: owned(ns, "priority"),
        assignee: owned(ns, "assignee"),
        limit: ns.int("limit").cloned().unwrap_or_else(|| Int::from(0)),
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
/// `Error: message`, painted when `color`. The status is the command's (2 for `USAGE`, else 1),
/// whatever stderr does.
fn report(failure: &Failure, as_json: bool, color: bool) -> u8 {
    let text = match (as_json, &failure.raw) {
        // Escape bytes that are not UTF-8 as lone surrogates.
        (true, Some(raw)) => format!(
            "{{\"error\": {{\"code\": {}, \"message\": {}}}}}",
            json_ascii_bytes(failure.code.as_str().as_bytes()),
            json_ascii_bytes(raw)
        ),
        (true, None) => {
            let mut error = Object::new();
            error.insert("code".into(), Value::from(failure.code.as_str()));
            error.insert("message".into(), Value::from(failure.message.as_str()));
            let mut body = Object::new();
            body.insert("error".into(), Value::from(error));
            json::write(&Value::from(body), Format::ASCII).unwrap_or_default()
        }
        (false, _) => {
            let mut lines = vec![
                Line::toned(Tone::Error, "Error:")
                    .with(Tone::Plain, " ")
                    .with(Tone::Plain, failure.printed()),
            ];
            if let Some(after) = &failure.after {
                lines.push(Line::new());
                lines.extend(
                    after
                        .split('\n')
                        .map(|line| match line.strip_prefix("Usage:") {
                            Some(usage) => {
                                Line::toned(Tone::Heading, "Usage:").with(Tone::Plain, usage)
                            }
                            None => Line::from(line),
                        }),
                );
            }
            paint::render_all(&lines, color)
        }
    };
    let _ = emit(Some(&text), Stream::Stderr);
    if failure.code == Code::Usage { 2 } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn another_users_home_comes_from_the_password_database() {
        // root has an entry everywhere, even where the user running the tests has none (a
        // container's arbitrary uid).
        let root = nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(0))
            .unwrap()
            .unwrap();
        assert_eq!(
            home_of(std::ffi::OsStr::new(&root.name)),
            Some(root.dir.into_os_string())
        );
        assert_eq!(
            home_of(std::ffi::OsStr::new("no-such-user-bead-test")),
            None
        );
    }
}
