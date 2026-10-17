//! Every byte bead writes to its standard streams (`CONTRIBUTING.md`). A reader that went
//! away (a closed pipe, `EPIPE`, or a terminal that hung up, `EIO` on a character device) is not
//! an error: nobody is reading, so bead stops writing and keeps its exit status.

use std::io::{self, Write as _};
use std::os::fd::AsFd;

/// One of bead's two output streams.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stream {
    Stdout,
    Stderr,
}

/// Why a block could not be written.
#[derive(Debug)]
pub(crate) enum Unwritten {
    /// Nobody is reading.
    Gone,
    /// The write failed (a full volume, say).
    Failed(io::Error),
}

/// Writes `text` and a newline to `stream` and flushes it; `None` only flushes.
pub(crate) fn emit(text: Option<&str>, stream: Stream) -> Result<(), Unwritten> {
    let written = match stream {
        Stream::Stdout => write_to(&mut io::stdout().lock(), text),
        Stream::Stderr => write_to(&mut io::stderr().lock(), text),
    };
    written.map_err(|error| hung_up(error, stream))
}

/// Writes an answer that will be acknowledged on disk. Rust's standard-output wrapper treats
/// EBADF as success; a read-only stdout descriptor must instead leave recap's bookmark intact.
/// A file over a duplicate of stdout preserves that error, while the stdout lock keeps writes
/// ordered with the usual output path. File writes are unbuffered; flushing needs no fsync.
pub(crate) fn emit_delivered(text: &str) -> Result<(), Unwritten> {
    let stdout = io::stdout();
    let mut held = stdout.lock();
    let written = held.flush().and_then(|()| {
        let fd = stdout.as_fd().try_clone_to_owned()?;
        write_to(&mut std::fs::File::from(fd), Some(text))
    });
    written.map_err(|error| hung_up(error, Stream::Stdout))
}

fn write_to(out: &mut impl io::Write, text: Option<&str>) -> io::Result<()> {
    if let Some(text) = text {
        out.write_all(text.as_bytes())?;
        out.write_all(b"\n")?;
    }
    out.flush()
}

/// Writes `data` as it is to standard output and flushes it.
pub(crate) fn emit_raw(data: &[u8]) -> Result<(), Unwritten> {
    let mut out = io::stdout().lock();
    out.write_all(data)
        .and_then(|()| out.flush())
        .map_err(|error| hung_up(error, Stream::Stdout))
}

/// A terminal that hung up fails with `EIO` where a closed pipe fails with `EPIPE`: both mean
/// nobody is reading. `EIO` from a file is a failed write and stays one.
fn hung_up(error: io::Error, stream: Stream) -> Unwritten {
    if error.kind() == io::ErrorKind::BrokenPipe {
        return Unwritten::Gone;
    }
    if error.raw_os_error() == Some(libc_eio()) && is_character_device(stream) {
        return Unwritten::Gone;
    }
    Unwritten::Failed(error)
}

fn libc_eio() -> i32 {
    rustix::io::Errno::IO.raw_os_error()
}

fn is_character_device(stream: Stream) -> bool {
    let stat = match stream {
        Stream::Stdout => rustix::fs::fstat(io::stdout().as_fd()),
        Stream::Stderr => rustix::fs::fstat(io::stderr().as_fd()),
    };
    stat.is_ok_and(|stat| {
        rustix::fs::FileType::from_raw_mode(stat.st_mode) == rustix::fs::FileType::CharacterDevice
    })
}

/// Escapes control characters in human output. Titles, comments and names come from journals and
/// imports, and must never be interpreted by the terminal. C0, DEL, C1, U+061C, U+200E/F,
/// U+202A-202E, U+2028/9 and U+2066-2069 become `\xNN` or `\uNNNN`.
pub(crate) fn visible(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let code = u32::from(c);
        let control = code <= 0x1f
            || (0x7f..=0x9f).contains(&code)
            || matches!(code, 0x061c | 0x2028 | 0x2029 | 0x200e | 0x200f)
            || (0x202a..=0x202e).contains(&code)
            || (0x2066..=0x2069).contains(&code);
        if !control {
            out.push(c);
        } else if code < 0x100 {
            out.push_str(&format!("\\x{code:02x}"));
        } else {
            out.push_str(&format!("\\u{code:04x}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_print_as_visible_escapes() {
        assert_eq!(
            visible("a\tb\u{1b}[31m\u{9b}\u{202e}é"),
            "a\\x09b\\x1b[31m\\x9b\\u202eé"
        );
        assert_eq!(visible("\u{2028}\u{61c}\u{7f}"), "\\u2028\\u061c\\x7f");
    }
}
