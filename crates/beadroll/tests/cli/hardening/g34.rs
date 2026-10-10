//! G34: what git and `ps` print is not always UTF-8: a branch, an origin URL, origin/HEAD's target,
//! a folder of the published journal or a parent process's arguments named in Latin-1 bytes. Every
//! command reads them without crashing.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io::Write as _;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;
use std::process::{Command, Stdio};

/// `_g34_tail`: the last line of `text` that is not blank, or nothing.
fn tail(text: &str) -> &str {
    let mut lines = text.trim().split('\n');
    lines.rfind(|line| !line.trim().is_empty()).unwrap_or("")
}

/// Whether `byte` is ASCII whitespace as Python's `bytes.strip()` and `bytes.split()` take it.
fn is_space(byte: &u8) -> bool {
    b" \t\n\r\x0b\x0c".contains(byte)
}

/// `data` without the ASCII whitespace at either end, as `bytes.strip()` leaves it.
fn strip(data: &[u8]) -> &[u8] {
    let start = data.iter().position(|byte| !is_space(byte));
    let start = start.unwrap_or(data.len());
    let end = data.iter().rposition(|byte| !is_space(byte));
    &data[start..end.map_or(start, |at| at + 1)]
}

/// `_g34_git`: git with byte arguments allowed, in `cwd`, with exactly the environment `env` when
/// one is given, `data` on its standard input; its standard output, stripped. A failure panics,
/// as `check=True` raises.
fn git_bytes(
    args: &[&[u8]],
    cwd: &Path,
    env: Option<&BTreeMap<OsString, OsString>>,
    data: Option<&[u8]>,
) -> Vec<u8> {
    let mut command = Command::new("git");
    command
        .args(args.iter().map(|arg| OsStr::from_bytes(arg)))
        .current_dir(cwd)
        .stdin(if data.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(env) = env {
        command.env_clear().envs(env);
    }
    let mut child = command.spawn().expect("git starts");
    if let (Some(mut input), Some(data)) = (child.stdin.take(), data) {
        input.write_all(data).expect("git reads its input");
    }
    let output = child.wait_with_output().expect("git's output");
    assert!(
        output.status.success(),
        "{command:?} failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    strip(&output.stdout).to_vec()
}

/// What follows the first space of a `packed-refs` row: the name of its ref.
fn ref_name(row: &[u8]) -> &[u8] {
    let space = row.iter().position(|byte| *byte == b' ');
    &row[space.expect("a packed ref: an object and a name") + 1..]
}

/// `_g34_add_packed_ref`: a ref whose name is not UTF-8. APFS refuses such file names, so it goes
/// into packed-refs.
fn add_packed_ref(checkout: &Path, name: &[u8], sha: &[u8]) {
    let path = checkout.join(".git").join("packed-refs");
    let old = if path.exists() {
        std::fs::read(&path).unwrap()
    } else {
        Vec::new()
    };
    let mut rows: Vec<Vec<u8>> = old
        .split(|byte| *byte == b'\n')
        .filter(|row| !row.is_empty() && !row.starts_with(b"#"))
        .map(<[u8]>::to_vec)
        .collect();
    rows.push([sha, b" ", name].concat());
    rows.sort_by(|a, b| ref_name(a).cmp(ref_name(b)));
    let mut text = b"# pack-refs with: peeled fully-peeled sorted \n".to_vec();
    text.extend(rows.join(&b'\n'));
    text.push(b'\n');
    std::fs::write(&path, text).unwrap();
}

/// `items`, each followed by a NUL byte, then `last`: the input `git mktree -z` reads.
fn nul_terminated(items: &[&[u8]], last: &[u8]) -> Vec<u8> {
    let mut data = Vec::new();
    for item in items {
        data.extend_from_slice(item);
        data.push(0);
    }
    data.extend_from_slice(last);
    data
}

mod g34_non_utf8_git_and_ps_output_is_not_a_traceback {
    use std::collections::BTreeMap;
    use std::ffi::{OsStr, OsString};
    use std::os::unix::ffi::OsStrExt as _;
    use std::process::{Command, Output};

    use beadroll_test_support::{Machine, Run, Sandbox, parse_json};

    use super::{add_packed_ref, git_bytes, is_space, nul_terminated, tail};

    /// `output` as a [`Run`], decoded as `text=True` decodes it.
    fn finished(output: Output) -> Run {
        Run {
            status: output.status,
            stdout: String::from_utf8(output.stdout).expect("bead's output is UTF-8"),
            stderr: String::from_utf8(output.stderr).expect("bead's errors are UTF-8"),
        }
    }

    /// `assert_no_traceback`: `proc` did not crash.
    fn assert_no_traceback(proc: &Run, what: &str) {
        assert!(!proc.crashed(), "{what} crashed: {}", tail(&proc.stderr));
    }

    /// G34: `capture_where` strictly decodes `rev-parse --abbrev-ref HEAD`, so `create` on a
    /// branch named with Latin-1 bytes is a UnicodeDecodeError traceback; it must work (the branch
    /// is not recorded).
    #[test]
    fn create_on_a_branch_whose_name_is_not_utf8() {
        let sandbox = crate::sandbox();
        let m = Machine::new(&sandbox, "g34a");
        m.bead(["init", "--prefix", "ex"]);
        let sha = git_bytes(&[b"rev-parse", b"HEAD"], m.checkout(), None, None);
        add_packed_ref(m.checkout(), b"refs/heads/caf\xe9", &sha);
        let head: &[&[u8]] = &[b"symbolic-ref", b"HEAD", b"refs/heads/caf\xe9"];
        git_bytes(head, m.checkout(), None, None);
        let abbrev: &[&[u8]] = &[b"rev-parse", b"--abbrev-ref", b"HEAD"];
        assert_eq!(git_bytes(abbrev, m.checkout(), None, None), b"caf\xe9");
        let proc = m.run(["create", "on a latin-1 branch", "--json"]);
        assert_no_traceback(&proc, "create on branch caf\\xe9");
        assert_eq!(proc.returncode(), 0, "{}", proc.stderr);
        let created = parse_json(&proc.stdout).unwrap();
        let shown = m.show(created["id"].as_str().expect("the new issue's id"));
        let issue = shown.as_object().expect("an issue");
        assert!(!issue.contains_key("branch"), "{shown:?}");
    }

    /// G34: `find_workspace` strictly decodes remote.origin.url, and help/prime catch only
    /// BeadError, so a Latin-1 byte in the origin URL makes every command in the checkout a
    /// traceback, help included.
    #[test]
    fn every_command_with_an_origin_url_that_is_not_utf8() {
        let sandbox = crate::sandbox();
        let m = Machine::new(&sandbox, "g34b");
        let url: &[&[u8]] = &[
            b"config",
            b"remote.origin.url",
            b"https://example.invalid/caf\xe9.git",
        ];
        git_bytes(url, m.checkout(), None, None);
        let commands: [&[&str]; 5] = [
            &["help"],
            &["prime"],
            &["init", "--prefix", "ex"],
            &["create", "x"],
            &["info"],
        ];
        for args in commands {
            let proc = m.run(args.iter().chain(&["--json"]));
            assert_no_traceback(&proc, &format!("bead {}", args[0]));
            assert_eq!(proc.returncode(), 0, "bead {}: {}", args[0], proc.stderr);
        }
        let info = m.bead(["info"]).unwrap();
        assert_eq!(
            (info["workspace_source"].as_str(), info["name"].as_str()),
            (Some("origin url"), Some("caf\u{fffd}")),
            "{info:?}"
        );
    }

    /// G34: `merged_into_default` strictly decodes the origin/HEAD target, so `show` of a closed
    /// issue is a traceback when origin/HEAD names a branch that is not UTF-8.
    #[test]
    fn show_closed_issue_when_origin_head_names_a_latin1_branch() {
        let sandbox = crate::sandbox();
        let m = Machine::new(&sandbox, "g34c");
        m.bead(["init", "--prefix", "ex"]);
        let created = m.bead(["create", "x"]).unwrap();
        let key = created["id"].as_str().expect("the new issue's id");
        m.bead(["close", key]);
        let head: &[&[u8]] = &[
            b"symbolic-ref",
            b"refs/remotes/origin/HEAD",
            b"refs/remotes/origin/caf\xe9",
        ];
        git_bytes(head, m.checkout(), None, None);
        let proc = m.run(["show", key, "--json"]);
        assert_no_traceback(&proc, "show of a closed issue");
        assert_eq!(proc.returncode(), 0, "{}", proc.stderr);
    }

    /// `_plant_foreign_folder`: adds `events/caf\xe9/1.jsonl` to the published coordination ref
    /// on the shared bare remote.
    fn plant_foreign_folder(sandbox: &Sandbox) {
        let mut env: BTreeMap<OsString, OsString> = std::env::vars_os()
            .filter(|(key, _)| !key.as_encoded_bytes().starts_with(b"GIT_"))
            .collect();
        env.insert("GIT_DIR".into(), sandbox.origin().into());
        for (key, value) in [
            ("GIT_AUTHOR_NAME", "x"),
            ("GIT_AUTHOR_EMAIL", "x@example.invalid"),
            ("GIT_COMMITTER_NAME", "x"),
            ("GIT_COMMITTER_EMAIL", "x@example.invalid"),
        ] {
            env.insert(key.into(), value.into());
        }
        let root = sandbox.root();
        let git = |args: &[&[u8]], data: Option<&[u8]>| git_bytes(args, root, Some(&env), data);
        let tip = git(&[b"rev-parse", b"refs/beads/journal"], None);
        let tip_tree = [tip.as_slice(), b"^{tree}"].concat();
        let top = git(&[b"ls-tree", b"-z", &tip_tree], None);
        let top: Vec<&[u8]> = top.split(|byte| *byte == 0).collect();
        // r.split(b"\t")[0].split()[2] of the row that ends with "\tevents": its tree
        let events_row = top.iter().find(|row| row.ends_with(b"\tevents"));
        let events_row = events_row.expect("the journal's events folder");
        let head = events_row.split(|byte| *byte == b'\t').next().unwrap();
        let mut words = head.split(is_space).filter(|word| !word.is_empty());
        let events_sha = words.nth(2).expect("the events folder's tree");
        let blob = git(&[b"hash-object", b"-w", b"--stdin"], Some(b""));
        let sub_entry: [&[u8]; 3] = [b"100644 blob ", &blob, b"\t1.jsonl\0"];
        let sub = git(&[b"mktree", b"-z"], Some(&sub_entry.concat()));
        let old = git(&[b"ls-tree", b"-z", events_sha], None);
        let old: Vec<&[u8]> = old
            .split(|byte| *byte == 0)
            .filter(|row| !row.is_empty())
            .collect();
        let folder: [&[u8]; 3] = [b"040000 tree ", &sub, b"\tcaf\xe9\0"];
        let listing = nul_terminated(&old, &folder.concat());
        let events = git(&[b"mktree", b"-z"], Some(&listing));
        let rows: Vec<&[u8]> = top
            .iter()
            .copied()
            .filter(|row| !row.is_empty() && !row.ends_with(b"\tevents"))
            .collect();
        let folder: [&[u8]; 3] = [b"040000 tree ", &events, b"\tevents\0"];
        let listing = nul_terminated(&rows, &folder.concat());
        let tree = git(&[b"mktree", b"-z"], Some(&listing));
        let commit: [&[u8]; 6] = [b"commit-tree", &tree, b"-p", &tip, b"-m", b"foreign"];
        let commit = git(&commit, None);
        let update: [&[u8]; 4] = [b"update-ref", b"refs/beads/journal", &commit, &tip];
        git(&update, None);
    }

    /// G34: `Link.merge` decodes `ls-tree -r` strictly (`git_text`); with core.quotePath=false git
    /// prints a non-UTF-8 folder name raw and `sync` is a traceback. What merge sees must not
    /// depend on quotePath.
    #[test]
    fn sync_with_a_foreign_folder_named_in_latin1_and_quotepath_false() {
        let sandbox = crate::sandbox();
        let m = Machine::new(&sandbox, "g34d");
        m.bead(["init", "--prefix", "ex"]);
        m.bead(["create", "x"]);
        m.bead(["sync"]);
        plant_foreign_folder(&sandbox);
        let quote = sandbox.root().join("quote.gitconfig");
        std::fs::write(&quote, "[core]\n\tquotePath = false\n").unwrap();
        let empty = sandbox.root().join("empty.gitconfig");
        std::fs::write(&empty, "").unwrap();
        // quotePath=false first: a default-config sync would skip the quoted path and publish a
        // tree without it
        let proc = m
            .cmd(["sync", "--json"])
            .env("GIT_CONFIG_GLOBAL", &quote)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .run();
        assert_no_traceback(&proc, "sync with core.quotePath=false");
        assert_eq!(proc.returncode(), 0, "{}", proc.stderr);
        let control = m
            .cmd(["sync", "--json"])
            .env("GIT_CONFIG_GLOBAL", &empty)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .run();
        assert_no_traceback(&control, "sync with the default quotePath");
        assert_eq!(control.returncode(), 0, "{}", control.stderr);
    }

    /// G34: `nearest_harness` reads `ps` with `text=True` (strict decode); with both
    /// CLAUDE_CODE_SESSION_ID and CODEX_THREAD_ID set, a parent process whose argv holds Latin-1
    /// bytes makes even `help` a traceback.
    #[test]
    fn nested_harness_whose_parent_argv_is_not_utf8() {
        let sandbox = crate::sandbox();
        let m = Machine::new(&sandbox, "g34e");
        m.bead(["init", "--prefix", "ex"]);
        let mut env = m.env(None, "codex");
        for name in ["BEAD_AGENT", "BEAD_THREAD"] {
            assert!(env.remove(OsStr::new(name)).is_some(), "{name} is set");
        }
        env.insert(
            "CLAUDE_CODE_SESSION_ID".into(),
            "claude-session-0001".into(),
        );
        env.insert("CODEX_THREAD_ID".into(), "codex-thread-0001".into());
        // `; exit $?` stops the shell from exec-ing bead, so that the shell stays its parent, and
        // hands on bead's own status, which the checks below read
        let script = r#""$0" "$1" --json; exit $?"#;
        // the ASCII control first
        for argument in [b"resume.txt".as_slice(), b"r\xe9sum\xe9.txt"] {
            let output = Command::new("sh")
                .arg("-c")
                .arg(script)
                .arg(m.executable())
                .arg("help")
                .arg(OsStr::from_bytes(argument))
                .current_dir(m.checkout())
                .env_clear()
                .envs(&env)
                .output()
                .expect("sh starts");
            let proc = finished(output);
            let what = format!(
                "help under a parent whose argv holds {}",
                argument.escape_ascii()
            );
            assert_no_traceback(&proc, &what);
            assert_eq!(proc.returncode(), 0, "{what}: {}", proc.stderr);
        }
    }
}
