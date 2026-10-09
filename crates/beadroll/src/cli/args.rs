//! The command line's shape (Python's `build_parser`), in clap configured for argparse parity
//! (`docs/RUST-PORT.md` §2.3h): unambiguous prefixes of long options, the last value of a repeated
//! option, `--` ending options, plain text without colour, every parse error a `USAGE` error.

use std::ffi::OsString;

use beadroll_core::json::Int;
use clap::{
    ArgAction, Args, ColorChoice, CommandFactory as _, FromArgMatches as _, Parser, Subcommand,
};

/// `bead COMMAND ...`.
#[derive(Debug, Parser)]
#[command(
    name = "bead",
    color = ColorChoice::Never,
    disable_help_subcommand = true,
    disable_version_flag = true,
    subcommand_required = true
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

/// The commands.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// create the tracker for this repository
    Init(Init),
    /// create an issue
    Create(Create),
    /// full detail for one or more issues
    Show(Show),
    /// list issues (open ones by default)
    List(List),
    /// open issues with nothing blocking them
    Ready(Ready),
    /// exchange the journal with other machines through the coordination ref
    Sync(Sync),
}

/// `int(text)`, as argparse's `type=int` reads a value: exact, within Python's 4,300 digits.
fn int(value: &str) -> Result<Int, String> {
    beadroll_core::text::py_int(value).ok_or_else(|| format!("invalid int value: '{value}'"))
}

#[derive(Debug, Args)]
pub(crate) struct Init {
    #[arg(long)]
    pub(crate) prefix: Option<OsString>,
    #[arg(long = "import", value_name = "BEADS_JSONL")]
    pub(crate) import_path: Option<OsString>,
    /// write .beadroll.json with an explicit workspace id
    #[arg(long)]
    pub(crate) tracked: bool,
    /// coordination remote (name or URL) if it is not origin
    #[arg(long)]
    pub(crate) remote: Option<OsString>,
    /// record locally only; the next sync or online command publishes it
    #[arg(long)]
    pub(crate) offline: bool,
}

#[derive(Debug, Args)]
pub(crate) struct Create {
    #[arg(value_name = "TITLE", allow_negative_numbers = true)]
    pub(crate) title_pos: Option<OsString>,
    #[arg(long)]
    pub(crate) title: Option<OsString>,
    #[arg(short = 'd', long)]
    pub(crate) description: Option<OsString>,
    #[arg(long = "description-file")]
    pub(crate) description_file: Option<OsString>,
    #[arg(long = "acceptance-criteria", visible_alias = "acceptance")]
    pub(crate) acceptance_criteria: Option<OsString>,
    #[arg(long)]
    pub(crate) notes: Option<OsString>,
    #[arg(long)]
    pub(crate) design: Option<OsString>,
    #[arg(short = 'p', long, allow_negative_numbers = true)]
    pub(crate) priority: Option<OsString>,
    #[arg(short = 't', long = "type")]
    pub(crate) issue_type: Option<OsString>,
    #[arg(long, short = 's')]
    pub(crate) status: Option<OsString>,
    #[arg(long)]
    pub(crate) assignee: Option<OsString>,
    #[arg(long, value_parser = int, allow_negative_numbers = true)]
    pub(crate) estimate: Option<Int>,
    #[arg(long = "external-ref")]
    pub(crate) external_ref: Option<OsString>,
    #[arg(long)]
    pub(crate) parent: Option<OsString>,
    #[arg(short = 'l', long = "labels", visible_alias = "label", action = ArgAction::Append)]
    pub(crate) labels: Vec<OsString>,
    /// comma list of [type:]ISSUE
    #[arg(long, action = ArgAction::Append)]
    pub(crate) deps: Vec<OsString>,
    /// print only the new id
    #[arg(long)]
    pub(crate) silent: bool,
    /// record locally only; the next sync or online command publishes it
    #[arg(long)]
    pub(crate) offline: bool,
}

#[derive(Debug, Args)]
pub(crate) struct Show {
    #[arg(required = true, num_args = 1.., allow_negative_numbers = true)]
    pub(crate) ids: Vec<OsString>,
}

/// The filters `list` and `ready` share.
#[derive(Debug, Args)]
pub(crate) struct Filters {
    #[arg(short = 't', long = "type", action = ArgAction::Append)]
    pub(crate) issue_type: Vec<OsString>,
    #[arg(short = 'l', long, action = ArgAction::Append)]
    pub(crate) label: Vec<OsString>,
    #[arg(short = 'p', long, allow_negative_numbers = true)]
    pub(crate) priority: Option<OsString>,
    #[arg(long)]
    pub(crate) assignee: Option<OsString>,
}

#[derive(Debug, Args)]
pub(crate) struct List {
    #[arg(short = 's', long, action = ArgAction::Append)]
    pub(crate) status: Vec<OsString>,
    #[arg(short = 'a', long)]
    pub(crate) all: bool,
    #[arg(long)]
    pub(crate) deferred: bool,
    #[arg(long = "title-contains")]
    pub(crate) title_contains: Option<OsString>,
    #[arg(long, action = ArgAction::Append)]
    pub(crate) id: Vec<OsString>,
    /// only ID's direct children
    #[arg(long, value_name = "ID", action = ArgAction::Append)]
    pub(crate) parent: Vec<OsString>,
    #[command(flatten)]
    pub(crate) filters: Filters,
    #[arg(long, value_parser = int, default_value = "50", allow_negative_numbers = true)]
    pub(crate) limit: Int,
}

#[derive(Debug, Args)]
pub(crate) struct Ready {
    /// only issues in ID's subtree: its children, theirs, and so on
    #[arg(value_name = "ID", conflicts_with_all = ["epic", "parent"], allow_negative_numbers = true)]
    pub(crate) root: Option<OsString>,
    /// the same as ID, as in br
    #[arg(long, value_name = "ID", action = ArgAction::Append, conflicts_with = "parent")]
    pub(crate) epic: Vec<OsString>,
    /// only ID's direct children
    #[arg(long, value_name = "ID", action = ArgAction::Append)]
    pub(crate) parent: Vec<OsString>,
    #[command(flatten)]
    pub(crate) filters: Filters,
    #[arg(long, value_parser = int, default_value = "0", allow_negative_numbers = true)]
    pub(crate) limit: Int,
}

#[derive(Debug, Args)]
pub(crate) struct Sync {
    #[arg(long)]
    pub(crate) remote: Option<OsString>,
}

/// What parsing the command line gave: the command, or a page of help to print, or a usage error.
pub(crate) enum Parsed {
    Command(Box<Command>),
    Help(String),
    Usage(String),
}

/// Parses `args` (the command line after `--json` and `--actor` are taken out).
pub(crate) fn parse(args: &[OsString]) -> Parsed {
    let command = Cli::command().mut_subcommands(|sub| {
        sub.infer_long_args(true)
            .args_override_self(true)
            .disable_help_subcommand(true)
    });
    let argv = std::iter::once(OsString::from("bead")).chain(args.iter().cloned());
    let matches = match command.try_get_matches_from(argv) {
        Ok(matches) => matches,
        Err(error) => return failed(&error),
    };
    match Cli::from_arg_matches(&matches) {
        Ok(cli) => match once(&cli.command) {
            Some(message) => Parsed::Usage(message),
            None => Parsed::Command(Box::new(cli.command)),
        },
        Err(error) => failed(&error),
    }
}

/// An option that names a scope, given twice, is refused, as br refuses it, rather than the first
/// being silently dropped (Python's `Once`).
fn once(command: &Command) -> Option<String> {
    let twice = |values: &[OsString], name: &str| {
        (values.len() > 1).then(|| format!("argument {name}: given more than once"))
    };
    match command {
        Command::List(list) => twice(&list.parent, "--parent"),
        Command::Ready(ready) => {
            twice(&ready.epic, "--epic").or_else(|| twice(&ready.parent, "--parent"))
        }
        _ => None,
    }
}

fn failed(error: &clap::Error) -> Parsed {
    use clap::error::ErrorKind;
    if matches!(
        error.kind(),
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
    ) {
        return Parsed::Help(error.render().to_string());
    }
    let rendered = error.render().to_string();
    let first = rendered.lines().next().unwrap_or_default();
    Parsed::Usage(first.strip_prefix("error: ").unwrap_or(first).to_owned())
}
