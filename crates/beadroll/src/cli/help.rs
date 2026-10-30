//! Help pages: clap renders a command's page from the table (`parse::command`), laid out at the
//! reader's width, in cargo's colours when they are shown.

use super::parse::command;
use super::spec::CommandSpec;
use crate::cli::paint::Color;

/// `spec`'s help page, `width` wide (none: unwrapped), coloured when `color`.
pub(crate) fn page(spec: &CommandSpec, width: Option<usize>, color: Color) -> String {
    let color = matches!(color, Color::Ansi);
    let mut root = command(width);
    root.build();
    let rendered = match root.find_subcommand_mut(spec.name) {
        Some(sub) => sub.render_help(),
        None => root.render_help(),
    };
    if color {
        rendered.ansi().to_string()
    } else {
        rendered.to_string()
    }
}

#[cfg(test)]
mod tests;
