//! The command line read as Python's argparse 3.14 reads it (`docs/RUST-PORT.md` §2.3h, §5 row
//! 13): `_parse_known_args` and the subparsers action, ported for the actions `build_parser` uses.
//! Every argument is first classed as an option or not, the way argparse classes it (an
//! unambiguous prefix of a long option is that option; `-1` and anything with a space in it are
//! values); positionals are then taken greedily before each option, so `update A --status open B`
//! leaves `B` unrecognised as it does in Python; the last value of an option wins, a scope given
//! twice is refused, and every error is `USAGE` with argparse's own text. The arguments stay the
//! bytes the OS gave until a command reads them.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};

use beadroll_core::json::Int;
use beadroll_core::text::repr_bytes;

use super::spec::{Action, ArgSpec, COMMAND, CommandSpec, HELP_FLAG, Nargs, find};
use classify::{Candidate, parse_optional};
use values::{default, option_count, positional_count, values_of};

/// A value in the namespace argparse builds.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    /// Python's `None`: not given, and no default.
    None,
    Bool(bool),
    Text(OsString),
    Int(Int),
    /// `append`, or a positional that takes several values.
    List(Vec<OsString>),
}

/// A help page: `bead`'s own (`bead -h COMMAND`), or a command's.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Page {
    Top,
    Command(&'static CommandSpec),
}

/// What a command line asks for.
#[derive(Debug)]
pub(crate) enum Parsed {
    /// Run a command with its arguments.
    Command(Namespace),
    /// Show a help page, and do nothing else.
    Help(Page),
    /// The command line is wrong (`USAGE`): argparse's message, with the arguments' bytes in it.
    Usage(Vec<u8>),
}

/// A command and its arguments as parsed: argparse's namespace, read by destination.
#[derive(Debug)]
pub(crate) struct Namespace {
    pub(crate) spec: &'static CommandSpec,
    values: Vec<(&'static str, Value)>,
}

impl Namespace {
    fn get(&self, dest: &str) -> &Value {
        self.values
            .iter()
            .find(|(name, _)| *name == dest)
            .map_or(&Value::None, |(_, value)| value)
    }

    /// A text value, or `None`.
    pub(crate) fn text(&self, dest: &str) -> Option<&OsStr> {
        match self.get(dest) {
            Value::Text(text) => Some(text),
            _ => None,
        }
    }

    /// Every value of an `append` option or a positional that takes several; none when absent.
    pub(crate) fn texts(&self, dest: &str) -> &[OsString] {
        match self.get(dest) {
            Value::List(items) => items,
            _ => &[],
        }
    }

    /// Whether a flag was given.
    pub(crate) fn flag(&self, dest: &str) -> bool {
        matches!(self.get(dest), Value::Bool(true))
    }

    /// A whole number (`type=int`), or `None`.
    pub(crate) fn int(&self, dest: &str) -> Option<&Int> {
        match self.get(dest) {
            Value::Int(int) => Some(int),
            _ => None,
        }
    }
}

/// Parses `args`, the command line once `--json`, `--actor` and their values are taken out.
pub(crate) fn parse(args: &[OsString]) -> Parsed {
    let top = Parser::top();
    let mut run = Run::new(&top, args);
    if let Err(stop) = run.parse_known() {
        return match stop {
            Stop::Usage(message) => Parsed::Usage(message),
            Stop::Help(page) => Parsed::Help(page),
        };
    }
    // argparse's own extras first, then the command's (`parse_args`)
    let mut extras = run.extras;
    extras.extend(run.unrecognized);
    if !extras.is_empty() {
        let mut message = b"unrecognized arguments: ".to_vec();
        message.extend(
            extras
                .iter()
                .map(|a| a.as_bytes())
                .collect::<Vec<_>>()
                .join(&b' '),
        );
        return Parsed::Usage(message);
    }
    match run.command {
        Some((spec, values)) => Parsed::Command(Namespace { spec, values }),
        None => Parsed::Usage(b"the following arguments are required: command".to_vec()),
    }
}

/// One parser: `bead`'s, whose one positional is the command, or a command's. Either has
/// `-h`/`--help` first.
struct Parser {
    page: Page,
    actions: Vec<&'static ArgSpec>,
}

impl Parser {
    fn top() -> Parser {
        Parser {
            page: Page::Top,
            actions: vec![&HELP_FLAG, &COMMAND],
        }
    }

    fn command(spec: &'static CommandSpec) -> Parser {
        Parser {
            page: Page::Command(spec),
            actions: std::iter::once(&HELP_FLAG).chain(spec.args).collect(),
        }
    }

    /// Every option string with its action, in the order argparse registered them.
    fn options(&self) -> impl Iterator<Item = (&'static str, usize)> + '_ {
        self.actions
            .iter()
            .enumerate()
            .flat_map(|(index, action)| action.flags.iter().map(move |flag| (*flag, index)))
    }

    /// The option `name` is, exactly.
    fn option(&self, name: &[u8]) -> Option<(&'static str, usize)> {
        self.options().find(|(flag, _)| flag.as_bytes() == name)
    }
}

/// Why parsing stopped early.
enum Stop {
    Usage(Vec<u8>),
    Help(Page),
}

impl Stop {
    fn usage(message: impl Into<String>) -> Stop {
        Stop::Usage(message.into().into_bytes())
    }
}

/// argparse's `ArgumentError`: the message, prefixed with the argument it is about.
fn argument_error(action: &ArgSpec, message: impl std::fmt::Display) -> Stop {
    Stop::usage(format!("argument {}: {message}", action.name()))
}

/// One parse of one parser's arguments (`_parse_known_args`).
struct Run<'a> {
    parser: &'a Parser,
    args: &'a [OsString],
    /// `O` for an option, `A` for anything else, `-` for the first `--`.
    pattern: Vec<u8>,
    found: BTreeMap<usize, Vec<Candidate>>,
    values: Vec<(&'static str, Value)>,
    seen: Vec<bool>,
    seen_non_default: Vec<bool>,
    positionals: Vec<usize>,
    extras: Vec<OsString>,
    /// The command the subparsers action ran, with its namespace.
    command: Option<(&'static CommandSpec, Vec<(&'static str, Value)>)>,
    /// What the command did not recognise.
    unrecognized: Vec<OsString>,
}

impl<'a> Run<'a> {
    fn new(parser: &'a Parser, args: &'a [OsString]) -> Run<'a> {
        let values = parser
            .actions
            .iter()
            .filter(|action| action.action != Action::Help)
            .map(|action| (action.dest, default(action)))
            .collect();
        Run {
            parser,
            args,
            pattern: Vec::with_capacity(args.len()),
            found: BTreeMap::new(),
            values,
            seen: vec![false; parser.actions.len()],
            seen_non_default: vec![false; parser.actions.len()],
            positionals: (0..parser.actions.len())
                .filter(|index| parser.actions[*index].is_positional())
                .collect(),
            extras: Vec::new(),
            command: None,
            unrecognized: Vec::new(),
        }
    }

    fn parse_known(&mut self) -> Result<(), Stop> {
        let mut rest = false;
        for (index, arg) in self.args.iter().enumerate() {
            if rest {
                self.pattern.push(b'A');
            } else if arg.as_bytes() == b"--" {
                // everything after `--` is a value
                self.pattern.push(b'-');
                rest = true;
            } else if let Some(candidates) = parse_optional(self.parser, arg.as_bytes()) {
                self.found.insert(index, candidates);
                self.pattern.push(b'O');
            } else {
                self.pattern.push(b'A');
            }
        }
        // positionals and options alternately, up to the last option
        let mut start = 0;
        if let Some(&last) = self.found.keys().next_back() {
            while start <= last {
                let next = (start..=last)
                    .find(|index| self.found.contains_key(index))
                    .unwrap_or(last + 1);
                if start != next {
                    let end = self.consume_positionals(start)?;
                    if end > start {
                        start = end;
                        continue;
                    }
                }
                if !self.found.contains_key(&start) {
                    self.extras.extend_from_slice(&self.args[start..next]);
                    start = next;
                }
                start = self.consume_optional(start)?;
            }
        }
        let stop = self.consume_positionals(start)?;
        self.extras.extend_from_slice(&self.args[stop..]);
        let missing: Vec<String> = self
            .parser
            .actions
            .iter()
            .enumerate()
            .filter(|(index, action)| !self.seen[*index] && action.is_required())
            .map(|(_, action)| action.name())
            .collect();
        if !missing.is_empty() {
            return Err(Stop::usage(format!(
                "the following arguments are required: {}",
                missing.join(", ")
            )));
        }
        Ok(())
    }

    /// Takes the option at `start` and its values; the index after them (`consume_optional`).
    fn consume_optional(&mut self, start: usize) -> Result<usize, Stop> {
        let candidates = self.found.get(&start).cloned().unwrap_or_default();
        if candidates.len() > 1 {
            let options: Vec<&str> = candidates.iter().map(|c| c.option).collect();
            let mut message = b"ambiguous option: ".to_vec();
            message.extend_from_slice(self.args[start].as_bytes());
            message.extend_from_slice(format!(" could match {}", options.join(", ")).as_bytes());
            return Err(Stop::Usage(message));
        }
        let Some(Candidate {
            mut action,
            mut option,
            mut eq,
            mut explicit,
        }) = candidates.into_iter().next()
        else {
            return Ok(start + 1);
        };
        let mut taken: Vec<(usize, Vec<OsString>, &'static str)> = Vec::new();
        let stop = loop {
            let Some(index) = action else {
                self.extras.push(self.args[start].clone());
                return Ok(start + 1);
            };
            let spec = self.parser.actions[index];
            let Some(value) = explicit.take() else {
                let first = start + 1;
                let count = option_count(spec, &self.pattern[first..])?;
                taken.push((index, self.args[first..first + count].to_vec(), option));
                break first + count;
            };
            let count = option_count(spec, b"A")?;
            if count == 0 && option.as_bytes().get(1) != Some(&b'-') && !value.is_empty() {
                // `-ab`: a flag, then another short option in the same argument
                if eq || value[0] == b'-' {
                    let message = format!("ignored explicit argument {}", repr_bytes(&value));
                    return Err(argument_error(spec, message));
                }
                taken.push((index, Vec::new(), option));
                match self.parser.option(&[b'-', value[0]]) {
                    Some((next_option, next)) => {
                        (action, option) = (Some(next), next_option);
                        let rest = &value[1..];
                        (eq, explicit) = match rest.first() {
                            None => (false, None),
                            Some(b'=') => (true, Some(rest[1..].to_vec())),
                            Some(_) => (false, Some(rest.to_vec())),
                        };
                    }
                    None => {
                        let mut unknown = vec![b'-'];
                        unknown.extend_from_slice(&value);
                        self.extras.push(OsString::from_vec(unknown));
                        break start + 1;
                    }
                }
            } else if count == 1 {
                taken.push((index, vec![OsString::from_vec(value)], option));
                break start + 1;
            } else {
                let message = format!("ignored explicit argument {}", repr_bytes(&value));
                return Err(argument_error(spec, message));
            }
        };
        for (index, values, option) in taken {
            self.take_action(index, values, Some(option))?;
        }
        Ok(stop)
    }

    /// Takes the next positional's values from `start` on, as many as its `nargs` matches; the
    /// index after them (`consume_positionals`). Each of bead's parsers has one positional at most.
    fn consume_positionals(&mut self, start: usize) -> Result<usize, Stop> {
        let Some(&index) = self.positionals.first() else {
            return Ok(start);
        };
        let pattern = &self.pattern[start..];
        let Some(count) = positional_count(self.parser.actions[index].nargs, pattern) else {
            return Ok(start);
        };
        // one that matched nothing just before an option waits for what follows it
        if count == 0 && pattern.first() == Some(&b'O') {
            return Ok(start);
        }
        let mut values = self.args[start..start + count].to_vec();
        if self.parser.actions[index].nargs == Nargs::Parser {
            if pattern.first() == Some(&b'-') {
                values.remove(0);
            }
        } else if pattern[..count].contains(&b'-')
            && let Some(at) = values.iter().position(|v| v.as_bytes() == b"--")
        {
            values.remove(at);
        }
        self.positionals.remove(0);
        self.take_action(index, values, None)?;
        Ok(start + count)
    }

    /// Converts the values, checks the one-scope rule, and acts (`take_action`).
    fn take_action(
        &mut self,
        index: usize,
        strings: Vec<OsString>,
        option: Option<&'static str>,
    ) -> Result<(), Stop> {
        let spec = self.parser.actions[index];
        self.seen[index] = true;
        let value = values_of(spec, &strings)?;
        if !spec.flags.is_empty() || !strings.is_empty() {
            self.seen_non_default[index] = true;
            if spec.exclusive {
                let conflict = self.parser.actions.iter().enumerate().find(|(other, a)| {
                    *other != index && a.exclusive && self.seen_non_default[*other]
                });
                if let Some((_, other)) = conflict {
                    let message = format!("not allowed with argument {}", other.name());
                    return Err(argument_error(spec, message));
                }
            }
        }
        match spec.action {
            Action::Help => Err(Stop::Help(self.parser.page)),
            Action::Store => {
                self.set(spec.dest, value);
                Ok(())
            }
            Action::StoreTrue => {
                self.set(spec.dest, Value::Bool(true));
                Ok(())
            }
            Action::Append => {
                let mut items = match self.value(spec.dest) {
                    Value::List(items) => items.clone(),
                    _ => Vec::new(),
                };
                if let Value::Text(text) = value {
                    items.push(text);
                }
                self.set(spec.dest, Value::List(items));
                Ok(())
            }
            Action::Once => {
                if *self.value(spec.dest) != Value::None {
                    let shown = option.unwrap_or(spec.dest);
                    return Err(Stop::usage(format!(
                        "argument {shown}: given more than once"
                    )));
                }
                self.set(spec.dest, value);
                Ok(())
            }
            Action::Commands => {
                let Some(spec) = strings.first().and_then(|n| n.to_str()).and_then(find) else {
                    return Ok(());
                };
                let parser = Parser::command(spec);
                let mut run = Run::new(&parser, &strings[1..]);
                run.parse_known()?;
                self.command = Some((spec, run.values));
                self.unrecognized = run.extras;
                Ok(())
            }
        }
    }

    fn value(&self, dest: &str) -> &Value {
        self.values
            .iter()
            .find(|(name, _)| *name == dest)
            .map_or(&Value::None, |(_, value)| value)
    }

    fn set(&mut self, dest: &'static str, value: Value) {
        match self.values.iter_mut().find(|(name, _)| *name == dest) {
            Some(slot) => slot.1 = value,
            None => self.values.push((dest, value)),
        }
    }
}

mod classify;
mod values;

#[cfg(test)]
mod tests;
