//! The command table: every command has a page of its own.

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
