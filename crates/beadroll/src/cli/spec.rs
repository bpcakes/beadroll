//! The command argument table: option strings in display order, destination, metavar, value
//! count, action, value type and help text. The parser, help pages and receipt signatures all use
//! this table.

/// The number of values an argument takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Nargs {
    /// A flag: no value.
    Zero,
    /// Exactly one value.
    One,
    /// `?`: a positional that may be absent.
    Optional,
    /// `*`: any number of positionals.
    ZeroOrMore,
    /// `+`: at least one positional.
    OneOrMore,
    /// The command and everything after it (the subparsers action's `PARSER`).
    Parser,
}

/// The action taken when an argument is supplied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// `store`: the last value wins.
    Store,
    /// `store_true`.
    StoreTrue,
    /// `append`: every value is kept.
    Append,
    /// bead's `Once`: an option that names a scope; a second one is a usage error.
    Once,
    /// `-h`/`--help`: the page, and nothing else is done.
    Help,
    /// The subparsers action: the command, which parses the rest of the line.
    Commands,
}

/// The type a value is read as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Text, kept as the OS gave it until it is used.
    Text,
    /// An integer parsed by `beadroll_core::text::parse_integer`.
    Int,
}

/// One argument of one command.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ArgSpec {
    /// Its option strings, in declaration order; none for a positional.
    pub(crate) flags: &'static [&'static str],
    /// Its destination in the namespace.
    pub(crate) dest: &'static str,
    /// Its metavar, when it is not the destination (upper-cased for an option).
    pub(crate) metavar: Option<&'static str>,
    pub(crate) nargs: Nargs,
    pub(crate) action: Action,
    pub(crate) kind: Kind,
    /// The default when not given: `None` means no value, or false for a flag.
    pub(crate) default: Option<&'static str>,
    pub(crate) help: Option<&'static str>,
    /// Whether it belongs to the command's one mutually exclusive group (`ready`'s scope).
    pub(crate) exclusive: bool,
}

/// One command: its name, its line in `bead -h`'s list and its arguments, in declaration order.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CommandSpec {
    pub(crate) name: &'static str,
    pub(crate) help: &'static str,
    pub(crate) args: &'static [ArgSpec],
    /// Help, prime and onboard work outside a repository.
    pub(crate) workspace_free: bool,
}

const fn opt(flags: &'static [&'static str], dest: &'static str) -> ArgSpec {
    ArgSpec {
        flags,
        dest,
        metavar: None,
        nargs: Nargs::One,
        action: Action::Store,
        kind: Kind::Text,
        default: None,
        help: None,
        exclusive: false,
    }
}

const fn flag(flags: &'static [&'static str], dest: &'static str) -> ArgSpec {
    ArgSpec {
        nargs: Nargs::Zero,
        action: Action::StoreTrue,
        ..opt(flags, dest)
    }
}

const fn positional(dest: &'static str, nargs: Nargs) -> ArgSpec {
    ArgSpec {
        nargs,
        ..opt(&[], dest)
    }
}

impl ArgSpec {
    const fn help(mut self, text: &'static str) -> ArgSpec {
        self.help = Some(text);
        self
    }

    const fn metavar(mut self, name: &'static str) -> ArgSpec {
        self.metavar = Some(name);
        self
    }

    const fn append(mut self) -> ArgSpec {
        self.action = Action::Append;
        self
    }

    const fn once(mut self) -> ArgSpec {
        self.action = Action::Once;
        self
    }

    const fn int(mut self, default: Option<&'static str>) -> ArgSpec {
        self.kind = Kind::Int;
        self.default = default;
        self
    }

    const fn exclusive(mut self) -> ArgSpec {
        self.exclusive = true;
        self
    }

    /// Whether it is a positional.
    pub(crate) fn is_positional(&self) -> bool {
        self.flags.is_empty()
    }

    /// Whether it must be given: a positional that takes one value or more, and the command.
    pub(crate) fn is_required(&self) -> bool {
        self.is_positional() && matches!(self.nargs, Nargs::One | Nargs::OneOrMore | Nargs::Parser)
    }

    /// The name used in diagnostics: option strings joined by `/`, else metavar, else
    /// destination.
    pub(crate) fn name(&self) -> String {
        match (self.flags, self.metavar) {
            ([], Some(metavar)) => metavar.to_owned(),
            ([], None) => self.dest.to_owned(),
            (flags, _) => flags.join("/"),
        }
    }
}

/// `-h`/`--help`, which every parser has first.
pub(crate) const HELP_FLAG: ArgSpec = ArgSpec {
    action: Action::Help,
    ..flag(&["-h", "--help"], "help")
}
.help("show this help message and exit");

/// The command, `bead`'s one positional: its choices are the commands' names.
pub(crate) const COMMAND: ArgSpec = ArgSpec {
    action: Action::Commands,
    ..positional("command", Nargs::Parser)
};

/// The description shown by `bead -h`.
pub(crate) const DESCRIPTION: &str =
    "The command line: parser, dispatch, the texts an agent reads.";

const OFFLINE_LOCAL: ArgSpec = flag(&["--offline"], "offline")
    .help("record locally only; the next sync or online command publishes it");
const OFFLINE_COORDINATED: ArgSpec = flag(&["--offline"], "offline")
    .help("do not contact the remote; a claim made this way is provisional");
const REMOTE_COORDINATED: ArgSpec =
    opt(&["--remote"], "remote").help("coordination remote (name or URL) for this call");
const IDS: ArgSpec = positional("ids", Nargs::OneOrMore);
const REASON: ArgSpec = opt(&["-r", "--reason"], "reason");

/// `content_flags`: the fields `create` and `update` set.
macro_rules! content_flags {
    () => {
        [
            opt(&["-d", "--description"], "description"),
            opt(&["--description-file"], "description_file"),
            opt(
                &["--acceptance-criteria", "--acceptance"],
                "acceptance_criteria",
            ),
            opt(&["--notes"], "notes"),
            opt(&["--design"], "design"),
            opt(&["-p", "--priority"], "priority"),
            opt(&["-t", "--type"], "type"),
            opt(&["--status", "-s"], "status"),
            opt(&["--assignee"], "assignee"),
            opt(&["--estimate"], "estimate").int(None),
            opt(&["--external-ref"], "external_ref"),
            opt(&["--parent"], "parent"),
        ]
    };
}

const CONTENT: [ArgSpec; 12] = content_flags!();

/// `filters`: what `list` and `ready` share, `--limit`'s default being the command's.
const fn filters(limit: &'static str) -> [ArgSpec; 5] {
    [
        opt(&["-t", "--type"], "type").append(),
        opt(&["-l", "--label"], "label").append(),
        opt(&["-p", "--priority"], "priority"),
        opt(&["--assignee"], "assignee"),
        opt(&["--limit"], "limit").int(Some(limit)),
    ]
}

const LIST_FILTERS: [ArgSpec; 5] = filters("50");
const READY_FILTERS: [ArgSpec; 5] = filters("0");

const INIT: &[ArgSpec] = &[
    opt(&["--prefix"], "prefix"),
    opt(&["--import"], "import_path").metavar("BEADS_JSONL"),
    flag(&["--tracked"], "tracked")
        .help("write .beadroll.json naming this tracker, to commit: only for a remote other than origin, or \"where\": false"),
    opt(&["--remote"], "remote").help("coordination remote (name or URL) if it is not origin"),
    OFFLINE_LOCAL,
];

const IMPORT: &[ArgSpec] = &[positional("path", Nargs::One), OFFLINE_LOCAL];

const EXPORT: &[ArgSpec] = &[opt(&["-o", "--output"], "output")];

const CREATE: &[ArgSpec] = &[
    positional("title_pos", Nargs::Optional).metavar("TITLE"),
    opt(&["--title"], "title"),
    CONTENT[0],
    CONTENT[1],
    CONTENT[2],
    CONTENT[3],
    CONTENT[4],
    CONTENT[5],
    CONTENT[6],
    CONTENT[7],
    CONTENT[8],
    CONTENT[9],
    CONTENT[10],
    CONTENT[11],
    opt(&["-l", "--labels", "--label"], "labels").append(),
    opt(&["--deps"], "deps")
        .append()
        .help("comma list of [type:]ISSUE"),
    flag(&["--silent"], "silent").help("print only the new id"),
    OFFLINE_LOCAL,
];

const UPDATE: &[ArgSpec] = &[
    IDS,
    opt(&["--title"], "title"),
    CONTENT[0],
    CONTENT[1],
    CONTENT[2],
    CONTENT[3],
    CONTENT[4],
    CONTENT[5],
    CONTENT[6],
    CONTENT[7],
    CONTENT[8],
    CONTENT[9],
    CONTENT[10],
    CONTENT[11],
    flag(&["--claim"], "claim"),
    flag(&["--force"], "force").help("change the status of an issue someone else has claimed"),
    opt(&["--add-label"], "add_label").append(),
    opt(&["--remove-label"], "remove_label").append(),
    opt(&["--set-labels"], "set_labels"),
    OFFLINE_COORDINATED,
    REMOTE_COORDINATED,
];

const CLAIM: &[ArgSpec] = &[IDS, OFFLINE_COORDINATED, REMOTE_COORDINATED];

const RELEASE: &[ArgSpec] = &[IDS, REASON, OFFLINE_COORDINATED, REMOTE_COORDINATED];

const TRANSITION: &[ArgSpec] = &[
    IDS,
    REASON,
    flag(&["--force"], "force").help("act on an issue someone else has claimed"),
    OFFLINE_COORDINATED,
    REMOTE_COORDINATED,
];

const SHOW: &[ArgSpec] = &[IDS];

const LIST: &[ArgSpec] = &[
    opt(&["-s", "--status"], "status").append(),
    flag(&["-a", "--all"], "all"),
    flag(&["--deferred"], "deferred"),
    opt(&["--title-contains"], "title_contains"),
    opt(&["--id"], "id").append(),
    opt(&["--parent"], "parent")
        .once()
        .metavar("ID")
        .help("only ID's direct children"),
    LIST_FILTERS[0],
    LIST_FILTERS[1],
    LIST_FILTERS[2],
    LIST_FILTERS[3],
    LIST_FILTERS[4],
];

const READY: &[ArgSpec] = &[
    positional("root", Nargs::Optional)
        .metavar("ID")
        .help("only issues in ID's subtree: its children, theirs, and so on")
        .exclusive(),
    opt(&["--epic"], "epic")
        .once()
        .metavar("ID")
        .help("the same as ID, as in br")
        .exclusive(),
    opt(&["--parent"], "parent")
        .once()
        .metavar("ID")
        .help("only ID's direct children")
        .exclusive(),
    READY_FILTERS[0],
    READY_FILTERS[1],
    READY_FILTERS[2],
    READY_FILTERS[3],
    READY_FILTERS[4],
];

const SEARCH: &[ArgSpec] = &[
    positional("query", Nargs::One),
    flag(&["-a", "--all"], "all"),
];

const DEP: &[ArgSpec] = &[
    positional("words", Nargs::ZeroOrMore),
    opt(&["--type"], "type"),
    flag(&["--blocking-only"], "blocking_only"),
    OFFLINE_LOCAL,
];

const COMMENTS: &[ArgSpec] = &[
    positional("words", Nargs::ZeroOrMore),
    opt(&["-m", "--message"], "message"),
    opt(&["-f", "--file"], "file"),
    opt(&["--author"], "author"),
    OFFLINE_LOCAL,
];

const SYNC: &[ArgSpec] = &[opt(&["--remote"], "remote")];

const LOG: &[ArgSpec] = &[
    positional("key", Nargs::Optional),
    opt(&["--limit"], "limit").int(Some("30")),
];

const MACHINES: &[ArgSpec] = &[opt(&["--name"], "name")
    .metavar("ID=ALIAS")
    .help("give a machine a local display name")];

const HELP: &[ArgSpec] = &[positional("command_name", Nargs::Optional).metavar("COMMAND")];

const fn command(name: &'static str, help: &'static str, args: &'static [ArgSpec]) -> CommandSpec {
    CommandSpec {
        name,
        help,
        args,
        workspace_free: false,
    }
}

const fn free(name: &'static str, help: &'static str, args: &'static [ArgSpec]) -> CommandSpec {
    CommandSpec {
        name,
        help,
        args,
        workspace_free: true,
    }
}

/// Every command, in help display order.
pub(crate) const COMMANDS: [CommandSpec; 27] = [
    command("init", "create the tracker for this repository", INIT),
    command("import", "import a Beads JSONL export", IMPORT),
    command("export", "write Beads-compatible JSONL", EXPORT),
    command("create", "create an issue", CREATE),
    command(
        "update",
        "change fields, labels or parent; --claim or --status in_progress claims",
        UPDATE,
    ),
    command(
        "claim",
        "take ownership; it lasts until release, close or takeover",
        CLAIM,
    ),
    command("release", "give up your claim", RELEASE),
    command(
        "takeover",
        "replace the current owner after investigating; needs --reason",
        RELEASE,
    ),
    command(
        "claims",
        "who holds what, and when each owner was last active",
        &[],
    ),
    command("close", "close issues", TRANSITION),
    command("reopen", "reopen issues", TRANSITION),
    command("delete", "delete issues", TRANSITION),
    command("show", "full detail for one or more issues", SHOW),
    command("list", "list issues (open ones by default)", LIST),
    command("ready", "open issues with nothing blocking them", READY),
    command("blocked", "issues waiting on something", &[]),
    command(
        "search",
        "text search over ids, titles, bodies and comments",
        SEARCH,
    ),
    command(
        "dep",
        "dependencies: dep add A B [--type blocks|parent-child], dep remove A B, dep list ID, \
         dep tree ID, dep cycles",
        DEP,
    ),
    command(
        "comments",
        "read an issue's comments, or `comments add ID TEXT` to record one",
        COMMENTS,
    ),
    command("lint", "open work items without acceptance criteria", &[]),
    command(
        "sync",
        "exchange the journal with other machines through the coordination ref",
        SYNC,
    ),
    command("log", "who did what: machine, agent, thread", LOG),
    command("info", "where the tracker lives and who you are", &[]),
    command("machines", "machines seen in the journal", MACHINES),
    free(
        "help",
        "this overview, or `help COMMAND` for one command's usage",
        HELP,
    ),
    free(
        "prime",
        "the workflow an agent should load at the start of a session",
        &[],
    ),
    free("onboard", "a snippet for CLAUDE.md / AGENTS.md", &[]),
];

/// The command named `name`.
pub(crate) fn find(name: &str) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|c| c.name == name)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::ffi::OsString;

    use super::{COMMANDS, find};
    use crate::cli::help::render;
    use crate::cli::parse::{Page, Parsed, parse};

    /// `HelpAndPrime.test_every_command_has_help` over the table (`docs/RUST-PORT.md` §4.6): more
    /// than 20 commands, each under a name of its own, which `help NAME` finds; `NAME --help`
    /// shows that command's page, which starts `usage: bead NAME`.
    #[test]
    fn every_command_has_help() {
        let names: BTreeSet<&str> = COMMANDS.iter().map(|command| command.name).collect();
        assert_eq!(names.len(), COMMANDS.len(), "two commands share a name");
        assert!(names.len() > 20, "{names:?}");
        for name in names {
            assert!(find(name).is_some(), "help {name} finds no command");
            let page = match parse(&[OsString::from(name), OsString::from("--help")]) {
                Parsed::Help(page @ Page::Command(spec)) if spec.name == name => page,
                other => panic!("{name} --help: {other:?}"),
            };
            let text = render(page, 80);
            assert!(text.starts_with(&format!("usage: bead {name}")), "{text}");
        }
    }
}
