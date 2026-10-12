//! Expected help pages at the specified terminal widths.

use super::*;
use crate::cli::spec::find;

#[test]
fn every_page_starts_with_its_usage_line_and_is_plain_text() {
    for command in &COMMANDS {
        for columns in [1, 12, 40, 80, 200] {
            let text = render(Page::Command(command), columns);
            assert!(
                text.starts_with(&format!("usage: bead {}", command.name)),
                "{text}"
            );
            assert!(!text.contains('\u{1b}'));
            assert!(text.ends_with('\n') && !text.ends_with("\n\n"));
        }
    }
}

#[test]
fn columns_come_from_columns_when_it_is_a_positive_number() {
    assert_eq!(columns(Some(OsStr::new(" 1_00 "))), 100);
    assert_eq!(columns(Some(OsStr::new("９０"))), 90);
    let huge = "9".repeat(400);
    assert_eq!(columns(Some(OsStr::new(&huge))), 1 << 32);
}

#[test]
#[allow(clippy::unwrap_used, reason = "the table has the command")]
fn ready_at_80() {
    let expected = r#"usage: bead ready [-h] [-t TYPE] [-l LABEL] [-p PRIORITY]
                  [--assignee ASSIGNEE] [--limit LIMIT]
                  [--epic ID | --parent ID | ID]

positional arguments:
  ID                    only issues in ID's subtree: its children, theirs, and
                        so on

options:
  -h, --help            show this help message and exit
  --epic ID             the same as ID, as in br
  --parent ID           only ID's direct children
  -t, --type TYPE
  -l, --label LABEL
  -p, --priority PRIORITY
  --assignee ASSIGNEE
  --limit LIMIT
"#;
    assert_eq!(render(Page::Command(find("ready").unwrap()), 80), expected);
}

#[test]
#[allow(clippy::unwrap_used, reason = "the table has the command")]
fn claims_at_80() {
    let expected = r#"usage: bead claims [-h]

options:
  -h, --help  show this help message and exit
"#;
    assert_eq!(render(Page::Command(find("claims").unwrap()), 80), expected);
}

#[test]
#[allow(clippy::unwrap_used, reason = "the table has the command")]
fn top_at_52() {
    let expected = r#"usage: bead [-h]
            {init,import,export,create,update,claim,release,takeover,claims,close,reopen,delete,show,list,ready,blocked,search,dep,comments,lint,sync,log,info,machines,help,prime,onboard} ...

The command line: parser, dispatch, the texts an
agent reads.

positional arguments:
  {init,import,export,create,update,claim,release,takeover,claims,close,reopen,delete,show,list,ready,blocked,search,dep,comments,lint,sync,log,info,machines,help,prime,onboard}
    init                create the tracker for
                        this repository
    import              import a Beads JSONL
                        export
    export              write Beads-compatible
                        JSONL
    create              create an issue
    update              change fields, labels or
                        parent; --claim or
                        --status in_progress
                        claims
    claim               take ownership; it lasts
                        until release, close or
                        takeover
    release             give up your claim
    takeover            replace the current owner
                        after investigating; needs
                        --reason
    claims              who holds what, and when
                        each owner was last active
    close               close issues
    reopen              reopen issues
    delete              delete issues
    show                full detail for one or
                        more issues
    list                list issues (open ones by
                        default)
    ready               open issues with nothing
                        blocking them
    blocked             issues waiting on
                        something
    search              text search over ids,
                        titles, bodies and
                        comments
    dep                 dependencies: dep add A B
                        [--type blocks|parent-
                        child], dep remove A B,
                        dep list ID, dep tree ID,
                        dep cycles
    comments            read an issue's comments,
                        or `comments add ID TEXT`
                        to record one
    lint                open work items without
                        acceptance criteria
    sync                exchange the journal with
                        other machines through the
                        coordination ref
    log                 who did what: machine,
                        agent, thread
    info                where the tracker lives
                        and who you are
    machines            machines seen in the
                        journal
    help                this overview, or `help
                        COMMAND` for one command's
                        usage
    prime               the workflow an agent
                        should load at the start
                        of a session
    onboard             a snippet for CLAUDE.md /
                        AGENTS.md

options:
  -h, --help            show this help message and
                        exit
"#;
    assert_eq!(render(Page::Top, 52), expected);
}

#[test]
#[allow(clippy::unwrap_used, reason = "the table has the command")]
fn search_at_12() {
    let expected = r#"usage: bead search
       [-h]
       [-a]
       query

positional arguments:
  query

options:
  -h, --help
    show this
    help
    message and
    exit
  -a, --all
"#;
    assert_eq!(render(Page::Command(find("search").unwrap()), 12), expected);
}
