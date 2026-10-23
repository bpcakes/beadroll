//! bead's own texts laid out when they are printed: the overview, `prime` and the ends of help
//! pages. A paragraph wraps to the width at spaces; a table keeps a column for its headings and
//! one for its terms, each description wrapping under its own column, and stacks its rows when
//! the width leaves descriptions too little room. With no width (standard output is not a
//! terminal and `COLUMNS` says nothing) nothing wraps: the reader wraps for itself.

use std::ffi::OsStr;

use beadroll_core::text::parse_integer;
use textwrap::core::display_width;
use textwrap::{Options, WordSeparator, WordSplitter};

use crate::commands::{Line, Tone};

/// The widest a text is laid out, however wide the terminal: longer lines are hard to read.
const READABLE: usize = 100;

/// The narrowest a table's description column may be; narrower than this, its rows stack.
const NARROWEST: usize = 20;

/// The widest a table's term column is, its gap included: a longer term has the line to itself,
/// its description starting under the column on the next (argparse's `max_help_position` rule).
const WIDEST_TERM: usize = 30;

/// One part of a text, a blank line from the next.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Block {
    /// Prose.
    Text(&'static str),
    /// Rows laid out in columns.
    Rows(&'static [Row]),
}

/// A row of a table: the heading of the group it starts (empty to continue the group), a term (a
/// command or an option) and what it does. A row without a term is prose under its heading; a
/// row without a description lists terms, three spaces apart; a row with neither is a blank line.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Row {
    pub(crate) heading: &'static str,
    pub(crate) term: &'static str,
    pub(crate) about: &'static str,
}

/// A row of a table ([`Row`]).
pub(crate) const fn row(heading: &'static str, term: &'static str, about: &'static str) -> Row {
    Row {
        heading,
        term,
        about,
    }
}

/// A blank line in a table.
pub(crate) const BLANK: Row = row("", "", "");

/// The width bead's own texts are laid out at: `COLUMNS` when it holds a positive number, else the
/// width of the terminal on standard output, at most [`READABLE`]; none when standard output is
/// not a terminal.
pub(crate) fn width(variable: Option<&OsStr>) -> Option<usize> {
    let given = variable
        .and_then(OsStr::to_str)
        .and_then(parse_integer)
        .map(|number| number.as_i128())
        .filter(|number| *number > 0)
        .map(|number| usize::try_from(number).unwrap_or(usize::MAX));
    let width = given.or_else(|| match rustix::termios::tcgetwinsize(std::io::stdout()) {
        Ok(size) if size.ws_col > 0 => Some(usize::from(size.ws_col)),
        _ => None,
    })?;
    Some(width.min(READABLE))
}

/// `blocks` laid out `width` wide as plain text, a blank line between them, ending with a newline.
pub(crate) fn lay_out(blocks: &[Block], width: Option<usize>) -> String {
    let lines: Vec<String> = lay_out_lines(blocks, width)
        .iter()
        .map(Line::text)
        .collect();
    format!("{}\n", lines.join("\n"))
}

/// `blocks` laid out `width` wide, a blank line between them: lines whose headings and terms say
/// what they are, for a terminal to paint.
pub(crate) fn lay_out_lines(blocks: &[Block], width: Option<usize>) -> Vec<Line> {
    let mut lines = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 {
            lines.push(Line::new());
        }
        match block {
            Block::Text(text) => lines.extend(paragraph_lines(text, width)),
            Block::Rows(rows) => lines.extend(table(rows, width)),
        }
    }
    lines
}

/// Prose wrapped `width` wide at its spaces, without a final newline.
pub(crate) fn paragraph(text: &str, width: Option<usize>) -> String {
    fill(text, width, 0, 0).join("\n")
}

/// Prose wrapped `width` wide at its spaces, as lines.
pub(crate) fn paragraph_lines(text: &str, width: Option<usize>) -> Vec<Line> {
    fill(text, width, 0, 0)
        .into_iter()
        .map(Line::from)
        .collect()
}

/// How many columns `text` takes on a terminal: two for a wide character, none for a combining
/// one.
fn length(text: &str) -> usize {
    display_width(text)
}

/// A space no line breaks at while a text is wrapped.
const KEPT: char = '\u{a0}';

/// What separates the terms a row lists.
const BETWEEN_TERMS: &str = "   ";

/// `text` in lines no longer than `width` columns, the first indented `first` columns and the
/// others `rest`. It breaks only at spaces: never inside a word or after a hyphen, nor inside
/// brackets (`[-l LABEL]`), nor inside a term of a list of terms three spaces apart unless that
/// term is longer than a line. A word longer than a line has one to itself. The spaces between
/// words on a line stay as written, so terms three apart stay three apart.
fn fill(text: &str, width: Option<usize>, first: usize, rest: usize) -> Vec<String> {
    let line = width.map(|width| width.saturating_sub(first.max(rest)));
    let (first, rest) = (" ".repeat(first), " ".repeat(rest));
    let Some(width) = width else {
        return vec![format!("{first}{text}")];
    };
    let listed = text.contains(BETWEEN_TERMS);
    let kept: Vec<String> = text
        .split(BETWEEN_TERMS)
        .map(|term| {
            let whole = listed && line.is_some_and(|line| length(term) <= line);
            let mut depth = 0_usize;
            term.chars()
                .map(|c| {
                    match c {
                        '[' => depth += 1,
                        ']' => depth = depth.saturating_sub(1),
                        ' ' if whole || depth > 0 => return KEPT,
                        _ => {}
                    }
                    c
                })
                .collect()
        })
        .collect();
    let kept = kept.join(BETWEEN_TERMS);
    let options = Options::new(width)
        .initial_indent(&first)
        .subsequent_indent(&rest)
        .break_words(false)
        .word_separator(WordSeparator::AsciiSpace)
        .word_splitter(WordSplitter::NoHyphenation);
    textwrap::wrap(&kept, options)
        .into_iter()
        .map(|line| line.replace(KEPT, " "))
        .collect()
}

/// `rows` as lines: in columns when the width leaves the descriptions [`NARROWEST`] or more,
/// else stacked.
fn table(rows: &[Row], width: Option<usize>) -> Vec<Line> {
    let headings = rows
        .iter()
        .map(|row| length(row.heading))
        .max()
        .unwrap_or(0);
    let heading_width = if headings == 0 { 0 } else { headings + 2 };
    let room = width.map(|width| width.saturating_sub(heading_width));
    // the term column fits the longest described term that fits in WIDEST_TERM and half the
    // room; a longer one has its line to itself
    let widest = room.map_or(WIDEST_TERM, |room| WIDEST_TERM.min(room / 2));
    let term_width = rows
        .iter()
        .filter(|row| !row.term.is_empty() && !row.about.is_empty())
        .map(|row| length(row.term) + 2)
        .filter(|term| *term <= widest)
        .max()
        .unwrap_or(widest);
    if room.is_some_and(|room| room < term_width + NARROWEST) {
        return stacked(rows, width, if headings == 0 { 0 } else { 2 });
    }
    let column = heading_width + term_width;
    let mut lines = Vec::new();
    for row in rows {
        if row.heading.is_empty() && row.term.is_empty() && row.about.is_empty() {
            lines.push(Line::new());
            continue;
        }
        // the row's lines after the heading column, which the first line's heading fills
        let mut text: Vec<Line> = Vec::new();
        if row.term.is_empty() {
            text.extend(plain(
                fill(row.about, width, heading_width, heading_width),
                heading_width,
            ));
        } else if row.about.is_empty() || length(row.term) + 2 > term_width {
            let term = fill(row.term, width, heading_width, heading_width + 2);
            text.extend(terms(term, heading_width));
            if !row.about.is_empty() {
                text.extend(plain(fill(row.about, width, column, column), heading_width));
            }
        } else {
            let mut about = fill(row.about, width, column, column).into_iter();
            let first = about.next().unwrap_or_default();
            text.push(
                Line::toned(Tone::Term, row.term)
                    .with(Tone::Plain, " ".repeat(term_width - length(row.term)))
                    .with(Tone::Plain, first.trim_start()),
            );
            text.extend(plain(about.collect(), heading_width));
        }
        for (index, line) in text.into_iter().enumerate() {
            let lead = if index == 0 {
                let gap = heading_width.saturating_sub(length(row.heading));
                Line::toned(Tone::Heading, row.heading).with(Tone::Plain, " ".repeat(gap))
            } else {
                Line::toned(Tone::Plain, " ".repeat(heading_width))
            };
            lines.push(lead.then(line));
        }
    }
    lines
}

/// Laid-out lines without their first `at` columns (spaces), as prose.
fn plain(lines: Vec<String>, at: usize) -> Vec<Line> {
    lines
        .into_iter()
        .map(|line| Line::from(line.get(at..).unwrap_or_default().to_owned()))
        .collect()
}

/// Laid-out lines without their first `at` columns (spaces), as terms after their indent.
fn terms(lines: Vec<String>, at: usize) -> Vec<Line> {
    lines
        .into_iter()
        .map(|line| {
            let rest = line.get(at..).unwrap_or_default();
            let term = rest.trim_start();
            Line::toned(Tone::Plain, &rest[..rest.len() - term.len()]).with(Tone::Term, term)
        })
        .collect()
}

/// `rows` one above another: a heading on its own line, a term `indent` in and its description
/// two columns further, prose `indent` in.
fn stacked(rows: &[Row], width: Option<usize>, indent: usize) -> Vec<Line> {
    let mut lines = Vec::new();
    for row in rows {
        if !row.heading.is_empty() {
            lines.push(Line::toned(Tone::Heading, row.heading));
        }
        if row.term.is_empty() && row.about.is_empty() {
            if row.heading.is_empty() {
                lines.push(Line::new());
            }
            continue;
        }
        if row.term.is_empty() {
            lines.extend(plain(fill(row.about, width, indent, indent), 0));
            continue;
        }
        lines.extend(terms(fill(row.term, width, indent, indent + 2), 0));
        if !row.about.is_empty() {
            lines.extend(plain(fill(row.about, width, indent + 2, indent + 2), 0));
        }
    }
    lines
}

#[cfg(test)]
mod tests;
