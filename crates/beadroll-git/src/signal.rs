//! Signal names as Python's `signal.Signals(n).name` gives them on this platform, for the `killed by
//! SIGNAME` git errors bead reports.

/// The name Python gives signal `number` here, or `signal N` for one it has no name for.
pub fn signal_name(number: i32) -> String {
    named(number).map_or_else(|| format!("signal {number}"), str::to_owned)
}

/// Python's `Signals` members, the first name by value then name where a number has several
/// (`SIGABRT` before `SIGIOT`, `SIGIO` before `SIGPOLL`, `SIGCHLD` before `SIGCLD`).
fn named(number: i32) -> Option<&'static str> {
    let common = [
        (libc::SIGHUP, "SIGHUP"),
        (libc::SIGINT, "SIGINT"),
        (libc::SIGQUIT, "SIGQUIT"),
        (libc::SIGILL, "SIGILL"),
        (libc::SIGTRAP, "SIGTRAP"),
        (libc::SIGABRT, "SIGABRT"),
        (libc::SIGBUS, "SIGBUS"),
        (libc::SIGFPE, "SIGFPE"),
        (libc::SIGKILL, "SIGKILL"),
        (libc::SIGUSR1, "SIGUSR1"),
        (libc::SIGSEGV, "SIGSEGV"),
        (libc::SIGUSR2, "SIGUSR2"),
        (libc::SIGPIPE, "SIGPIPE"),
        (libc::SIGALRM, "SIGALRM"),
        (libc::SIGTERM, "SIGTERM"),
        (libc::SIGCHLD, "SIGCHLD"),
        (libc::SIGCONT, "SIGCONT"),
        (libc::SIGSTOP, "SIGSTOP"),
        (libc::SIGTSTP, "SIGTSTP"),
        (libc::SIGTTIN, "SIGTTIN"),
        (libc::SIGTTOU, "SIGTTOU"),
        (libc::SIGURG, "SIGURG"),
        (libc::SIGXCPU, "SIGXCPU"),
        (libc::SIGXFSZ, "SIGXFSZ"),
        (libc::SIGVTALRM, "SIGVTALRM"),
        (libc::SIGPROF, "SIGPROF"),
        (libc::SIGWINCH, "SIGWINCH"),
        (libc::SIGIO, "SIGIO"),
        (libc::SIGSYS, "SIGSYS"),
    ];
    if let Some((_, name)) = common.iter().find(|(value, _)| *value == number) {
        return Some(name);
    }
    platform(number)
}

#[cfg(target_os = "linux")]
fn platform(number: i32) -> Option<&'static str> {
    match number {
        libc::SIGSTKFLT => Some("SIGSTKFLT"),
        libc::SIGPWR => Some("SIGPWR"),
        _ if number == libc::SIGRTMIN() => Some("SIGRTMIN"),
        _ if number == libc::SIGRTMAX() => Some("SIGRTMAX"),
        _ => None,
    }
}

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
fn platform(number: i32) -> Option<&'static str> {
    match number {
        libc::SIGEMT => Some("SIGEMT"),
        libc::SIGINFO => Some("SIGINFO"),
        _ => None,
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd"
)))]
fn platform(_number: i32) -> Option<&'static str> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signals_read_as_python_names_them() {
        assert_eq!(signal_name(libc::SIGKILL), "SIGKILL");
        assert_eq!(signal_name(libc::SIGXFSZ), "SIGXFSZ");
        assert_eq!(signal_name(libc::SIGABRT), "SIGABRT");
        assert_eq!(signal_name(libc::SIGIO), "SIGIO");
        assert_eq!(signal_name(0), "signal 0");
        assert_eq!(signal_name(100), "signal 100");
    }
}
