//! Help pages: every command's, plain unless coloured.

use super::*;
use crate::cli::paint::Color;
use crate::cli::spec::COMMANDS;

#[test]
fn every_page_says_what_its_command_does_and_how_to_call_it() {
    for spec in &COMMANDS {
        for width in [None, Some(40), Some(100)] {
            let text = page(spec, width, Color::Plain);
            let words = text.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(words.starts_with(spec.help), "{text}");
            assert!(
                text.contains(&format!("Usage: bead {}", spec.name)),
                "{text}"
            );
            assert!(!text.contains('\u{1b}'), "{text}");
        }
    }
}

#[test]
fn a_coloured_page_differs_only_by_its_colour_codes() {
    let spec = &COMMANDS[3];
    let coloured = page(spec, Some(80), Color::Ansi);
    assert!(coloured.contains("\u{1b}["), "{coloured}");
    let mut plain = String::new();
    let mut escape = false;
    for c in coloured.chars() {
        match (escape, c) {
            (false, '\u{1b}') => escape = true,
            (true, 'm') => escape = false,
            (false, c) => plain.push(c),
            _ => {}
        }
    }
    assert_eq!(plain, page(spec, Some(80), crate::cli::paint::Color::Plain));
}
