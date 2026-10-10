//! An actor whose text is not UTF-8 (`docs/REVIEWING.md` §2; V3 in `test_hardening`): Python holds
//! its bytes as lone surrogates, so it prints as `?` for each but is no owner's, not even one whose
//! name is a literal `?`, and an error that names it keeps the bytes (`\udcXX` in JSON). Either
//! binary answers as Python does.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;
use std::process::Command;

use beadroll_test_support::Machine;

use super::{id, json_as, python, rust};

#[test]
fn an_actor_that_is_not_utf8_holds_nothing_as_in_python() {
    let sandbox = crate::sandbox();
    let (py, rs) = (python(), rust());
    let m = Machine::new(&sandbox, "who");
    json_as(&py, &m, &["init", "--prefix", "ex"]);
    let held = id(&json_as(&py, &m, &["create", "held by ?"]));
    let free = id(&json_as(&py, &m, &["create", "held by nobody"]));
    json_as(&py, &m, &["--actor", "?", "claim", &held]);
    let ask = |bead: &Path, args: &[&[u8]]| {
        let args: Vec<&OsStr> = args.iter().map(|a| OsStr::from_bytes(a)).collect();
        let out = Command::new(bead)
            .args(&args)
            .current_dir(m.checkout())
            .env_clear()
            .envs(m.env(None, "codex"))
            .output()
            .unwrap();
        (out.status.code(), out.stdout, out.stderr)
    };
    let before = ask(&py, &[b"export"]);
    let (held, free) = (held.as_bytes(), free.as_bytes());
    for (actor, command) in [
        (&b"\xff"[..], &[&b"claim"[..], held][..]),
        (b"\xff", &[b"update", held, b"--claim"]),
        (b"\xff", &[b"takeover", held, b"-r", b"stalled"]),
        (b"\xff", &[b"release", held]),
        (b"\xff", &[b"close", held]),
        (b"\xff", &[b"delete", held]),
        (b"\xff", &[b"update", held, b"--status", b"blocked"]),
        (b"\xff", &[b"claim", free]),
        (b"\xff", &[b"release", free]),
        (b"\xff", &[b"prime"]),
        (b"?", &[b"prime"]),
    ] {
        for json in [false, true] {
            let mut args = vec![&b"--actor"[..], actor];
            args.extend_from_slice(command);
            if json {
                args.push(b"--json");
            }
            assert_eq!(ask(&rs, &args), ask(&py, &args), "{args:?}");
        }
    }
    assert_eq!(ask(&rs, &[b"export"]), before, "nothing was written");
}
