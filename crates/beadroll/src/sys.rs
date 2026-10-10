//! What bead needs from the operating system around `main`, where CPython supplied it (the first of
//! the two audited unsafe spots, `docs/RUST-PORT.md` §2.2 and §2.3f): whether standard input was
//! open when the process started, which Rust's runtime hides by reopening a closed descriptor on
//! `/dev/null` before `main` runs, and ignoring SIGXFSZ, so that a write past the file-size limit
//! fails with `EFBIG` (an `IO` error) instead of killing bead. The git runner gives children the
//! default action back.
#![allow(
    unsafe_code,
    reason = "one of the two audited unsafe spots (docs/RUST-PORT.md §2.2): a constructor that runs before main, and the signal disposition"
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
