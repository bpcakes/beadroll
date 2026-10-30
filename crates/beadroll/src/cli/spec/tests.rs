//! The command table: every command has a page of its own.

use std::collections::BTreeSet;
use std::ffi::OsString;

use super::{COMMANDS, find};
use crate::cli::help::page;
use crate::cli::paint::Color;
use crate::cli::parse::{Page, Parsed, parse};

/// `HelpAndPrime.test_every_command_has_help` over the table: more
/// than 20 commands, each under a name of its own, which `help NAME` finds; `NAME --help`
/// shows that command's page, whose usage is `bead NAME`.
#[test]
fn every_command_has_help() {
    let names: BTreeSet<&str> = COMMANDS.iter().map(|command| command.name).collect();
    assert_eq!(names.len(), COMMANDS.len(), "two commands share a name");
    assert!(names.len() > 20, "{names:?}");
    for name in names {
        assert!(find(name).is_some(), "help {name} finds no command");
        let spec = match parse(&[OsString::from(name), OsString::from("--help")]) {
            Parsed::Help(Page::Command(spec)) if spec.name == name => spec,
            other => panic!("{name} --help: {other:?}"),
        };
        let text = page(spec, Some(80), Color::Plain);
        assert!(text.contains(&format!("Usage: bead {name}")), "{text}");
    }
}
