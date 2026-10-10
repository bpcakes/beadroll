//! The processes bead starts get standard input, output and error and nothing else, whatever bead
//! was started with: Python starts them through `subprocess` with `close_fds=True`. And since
//! Phase 4 (`docs/RUST-PORT.md` Appendix B): which gits bead starts, what it asks them and when,
//! and what it reads from git's files instead, each answered as Python's git answers it.

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

/// The gits of Phase 4 (`docs/RUST-PORT.md` Appendix B), on the Rust binary: a read asks the
/// checkout one `rev-parse`, origin's `config` and the committed configuration's `cat-file`; a
/// sync with nothing new adds the transport settings' `config` its link asks, the fetch, Python's
/// two `update-ref`s after it and the `cat-file`, `hash-object` and `mktree` a link keeps; a
/// publishing write, a claim included, adds the transaction's own `rev-parse` of HEAD and its
/// branch, `commit-tree`, the push and the link's `update-ref` for the ref after it, and nothing
/// else. A git first on PATH notes each one bead starts. Every other test would still pass if the
/// in-process answers always fell back to Python's one git per question; this one would not.
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
        "update-ref",
        "update-ref",
    ];
    assert_eq!(started(&["show", &key, "--json"]), read);
    assert_eq!(started(&["info", "--json"]), read);
    assert_eq!(started(&["sync", "--json"]), sync);
    assert_eq!(started(&["create", "y", "--json"]), write);
    assert_eq!(started(&["claim", &key, "--json"]), write);
}

/// The two refs a fetch leaves are set as Python sets them, each by a git of its own and in its
/// order: `refs/beads/remote` at the tip fetched, then the fetch's own ref deleted. One
/// transaction ready the moment the fetch ends would delete that ref while the fetches other bead
/// processes of the store started with this one still list every ref, and git fails a fetch that
/// finds a listed ref gone (G16). A git first on PATH notes the arguments of each `update-ref` a
/// sync starts.
#[test]
fn the_refs_a_fetch_leaves_are_set_by_pythons_two_update_refs() {
    let sandbox = crate::sandbox();
    let m = Machine::new(&sandbox, "refs");
    m.bead(["init", "--prefix", "ex"]);
    m.bead(["create", "x"]);
    let shims = sandbox.root().join("update-ref-shims");
    std::fs::create_dir(&shims).unwrap();
    let log = sandbox.root().join("update-refs.log");
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = update-ref ]; then echo \"$*\" >> {log}; fi\nexec {git} \"$@\"\n",
        log = quoted(&log.display().to_string()),
        git = quoted(&real_git().display().to_string()),
    );
    std::fs::write(shims.join("git"), script).unwrap();
    std::fs::set_permissions(shims.join("git"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(std::iter::once(shims).chain(std::env::split_paths(&path)));
    let run = m.cmd(["sync", "--json"]).env("PATH", path.unwrap()).run();
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    let logged = std::fs::read_to_string(&log).unwrap_or_default();
    let calls: Vec<Vec<&str>> = logged.lines().map(|l| l.split(' ').collect()).collect();
    let [remote, fetched] = calls.as_slice() else {
        panic!("not two update-refs: {logged}");
    };
    let tip = remote.get(2).copied().unwrap_or_default();
    assert_eq!(remote[..2], ["update-ref", "refs/beads/remote"], "{logged}");
    assert!(
        tip.len() == 40 && tip.bytes().all(|b| b.is_ascii_hexdigit()),
        "{logged}"
    );
    assert_eq!(fetched.len(), 3, "{logged}");
    assert_eq!(fetched[..2], ["update-ref", "-d"], "{logged}");
    assert!(fetched[2].starts_with("refs/beads/fetch/"), "{logged}");
}

/// The transport settings a publish goes by are the checkout's when its link is made, where
/// Python's `Link` asks for them, not when bead found its workspace: a write's link is made for its
/// publish, after its transaction, and a setting made in between is the one the push uses. Machine
/// a writes to its remote under a name that only `url.<base>.insteadOf` resolves, and a git first
/// on PATH adds that setting to a's checkout when the transaction asks for HEAD: after the
/// workspace is found, before the publish. Machine b then sees what a wrote.
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

/// When none of the gits a link keeps can answer, every question goes to the git Python runs for
/// it, and the commands come to what they come to with them: here a git first on PATH ends at once
/// any git whose standard input is a pipe, which only a kept git's is. Machine a writes, claims and
/// syncs that way; b, without it, sees exactly what a has, and a's gits were Python's.
#[test]
fn with_no_kept_git_every_question_goes_to_pythons_git() {
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

/// The branch an event records is asked of git when its transaction starts, as Python's `Tx` asks
/// it once the store lock is taken, and again in each round of an ownership command: a branch
/// switched after bead found its workspace, before its transaction began, is the one recorded. A
/// `pause-until` failpoint stops bead at the first journal it loads, after discovery and before the
/// transaction reads HEAD; the checkout moves to a new branch there, and bead goes on. A create,
/// then a claim, each moved to a branch of its own.
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

/// sync.git's object format is the one git reads, which its config alone does not say: git reads
/// a config without `core.repositoryformatversion` without its extensions, so a SHA-256 sync.git
/// whose config lost that line is SHA-1 to git. Python's `rev-parse --show-object-format` before
/// a fetch then finds it made in another format than the project's, and makes it again. A SHA-256
/// project's machine whose sync.git config lost the line syncs as before, its sync.git SHA-256
/// again. The project is made by `git init`, not cloned from its empty origin as G26's is: this
/// runs under git 2.40 too.
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
