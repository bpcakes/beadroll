//! Operating-system setup around `main`, where CPython supplied defaults. Record whether
//! standard input was open before the runtime can reopen a closed descriptor on `/dev/null`.
//! Ignore SIGXFSZ so writes past the file-size limit fail with `EFBIG` instead of killing bead;
//! the git runner restores its default action in children. Mark inherited descriptors
//! close-on-exec so child processes receive only standard input, output and error.
#![allow(
    unsafe_code,
    reason = "one of the two audited unsafe spots (CONTRIBUTING.md): a constructor that runs before main, the signal disposition and descriptor flags"
)]

use std::sync::atomic::{AtomicBool, Ordering};

/// Whether descriptor 0 was closed when the process started.
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

/// Ignores SIGXFSZ so a write past the file-size limit fails with `EFBIG` instead of killing
/// bead.
pub(crate) fn ignore_file_size_signal() {
    // SAFETY: setting a signal's disposition to SIG_IGN installs no handler, so no code can run in
    // a signal context; it is called once, at start-up, before bead starts any thread.
    unsafe {
        libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
    }
}

/// Marks inherited descriptors beyond standard input, output and error close-on-exec. Descriptors
/// bead opens itself already have that flag. Called once at startup, before any thread or process
/// is started.
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

/// Marks all descriptors from 3 upward close-on-exec at once where the kernel supports it (Linux
/// 5.11 and later). Returns whether it succeeded.
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

/// Descriptor numbers from 3 up to the soft limit on open files, used when open descriptors
/// cannot be listed. Without a limit, use Linux's default ceiling. No descriptor is numbered
/// beyond the soft limit.
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
