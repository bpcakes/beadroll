//! Help pages built from the command table: wrapped usage, positional arguments and options. Help
//! text wraps at the terminal width less two. `bead help COMMAND` and `bead COMMAND --help` print
//! the same plain-text page, without colour.

use std::ffi::OsStr;

use beadroll_core::text::parse_integer;

use super::parse::Page;
use super::spec::{Action, ArgSpec, COMMAND, COMMANDS, DESCRIPTION, HELP_FLAG, Nargs};
use super::wrap::{collapsed, wrap};

/// The help width: a positive integer from `COLUMNS`, else the width of the terminal on standard
/// output, else 80.
pub(crate) fn columns(variable: Option<&OsStr>) -> i64 {
    // any width beyond the longest line wraps nothing, so a larger number reads as this one
    const WIDE: i128 = 1 << 32;
    if let Some(number) = variable.and_then(OsStr::to_str).and_then(parse_integer)
        && number.saturating_i128() > 0
    {
        return i64::try_from(number.saturating_i128().min(WIDE)).unwrap_or(80);
    }
    match rustix::termios::tcgetwinsize(std::io::stdout()) {
        Ok(size) if size.ws_col > 0 => i64::from(size.ws_col),
        _ => 80,
    }
}

/// The page for `page`, formatted for a terminal `columns` wide (`format_help`).
pub(crate) fn render(page: Page, columns: i64) -> String {
    let (prog, actions, description) = match page {
        Page::Top => (
            "bead".to_owned(),
            vec![&HELP_FLAG, &COMMAND],
            Some(DESCRIPTION),
        ),
        Page::Command(spec) => (
            format!("bead {}", spec.name),
            std::iter::once(&HELP_FLAG).chain(spec.args).collect(),
            None,
        ),
    };
    let width = columns - 2;
    let positionals: Vec<&ArgSpec> = actions
        .iter()
        .copied()
        .filter(|a| a.is_positional())
        .collect();
    let optionals: Vec<&ArgSpec> = actions
        .iter()
        .copied()
        .filter(|a| !a.is_positional())
        .collect();
    // the widest invocation, indented as its section indents it, sets where help texts start
    let mut widest = 0;
    for action in positionals.iter().chain(&optionals) {
        widest = widest.max(length(&invocation(action)) + 2);
        if action.action == Action::Commands {
            for command in &COMMANDS {
                widest = widest.max(length(command.name) + 4);
            }
        }
    }
    let layout = Layout {
        width,
        help_position: (widest + 2).min(24.min((width - 20).max(4))),
    };
    let mut out = usage(&prog, &actions, width);
    if let Some(text) = description {
        out.push_str(&wrap(&collapsed(text), width.max(11), "").join("\n"));
        out.push_str("\n\n");
    }
    for (heading, members) in [
        ("positional arguments", &positionals),
        ("options", &optionals),
    ] {
        if members.is_empty() {
            continue;
        }
        out.push('\n');
        out.push_str(heading);
        out.push_str(":\n");
        for action in members {
            layout.action(&mut out, &invocation(action), action.help, 2);
            if action.action == Action::Commands {
                for command in &COMMANDS {
                    layout.action(&mut out, command.name, Some(command.help), 4);
                }
            }
        }
        out.push('\n');
    }
    // `format_help`: no run of more than one blank line, no blank line at either end
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    format!("{}\n", out.trim_matches('\n'))
}

/// A string's length in Unicode scalar values.
fn length(text: &str) -> i64 {
    i64::try_from(text.chars().count()).unwrap_or(i64::MAX)
}

/// Where help texts go on a page.
struct Layout {
    width: i64,
    help_position: i64,
}

impl Layout {
    /// One argument's entry (`_format_action`): its invocation, then its help beside it when the
    /// invocation leaves room, else on the next line at the help column.
    fn action(&self, out: &mut String, invocation: &str, help: Option<&str>, indent: i64) {
        let action_width = self.help_position - indent - 2;
        let help_width = (self.width - self.help_position).max(11);
        out.push_str(&spaces(indent));
        out.push_str(invocation);
        let Some(help) = help.filter(|h| !h.is_empty()) else {
            out.push('\n');
            return;
        };
        let indent_first = if length(invocation) <= action_width {
            out.push_str(&spaces(action_width - length(invocation) + 2));
            0
        } else {
            out.push('\n');
            self.help_position
        };
        let text = collapsed(help);
        if text.is_empty() {
            // a help of only whitespace: the invocation, padded, ends its line
            if indent_first == 0 {
                out.push('\n');
            }
            return;
        }
        for (index, line) in wrap(&text, help_width, "").iter().enumerate() {
            let at = if index == 0 {
                indent_first
            } else {
                self.help_position
            };
            out.push_str(&spaces(at));
            out.push_str(line);
            out.push('\n');
        }
    }
}

fn spaces(count: i64) -> String {
    " ".repeat(usize::try_from(count).unwrap_or(0))
}

/// The argument placeholder: an explicit metavar, else the command names as `{init,import,...}`
/// for command selection, else the default.
fn metavar(action: &ArgSpec, default: &str) -> String {
    match action.metavar {
        Some(name) => name.to_owned(),
        None if action.action == Action::Commands => {
            let names: Vec<&str> = COMMANDS.iter().map(|c| c.name).collect();
            format!("{{{}}}", names.join(","))
        }
        None => default.to_owned(),
    }
}

/// What an argument's values look like (`_format_args`).
fn args_shape(action: &ArgSpec, default: &str) -> String {
    let m = metavar(action, default);
    match action.nargs {
        Nargs::One => m,
        Nargs::Optional => format!("[{m}]"),
        Nargs::ZeroOrMore => format!("[{m} ...]"),
        Nargs::OneOrMore => format!("{m} [{m} ...]"),
        Nargs::Parser => format!("{m} ..."),
        Nargs::Zero => String::new(),
    }
}

/// An option's metavar when it names none: its destination upper-cased.
fn option_default(action: &ArgSpec) -> String {
    action.dest.to_ascii_uppercase()
}

/// How an argument is written in its section (`_format_action_invocation`).
fn invocation(action: &ArgSpec) -> String {
    if action.is_positional() {
        return metavar(action, action.dest);
    }
    let flags = action.flags.join(", ");
    if action.nargs == Nargs::Zero {
        flags
    } else {
        format!("{flags} {}", args_shape(action, &option_default(action)))
    }
}

/// The usage line (`_format_usage`), wrapped when it is longer than the width: the options'
/// parts, then the positionals', each line indented under the first.
fn usage(prog: &str, actions: &[&ArgSpec], width: i64) -> String {
    const PREFIX: &str = "usage: ";
    let (parts, pos_start) = usage_parts(actions);
    let mut text = std::iter::once(prog.to_owned())
        .chain(parts.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    if length(PREFIX) + length(&text) > width {
        let (opt_parts, pos_parts) = parts.split_at(pos_start);
        let lines_of = |parts: &[String], indent: &str, prefix: Option<&str>| {
            usage_lines(parts, indent, prefix, width)
        };
        let lines = if ((length(PREFIX) + length(prog)) as f64) <= 0.75 * width as f64 {
            let indent = spaces(length(PREFIX) + length(prog) + 1);
            let with_prog = |rest: &[String]| {
                std::iter::once(prog.to_owned())
                    .chain(rest.iter().cloned())
                    .collect::<Vec<_>>()
            };
            if !opt_parts.is_empty() {
                let mut lines = lines_of(&with_prog(opt_parts), &indent, Some(PREFIX));
                lines.extend(lines_of(pos_parts, &indent, None));
                lines
            } else if !pos_parts.is_empty() {
                lines_of(&with_prog(pos_parts), &indent, Some(PREFIX))
            } else {
                vec![prog.to_owned()]
            }
        } else {
            let indent = spaces(length(PREFIX));
            let mut lines = lines_of(&parts, &indent, None);
            if lines.len() > 1 {
                lines = lines_of(opt_parts, &indent, None);
                lines.extend(lines_of(pos_parts, &indent, None));
            }
            std::iter::once(prog.to_owned()).chain(lines).collect()
        };
        text = lines.join("\n");
    }
    format!("{PREFIX}{text}\n\n")
}

/// `get_lines` in `_format_usage`: the parts, as many on a line as fit the width.
fn usage_lines(parts: &[String], indent: &str, prefix: Option<&str>, width: i64) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line: Vec<&str> = Vec::new();
    let indent_length = length(indent);
    let mut line_len = prefix.map_or(indent_length, length) - 1;
    for part in parts {
        if line_len + 1 + length(part) > width && !line.is_empty() {
            lines.push(format!("{indent}{}", line.join(" ")));
            line.clear();
            line_len = indent_length - 1;
        }
        line.push(part);
        line_len += length(part) + 1;
    }
    if !line.is_empty() {
        lines.push(format!("{indent}{}", line.join(" ")));
    }
    if prefix.is_some()
        && let Some(first) = lines.first_mut()
    {
        *first = first.chars().skip(indent.chars().count()).collect();
    }
    lines
}

/// The usage line's parts and where the positionals' start (`_get_actions_usage_parts`): each
/// option once in brackets, then each positional; `ready`'s scope as one bracketed group in the
/// positionals' place, `[--epic ID | --parent ID | ID]`.
fn usage_parts(actions: &[&ArgSpec]) -> (Vec<String>, usize) {
    let (optionals, positionals) = usage_groups(actions);
    let mut parts: Vec<String> = Vec::new();
    let mut pos_start = None;
    for (number, entry) in optionals.iter().chain(&positionals).enumerate() {
        let start = parts.len();
        if number == optionals.len() {
            pos_start = Some(start);
        }
        let in_group = entry.len() > 1;
        parts.extend(
            entry
                .iter()
                .map(|index| usage_part(actions[*index], in_group)),
        );
        if in_group {
            parts[start].insert(0, '[');
            let last = parts.len() - 1;
            for part in &mut parts[start..last] {
                part.push_str(" |");
            }
            parts[last].push(']');
        }
    }
    let pos_start = pos_start.unwrap_or(parts.len());
    (parts, pos_start)
}

/// The options' entries and the positionals' (indexes into `actions`): a positional in the
/// exclusive group brings along the group's options not yet placed, so the group is shown where
/// the positional stands.
fn usage_groups(actions: &[&ArgSpec]) -> (Vec<Vec<usize>>, Vec<Vec<usize>>) {
    let mut taken = vec![false; actions.len()];
    let group: Vec<usize> = (0..actions.len())
        .filter(|index| actions[*index].exclusive)
        .collect();
    let join_group = |taken: &mut Vec<bool>, entry: &mut Vec<usize>| {
        for other in &group {
            if !actions[*other].is_positional() && !taken[*other] {
                taken[*other] = true;
                entry.push(*other);
            }
        }
    };
    let mut positionals = Vec::new();
    for index in (0..actions.len()).filter(|i| actions[*i].is_positional()) {
        taken[index] = true;
        let mut entry = Vec::new();
        if actions[index].exclusive {
            join_group(&mut taken, &mut entry);
        }
        entry.push(index);
        positionals.push(entry);
    }
    let mut optionals = Vec::new();
    for index in 0..actions.len() {
        if actions[index].is_positional() || taken[index] {
            continue;
        }
        taken[index] = true;
        let mut entry = vec![index];
        if actions[index].exclusive {
            join_group(&mut taken, &mut entry);
        }
        optionals.push(entry);
    }
    (optionals, positionals)
}

/// One argument in the usage line: a positional's values (without their brackets inside a group),
/// an option's first string with its metavar, bracketed unless it is in a group.
fn usage_part(action: &ArgSpec, in_group: bool) -> String {
    if action.is_positional() {
        let part = args_shape(action, action.dest);
        return match part.strip_prefix('[').and_then(|p| p.strip_suffix(']')) {
            Some(inner) if in_group => inner.to_owned(),
            _ => part,
        };
    }
    let first = action.flags.first().copied().unwrap_or_default();
    let part = if action.nargs == Nargs::Zero {
        first.to_owned()
    } else {
        format!("{first} {}", args_shape(action, &option_default(action)))
    };
    if in_group { part } else { format!("[{part}]") }
}

#[cfg(test)]
mod tests;
