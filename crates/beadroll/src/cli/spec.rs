//! The command argument table: option strings in display order, destination, metavar, value
//! count, action, value type and help text. clap's command line, its help pages and receipt
//! signatures are all built from it (`parse`).

use super::layout::Block;
use super::texts;
use args::{
    CLAIM, CLOSE, COMMENTS, CREATE, DEP, EXPORT, HELP, IMPORT, INIT, LIST, LOG, MACHINES, READY,
    RELEASE, SEARCH, SHOW, SYNC, TAKEOVER, TRANSITION, UPDATE,
};

mod args;

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

/// One command: its name, its line in `bead -h`'s list (its page's description too), its
/// arguments, in declaration order, and what its page says after them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CommandSpec {
    pub(crate) name: &'static str,
    pub(crate) help: &'static str,
    pub(crate) args: &'static [ArgSpec],
    /// Help, prime and onboard work outside a repository.
    pub(crate) workspace_free: bool,
    /// The forms and rules its options do not tell, after them.
    pub(crate) epilog: Option<&'static [Block]>,
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
}

/// What `bead`'s own clap command says of itself.
pub(crate) const DESCRIPTION: &str = "bead: a Beads-compatible issue tracker for agent crews \
     working across worktrees and machines. `bead help` gives the overview, `bead help COMMAND` \
     one command's page.";

const fn command(name: &'static str, help: &'static str, args: &'static [ArgSpec]) -> CommandSpec {
    CommandSpec {
        name,
        help,
        args,
        workspace_free: false,
        epilog: None,
    }
}

const fn free(name: &'static str, help: &'static str, args: &'static [ArgSpec]) -> CommandSpec {
    CommandSpec {
        workspace_free: true,
        ..command(name, help, args)
    }
}

impl CommandSpec {
    const fn epilog(mut self, text: &'static [Block]) -> CommandSpec {
        self.epilog = Some(text);
        self
    }
}

/// Every command, in help display order.
pub(crate) const COMMANDS: [CommandSpec; 27] = [
    command("init", "create the tracker for this repository", INIT),
    command("import", "import a Beads JSONL export", IMPORT),
    command("export", "write Beads-compatible JSONL", EXPORT),
    command("create", "create an issue", CREATE).epilog(texts::CREATE_EPILOG),
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
        TAKEOVER,
    ),
    command(
        "claims",
        "who holds what, and when each owner was last active",
        &[],
    ),
    command("close", "close issues", CLOSE),
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
        "dependencies between issues: add, remove, list, tree, cycles",
        DEP,
    )
    .epilog(texts::DEP_EPILOG),
    command(
        "comments",
        "read an issue's comments, or `comments add ID TEXT` to record one",
        COMMENTS,
    )
    .epilog(texts::COMMENTS_EPILOG),
    command("lint", "open work items without acceptance criteria", &[]),
    command(
        "sync",
        "exchange the journal with other machines through the coordination ref",
        SYNC,
    ),
    command(
        "log",
        "who did what: time, machine, actor (its name, else agent and thread), operation",
        LOG,
    ),
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
mod tests;
