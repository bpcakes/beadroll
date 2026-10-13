//! Child processes inherit only standard input, output and error. These tests also pin which git
//! processes bead starts, their arguments and timing, and the equivalence of in-process object
//! reads to git's answers (`crates/beadroll-git/AGENTS.md`).

use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::process::Command;

use beadroll_test_support::{Json, Machine, git, parse_json};

use crate::hardening::gate::{Gate, beside};

/// The git on the test's PATH.
fn real_git() -> PathBuf {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|dir| dir.join("git"))
        .find(|git| git.is_file())
        .expect("git on PATH")
}

/// `text` quoted for the shell.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// bead started with descriptor 7 open, as a caller's pipe or a make jobserver would be, writes
/// through git; a git first on PATH notes, each time bead starts it, whether descriptor 7 reached
/// it, then runs the real one.
#[test]
fn git_inherits_no_descriptor_beyond_the_standard_three() {
    let sandbox = crate::sandbox();
    let m = Machine::new(&sandbox, "fds");
    m.bead(["init", "--prefix", "ex"]);
    let shims = sandbox.root().join("shims");
    std::fs::create_dir(&shims).unwrap();
    let log = sandbox.root().join("descriptor-7.log");
    let script = format!(
        // the probe runs in a subshell: dash leaves the whole script when a redirection of `:` fails
        "#!/bin/sh\nif ( : >&7 ) 2>/dev/null; then echo inherited >> {log}; else echo closed >> \
         {log}; fi\nexec {git} \"$@\"\n",
        log = quoted(&log.display().to_string()),
        git = quoted(&real_git().display().to_string()),
    );
    std::fs::write(shims.join("git"), script).unwrap();
    std::fs::set_permissions(shims.join("git"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut env = m.env(None, "codex");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dirs = std::iter::once(shims).chain(std::env::split_paths(&path));
    env.insert("PATH".into(), std::env::join_paths(dirs).unwrap());
    let bead = quoted(&m.executable().display().to_string());
    let out = Command::new("/bin/sh")
        .args([
            "-c",
            &format!("exec 7</dev/null; exec {bead} create probe --json"),
        ])
        .current_dir(m.checkout())
        .env_clear()
        .envs(env)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let seen = std::fs::read_to_string(&log).unwrap_or_default();
    let seen: Vec<&str> = seen.lines().collect();
    assert!(!seen.is_empty(), "bead started no git through the shim");
    assert!(seen.iter().all(|s| *s == "closed"), "{seen:?}");
}

/// Pins the number and arguments of git invocations. A read asks for checkout facts, origin
/// configuration and committed configuration. Sync adds transport settings, fetch, one ref
/// transaction and persistent object readers and writers. Publishing adds the transaction's HEAD
/// lookup, commit and push, reusing the ref writer. A git wrapper records each invocation, catching
/// accidental fallback to one process per question.
#[test]
fn a_command_starts_the_consolidated_gits_and_no_others() {
    let sandbox = crate::sandbox();
    let m = crate::rust_only(&sandbox, "counted");
    m.bead(["init", "--prefix", "ex"]);
    let created = m.bead(["create", "x"]).unwrap();
    let key = created["id"].as_str().unwrap().to_owned();
    m.bead(["sync"]);
    let shims = sandbox.root().join("counting-shims");
    std::fs::create_dir(&shims).unwrap();
    let log = sandbox.root().join("gits.log");
    let script = format!(
        "#!/bin/sh\necho \"$1\" >> {log}\nexec {git} \"$@\"\n",
        log = quoted(&log.display().to_string()),
        git = quoted(&real_git().display().to_string()),
    );
    std::fs::write(shims.join("git"), script).unwrap();
    std::fs::set_permissions(shims.join("git"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(std::iter::once(shims).chain(std::env::split_paths(&path)));
    let path = path.unwrap();
    let started = |args: &[&str]| -> Vec<String> {
        std::fs::write(&log, "").unwrap();
        let run = m.cmd(args).env("PATH", &path).run();
        assert_eq!(run.returncode(), 0, "{args:?}: {}", run.stderr);
        let mut seen: Vec<String> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        seen.sort();
        seen
    };
    let read = ["cat-file", "config", "rev-parse"];
    let sync = [
        "cat-file",
        "cat-file",
        "config",
        "config",
        "fetch",
        "hash-object",
        "mktree",
        "rev-parse",
        "update-ref",
    ];
    let write = [
        "cat-file",
        "cat-file",
        "commit-tree",
        "config",
        "config",
        "fetch",
        "hash-object",
        "mktree",
        "push",
        "rev-parse",
        "rev-parse",
        "update-ref",
    ];
    assert_eq!(started(&["show", &key, "--json"]), read);
    assert_eq!(started(&["info", "--json"]), read);
    assert_eq!(started(&["sync", "--json"]), sync);
    assert_eq!(started(&["create", "y", "--json"]), write);
    assert_eq!(started(&["claim", &key, "--json"]), write);
}

/// After fetch, keep the fetched head and delete the private fetch ref in one transaction. The
/// wrapper records both the invocation and its input; G16 checks the concurrent fetch retries.
#[test]
fn the_refs_a_fetch_leaves_are_set_in_one_transaction() {
    let sandbox = crate::sandbox();
    let m = crate::rust_only(&sandbox, "refs");
    m.bead(["init", "--prefix", "ex"]);
    m.bead(["create", "x"]);
    let shims = sandbox.root().join("update-ref-shims");
    std::fs::create_dir(&shims).unwrap();
    let log = sandbox.root().join("update-refs.log");
    let input = sandbox.root().join("update-refs-input.log");
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = update-ref ]; then\n  echo \"$*\" >> {log}\n  \
         tee -a {input} | {git} \"$@\"\n  exit $?\nfi\nexec {git} \"$@\"\n",
        log = quoted(&log.display().to_string()),
        input = quoted(&input.display().to_string()),
        git = quoted(&real_git().display().to_string()),
    );
    std::fs::write(shims.join("git"), script).unwrap();
    std::fs::set_permissions(shims.join("git"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(std::iter::once(shims).chain(std::env::split_paths(&path)));
    let run = m.cmd(["sync", "--json"]).env("PATH", path.unwrap()).run();
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    let logged = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(logged, "update-ref --stdin\n");
    let input = std::fs::read_to_string(&input).unwrap();
    let lines: Vec<&str> = input.lines().collect();
    let ["start", remote, fetched, "commit"] = lines.as_slice() else {
        panic!("not one update-ref transaction: {input}");
    };
    let remote: Vec<&str> = remote.split(' ').collect();
    let tip = remote.get(2).copied().unwrap_or_default();
    assert_eq!(remote[..2], ["update", "refs/beads/remote"], "{input}");
    assert!(
        tip.len() == 40 && tip.bytes().all(|b| b.is_ascii_hexdigit()),
        "{input}"
    );
    assert!(fetched.starts_with("delete refs/beads/fetch/"), "{input}");
    let sync_git = m.store().join("sync.git");
    let kept = git(
        [
            "--git-dir",
            sync_git.to_str().unwrap(),
            "rev-parse",
            "refs/beads/remote",
        ],
        None,
    );
    assert_eq!(kept.trim(), tip);
    let temporary = git(
        [
            "--git-dir",
            sync_git.to_str().unwrap(),
            "for-each-ref",
            "refs/beads/fetch/",
        ],
        None,
    );
    assert!(temporary.is_empty(), "{temporary}");
}

/// Transport settings are read when a link is made, after a write's transaction. A git wrapper
/// adds an `insteadOf` rule when the transaction asks for HEAD, after workspace discovery but
/// before publication. The subsequent push must use it, and another machine must see the write.
#[test]
fn a_publish_goes_by_the_transport_settings_of_when_its_link_is_made() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let b = Machine::new(&sandbox, "b");
    a.bead(["init", "--prefix", "ex"]);
    b.bead(["sync"]);
    let origin = sandbox.origin();
    let real = origin.parent().unwrap().display().to_string();
    let elsewhere = sandbox.root().join("not-a-directory");
    let remote = elsewhere.join(origin.file_name().unwrap());
    let added = sandbox.root().join("setting-added");
    let config = a.checkout().join(".git").join("config");
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = rev-parse ] && [ \"$2\" = HEAD ] && mkdir {added} 2>/dev/null; \
         then\n  {git} config --file {config} {key} {prefix}\nfi\nexec {git} \"$@\"\n",
        added = quoted(&added.display().to_string()),
        git = quoted(&real_git().display().to_string()),
        config = quoted(&config.display().to_string()),
        key = quoted(&format!("url.{real}/.insteadOf")),
        prefix = quoted(&format!("{}/", elsewhere.display())),
    );
    let shims = sandbox.root().join("setting-shims");
    std::fs::create_dir(&shims).unwrap();
    std::fs::write(shims.join("git"), script).unwrap();
    std::fs::set_permissions(shims.join("git"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(std::iter::once(shims).chain(std::env::split_paths(&path)));
    let run = a
        .cmd(["create", "published where the setting says", "--json"])
        .env("PATH", path.unwrap())
        .env("BEAD_REMOTE", &remote)
        .run();
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    assert!(added.is_dir(), "the transaction never asked for HEAD");
    b.bead(["sync"]);
    assert_eq!(
        a.export(),
        b.export(),
        "the publish went by the settings bead found with its workspace: {}",
        run.stdout
    );
}

/// If persistent git processes cannot answer, fall back to individual git invocations. A wrapper
/// immediately ends every git whose standard input is a pipe. Machine a writes, claims and syncs
/// through the fallback; machine b must see the same state.
#[test]
fn with_no_kept_git_every_question_uses_a_separate_git_call() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    let b = crate::rust_only(&sandbox, "b");
    a.bead(["init", "--prefix", "ex"]);
    b.bead(["sync"]);
    let shims = sandbox.root().join("refusing-shims");
    std::fs::create_dir(&shims).unwrap();
    let log = sandbox.root().join("gits.log");
    let script = format!(
        "#!/bin/sh\nif [ -p /dev/stdin ]; then echo \"refused $1\" >> {log}; exit 1; fi\n\
         echo \"$1\" >> {log}\nexec {git} \"$@\"\n",
        log = quoted(&log.display().to_string()),
        git = quoted(&real_git().display().to_string()),
    );
    std::fs::write(shims.join("git"), script).unwrap();
    std::fs::set_permissions(shims.join("git"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(std::iter::once(shims).chain(std::env::split_paths(&path)));
    let path = path.unwrap();
    let refusing = |args: &[&str]| {
        let run = a.cmd(args).env("PATH", &path).run();
        assert_eq!(run.returncode(), 0, "{args:?}: {}", run.stderr);
        beadroll_test_support::parse_json(&run.stdout).unwrap()
    };
    b.bead(["create", "from b"]);
    b.bead(["sync"]);
    let created = refusing(&["create", "x", "--json"]);
    let key = created["id"].as_str().unwrap().to_owned();
    refusing(&["claim", &key, "--json"]);
    refusing(&["comments", "add", &key, "noted", "--json"]);
    let synced = refusing(&["sync", "--json"]);
    assert_eq!(synced["pulled"].as_i64(), Some(0), "{synced:?}");
    refusing(&["close", &key, "-r", "done", "--json"]);
    b.bead(["sync"]);
    assert_eq!(a.export(), b.export());
    let gits = std::fs::read_to_string(&log).unwrap();
    for asked in [
        "refused cat-file",
        "refused hash-object",
        "refused mktree",
        "refused update-ref",
        "ls-tree",
    ] {
        assert!(gits.lines().any(|line| line == asked), "{asked}: {gits}");
    }
}

/// Read the event's branch when its transaction starts under the store lock, and again for each
/// ownership round. A failpoint pauses after discovery but before the transaction reads HEAD; the
/// test switches branches there. Both create and claim must record the branch selected after
/// discovery.
#[test]
fn the_branch_an_event_records_is_read_when_its_transaction_starts() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let moved_during = |args: &[&str], branch: &str| -> Json {
        let gate = Gate::new(&sandbox, &format!("gate-{branch}"));
        let point = format!("journal.after-enumerate={}", gate.action());
        let switch = ["checkout", "-q", "-b", branch];
        let (run, met) = beside(
            || a.cmd(args).env("BEAD_FAILPOINTS", &point).run(),
            |finished| gate.meet(finished, || git(switch, Some(a.checkout()))),
        );
        assert!(met.is_some(), "{args:?} passed no gate: {}", run.stderr);
        assert_eq!(run.returncode(), 0, "{args:?}: {}", run.stderr);
        parse_json(&run.stdout).unwrap()
    };
    let branches = |key: &str| -> Vec<Json> {
        let log = a.bead(["log", key]).unwrap();
        let rows = log.as_array().expect("log lists events");
        rows.iter().map(|row| row["branch"].clone()).collect()
    };
    let created = moved_during(&["create", "x", "--json"], "during-create");
    let key = created["id"].as_str().unwrap().to_owned();
    assert_eq!(branches(&key), [Json::from("during-create")]);
    moved_during(&["claim", &key, "--json"], "during-claim");
    assert_eq!(branches(&key).last(), Some(&Json::from("during-claim")));
}

/// Git ignores extensions when `core.repositoryformatversion` is absent, so a SHA-256 sync
/// repository missing that setting is read as SHA-1 and must be rebuilt. The test removes it and
/// verifies sync restores a SHA-256 repository. The project is created with `git init` so this
/// also runs under git 2.40.
#[test]
fn a_sync_git_whose_config_lost_its_format_version_is_made_again() {
    let sandbox = crate::sandbox();
    let origin = sandbox.root().join("origin256.git");
    let origin_path = origin.to_str().unwrap();
    git(
        [
            "init",
            "-q",
            "--bare",
            "--object-format=sha256",
            origin_path,
        ],
        None,
    );
    let project = sandbox.root().join("project256");
    let project_path = project.to_str().unwrap();
    git(["init", "-q", "--object-format=sha256", project_path], None);
    git(["remote", "add", "origin", origin_path], Some(&project));
    let a = Machine::new(&sandbox, "a");
    let init = a.cmd(["init", "--prefix", "of"]).cwd(&project).run();
    assert_eq!(init.returncode(), 0, "{}", init.stderr);
    let sync_git = a.store().join("sync.git");
    let sync_git_path = sync_git.to_str().unwrap().to_owned();
    let format = || {
        let args = [
            "--git-dir",
            &sync_git_path,
            "rev-parse",
            "--show-object-format",
        ];
        git(args, None).trim().to_owned()
    };
    assert_eq!(format(), "sha256");
    let config = sync_git.join("config");
    let text = std::fs::read_to_string(&config).unwrap();
    let lost: String = text
        .lines()
        .filter(|line| !line.trim().starts_with("repositoryformatversion"))
        .map(|line| format!("{line}\n"))
        .collect();
    assert_ne!(lost, text, "a config with a format version: {text}");
    std::fs::write(&config, lost).unwrap();
    assert_eq!(
        format(),
        "sha1",
        "git reads the config without its extensions"
    );
    let create = a.cmd(["create", "one", "--json"]).cwd(&project).run();
    assert_eq!(create.returncode(), 0, "{}", create.stderr);
    let sync = a.cmd(["sync", "--json"]).cwd(&project).run();
    assert_eq!(sync.returncode(), 0, "{}", sync.stderr);
    assert_eq!(
        format(),
        "sha256",
        "sync.git made again in the project's format"
    );
    let published = git(["ls-remote", origin_path, "refs/beads/journal"], None);
    assert!(!published.trim().is_empty(), "nothing published");
}
