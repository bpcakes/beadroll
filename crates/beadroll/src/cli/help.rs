//! Help pages: clap renders a command's page from the table (`parse::command`), laid out at the
//! reader's width, in cargo's colours when they are shown.

use super::parse::command;
use super::spec::CommandSpec;

/// `spec`'s help page, `width` wide (none: unwrapped), coloured when `color`.
pub(crate) fn page(spec: &CommandSpec, width: Option<usize>, color: bool) -> String {
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
