//! Commands: typed arguments in, a result and its human-readable lines out.

use beadroll_core::op::OpName;

mod args;
mod claims;
mod comments;
mod create;
mod dep;
mod export;
mod failure;
#[cfg(test)]
mod held;
mod history;
mod import;
mod info;
mod init;
mod line;
mod list;
mod ownership;
mod queries;
mod recap;
mod select;
mod show;
mod sync;
mod transition;
mod update;

use std::ffi::OsStr;
use std::io::Read as _;
use std::os::unix::ffi::OsStrExt as _;

use beadroll_core::code::Code;
use beadroll_core::id::EventId;
use beadroll_core::json::{Int, Object, Value};
use beadroll_core::state::{Issue, State, Target};
use beadroll_core::text::{repr_str, strip};
use beadroll_store::{
    Actor, Label, Workspace, normalize_path, pending_listed, read_texts, view, view_outline,
};
use beadroll_sync::{Emitter, Spent, unpublished_count};

/// Which issues a search considers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchScope {
    /// Open issues only, the default.
    Open,
    /// Closed and deleted issues too (`--all`).
    All,
}

/// Whether an import is applied or previewed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImportMode {
    /// The records are imported.
    Apply,
    /// `--dry-run`: what would be imported is reported, and nothing is written.
    DryRun,
}

pub(crate) use args::{
    ContentArgs, Payload, arg_text, arg_texts, encodable, field_args, resolve_arg, rpartition,
    split_args,
};
pub(crate) use claims::claims;
pub(crate) use comments::{CommentArgs, comments};
pub(crate) use create::{CreateArgs, create};
pub(crate) use dep::dep;
pub(crate) use export::export;
pub(crate) use failure::Failure;
pub(crate) use history::{log, machines};
pub(crate) use import::import;
pub(crate) use info::info;
pub(crate) use init::{InitArgs, init};
pub(crate) use line::{Line, Tone};
pub(crate) use list::{FilterArgs, ListArgs, ReadyArgs, list, ready};
pub(crate) use ownership::{OwnershipArgs, claim, release, takeover};
pub(crate) use queries::{blocked, lint, search};
pub(crate) use recap::{Layout, RecapArgs, recap};
pub(crate) use select::{Filters, first, row, select};
pub(crate) use show::show;
pub(crate) use sync::sync;
pub(crate) use transition::{TransitionArgs, transition};
pub(crate) use update::{UpdateArgs, update};

/// What a command answers: the result `--json` prints, and the lines a human reads; or, for a
/// command that writes its own output (`export`), that output as it is.
#[derive(Debug)]
pub(crate) struct Outcome {
    pub(crate) result: Option<Value>,
    pub(crate) lines: Option<Vec<Line>>,
    pub(crate) raw: Option<String>,
    /// The command published and counted what it left unpublished itself
    /// ([`Outcome::published_itself`]), as `init` does, and a command whose own publish of a claim
    /// it ended did not go through.
    pub(crate) settled: bool,
    /// A failed count after a committed write: the result stands, and CLI reports this on stderr.
    pub(crate) count_warning: Option<Failure>,
    /// What the command made and is done with ([`Outcome::answered_from`], [`Outcome::wrote_in`]):
    /// one thing. That limits what the outcome carries, not what the command holds on the way to
    /// it: a command that publishes or reads a state of its own before it answers has to let go of
    /// what it made first, and nothing in the types makes it. The order of its code does
    /// (`cli::finish` calls `ended_with` before `publish_soon_at`, `transition` drops its transaction
    /// before it publishes), and the tests that pin it: `cli::ending` the order, `xtask/tests/heap`
    /// the heap.
    pub(crate) held: Option<Box<dyn Held>>,
}

/// What a command leaves for the command line to keep until the process exits, or to free first
/// when the run goes on to publish (`cli::ended_with`, roll-lqjj.32): the state it answered from,
/// or the transaction a write was made in. Freeing either takes about as long as reading it did,
/// work nobody waits for once the answer is written.
pub(crate) trait Held: std::fmt::Debug {}

impl Held for State {}

impl Held for Spent {}

impl Outcome {
    pub(crate) fn new<L: Into<Line>>(result: impl Into<Value>, lines: Vec<L>) -> Outcome {
        Outcome {
            result: Some(result.into()),
            lines: Some(lines.into_iter().map(Into::into).collect()),
            raw: None,
            settled: false,
            count_warning: None,
            held: None,
        }
    }

    /// Output written as it is, whatever `--json` says, with no result or lines after it.
    pub(crate) fn raw(text: String) -> Outcome {
        Outcome {
            result: None,
            lines: None,
            raw: Some(text),
            settled: false,
            count_warning: None,
            held: None,
        }
    }

    /// This outcome, of a command that has tried to publish what it wrote in `ws` itself: it counts
    /// what is left unpublished and is settled, so that the command line neither tries the remote
    /// that has just answered once more nor says twice that it did not ([`note_unpublished`]).
    pub(crate) fn published_itself(mut self, ws: &Workspace) -> Result<Outcome, Failure> {
        self.count_warning = note_unpublished(ws, &mut self.result, &mut self.lines)?;
        self.settled = true;
        Ok(self)
    }

    /// This outcome, of a command whose own publish of what it wrote in `ws` said `warning`, if
    /// anything: with a warning it has [`Outcome::published_itself`], said once.
    pub(crate) fn after_its_publish(
        self,
        ws: &Workspace,
        warning: Option<&str>,
    ) -> Result<Outcome, Failure> {
        match warning {
            Some(_) => self.published_itself(ws),
            None => Ok(self),
        }
    }

    /// This outcome, made from `state`, which the command line keeps until the process exits
    /// rather than freeing it, unless it has what the command wrote to publish first
    /// (`cli::ended_with`, roll-lqjj.32). Only the state a command answers from is given, the last
    /// it makes; one it made and does not answer from is freed where the command is done with it.
    pub(crate) fn answered_from(self, state: State) -> Outcome {
        Outcome {
            held: Some(Box::new(state)),
            ..self
        }
    }

    /// This outcome, of a write made in the transaction `spent` came from, which the command line
    /// keeps as it keeps a state answered from (roll-lqjj.42). A command that goes on to read a
    /// state or to publish before it answers drops `spent` first, so that it is not held beside
    /// the state it reads: this method cannot see that, the command's order does.
    pub(crate) fn wrote_in(self, spent: Spent) -> Outcome {
        Outcome {
            held: Some(Box::new(spent)),
            ..self
        }
    }
}

/// What a command does once its answer has reached standard output, and only then: `recap` marks
/// what it showed as seen. A run whose answer is never written, to a reader that went away or by a
/// write that failed, does nothing of it.
pub(crate) struct Delivered(Box<dyn FnOnce() -> Result<(), Failure>>);

impl Delivered {
    pub(crate) fn new(then: impl FnOnce() -> Result<(), Failure> + 'static) -> Delivered {
        Delivered(Box::new(then))
    }

    /// Does it, the answer written.
    pub(crate) fn done(self) -> Result<(), Failure> {
        (self.0)()
    }
}

/// What `ws` has not published, said in `result` (`unpublished`) and `lines`, when there is any:
/// after `--offline`, a remote that did not answer, or on a store that has never published.
/// A count failure after a write is a warning, not a failed command: the write already committed.
pub(crate) fn note_unpublished(
    ws: &Workspace,
    result: &mut Option<Value>,
    lines: &mut Option<Vec<Line>>,
) -> Result<Option<Failure>, Failure> {
    let counted = match unpublished_count(ws) {
        Ok(counted) => counted,
        Err(error) => {
            let failure = Failure::from(error);
            if !ws.committed() {
                return Err(failure);
            }
            let mut raw = b"write recorded locally; unpublished count unavailable: ".to_vec();
            raw.extend_from_slice(failure.raw.as_deref().unwrap_or(failure.message.as_bytes()));
            return Ok(Some(Failure::bytes(failure.code, raw)));
        }
    };
    let Some(behind) = counted.filter(|n| *n != Int::from(0)) else {
        return Ok(None);
    };
    if let Some(Value::Object(result)) = result {
        result.insert("unpublished".into(), Value::Int(behind.clone()));
    }
    if let Some(lines) = lines {
        let plural = if behind == Int::from(1) { "" } else { "s" };
        lines.push(Line::toned(
            Tone::Warning,
            format!("{behind} event{plural} not yet published: bead sync"),
        ));
    }
    Ok(None)
}

/// What every command is given: the workspace, whose settings it reads, and the actor.
pub(crate) struct Context<'a> {
    pub(crate) ws: &'a Workspace,
    pub(crate) actor: &'a Actor,
}

/// An argument as text: text that is not valid UTF-8 cannot be written to an event.
pub(crate) fn text(value: &OsStr) -> Result<String, Failure> {
    value
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| Failure::new(Code::Validation, "text is not valid UTF-8"))
}

/// The state as `view` gives it, with the texts of the issues `keys` name read in: what a command
/// that prints those issues whole needs, where its others' texts would only be read in vain
/// (roll-lqjj.6). Every text, when theirs cannot be read.
pub(crate) fn view_of(ws: &Workspace, keys: &[impl AsRef<OsStr>]) -> Result<State, Failure> {
    texts_of(ws, view_outline(ws)?, keys)
}

/// [`view_of`], from `st`, the state a write left the store's journal in
/// ([`beadroll_sync::writing_state`]), which the command has already: this machine's pending
/// attempts are listed on it as `view` lists them, and the store is not read again (roll-lqjj.29).
pub(crate) fn view_of_written(
    ws: &Workspace,
    st: State,
    keys: &[impl AsRef<OsStr>],
) -> Result<State, Failure> {
    texts_of(ws, pending_listed(ws, st)?, keys)
}

/// `st` with the texts of the issues `keys` name read in; the store's state with every text, when
/// theirs cannot be.
fn texts_of(ws: &Workspace, st: State, keys: &[impl AsRef<OsStr>]) -> Result<State, Failure> {
    let uids: Vec<_> = (keys.iter())
        .filter_map(|key| resolve_arg(&st, key.as_ref()).ok().map(Issue::uid))
        .collect();
    texts_in(ws, st, uids)
}

/// `st` with the texts of the issues `uids` names read in; the store's state with every text, when
/// theirs cannot be.
pub(crate) fn texts_in(
    ws: &Workspace,
    mut st: State,
    uids: impl IntoIterator<Item = EventId>,
) -> Result<State, Failure> {
    if read_texts(ws, &mut st, uids) {
        return Ok(st);
    }
    Ok(view(ws)?)
}

/// There must be a tracker here.
pub(crate) fn need_tracker(st: &State) -> Result<(), Failure> {
    if st.prefix().is_none() {
        return Err(Failure::new(
            Code::NotInitialized,
            "no tracker for this repository here yet: run `bead sync` to fetch it from its remote, or \
             `bead init` to start one if it has none",
        ));
    }
    Ok(())
}

/// The issue a key (or a bare suffix) names.
pub(crate) fn resolve<'s>(st: &'s State, key: &str) -> Result<&'s Issue, Failure> {
    st.resolve(key)
        .ok_or_else(|| Failure::new(Code::IssueNotFound, format!("Issue not found: {key}")))
}

/// A priority given as `0`-`4` or `P0`-`P4`.
pub(crate) fn parse_priority(value: &str) -> Result<i64, Failure> {
    let digits = value.strip_prefix(['P', 'p']).unwrap_or(value);
    match digits.as_bytes() {
        [d @ b'0'..=b'4'] => Ok(i64::from(d - b'0')),
        _ => Err(Failure::new(
            Code::Validation,
            format!("priority must be 0-4, got {}", repr_str(value)),
        )),
    }
}

/// Comma-separated lists, flattened, each item stripped, empty ones dropped.
pub(crate) fn split_list(values: &[String]) -> Vec<String> {
    values
        .iter()
        .flat_map(|value| value.split(','))
        .map(strip)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A text argument from a file, or standard input for `-`, decoded as UTF-8 regardless of locale.
/// The path is normalised (`dir//name/.` becomes `dir/name`); `what` names it in the error as
/// given.
pub(crate) fn read_utf8(path: &OsStr, what: &OsStr) -> Result<String, Failure> {
    if path == "-" && crate::sys::stdin_closed() {
        return Err(Failure::new(
            Code::Io,
            "standard input is closed, so - cannot be read",
        ));
    }
    let raw = if path == "-" {
        let mut raw = Vec::new();
        std::io::stdin()
            .read_to_end(&mut raw)
            .map_err(|e| Failure::from(beadroll_git::IoError::bare(&e)))?;
        raw
    } else {
        let path = normalize_path(path);
        std::fs::read(&path).map_err(|e| Failure::from(beadroll_git::IoError::at(&e, &path)))?
    };
    String::from_utf8(raw).map_err(|_| {
        Failure::bytes(
            Code::Validation,
            [what.as_bytes(), b" is not valid UTF-8"].concat(),
        )
    })
}

/// Refuses to change `issue` while someone other than `me` holds its claim: the error every write
/// that a claim guards gives, which `--force` lets the caller skip.
pub(crate) fn refuse_foreign_claim(issue: &Issue, me: &Label) -> Result<(), Failure> {
    match issue.claim() {
        Some(held) if !me.is(&held.owner) => Err(Failure::new(
            Code::ClaimedByOther,
            format!(
                "{} is claimed by {}: use `bead takeover` first, or --force",
                issue.key(),
                held.owner
            ),
        )),
        _ => Ok(()),
    }
}

/// Whether `me` ended the issue's last claim by closing or deleting it. That run may have died
/// before publishing, so a retry publishes as promptly as the first run. Asked of issues whose
/// texts, which hold their ownership history, are not read in (roll-lqjj.40).
pub(crate) fn ended_by(issue: &Issue, me: &Label) -> bool {
    (issue.last_ended())
        .is_some_and(|last| me.is(&last.by) && matches!(last.how, "close" | "delete"))
}

/// Removes edges from `issue` to `to` that this writer has seen; `kind` set to `None` selects
/// every kind. For an imported dependency tagged with the issue's create, its key is removed too:
/// a concurrent import sorting first keeps that key, and the dependency binds to that copy after
/// merging.
pub(crate) fn remove_edge(
    tx: &mut Emitter<'_>,
    st: &State,
    issue: &Issue,
    to: &Target,
    kind: Option<&str>,
) -> Result<(), Failure> {
    let uid = issue.uid();
    let imported = issue.dep_tags().iter().any(|(edge, tags)| {
        edge.to() == to && kind.is_none_or(|kind| kind == edge.kind()) && tags.contains_key(&uid)
    });
    let kind = kind.map_or(Value::Null, Value::from);
    let mut data = Object::new();
    data.insert("to".into(), Value::from(to.to_string()));
    data.insert("type".into(), kind.clone());
    tx.emit(OpName::DepRemove, Some(&uid.to_string()), data, None)?;
    if imported {
        let mut data = Object::new();
        data.insert("key".into(), Value::from(&*st.target_key(to)));
        data.insert("type".into(), kind);
        tx.emit(OpName::DepRemove, Some(&uid.to_string()), data, None)?;
    }
    Ok(())
}
