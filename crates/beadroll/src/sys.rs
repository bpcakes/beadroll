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
    if mark_all_from_three() {
        return;
    }
    let numbers = listed(&["/dev/fd", "/proc/self/fd"]).unwrap_or_else(|| {
        let limit = rustix::process::getrlimit(rustix::process::Resource::Nofile).current;
        every_number(limit).collect()
    });
    mark(numbers);
}

/// Marks every descriptor from 3 up close-on-exec at once, where the kernel can (Linux 5.11 and
/// later, as CPython's `close_range` closes them): whether it did.
fn mark_all_from_three() -> bool {
    cfg_select! {
        target_os = "linux" => {
            // SAFETY: close_range with CLOSE_RANGE_CLOEXEC only sets the close-on-exec flag of the
            // open descriptors in the range; a kernel without it fails with ENOSYS or EINVAL and
            // changes nothing.
            let done = unsafe {
                libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, libc::CLOSE_RANGE_CLOEXEC)
            };
            done == 0
        }
        _ => false,
    }
}

/// The descriptor numbers the first readable of `dirs` lists (its own descriptor among them, closed
/// again by the time they are marked).
fn listed(dirs: &[&str]) -> Option<Vec<i32>> {
    dirs.iter().find_map(|dir| {
        let entries = std::fs::read_dir(dir).ok()?;
        let numbers = entries
            .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
            .collect();
        Some(numbers)
    })
}

/// Every descriptor number from 3 below `limit`, the soft limit on open files (CPython's
/// `sysconf(_SC_OPEN_MAX)`), when the open ones cannot be listed; with none, Linux's default
/// ceiling on descriptors. No descriptor is numbered beyond the soft limit.
fn every_number(limit: Option<u64>) -> std::ops::Range<i32> {
    let limit = limit.unwrap_or(1 << 20);
    3..i32::try_from(limit).unwrap_or(i32::MAX)
}

/// Marks each of `numbers` from 3 up close-on-exec.
fn mark(numbers: impl IntoIterator<Item = i32>) {
    for fd in numbers.into_iter().filter(|fd| *fd > 2) {
        // SAFETY: fcntl with F_GETFD and F_SETFD only reads and sets a descriptor's flags; on a
        // number that is not open it fails with EBADF and changes nothing.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags >= 0 && flags & libc::FD_CLOEXEC == 0 {
                libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsRawFd as _;

    use super::*;

    /// Without a listing, every number below the soft limit is marked, however high: 70,000 under
    /// a limit of 1,048,576.
    #[test]
    fn without_a_listing_every_number_below_the_limit_is_tried() {
        let numbers = every_number(Some(1_048_576));
        assert_eq!((numbers.start, numbers.end), (3, 1_048_576));
        assert!(numbers.contains(&70_000));
        assert!(every_number(None).contains(&70_000));
    }

    /// The fallback marks a descriptor no listing named: one well above those a test process
    /// holds, below every default soft limit.
    #[test]
    fn the_fallback_marks_a_high_numbered_descriptor() {
        let null = std::fs::File::open("/dev/null").unwrap();
        let high = 200;
        // SAFETY: dup2 onto a number this test owns; it is closed again below.
        let duplicated = unsafe { libc::dup2(null.as_raw_fd(), high) };
        assert_eq!(duplicated, high);
        // SAFETY: fcntl F_GETFD only reads the flags of the descriptor just made.
        let before = unsafe { libc::fcntl(high, libc::F_GETFD) };
        assert_eq!(
            before & libc::FD_CLOEXEC,
            0,
            "dup2 leaves the copy inheritable"
        );
        let none: Option<Vec<i32>> = listed(&["/nonexistent/fd"]);
        assert!(none.is_none());
        mark(every_number(Some(256)));
        // SAFETY: as above.
        let after = unsafe { libc::fcntl(high, libc::F_GETFD) };
        // SAFETY: closes the copy made above.
        unsafe {
            libc::close(high);
        }
        assert_ne!(after & libc::FD_CLOEXEC, 0);
    }
}
