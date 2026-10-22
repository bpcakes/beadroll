//! Command-line parsing with clap, built from the argument table (`cli/spec.rs`; roll-dih8.23,
//! `docs/REVIEWING.md` §2). A command reads what argparse gave it before, and a receipt
//! signs the same namespace: every destination with its value or default. An unambiguous prefix
//! of a long option names that option, the last value of an option wins, a scope (`--parent`,
//! `--epic`) is given once, and an option's value may start with `-`. Errors are clap's, with its
//! suggestions, as `USAGE`. Arguments keep their OS bytes until a command reads them.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt as _;

use beadroll_core::json::Int;
use beadroll_core::text::{json_ascii_bytes, parse_integer};
use clap::builder::TypedValueParser;
use clap::builder::styling::{AnsiColor, Styles};
use clap::error::ErrorKind;
use clap::parser::ValueSource;
use clap::{Arg, ArgAction, ArgGroup, ArgMatches, Command};

use super::layout::lay_out_lines;
use super::paint::render_all;
use super::spec::{Action, ArgSpec, COMMANDS, CommandSpec, DESCRIPTION, Kind, Nargs, find};

/// The options every command takes, which bead reads before clap does (`read_request`): clap
/// shows them on every page, and refuses them when they reach it, abbreviated.
const GLOBAL: [(&str, Option<&str>, &str); 3] = [
    (
        "json",
        None,
        "machine output: one JSON document; an error as {\"error\": {\"code\", \"message\"}} on \
         stderr",
    ),
    (
        "actor",
        Some("NAME"),
        "the name to record and hold claims as",
    ),
    (
        "color",
        Some("WHEN"),
        "auto (the default: on a terminal, unless NO_COLOR), always or never",
    ),
];

/// A parsed command-line value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    /// Not given, with no default.
    None,
    Bool(bool),
    Text(OsString),
    Int(Int),
    /// `append`, or a positional that takes several values.
    List(Vec<OsString>),
}

/// A help page: `bead`'s own overview, or a command's.
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
    /// A usage error.
    Usage(Usage),
}

/// A usage error as clap words it: what is wrong, with its tips, as the bytes of the arguments it
/// quotes; and the usage and pointer to help a human reads after it.
#[derive(Debug)]
pub(crate) struct Usage {
    pub(crate) message: Vec<u8>,
    pub(crate) usage: String,
}

/// A parsed command and its arguments, indexed by destination.
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

    /// A whole number, or `None`.
    pub(crate) fn int(&self, dest: &str) -> Option<&Int> {
        match self.get(dest) {
            Value::Int(int) => Some(int),
            _ => None,
        }
    }

    /// The namespace used in a receipt signature: `command` and every destination with its value
    /// or default, sorted by key and written as spaced, ASCII-escaped JSON. Bytes that are not
    /// UTF-8 become U+FFFD.
    pub(crate) fn asked(&self) -> String {
        let mut entries: Vec<(&str, String)> =
            vec![("command", json_ascii_bytes(self.spec.name.as_bytes()))];
        entries.extend(
            self.values
                .iter()
                .map(|(dest, value)| (*dest, json_of(value))),
        );
        entries.sort_by(|a, b| a.0.cmp(b.0));
        let members: Vec<String> = entries
            .iter()
            .map(|(dest, value)| format!("{}: {value}", json_ascii_bytes(dest.as_bytes())))
            .collect();
        format!("{{{}}}", members.join(", "))
    }
}

/// A namespace value as ASCII-escaped JSON. Bytes that are not UTF-8 become U+FFFD.
fn json_of(value: &Value) -> String {
    match value {
        Value::None => "null".to_owned(),
        Value::Bool(true) => "true".to_owned(),
        Value::Bool(false) => "false".to_owned(),
        Value::Int(int) => int.to_string(),
        Value::Text(text) => json_ascii_bytes(text.as_bytes()),
        Value::List(items) => {
            let items: Vec<String> = items
                .iter()
                .map(|i| json_ascii_bytes(i.as_bytes()))
                .collect();
            format!("[{}]", items.join(", "))
        }
    }
}

/// The styles of help pages and usage errors, cargo's: green headings, cyan literals.
pub(crate) fn styles() -> Styles {
    Styles::styled()
        .header(AnsiColor::Green.on_default().bold())
        .usage(AnsiColor::Green.on_default().bold())
        .literal(AnsiColor::Cyan.on_default().bold())
        .placeholder(AnsiColor::Cyan.on_default())
        .error(AnsiColor::Red.on_default().bold())
        .valid(AnsiColor::Cyan.on_default().bold())
        .invalid(AnsiColor::Yellow.on_default().bold())
}

/// bead's command line as clap reads it, its help laid out `width` wide (none: unwrapped): a
/// subcommand for every command in the table.
pub(crate) fn command(width: Option<usize>) -> Command {
    let subcommands = COMMANDS.iter().map(|spec| subcommand(spec, width));
    let globals = GLOBAL.iter().map(|(name, value, help)| {
        let arg = Arg::new(*name)
            .long(*name)
            .help(*help)
            .global(true)
            .help_heading("Global options");
        match value {
            Some(value) => arg.value_name(*value).action(ArgAction::Set),
            None => arg.action(ArgAction::SetTrue),
        }
    });
    settings(Command::new("bead"), width)
        .about(DESCRIPTION)
        .args(globals)
        .disable_help_subcommand(true)
        .disable_version_flag(true)
        .subcommand_required(true)
        .subcommand_value_name("COMMAND")
        .subcommands(subcommands)
}

/// What every command keeps of argparse: prefixes of long options, the last value winning; and
/// how its help looks.
fn settings(command: Command, width: Option<usize>) -> Command {
    command
        .infer_long_args(true)
        .args_override_self(true)
        .styles(styles())
        .term_width(width.unwrap_or(0))
}

/// One command, its arguments as the table declares them.
fn subcommand(spec: &'static CommandSpec, width: Option<usize>) -> Command {
    let mut command = settings(Command::new(spec.name), width)
        .about(spec.help)
        .args(spec.args.iter().map(argument));
    let scope: Vec<&str> = spec
        .args
        .iter()
        .filter(|arg| arg.exclusive)
        .map(|arg| arg.dest)
        .collect();
    if !scope.is_empty() {
        command = command.group(ArgGroup::new("scope").args(scope).multiple(false));
    }
    if let Some(epilog) = spec.epilog {
        // clap wraps nothing it is given at the width it was laid out for, and shows its colour
        // only on a page it paints
        command = command.after_help(render_all(&lay_out_lines(epilog, width), true));
    }
    command
}

/// One argument as clap takes it: its option strings, value name, help, how many values and of
/// what type.
fn argument(spec: &ArgSpec) -> Arg {
    let positional = spec.is_positional();
    let mut arg = Arg::new(spec.dest);
    let longs: Vec<&str> = spec
        .flags
        .iter()
        .filter_map(|flag| flag.strip_prefix("--"))
        .collect();
    let shorts: Vec<char> = spec
        .flags
        .iter()
        .filter(|flag| !flag.starts_with("--"))
        .filter_map(|flag| flag.strip_prefix('-')?.chars().next())
        .collect();
    if let Some((first, others)) = longs.split_first() {
        arg = arg.long(*first).visible_aliases(others.to_vec());
    }
    if let Some((first, others)) = shorts.split_first() {
        arg = arg.short(*first).visible_short_aliases(others.to_vec());
    }
    if let Some(help) = spec.help {
        arg = arg.help(help);
    }
    let name = spec
        .metavar
        .map_or_else(|| spec.dest.to_ascii_uppercase(), str::to_owned);
    if spec.nargs != Nargs::Zero {
        arg = arg.value_name(name);
    }
    arg = match (spec.action, spec.nargs) {
        (Action::StoreTrue, _) => arg.action(ArgAction::SetTrue),
        (_, Nargs::ZeroOrMore) => arg.action(ArgAction::Append).num_args(0..),
        (_, Nargs::OneOrMore) => arg.action(ArgAction::Append).num_args(1..).required(true),
        // a scope given twice is refused, after the parse
        (Action::Append | Action::Once, _) => arg.action(ArgAction::Append),
        (_, Nargs::One) if positional => arg.action(ArgAction::Set).required(true),
        _ => arg.action(ArgAction::Set),
    };
    if !positional && spec.nargs != Nargs::Zero {
        // an option's value is the next argument, whatever it starts with; a positional's starts
        // with `-` only after `--`, so that a mistyped option is never taken for a value
        arg = arg.allow_hyphen_values(true);
    }
    match (spec.nargs, spec.kind) {
        (Nargs::Zero, _) => arg,
        (_, Kind::Int) => arg.value_parser(Integer),
        (_, Kind::Text) => arg.value_parser(clap::value_parser!(OsString)),
    }
}

/// A number as Python's `int()` reads it (`" +5 "`, `5_000`); anything else, bytes that are not
/// UTF-8 included, is refused naming the option and the value.
#[derive(Clone, Copy, Debug)]
struct Integer;

impl TypedValueParser for Integer {
    type Value = Int;

    fn parse_ref(
        &self,
        command: &Command,
        arg: Option<&Arg>,
        value: &OsStr,
    ) -> Result<Int, clap::Error> {
        value.to_str().and_then(parse_integer).ok_or_else(|| {
            let name = arg.map_or_else(String::new, ToString::to_string);
            let message = format!(
                "invalid value '{}' for '{name}': not a whole number",
                value.to_string_lossy()
            );
            command.clone().error(ErrorKind::ValueValidation, message)
        })
    }
}

/// An argument's value before anything is given: its default, `false` for a flag.
fn default(spec: &ArgSpec) -> Value {
    match (spec.action, spec.kind, spec.default) {
        (Action::StoreTrue, _, _) => Value::Bool(false),
        (_, Kind::Int, Some(number)) => parse_integer(number).map_or(Value::None, Value::Int),
        (_, _, Some(text)) => Value::Text(OsString::from(text)),
        _ => Value::None,
    }
}

/// Parses `args`, the command line once `--json`, `--actor`, `--color` and their values are
/// taken out.
pub(crate) fn parse(args: &[OsString]) -> Parsed {
    let mut root = command(None);
    let argv = std::iter::once(OsString::from("bead")).chain(args.iter().cloned());
    let matches = match root.try_get_matches_from_mut(argv) {
        Ok(matches) => matches,
        Err(error) if error.kind() == ErrorKind::DisplayHelp => return Parsed::Help(page(args)),
        Err(error) => return Parsed::Usage(usage(&error, args)),
    };
    // `--json`, `--actor` and `--color` reach clap only abbreviated
    if let Some((name, _, _)) = GLOBAL.iter().find(|(name, _, _)| {
        matches
            .subcommand()
            .map_or(&matches, |(_, sub)| sub)
            .value_source(name)
            == Some(ValueSource::CommandLine)
    }) {
        let message = format!("write --{name} in full");
        return Parsed::Usage(usage(
            &root.error(ErrorKind::UnknownArgument, message),
            args,
        ));
    }
    // clap requires a command, and knows only the table's
    let Some((spec, matches)) = matches
        .subcommand()
        .and_then(|(name, matches)| Some((find(name)?, matches)))
    else {
        return Parsed::Usage(usage(
            &root.error(ErrorKind::MissingSubcommand, "no command"),
            args,
        ));
    };
    let mut values = Vec::with_capacity(spec.args.len());
    for arg in spec.args {
        match value(arg, matches) {
            Ok(value) => values.push((arg.dest, value)),
            Err(message) => {
                let error = match root.find_subcommand_mut(spec.name) {
                    Some(sub) => sub.error(ErrorKind::ArgumentConflict, message),
                    None => root.error(ErrorKind::ArgumentConflict, message),
                };
                return Parsed::Usage(usage(&error, args));
            }
        }
    }
    Parsed::Command(Namespace { spec, values })
}

/// What `arg` holds after the parse, as argparse held it: its default when not given (an empty
/// list for a positional that takes any number), else its value or values. A scope given more than
/// once is refused.
fn value(arg: &ArgSpec, matches: &ArgMatches) -> Result<Value, String> {
    if arg.action == Action::StoreTrue {
        return Ok(Value::Bool(matches.get_flag(arg.dest)));
    }
    if matches.value_source(arg.dest) != Some(ValueSource::CommandLine) {
        return Ok(match arg.nargs {
            Nargs::ZeroOrMore => Value::List(Vec::new()),
            _ => default(arg),
        });
    }
    if arg.kind == Kind::Int {
        return Ok(matches
            .get_one::<Int>(arg.dest)
            .cloned()
            .map_or(Value::None, Value::Int));
    }
    let given: Vec<OsString> = matches
        .get_many::<OsString>(arg.dest)
        .map(|values| values.cloned().collect())
        .unwrap_or_default();
    match (arg.action, arg.nargs) {
        (Action::Once, _) if given.len() > 1 => Err(format!(
            "the argument '{} <{}>' cannot be used multiple times",
            arg.flags.last().copied().unwrap_or(arg.dest),
            arg.metavar.unwrap_or(arg.dest)
        )),
        (Action::Append, _) | (_, Nargs::ZeroOrMore | Nargs::OneOrMore) => Ok(Value::List(given)),
        _ => Ok(given.into_iter().last().map_or(Value::None, Value::Text)),
    }
}

/// The page help was asked for: `bead`'s when `-h` or `--help` came before the command (the only
/// options `bead` itself has), else the command's.
fn page(args: &[OsString]) -> Page {
    match args.first() {
        Some(first) if !first.as_bytes().starts_with(b"-") => first
            .to_str()
            .and_then(find)
            .map_or(Page::Top, Page::Command),
        _ => Page::Top,
    }
}

/// A clap error as bead reports it: the message and its tips (`message`), and what follows them
/// (`usage`), both plain. Where the message quotes one of `args` that is not UTF-8, which clap
/// shows with U+FFFD, it holds that argument's own bytes, so that a human reads `?` for each byte
/// and JSON `\udcXX`, as every other message shows them.
fn usage(error: &clap::Error, args: &[OsString]) -> Usage {
    let rendered = error.render().to_string();
    let rendered = rendered.trim_end();
    let body = rendered.strip_prefix("error: ").unwrap_or(rendered);
    let (said, after) = match body.find("\n\nUsage:") {
        Some(at) => (&body[..at], body[at + 2..].trim_end()),
        None => (body, ""),
    };
    let said = said
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    let mut message = said.into_bytes();
    for arg in args.iter().filter(|arg| arg.to_str().is_none()) {
        let shown = String::from_utf8_lossy(arg.as_bytes());
        message = replaced(&message, shown.as_bytes(), arg.as_bytes());
    }
    Usage {
        message,
        usage: after.to_owned(),
    }
}

/// `bytes` with every `from` in it replaced by `to`.
fn replaced(bytes: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    while let Some(at) = rest.windows(from.len()).position(|window| window == from) {
        out.extend_from_slice(&rest[..at]);
        out.extend_from_slice(to);
        rest = &rest[at + from.len()..];
    }
    out.extend_from_slice(rest);
    out
}

#[cfg(test)]
mod tests;
