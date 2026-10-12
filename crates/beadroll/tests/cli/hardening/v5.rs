//! V5: store files remain private under any umask, checkout files follow the umask, and writes
//! retighten relaxed store permissions. Hook GIT_DIR cannot move the sync repository, journal
//! side files are ignored, and global signing does not affect internal commits. The rename crash
//! test uses `atomic-write:` failpoints.

use std::collections::BTreeSet;
use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use beadroll_test_support::{Json, Machine, Sandbox, parse_json};

/// `_v5_mode`: the permission bits of `path` itself, a symlink not followed (`os.lstat`).
fn mode(path: &Path) -> u32 {
    let meta = std::fs::symlink_metadata(path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    meta.permissions().mode() & 0o7777
}

/// `_v5_modes`: each of `paths` with its mode, in octal as `oct()` writes it.
fn modes(paths: &[PathBuf]) -> Vec<(String, String)> {
    paths
        .iter()
        .map(|path| (path.display().to_string(), format!("{:#o}", mode(path))))
        .collect()
}

/// `set(_v5_modes(paths).values())`: the modes `paths` have between them.
fn distinct_modes(paths: &[PathBuf]) -> BTreeSet<u32> {
    paths.iter().map(|path| mode(path)).collect()
}

/// The id of the issue a `--json` result is: `result["id"]`.
fn id(result: Option<Json>) -> String {
    let result = result.expect("bead printed a result");
    result["id"].as_str().expect("an id").to_owned()
}

/// The member `key` of `value`; absence fails the test.
fn member<'v>(value: &'v Json, key: &str) -> &'v Json {
    value
        .get(key)
        .unwrap_or_else(|| panic!("no {key:?} in {value:?}"))
}

/// `path` quoted for the shell.
fn quoted(path: &Path) -> String {
    let text = path.to_str().expect("a UTF-8 path");
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// `script` written to `path`, executable.
fn executable(path: &Path, script: &str) {
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, Permissions::from_mode(0o755)).unwrap();
}

/// The sandbox's remote, `str(self.box.origin)`, as bead's `--remote` takes it.
fn origin(sandbox: &Sandbox) -> &str {
    sandbox.origin().to_str().expect("a UTF-8 path")
}

/// The machine id from replica.json.
fn replica_id(machine: &Machine) -> String {
    let text = std::fs::read_to_string(machine.home().join("replica.json")).unwrap();
    let identity = parse_json(&text).unwrap();
    let id = member(&identity, "id");
    id.as_str().expect("a replica id").to_owned()
}

mod v5_data_file_modes {
    use super::*;

    /// A launcher that sets `umask` and execs the sandbox's bead. Each machine uses it through
    /// [`Machine::with_bead`], avoiding changes to the shared test process's umask.
    fn under_umask(sandbox: &Sandbox, umask: u32) -> PathBuf {
        let path = sandbox.root().join(format!("bead-umask-{umask:03o}"));
        let script = format!(
            "#!/bin/sh\numask {umask:03o}\nexec {} \"$@\"\n",
            quoted(sandbox.bead())
        );
        executable(&path, &script);
        path
    }

    /// The entries of `dir`, sorted: `sorted(dir.iterdir())`.
    fn entries(dir: &Path) -> Vec<PathBuf> {
        let listed = std::fs::read_dir(dir).unwrap_or_else(|error| panic!("{dir:?}: {error}"));
        let mut paths: Vec<PathBuf> = listed.map(|entry| entry.unwrap().path()).collect();
        paths.sort();
        paths
    }

    /// Matching entries one directory below dir, including hidden entries, whose names end with
    /// suffix.
    fn below(dir: &Path, suffix: &str) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for folder in entries(dir).into_iter().filter(|path| path.is_dir()) {
            let names = entries(&folder).into_iter().filter(|path| {
                let name = path.file_name().expect("a name");
                name.as_encoded_bytes().ends_with(suffix.as_bytes())
            });
            found.extend(names);
        }
        found
    }

    /// `events/*/claims/*.json` in `store`: every confirmed attempt file.
    fn claim_files(store: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for replica in entries(&store.join("events")) {
            let claims = replica.join("claims");
            if claims.is_dir() {
                let files = entries(&claims).into_iter().filter(|path| {
                    let name = path.file_name().expect("a name");
                    name.as_encoded_bytes().ends_with(b".json")
                });
                found.extend(files);
            }
        }
        found
    }

    /// V5: under umasks 022, 002 and 077, all store files are 0600 and BEAD_HOME and the
    /// workspace directory are 0700 after a write. Includes claim, segment, pending, receipt,
    /// metadata, identity and alias files, with snapshots before push and after a lost
    /// acknowledgement. Checkout configuration and exports follow the umask: 0644, 0664 and 0600
    /// respectively (F04).
    #[test]
    fn data_files_are_0600_and_checkout_files_follow_umask() {
        let sandbox = crate::sandbox();
        for (umask, expected) in [(0o022, 0o644), (0o002, 0o664), (0o077, 0o600)] {
            let m = Machine::new(&sandbox, &format!("v5-{umask:o}"))
                .with_bead(under_umask(&sandbox, umask));
            m.bead(["machines", "--name", "deadbeef=laptop"]);
            let remote = sandbox.bare(&format!("r{umask:o}.git"));
            let remote = remote.to_str().expect("a UTF-8 path");
            m.bead(["init", "--prefix", "ex", "--tracked", "--remote", remote]);
            let key = id(m.bead(["create", "x"]));
            let crashed = m
                .cmd(["claim", &key, "--json"])
                .env("BEAD_TEST_CRASH_BEFORE_PUSH", "1")
                .run();
            assert_eq!(crashed.returncode(), 98, "{}", crashed.stderr);
            let pending = entries(&m.store().join("pending"));
            assert_eq!(pending.len(), 2, "umask {umask:o}: {pending:?}");
            assert_eq!(
                distinct_modes(&pending),
                BTreeSet::from([0o600]),
                "umask {umask:o}: {:?}",
                modes(&pending)
            );
            m.bead(["claim", &key]);
            let other = id(m.bead(["create", "y"]));
            let lost = m
                .cmd(["claim", &other, "--json"])
                .env("BEAD_TEST_CRASH_AFTER_PUSH", "1")
                .run();
            assert_eq!(lost.returncode(), 99, "{}", lost.stderr);
            let after_ack = entries(&m.store().join("pending"));
            assert_eq!(after_ack.len(), 2, "umask {umask:o}: {after_ack:?}");
            assert_eq!(
                distinct_modes(&after_ack),
                BTreeSet::from([0o600]),
                "umask {umask:o}: {:?}",
                modes(&after_ack)
            );
            m.bead(["claim", &other]);
            m.bead(["export", "-o", "issues.jsonl"]);
            let store = m.store();
            let claims = claim_files(&store);
            let mut data = below(&store.join("events"), ".jsonl");
            data.extend(claims.iter().cloned());
            data.extend([
                store.join("meta.json"),
                m.home().join("replica.json"),
                m.home().join("aliases.json"),
            ]);
            assert!(data.iter().all(|path| path.exists()), "{data:?}");
            assert!(!claims.is_empty(), "umask {umask:o}: no claim file");
            assert_eq!(
                distinct_modes(&data),
                BTreeSet::from([0o600]),
                "umask {umask:o}: {:?}",
                modes(&data)
            );
            let folders = [m.home().to_path_buf(), store];
            assert_eq!(
                distinct_modes(&folders),
                BTreeSet::from([0o700]),
                "umask {umask:o}: {:?}",
                modes(&folders)
            );
            for path in [
                m.checkout().join(".beadroll.json"),
                m.checkout().join("issues.jsonl"),
            ] {
                let name = path.file_name().expect("a name").to_string_lossy();
                assert_eq!(
                    format!("{:#o}", mode(&path)),
                    format!("{expected:#o}"),
                    "{name} under {umask:o}"
                );
            }
        }
    }
}

mod v5_retighten {
    use super::*;

    /// Verified (V5): a write command chmods BEAD_HOME and <ws> back to 0700 and a read command
    /// (list) does not. The review measured relaxed 0755/0755 -> after list 0755/0755 -> after
    /// create 0700/0700.
    #[test]
    fn a_write_retightens_a_relaxed_store() {
        let sandbox = crate::sandbox();
        let m = Machine::new(&sandbox, "v5-relax");
        m.bead(["init", "--prefix", "ex"]);
        std::fs::set_permissions(m.home(), Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(m.store(), Permissions::from_mode(0o755)).unwrap();
        m.bead(["list"]);
        let folders = [m.home().to_path_buf(), m.store()];
        assert_eq!(
            distinct_modes(&folders),
            BTreeSet::from([0o755]),
            "{:?}",
            modes(&folders)
        );
        m.bead(["create", "x"]);
        assert_eq!(
            distinct_modes(&folders),
            BTreeSet::from([0o700]),
            "{:?}",
            modes(&folders)
        );
    }

    /// Verified (V5): an existing, user-chosen BEAD_HOME (a 0775 team directory) is silently made
    /// 0700 by the first write (design observation). The review measured 0775 -> 0700 after init.
    #[test]
    fn existing_group_directory_becomes_owner_only() {
        let sandbox = crate::sandbox();
        let m = Machine::new(&sandbox, "v5-group");
        std::fs::create_dir(m.home()).unwrap();
        std::fs::set_permissions(m.home(), Permissions::from_mode(0o775)).unwrap();
        assert_eq!(format!("{:#o}", mode(m.home())), "0o775");
        let made = m.run(["init", "--prefix", "ex", "--json"]);
        assert_eq!(made.returncode(), 0, "{}", made.stderr);
        assert_eq!(format!("{:#o}", mode(m.home())), "0o700");
    }
}

mod v5_git_dir_in_hook {
    use std::process::{Command, Stdio};

    use beadroll_test_support::git;

    use super::*;

    /// Verified (V5): bead run from a git hook (GIT_DIR set, relative '.git' or absolute) still
    /// creates sync.git in the store and leaves the checkout's .git alone. The review measured 2
    /// variants: sync rc 0, sync.git/HEAD present in the store, checkout core.bare 'false' before
    /// and after, `git status` rc 0.
    #[test]
    fn sync_repo_lands_in_the_store_with_git_dir_set() {
        let sandbox = crate::sandbox();
        let publisher = Machine::new(&sandbox, "v5-pub");
        publisher.bead(["init", "--prefix", "ex", "--remote", origin(&sandbox)]);
        publisher.bead(["create", "x"]);
        publisher.bead(["sync"]);
        for variant in ["rel", "abs"] {
            let m = Machine::new(&sandbox, &format!("v5-hook-{variant}"));
            let gitdir = if variant == "rel" {
                PathBuf::from(".git")
            } else {
                m.checkout().join(".git")
            };
            let before = git(["config", "--get", "core.bare"], Some(m.checkout()));
            assert_eq!(before.trim(), "false", "{variant}");
            let synced = m.cmd(["sync", "--json"]).env("GIT_DIR", gitdir).run();
            assert_eq!(synced.returncode(), 0, "{variant}: {}", synced.stderr);
            assert!(
                m.store().join("sync.git").join("HEAD").exists(),
                "{variant}"
            );
            assert!(!m.checkout().join("sync.git").exists(), "{variant}");
            let after = git(["config", "--get", "core.bare"], Some(m.checkout()));
            assert_eq!(after.trim(), "false", "{variant}");
            let status = Command::new("git")
                .args(["status", "--porcelain"])
                .current_dir(m.checkout())
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(
                status.status.success(),
                "{variant}: {}",
                String::from_utf8_lossy(&status.stderr)
            );
        }
    }
}

mod v5_side_files_ignored {
    use super::*;

    /// Verified (V5): side files next to journals (`.X.<pid>.tmp`, `*.torn-*`, `*.fork-*`, in
    /// events/<m>/ and in claims/) are never read as segments or attempts and never published. The
    /// review wrote garbage into all five name shapes, then measured sync integrity 0, info
    /// integrity [], segments 1, and export byte-identical before and after.
    #[test]
    fn leftover_side_files_are_never_read_as_journal() {
        let sandbox = crate::sandbox();
        let m = Machine::new(&sandbox, "v5-side");
        m.bead(["init", "--prefix", "ex", "--remote", origin(&sandbox)]);
        let key = id(m.bead(["create", "x"]));
        m.bead(["claim", &key]);
        let before = m.export();
        let own = m.store().join("events").join(replica_id(&m));
        assert!(own.join("claims").is_dir());
        for name in [
            ".000001.jsonl.1.tmp",
            "000001.jsonl.torn-1",
            "000001.jsonl.fork-1",
        ] {
            let garbage = "{\"garbage\": true}\n{\"v\":1,\"id\":\"not-an-event\"}\n";
            std::fs::write(own.join(name), garbage).unwrap();
        }
        for name in [".x.json.1.tmp", "x.json.fork-1"] {
            std::fs::write(own.join("claims").join(name), "{\n").unwrap();
        }
        let synced = m.bead(["sync"]).expect("a sync report");
        let info = m.bead(["info"]).expect("bead info prints a record");
        assert_eq!(
            (
                member(&synced, "integrity"),
                member(&info, "integrity"),
                member(&info, "segments"),
            ),
            (&Json::from(0), &Json::Array(Vec::new()), &Json::from(1))
        );
        assert_eq!(m.export(), before);
    }
}

mod v5_kill_at_rename {
    use super::*;

    /// The names in `dir` that start with `start` and end with `end`, as `glob("START*END")`.
    fn named(dir: &Path, start: &str, end: &str) -> Vec<String> {
        let listed = std::fs::read_dir(dir).unwrap_or_else(|error| panic!("{dir:?}: {error}"));
        let names = listed.map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned());
        names
            .filter(|name| name.starts_with(start) && name.ends_with(end))
            .collect()
    }

    /// V5: SIGKILL after a temporary file is written and fsynced but before rename leaves the
    /// target whole. An export retains its previous bytes; a claim killed while confirming its
    /// attempt is recognised on retry. Inject `atomic-write:<part>=sigkill` at the first matching
    /// write in this `rust_only` test.
    #[test]
    fn kill_at_rename_keeps_targets_whole() {
        let sandbox = crate::sandbox();
        let m = crate::rust_only(&sandbox, "v5k");
        m.bead(["init", "--prefix", "ex", "--remote", origin(&sandbox)]);
        let key = id(m.bead(["create", "one"]));
        let target = m.checkout().join("issues.jsonl");
        std::fs::write(&target, "previous export\n").unwrap();
        let killed = m
            .cmd(["export", "-o", "issues.jsonl"])
            .env("BEAD_FAILPOINTS", "atomic-write:issues.jsonl=sigkill")
            .run();
        assert_eq!(killed.returncode(), -9, "{}", killed.stderr);
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "previous export\n"
        );
        assert!(!named(m.checkout(), ".issues.jsonl.", ".tmp").is_empty());
        let killed = m
            .cmd(["claim", &key, "--json"])
            .env("BEAD_FAILPOINTS", "atomic-write:/claims/=sigkill")
            .run();
        assert_eq!(killed.returncode(), -9, "{}", killed.stderr);
        let events = std::fs::read_dir(m.store().join("events")).unwrap();
        let claims = events.map(|replica| replica.unwrap().path().join("claims"));
        let leftovers: Vec<String> = claims
            .filter(|dir| dir.is_dir())
            .flat_map(|dir| named(&dir, ".", ".tmp"))
            .filter(|name| name.contains(".json."))
            .collect();
        assert!(
            !leftovers.is_empty(),
            "no temporary attempt file in claims/"
        );
        assert_eq!(named(&m.store().join("pending"), "", "").len(), 2);
        let retried = m.run(["claim", &key, "--json"]);
        assert_eq!(retried.returncode(), 0, "{}", retried.stderr);
        let info = m.bead(["info"]).expect("bead info prints a record");
        let none = Json::Array(Vec::new());
        assert_eq!(
            (
                member(&info, "integrity"),
                member(&info, "pending_ownership")
            ),
            (&none, &none)
        );
        let machine = member(&info, "machine").as_str().expect("the machine's id");
        let shown = m.show(&key);
        assert_eq!(
            member(member(&shown, "claim"), "owner").as_str(),
            Some(format!("codex:v5k-thread-0001@{machine}").as_str())
        );
    }
}

mod v5_signing_config_is_inert {
    use super::*;

    /// V5: global `commit.gpgsign=true` must not reach internal `git commit-tree` calls. A fake
    /// gpg logs and fails if called; claims and sync must succeed without creating that log.
    /// Requires `/bin/sh` for the fake executable.
    #[test]
    fn global_commit_signing_does_not_affect_claims() {
        if !Path::new("/bin/sh").exists() {
            eprintln!("skipped: needs /bin/sh for the fake gpg program");
            return;
        }
        let sandbox = crate::sandbox();
        let log = sandbox.root().join("gpg.log");
        let gpg = sandbox.root().join("fakegpg");
        executable(
            &gpg,
            &format!(
                "#!/bin/sh\necho called >> {}\necho 'gpg: signing failed' >&2\nexit 2\n",
                quoted(&log)
            ),
        );
        let config = sandbox.root().join("global");
        let settings = format!(
            "[commit]\n\tgpgsign = true\n[gpg]\n\tprogram = {}\n",
            gpg.display()
        );
        std::fs::write(&config, settings).unwrap();
        let mut m = Machine::new(&sandbox, "v5s");
        m.bead(["init", "--prefix", "ex", "--remote", origin(&sandbox)]);
        let key = id(m.bead(["create", "one"]));
        m.extra.insert("GIT_CONFIG_GLOBAL".into(), config.into());
        m.extra.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
        let claimed = m.run(["claim", &key, "--json"]);
        assert_eq!(claimed.returncode(), 0, "{}", claimed.stderr);
        m.bead(["create", "two"]);
        let synced = m.run(["sync", "--json"]);
        assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
        assert!(!log.exists());
        let shown = m.show(&key);
        let info = m.bead(["info"]).expect("bead info prints a record");
        let machine = member(&info, "machine").as_str().expect("the machine's id");
        assert_eq!(
            member(member(&shown, "claim"), "owner").as_str(),
            Some(format!("codex:v5s-thread-0001@{machine}").as_str())
        );
    }
}
