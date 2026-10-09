//! Who is calling (Python's `detect_actor` and `nearest_harness`): an agent type and a thread from
//! the harness environment, overrides from `BEAD_*` and `--actor`.

use beadroll_core::json::{Object, Value};

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

/// The caller's actor: `{"agent", "thread"?, "name"?}` in that order. `BEAD_AGENT` (with
/// `BEAD_THREAD`) wins; with both a Claude and a Codex session in the environment, the nearest
/// harness in the process tree decides (Codex when none is found); then Claude, then Codex, else
/// `human`. `BEAD_THREAD` overrides the thread, and `name` (`--actor`) or `BEAD_ACTOR` names it.
pub fn detect_actor(
    settings: &Settings,
    name: Option<&str>,
    nearest: &dyn Fn() -> Option<&'static str>,
) -> Value {
    let var = |key: &str| settings.var(key).map(|v| v.to_string_lossy().into_owned());
    let claude_thread = var("CLAUDE_CODE_SESSION_ID");
    let codex_thread = var("CODEX_THREAD_ID").or_else(|| var("CODEX_SESSION_ID"));
    let (agent, thread) = if let Some(agent) = var("BEAD_AGENT") {
        (agent, var("BEAD_THREAD"))
    } else if claude_thread.is_some() && codex_thread.is_some() {
        let agent = nearest().unwrap_or("codex");
        let thread = if agent == "codex" {
            codex_thread
        } else {
            claude_thread
        };
        (agent.to_owned(), thread)
    } else if claude_thread.is_some() || var("CLAUDECODE").is_some() {
        ("claude".to_owned(), claude_thread)
    } else if codex_thread.is_some() {
        ("codex".to_owned(), codex_thread)
    } else {
        ("human".to_owned(), None)
    };
    let thread = var("BEAD_THREAD").or(thread);
    let mut actor = Object::new();
    actor.insert("agent".into(), Value::from(agent));
    if let Some(thread) = thread {
        actor.insert("thread".into(), Value::from(thread));
    }
    let named = name
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .or_else(|| var("BEAD_ACTOR"));
    if let Some(named) = named {
        actor.insert("name".into(), Value::from(named));
    }
    Value::Object(actor)
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
