//! Each command's arguments, in declaration order, with the help their pages show.

use super::{ArgSpec, Nargs, flag, opt, positional};

const OFFLINE_LOCAL: ArgSpec = flag(&["--offline"], "offline")
    .help("record locally only; the next sync or online command publishes it");
const OFFLINE_COORDINATED: ArgSpec = flag(&["--offline"], "offline")
    .help("do not contact the remote; a claim made this way is provisional");
const REMOTE_COORDINATED: ArgSpec =
    opt(&["--remote"], "remote").help("coordination remote (name or URL) for this call");
const IDS: ArgSpec = positional("ids", Nargs::OneOrMore)
    .metavar("ID")
    .help("issue ids, in full or without the prefix");
const REASON: ArgSpec = opt(&["-r", "--reason"], "reason");

/// `content_flags`: the fields `create` and `update` set.
macro_rules! content_flags {
    () => {
        [
            opt(&["-d", "--description"], "description").help("the body"),
            opt(&["--description-file"], "description_file")
                .metavar("FILE")
                .help("the body, read from FILE (- for standard input)"),
            opt(
                &["--acceptance-criteria", "--acceptance"],
                "acceptance_criteria",
            )
            .metavar("TEXT")
            .help("what done means (`bead lint` lists work without it)"),
            opt(&["--notes"], "notes").help("working notes"),
            opt(&["--design"], "design").help("design notes"),
            opt(&["-p", "--priority"], "priority").help("0 (most urgent) to 4"),
            opt(&["-t", "--type"], "type")
                .help("task, bug, feature, epic, chore, or a word of your own"),
            opt(&["--status", "-s"], "status")
                .help("open, blocked, deferred or closed; in_progress is reached by claiming"),
            opt(&["--assignee"], "assignee").help("who it is assigned to"),
            opt(&["--estimate"], "estimate")
                .int(None)
                .metavar("MINUTES")
                .help("estimated minutes"),
            opt(&["--external-ref"], "external_ref").help("a reference elsewhere: gh-12, a URL"),
            opt(&["--parent"], "parent").help("the issue it is a child of"),
        ]
    };
}

const CONTENT: [ArgSpec; 12] = content_flags!();

/// `filters`: what `list` and `ready` share, `--limit`'s default being the command's.
const fn filters(limit: &'static str, limit_help: &'static str) -> [ArgSpec; 5] {
    [
        opt(&["-t", "--type"], "type")
            .append()
            .help("only these types (repeat it or use commas)"),
        opt(&["-l", "--label"], "label")
            .append()
            .help("only issues with all these labels (repeat it or use commas)"),
        opt(&["-p", "--priority"], "priority").help("only this priority, 0 to 4"),
        opt(&["--assignee"], "assignee").help("only issues assigned to ASSIGNEE"),
        opt(&["--limit"], "limit")
            .int(Some(limit))
            .metavar("N")
            .help(limit_help),
    ]
}

const LIST_FILTERS: [ArgSpec; 5] = filters("50", "at most N issues (default 50; 0 lists all)");
const READY_FILTERS: [ArgSpec; 5] = filters("0", "at most N issues (default: all)");

pub(super) const INIT: &[ArgSpec] = &[
    opt(&["--prefix"], "prefix").help("the issue id prefix (default: from the repository's name)"),
    opt(&["--import"], "import_path")
        .metavar("BEADS_JSONL")
        .help("a Beads JSONL export to import (- for standard input)"),
    flag(&["--tracked"], "tracked")
        .help("write .beadroll.json naming this tracker, to commit: only for a remote other than origin, or \"where\": false"),
    opt(&["--remote"], "remote").help("coordination remote (name or URL) if it is not origin"),
    OFFLINE_LOCAL,
    DRY_RUN,
];

pub(super) const IMPORT: &[ArgSpec] = &[
    positional("path", Nargs::One)
        .metavar("FILE")
        .help("a Beads JSONL export (- for standard input)"),
    OFFLINE_LOCAL,
    DRY_RUN,
];

/// `--dry-run`, of `init --import` and `import`.
const DRY_RUN: ArgSpec = flag(&["--dry-run"], "dry_run")
    .help("say what the import would do and import nothing, asking no remote");

pub(super) const EXPORT: &[ArgSpec] = &[opt(&["-o", "--output"], "output")
    .metavar("FILE")
    .help("write to FILE instead of standard output")];

pub(super) const CREATE: &[ArgSpec] = &[
    positional("title_pos", Nargs::Optional)
        .metavar("TITLE")
        .help("the issue's title"),
    opt(&["--title"], "title").help("the title, given as an option instead"),
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
    opt(&["-l", "--labels", "--label"], "labels")
        .append()
        .metavar("LABEL")
        .help("a label (repeat it or use commas)"),
    opt(&["--deps"], "deps")
        .append()
        .metavar("[TYPE:]ISSUE")
        .help("issues it depends on, comma-separated (below)"),
    flag(&["--silent"], "silent").help("print only the new id"),
    OFFLINE_LOCAL,
];

pub(super) const UPDATE: &[ArgSpec] = &[
    IDS,
    opt(&["--title"], "title").help("the new title"),
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
    flag(&["--claim"], "claim").help("claim it too: the claim and the edits land together"),
    flag(&["--force"], "force").help("change the status of an issue someone else has claimed"),
    opt(&["--add-label"], "add_label")
        .append()
        .metavar("LABEL")
        .help("a label to add (repeat it or use commas)"),
    opt(&["--remove-label"], "remove_label")
        .append()
        .metavar("LABEL")
        .help("a label to remove (repeat it or use commas)"),
    opt(&["--set-labels"], "set_labels")
        .metavar("LABELS")
        .help("replace every label with these, comma-separated"),
    OFFLINE_COORDINATED,
    REMOTE_COORDINATED,
];

pub(super) const CLAIM: &[ArgSpec] = &[IDS, OFFLINE_COORDINATED, REMOTE_COORDINATED];

pub(super) const RELEASE: &[ArgSpec] = &[
    IDS,
    REASON.help("why, for whoever takes it next"),
    OFFLINE_COORDINATED,
    REMOTE_COORDINATED,
];

pub(super) const TAKEOVER: &[ArgSpec] = &[
    IDS,
    REASON.help("required: why, and what you checked first"),
    OFFLINE_COORDINATED,
    REMOTE_COORDINATED,
];

pub(super) const TRANSITION: &[ArgSpec] = &[
    IDS,
    REASON.help("recorded with it: what was done, or why"),
    flag(&["--force"], "force").help("act on an issue someone else has claimed"),
    OFFLINE_COORDINATED,
    REMOTE_COORDINATED,
];

/// `close` takes `reopen`'s and `delete`'s arguments, and `--recursive`, which only it has.
pub(super) const CLOSE: &[ArgSpec] = &[
    TRANSITION[0],
    TRANSITION[1],
    flag(&["--recursive"], "recursive")
        .help("close everything beneath each ID too, children first"),
    TRANSITION[2],
    TRANSITION[3],
    TRANSITION[4],
];

pub(super) const SHOW: &[ArgSpec] = &[IDS];

pub(super) const LIST: &[ArgSpec] = &[
    opt(&["-s", "--status"], "status")
        .append()
        .help("only these statuses (repeat it or use commas); all: any"),
    flag(&["-a", "--all"], "all").help("every status, closed and deleted included"),
    flag(&["--deferred"], "deferred").help("deferred issues too"),
    opt(&["--title-contains"], "title_contains")
        .metavar("TEXT")
        .help("only titles containing TEXT, in any case"),
    opt(&["--id"], "id")
        .append()
        .help("only these ids (repeat it or use commas)"),
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

pub(super) const READY: &[ArgSpec] = &[
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

pub(super) const SEARCH: &[ArgSpec] = &[
    positional("query", Nargs::One)
        .metavar("TEXT")
        .help("text to find, in any case"),
    flag(&["-a", "--all"], "all").help("closed and deleted issues too"),
];

pub(super) const DEP: &[ArgSpec] = &[
    positional("words", Nargs::ZeroOrMore)
        .metavar("ARGS")
        .help("a form and its issues (below)"),
    opt(&["--type"], "type").help("the dependency's type, for add and remove (below)"),
    flag(&["--blocking-only"], "blocking_only").help("cycles: blocks dependencies only"),
    OFFLINE_LOCAL,
];

pub(super) const COMMENTS: &[ArgSpec] = &[
    positional("words", Nargs::ZeroOrMore)
        .metavar("ARGS")
        .help("[add] ISSUE [TEXT ...] (below)"),
    opt(&["-m", "--message"], "message")
        .metavar("TEXT")
        .help("the comment's text"),
    opt(&["-f", "--file"], "file")
        .help("the comment's text, read from FILE (- for standard input)"),
    opt(&["--author"], "author").help("the author recorded, when it is not you"),
    OFFLINE_LOCAL,
];

pub(super) const SYNC: &[ArgSpec] =
    &[opt(&["--remote"], "remote").help("coordination remote (name or URL) for this sync")];

pub(super) const LOG: &[ArgSpec] = &[
    positional("key", Nargs::Optional)
        .metavar("ID")
        .help("only this issue's events"),
    opt(&["--limit"], "limit")
        .int(Some("30"))
        .metavar("N")
        .help("the last N events (default 30; 0: all; -N: all but the first N)"),
];

pub(super) const MACHINES: &[ArgSpec] = &[opt(&["--name"], "name")
    .metavar("ID=ALIAS")
    .help("give a machine a local display name")];

pub(super) const RECAP: &[ArgSpec] = &[
    opt(&["--format"], "format")
        .metavar("FORMAT")
        .help("events (the default), issues or full: how the activity is laid out"),
    flag(&["--all"], "all").help("every event, seen or not"),
    flag(&["--advance"], "advance")
        .exclusive()
        .help("mark what it shows as seen even when the output is not a terminal"),
    flag(&["--no-advance"], "no_advance")
        .exclusive()
        .help("mark nothing as seen, even on a terminal"),
];

pub(super) const HELP: &[ArgSpec] = &[positional("command_name", Nargs::Optional)
    .metavar("COMMAND")
    .help("the command to explain")];
