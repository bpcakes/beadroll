//! Actor text that is not UTF-8 prints `?` for each invalid byte, matches no owner (including a
//! literal `?` name), and retains its bytes as `\udcXX` in JSON errors. Fixed expected outputs
//! use `{held}` and `{free}` for issue keys and `<TIME>` for the claim time.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;

use beadroll_test_support::{Machine, parse_json};

use super::{id, json};

/// `text` with the time after each `since ` up to the `)` that closes it written `<TIME>`.
fn untimed(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("since ") {
        let (head, tail) = rest.split_at(at + "since ".len());
        out.push_str(head);
        out.push_str("<TIME>");
        rest = &tail[tail.find(')').unwrap_or(tail.len())..];
    }
    out.push_str(rest);
    out
}

/// Expected status and stderr for a command with empty stdout.
type Said = (i32, &'static str);

/// Commands run by actor `\xff`, with expected human and JSON results.
const REFUSED: [(&[&str], Said, Said); 9] = [
    (
        &["claim", "{held}"],
        (
            1,
            "Error: {held} is claimed by ? (generation 1, since <TIME>)",
        ),
        (
            1,
            r#"{"error": {"code": "ALREADY_CLAIMED", "message": "{held} is claimed by ? (generation 1, since <TIME>)"}}"#,
        ),
    ),
    (
        &["update", "{held}", "--claim"],
        (
            1,
            "Error: {held} is claimed by ? (generation 1, since <TIME>)",
        ),
        (
            1,
            r#"{"error": {"code": "ALREADY_CLAIMED", "message": "{held} is claimed by ? (generation 1, since <TIME>)"}}"#,
        ),
    ),
    (
        &["takeover", "{held}", "-r", "stalled"],
        (1, "Error: text is not valid UTF-8"),
        (
            1,
            r#"{"error": {"code": "VALIDATION", "message": "text is not valid UTF-8"}}"#,
        ),
    ),
    (
        &["release", "{held}"],
        (1, "Error: {held} is not claimed by ?; it is held by ?"),
        (
            1,
            r#"{"error": {"code": "NOT_OWNER", "message": "{held} is not claimed by \ufffd; it is held by ?"}}"#,
        ),
    ),
    (
        &["close", "{held}"],
        (
            1,
            "Error: {held} is claimed by ?: use `bead takeover` first, or --force",
        ),
        (
            1,
            r#"{"error": {"code": "CLAIMED_BY_OTHER", "message": "{held} is claimed by ?: use `bead takeover` first, or --force"}}"#,
        ),
    ),
    (
        &["delete", "{held}"],
        (
            1,
            "Error: {held} is claimed by ?: use `bead takeover` first, or --force",
        ),
        (
            1,
            r#"{"error": {"code": "CLAIMED_BY_OTHER", "message": "{held} is claimed by ?: use `bead takeover` first, or --force"}}"#,
        ),
    ),
    (
        &["update", "{held}", "--status", "blocked"],
        (
            1,
            "Error: {held} is claimed by ?: use `bead takeover` first, or --force",
        ),
        (
            1,
            r#"{"error": {"code": "CLAIMED_BY_OTHER", "message": "{held} is claimed by ?: use `bead takeover` first, or --force"}}"#,
        ),
    ),
    (
        &["claim", "{free}"],
        (1, "Error: text is not valid UTF-8"),
        (
            1,
            r#"{"error": {"code": "VALIDATION", "message": "text is not valid UTF-8"}}"#,
        ),
    ),
    (
        &["release", "{free}"],
        (1, "Error: {free} is not claimed by ?"),
        (
            1,
            r#"{"error": {"code": "NOT_OWNER", "message": "{free} is not claimed by \ufffd"}}"#,
        ),
    ),
];

#[test]
fn an_actor_that_is_not_utf8_holds_nothing() {
    let sandbox = crate::sandbox();
    let m = Machine::new(&sandbox, "who");
    json(&m, &["init", "--prefix", "ex"]);
    let held = id(&json(&m, &["create", "held by ?"]));
    let free = id(&json(&m, &["create", "held by nobody"]));
    json(&m, &["--actor", "?", "claim", &held]);
    let named = |text: &str| text.replace("{held}", &held).replace("{free}", &free);
    let before = m.export();
    for (command, human, in_json) in REFUSED {
        for ((status, stderr), json_mode) in [(human, false), (in_json, true)] {
            let mut args: Vec<Vec<u8>> = vec![b"--actor".to_vec(), b"\xff".to_vec()];
            args.extend(command.iter().map(|word| named(word).into_bytes()));
            if json_mode {
                args.push(b"--json".to_vec());
            }
            let run = m.cmd(args.iter().map(|a| OsStr::from_bytes(a))).run();
            let answer = (run.returncode(), run.stdout.as_str(), untimed(&run.stderr));
            let wanted = (status, "", format!("{}\n", named(stderr)));
            assert_eq!(answer, wanted, "{command:?}, --json {json_mode}");
        }
    }
    // `prime` is for anyone: \xff holds nothing, and `?` holds what `?` claimed
    for (actor, holds) in [
        (&b"\xff"[..], "0 held by you."),
        (b"?", "1 held by you ({held})."),
    ] {
        let tracker = format!("Tracker 'ex': 1 ready, {}", named(holds));
        let args = [
            OsStr::new("--actor"),
            OsStr::from_bytes(actor),
            OsStr::new("prime"),
        ];
        let primed = m.cmd(args).run();
        assert_eq!(primed.returncode(), 0, "{}", primed.stderr);
        for wanted in ["You are ? (machine, agent, thread).", &tracker] {
            assert!(
                primed.stdout.contains(wanted),
                "{wanted}: {}",
                primed.stdout
            );
        }
        let in_json = m.cmd(args.into_iter().chain([OsStr::new("--json")])).run();
        let document = parse_json(&in_json.stdout).expect("prime's JSON");
        let text = document["prime"].as_str().expect("the prime text");
        assert!(text.contains(&tracker), "{text}");
    }
    assert_eq!(m.export(), before, "nothing was written");
}
