//! Colour on a terminal (roll-dih8.23): whether each stream shows it, and how each tone looks.
//! `--color always` or `never` decides; with `auto`, the default, `NO_COLOR` turns colour off,
//! `CLICOLOR_FORCE` on, `CLICOLOR=0` or `TERM=dumb` off, and otherwise a stream shows colour when
//! it is a terminal. `--json` never does. Every part of a line is escaped ([`visible`]) before it
//! is painted, so the only escape sequences bead writes are its own.

use std::ffi::{OsStr, OsString};
use std::os::fd::AsFd;

use clap::builder::styling::{AnsiColor, Style};

use super::output::visible;
use crate::commands::{Line, Tone};

/// What `--color` asks for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum When {
    #[default]
    Auto,
    Always,
    Never,
}

impl When {
    /// The choice `text` names, or `None`.
    pub(crate) fn named(text: &OsStr) -> Option<When> {
        match text.to_str()? {
            "auto" => Some(When::Auto),
            "always" => Some(When::Always),
            "never" => Some(When::Never),
            _ => None,
        }
    }
}

/// Whether standard output and standard error show colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Colors {
    pub(crate) stdout: bool,
    pub(crate) stderr: bool,
}

/// Which streams show colour for `when`, the environment read through `var`.
pub(crate) fn colors(when: When, var: &dyn Fn(&str) -> Option<OsString>) -> Colors {
    Colors {
        stdout: shows(when, var, std::io::stdout()),
        stderr: shows(when, var, std::io::stderr()),
    }
}

/// Whether `stream` shows colour.
fn shows(when: When, var: &dyn Fn(&str) -> Option<OsString>, stream: impl AsFd) -> bool {
    let set = |key: &str| var(key).filter(|value| !value.is_empty());
    match when {
        When::Always => true,
        When::Never => false,
        When::Auto if set("NO_COLOR").is_some() => false,
        When::Auto if set("CLICOLOR_FORCE").is_some_and(|value| value != "0") => true,
        When::Auto if set("CLICOLOR").is_some_and(|value| value == "0") => false,
        When::Auto if set("TERM").is_some_and(|value| value == "dumb") => false,
        When::Auto => rustix::termios::isatty(stream),
    }
}

/// How `tone` looks.
fn style(tone: Tone) -> Style {
    let colour = |colour: AnsiColor| Style::new().fg_color(Some(colour.into()));
    match tone {
        Tone::Plain | Tone::Verbatim => Style::new(),
        Tone::Id | Tone::Term => colour(AnsiColor::Cyan),
        Tone::Strong => Style::new().bold(),
        Tone::Dim | Tone::Gone | Tone::Low => Style::new().dimmed(),
        Tone::Done => colour(AnsiColor::Green),
        Tone::Active | Tone::High | Tone::Warning => colour(AnsiColor::Yellow),
        Tone::Held => colour(AnsiColor::Red),
        Tone::Urgent | Tone::Error => colour(AnsiColor::Red).bold(),
        Tone::Deferred => colour(AnsiColor::Blue),
        Tone::Who => colour(AnsiColor::Magenta),
        Tone::Heading => colour(AnsiColor::Green).bold(),
    }
}

/// `line` for a reader: each part escaped, and painted when `color`. A plain line that starts
/// with a check mark (`✓ Closed …`) has the mark painted as done.
pub(crate) fn render(line: &Line, color: bool) -> String {
    let marked;
    let parts = match line.parts() {
        [(Tone::Plain, text)] if text.starts_with("✓ ") => {
            marked = [
                (Tone::Done, "✓".to_owned()),
                (Tone::Plain, text["✓".len()..].to_owned()),
            ];
            &marked[..]
        }
        parts => parts,
    };
    let mut out = String::new();
    for (tone, text) in parts {
        // bead's own text, painted already: a help page clap rendered
        let text = if *tone == Tone::Verbatim {
            text.clone()
        } else {
            visible(text)
        };
        let style = style(*tone);
        if color && style != Style::new() {
            out.push_str(&format!("{style}{text}{style:#}"));
        } else {
            out.push_str(&text);
        }
    }
    out
}

/// `lines` for a reader, one to a line.
pub(crate) fn render_all(lines: &[Line], color: bool) -> String {
    let rendered: Vec<String> = lines.iter().map(|line| render(line, color)).collect();
    rendered.join("\n")
}

#[cfg(test)]
mod tests;
