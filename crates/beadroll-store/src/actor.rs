//! Who is calling (Python's `detect_actor` and `nearest_harness`): an agent type and a thread from
//! the harness environment, overrides from `BEAD_*` and `--actor`.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};

use beadroll_core::json::{Object, Value};
use beadroll_core::state::actor_label;
use beadroll_core::text::printed_text;

use crate::settings::Settings;

/// What one `ps -o ppid=,args= -p PID` gave: its exit status and standard output (decoded leniently),
/// or `None` when `ps` could not be run. The CLI supplies the function that runs it, from `PATH`.
pub type Ps<'a> = dyn Fn(u32) -> Option<(i32, String)> + 'a;

/// Walks up the process tree from `parent` (the caller's parent process) to the closest coding
/// agent, at most 12 levels: `codex` or `claude`, or `None`. It settles nested harnesses, where an
/// agent started from another agent's session inherits both sets of variables.
pub fn nearest_harness(parent: u32, ps: &Ps<'_>) -> Option<&'static str> {
    let mut pid = parent;
    for _ in 0..12 {
        let (status, stdout) = ps(pid)?;
        let line = stdout.trim_matches(beadroll_core::text::is_space);
        if status != 0 || line.is_empty() {
            return None;
        }
        let (parent, args) = line.split_once(' ').unwrap_or((line, ""));
        let words: Vec<&str> = args
            .split(beadroll_core::text::is_space)
            .filter(|w| !w.is_empty())
            .collect();
        // A row that names no command ends the walk, where Python fails reading its first word
        // (`docs/REVIEWING.md` §2).
        let first = words.first().copied()?;
        let command = first.rsplit('/').next().unwrap_or_default();
        if command.starts_with("codex") || first.contains("/codex") {
            return Some("codex");
        }
        if command == "claude" || (command == "node" && args.contains("claude")) {
            return Some("claude");
        }
        let parent = parent.trim_matches(beadroll_core::text::is_space);
        if parent.is_empty() || !parent.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        match parent.parse::<u32>() {
            Ok(next) if next > 1 => pid = next,
            _ => return None,
        }
    }
    None
}

/// The caller, as events record it: the actor object, and whether all of its text came as valid
/// UTF-8. Python keeps bytes that are not (in `--actor` or the environment) as lone surrogates, so
/// none of its writes can be encoded; the actor still reads as itself, a `?` for each such byte, no
/// receipt is ever its, and it holds no claim, for no owner a journal names holds a lone surrogate.
#[derive(Clone, Debug, PartialEq)]
pub struct Actor {
    /// `{"agent", "thread"?, "name"?}`, as bead prints it.
    pub value: Value,
    /// Whether its text is valid UTF-8.
    pub writable: bool,
    /// Its members as they were given, byte for byte; `None` when `value` is all there is.
    given: Option<Given>,
}

/// The members of an actor as they were given.
#[derive(Clone, Debug, PartialEq)]
struct Given {
    agent: OsString,
    thread: Option<OsString>,
    name: Option<OsString>,
}

impl Actor {
    /// An actor whose text is valid UTF-8.
    pub fn writable(value: Value) -> Actor {
        Actor {
            value,
            writable: true,
            given: None,
        }
    }

    /// The owner this actor's claims are held by on `replica` (Python's `actor_label`): its name,
    /// else `agent:thread@replica`, or `agent@replica` without a thread; kept as the bytes it was
    /// given.
    pub fn label(&self, replica: &str) -> Label {
        let Some(given) = &self.given else {
            return Label(actor_label(Some(&self.value), replica).into_bytes());
        };
        let set = |member: &Option<OsString>| member.clone().filter(|m| !m.is_empty());
        if let Some(name) = set(&given.name) {
            return Label(name.into_vec());
        }
        let mut label = given.agent.clone().into_vec();
        if let Some(thread) = set(&given.thread) {
            label.push(b':');
            label.extend_from_slice(thread.as_bytes());
        }
        label.push(b'@');
        label.extend_from_slice(replica.as_bytes());
        Label(label)
    }
}

/// An actor's label (Python's `actor_label`), as the bytes it was given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label(Vec<u8>);

impl Label {
    /// Whether `owner`, as a journal names it, is this label. An owner is always UTF-8, so a label
    /// whose bytes are not is no one's: the string Python holds them as, with its lone surrogates,
    /// equals no text a journal holds.
    pub fn is(&self, owner: &str) -> bool {
        self.0 == owner.as_bytes()
    }

    /// As bead prints it: a `?` for each byte that is not UTF-8.
    pub fn shown(&self) -> String {
        printed_text(&self.0)
    }

    /// Its bytes, for a message that keeps them (`\udcXX` in JSON, as Python escapes them).
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// The caller's actor: `{"agent", "thread"?, "name"?}` in that order. `BEAD_AGENT` (with
/// `BEAD_THREAD`) wins; with both a Claude and a Codex session in the environment, the nearest
/// harness in the process tree decides (Codex when none is found); then Claude, then Codex, else
/// `human`. `BEAD_THREAD` overrides the thread, and `name` (`--actor`) or `BEAD_ACTOR` names it.
pub fn detect_actor(
    settings: &Settings,
    name: Option<&OsStr>,
    nearest: &dyn Fn() -> Option<&'static str>,
) -> Actor {
    let mut writable = true;
    let mut text = |value: &OsStr| -> String {
        writable &= value.to_str().is_some();
        printed_text(value.as_bytes())
    };
    let raw = |key: &str| settings.var(key).map(OsStr::to_os_string);
    let claude_thread = raw("CLAUDE_CODE_SESSION_ID");
    let codex_thread = raw("CODEX_THREAD_ID").or_else(|| raw("CODEX_SESSION_ID"));
    let (agent, thread): (Option<OsString>, Option<OsString>) =
        if let Some(agent) = raw("BEAD_AGENT") {
            (Some(agent), raw("BEAD_THREAD"))
        } else if claude_thread.is_some() && codex_thread.is_some() {
            let agent = nearest().unwrap_or("codex");
            let thread = if agent == "codex" {
                codex_thread
            } else {
                claude_thread
            };
            (Some(OsString::from(agent)), thread)
        } else if claude_thread.is_some() || raw("CLAUDECODE").is_some() {
            (Some(OsString::from("claude")), claude_thread)
        } else if codex_thread.is_some() {
            (Some(OsString::from("codex")), codex_thread)
        } else {
            (Some(OsString::from("human")), None)
        };
    let thread = raw("BEAD_THREAD").or(thread);
    let agent = agent.unwrap_or_default();
    let mut actor = Object::new();
    actor.insert("agent".into(), Value::from(text(&agent)));
    if let Some(thread) = &thread {
        actor.insert("thread".into(), Value::from(text(thread)));
    }
    let name = name
        .filter(|n| !n.is_empty())
        .map(OsStr::to_os_string)
        .or_else(|| raw("BEAD_ACTOR"));
    if let Some(name) = &name {
        actor.insert("name".into(), Value::from(text(name)));
    }
    Actor {
        value: Value::from(actor),
        writable,
        given: Some(Given {
            agent,
            thread,
            name,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `ps` that answers from a table of `pid -> output`, 1 for anything else.
    fn table(rows: &'static [(u32, &'static str)]) -> impl Fn(u32) -> Option<(i32, String)> {
        move |pid| {
            rows.iter()
                .find(|(at, _)| *at == pid)
                .map_or(Some((1, String::new())), |(_, out)| {
                    Some((0, (*out).to_owned()))
                })
        }
    }

    #[test]
    fn the_walk_finds_the_nearest_harness() {
        let ps = table(&[
            (10, "  20 /usr/bin/zsh -l\n"),
            (20, "30 node /opt/claude/cli.js"),
            (30, "1 codex"),
        ]);
        assert_eq!(nearest_harness(10, &ps), Some("claude"));
        let ps = table(&[(10, "20 /opt/bin/codex --worker")]);
        assert_eq!(nearest_harness(10, &ps), Some("codex"));
        let ps = table(&[(10, "1 /bin/sh")]);
        assert_eq!(nearest_harness(10, &ps), None);
        assert_eq!(nearest_harness(10, &|_| None), None);
    }

    #[test]
    fn a_row_without_a_command_ends_the_walk() {
        let ps = table(&[(10, "42"), (42, "1 claude")]);
        assert_eq!(nearest_harness(10, &ps), None);
        let ps = table(&[(10, "42 \t "), (42, "1 claude")]);
        assert_eq!(nearest_harness(10, &ps), None);
    }
}
