//! Commands that work anywhere, with or without a repository: help, prime and onboard.

use std::ffi::OsStr;

use beadroll_core::code::Code;
use beadroll_core::event::Field;
use beadroll_core::json::{Object, Value};
use beadroll_core::text::{repr, repr_bytes, str_of};
use beadroll_store::{
    Actor, Label, Settings, Workspace, replica_id, tracked_config_warning, view_outline,
};

use super::help;
use super::layout::{self, lay_out, paragraph};
use super::parse::{Namespace, Page};
use super::spec::find;
use super::texts::{ONBOARD, OVERVIEW, PRIME, PRIME_TITLE};
use crate::commands::{Failure, Outcome, need_tracker};

/// Runs `help`, `prime` or `onboard`; `ws` is the workspace when there is one.
pub(crate) fn run(
    ns: &Namespace,
    ws: Option<&Workspace>,
    actor: &Actor,
    settings: &Settings,
) -> Result<Outcome, Failure> {
    match ns.spec.name {
        "help" => help_page(ns.text("command_name"), settings),
        "prime" => prime(ws, actor, settings),
        _ => Ok(text_outcome("onboard", ONBOARD)),
    }
}

/// The text as `{name: text}`, and as its lines.
fn text_outcome(name: &str, text: &str) -> Outcome {
    laid_out(name, text, text)
}

/// `{name: text}`, `text` laid out for no width so that the reader wraps it, and `shown`, laid
/// out for this terminal, as its lines.
fn laid_out(name: &str, text: &str, shown: &str) -> Outcome {
    let mut result = Object::new();
    result.insert(name.into(), Value::from(text));
    let lines = shown
        .trim_end_matches('\n')
        .split('\n')
        .map(str::to_owned)
        .collect();
    Outcome::new(result, lines)
}

/// The overview, or one command's help page.
fn help_page(name: Option<&OsStr>, settings: &Settings) -> Result<Outcome, Failure> {
    let Some(name) = name.filter(|n| !n.is_empty()) else {
        let width = layout::width(settings.var("COLUMNS"));
        return Ok(laid_out(
            "help",
            &lay_out(OVERVIEW, None),
            &lay_out(OVERVIEW, width),
        ));
    };
    let Some(spec) = name.to_str().and_then(find) else {
        return Err(Failure::new(
            Code::Usage,
            format!(
                "unknown command {}; try `bead help`",
                repr_bytes(std::os::unix::ffi::OsStrExt::as_bytes(name))
            ),
        ));
    };
    let columns = help::columns(settings.var("COLUMNS"));
    Ok(text_outcome(
        "help",
        &help::render(Page::Command(spec), columns),
    ))
}

/// The workflow, with where this agent stands: who it is, and in a repository with a tracker how
/// many issues are ready, which it holds and when this machine last synced.
fn prime(ws: Option<&Workspace>, actor: &Actor, settings: &Settings) -> Result<Outcome, Failure> {
    let replica = replica_id(settings, false)?;
    let me = actor.label(replica.as_deref().unwrap_or("?"));
    let mut context = vec![format!("You are {} (machine, agent, thread).", me.shown())];
    match ws {
        Some(ws) => match standing(ws, &me) {
            Ok(line) => context.push(line),
            // the system's errors are not an answer here; bead's are
            Err(failure) if failure.code == Code::Io => return Err(failure),
            Err(failure) => context.push(format!(
                "No tracker here yet ({}): run `bead sync` to fetch this repository's tracker, or \
                 `bead init` to start one if it has none.",
                failure.code.as_str()
            )),
        },
        None => context.push(
            "Not inside a git repository: the commands below work in any checkout of a tracked \
             repo."
                .to_owned(),
        ),
    }
    // a tracked configuration the other checkouts do not see splits the tracker: said first
    let warning = ws.and_then(|ws| tracked_config_warning(ws).ok().flatten());
    if let Some(warning) = warning {
        context.insert(0, format!("Warning: {warning}."));
    }
    let text = |width| {
        let head: Vec<String> = std::iter::once(PRIME_TITLE)
            .chain(context.iter().map(String::as_str))
            .map(|line| paragraph(line, width))
            .collect();
        format!("{}\n\n{}", head.join("\n"), lay_out(PRIME, width))
    };
    let width = layout::width(settings.var("COLUMNS"));
    let mut outcome = laid_out("prime", &text(None), &text(width));
    if let Some(Value::Object(result)) = &mut outcome.result {
        result.insert("actor".into(), Value::from(me.shown()));
    }
    Ok(outcome)
}

/// The tracker's line in `prime`: how many issues are ready, which `me` holds, how many of this
/// machine's attempts await the remote, and the last sync.
fn standing(ws: &Workspace, me: &Label) -> Result<String, Failure> {
    let st = view_outline(ws)?;
    need_tracker(&st)?;
    let held: Vec<&str> = st
        .issues()
        .filter(|i| i.claim().is_some_and(|c| me.is(&c.owner)))
        .map(|i| i.key())
        .collect();
    let ready = st
        .issues()
        .filter(|i| {
            i.status() == "open"
                && i.claim().is_none()
                && !st.is_blocked(i)
                && !st.disputed(i, Field::Status)
        })
        .count();
    let waiting: usize = st.issues().map(|i| i.unconfirmed().len()).sum();
    let prefix = st.prefix().map_or_else(|| "None".to_owned(), repr);
    let mut line = format!(
        "Tracker {prefix}: {ready} ready, {} held by you",
        held.len()
    );
    if !held.is_empty() {
        line.push_str(&format!(" ({})", held.join(", ")));
    }
    if waiting > 0 {
        line.push_str(&format!(
            ", {waiting} attempt(s) of this machine awaiting the remote"
        ));
    }
    let meta = ws.meta(false)?;
    let synced = meta
        .get("last_sync")
        .filter(|v| v.truthy())
        .map_or_else(|| "never".to_owned(), str_of);
    line.push_str(&format!(". Last sync: {synced}."));
    Ok(line)
}
