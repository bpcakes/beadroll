//! What bead needs from the operating system around `main`, where CPython supplied it (the first of
//! the two audited unsafe spots, `docs/RUST-PORT.md` §2.2 and §2.3f): whether standard input was
//! open when the process started, which Rust's runtime hides by reopening a closed descriptor on
//! `/dev/null` before `main` runs; ignoring SIGXFSZ, so that a write past the file-size limit
//! fails with `EFBIG` (an `IO` error) instead of killing bead (the git runner gives children the
//! default action back); and keeping the descriptors bead was started with from the processes it
//! starts, as Python's `subprocess` does with `close_fds=True`.
#![allow(
    unsafe_code,
    reason = "one of the two audited unsafe spots (docs/RUST-PORT.md §2.2): a constructor that runs before main, the signal disposition and descriptor flags"
)]

use std::sync::atomic::{AtomicBool, Ordering};

/// Whether descriptor 0 was closed when the process started (Python's `sys.stdin is None`).
static STDIN_CLOSED: AtomicBool = AtomicBool::new(false);

/// Records whether descriptor 0 is open. It runs as a constructor, before `main` and before the
/// runtime reopens closed standard descriptors; the loader passes it arguments it does not read.
extern "C" fn probe_stdin() {
    // SAFETY: fcntl with F_GETFD only reads the flags of a descriptor number, and fails with EBADF
    // when it is not open; it touches no memory and is safe to call before main.
    let closed = unsafe { libc::fcntl(0, libc::F_GETFD) } == -1;
    STDIN_CLOSED.store(closed, Ordering::Relaxed);
}

cfg_select! {
    target_vendor = "apple" => {
        /// The probe, among the constructors dyld runs before `main`.
        #[used]
        #[unsafe(link_section = "__DATA,__mod_init_func")]
        static PROBE: extern "C" fn() = probe_stdin;
    }
    _ => {
        /// The probe, among the constructors the loader runs before `main`.
        #[used]
        #[unsafe(link_section = ".init_array")]
        static PROBE: extern "C" fn() = probe_stdin;
    }
}

/// Whether standard input was closed when bead started: `-` then names nothing to read.
pub(crate) fn stdin_closed() -> bool {
    STDIN_CLOSED.load(Ordering::Relaxed)
}

/// Ignores SIGXFSZ, as CPython does at start-up: a write past the file-size limit then fails with
/// `EFBIG` instead of killing bead.
pub(crate) fn ignore_file_size_signal() {
    // SAFETY: setting a signal's disposition to SIG_IGN installs no handler, so no code can run in
    // a signal context; it is called once, at start-up, before bead starts any thread.
    unsafe {
        libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
    }
}

/// Marks every descriptor bead was started with, beyond standard input, output and error,
/// close-on-exec, so that no process bead starts inherits one: Python starts git and `ps` with
/// `close_fds=True`, which leaves them only the standard three. Descriptors bead opens itself are
/// close-on-exec already. Called once, at start-up, before bead starts any thread or process.
pub(crate) fn keep_inherited_descriptors_from_children() {
    let listed = ["/dev/fd", "/proc/self/fd"].iter().find_map(|dir| {
        let entries = std::fs::read_dir(dir).ok()?;
        let numbers: Vec<i32> = entries
            .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
            .collect();
        Some(numbers)
    });
    // the listing's own descriptor is among the numbers, closed again by now: it is passed over
    let numbers = listed.unwrap_or_else(|| {
        // no listing to read: every number below the limit on open files
        let limit = rustix::process::getrlimit(rustix::process::Resource::Nofile)
            .current
            .unwrap_or(1024)
            .min(65_536);
        (3..i32::try_from(limit).unwrap_or(65_536)).collect()
    });
    for fd in numbers.into_iter().filter(|fd| *fd > 2) {
        // SAFETY: fcntl with F_GETFD and F_SETFD only reads and sets a descriptor's flags; on a
        // number that is not open it fails with EBADF and changes nothing. No other thread runs yet.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags >= 0 && flags & libc::FD_CLOEXEC == 0 {
                libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
            }
        }
    }
}
