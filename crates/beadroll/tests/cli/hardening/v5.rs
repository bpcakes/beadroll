//! V5, verified by the seventh review: the store's files and directories are private whatever the
//! umask while the files bead writes into a checkout follow it, a write retightens a store whose
//! modes were relaxed, a hook's `GIT_DIR` never moves the sync repository, the side files beside a
//! journal are never read as journal, and global commit signing never reaches bead's commits.
//! `V5_KillAtRename` needs the `atomic-write:` failpoint, and is ported with it.

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

/// `value[key]` as Python indexes a dict: the member, or the test fails.
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

/// The id `machine` has in its home's `replica.json`: `json.loads(replica.read_text())["id"]`.
fn replica_id(machine: &Machine) -> String {
    let text = std::fs::read_to_string(machine.home().join("replica.json")).unwrap();
    let identity = parse_json(&text).unwrap();
    let id = member(&identity, "id");
    id.as_str().expect("a replica id").to_owned()
}

mod v5_data_file_modes {
    use super::*;

    /// A `bead` that runs the sandbox's own under `umask`: a script that sets it, then execs that
    /// bead. The Python test sets the umask on its own process (`os.umask`), which every bead it
    /// starts inherits; a test binary's threads share one umask, so here each machine runs its
    /// bead through the script instead ([`Machine::with_bead`]), every run of it under the umask.
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

    /// The entries of the directories in `dir` whose names end with `suffix`, hidden ones
    /// included: `dir.glob("*/*SUFFIX")`, as pathlib globs.
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

    /// Verified (V5): files bead writes into its own store are 0600 under umask 022, 002 and 077:
    /// segments, claims/*.json, pending/*.json and *.command (after a crash before push and after a
    /// lost ack), meta.json, replica.json, aliases.json; BEAD_HOME and <ws> are 0700 after a write.
    /// Checkout files (.beadroll.json, export -o) follow the umask: 0644 / 0664 / 0600 (F04).
    /// The review measured 3 umasks x 1 full sequence; per umask 5 own claim files, 2 segments,
    /// 2 pending files in each crash snapshot, and meta/replica/aliases all -rw-------. Each
    /// machine's bead runs under its umask through a script (`under_umask`), where the Python test
    /// sets the umask of its own process.
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

mod v5_signing_config_is_inert {
    use super::*;

    /// Verified (V5): a global commit.gpgsign=true does not reach bead's `git commit-tree`
    /// (git 2.55): claims and syncs succeed and gpg is never called. The review measured, with a
    /// gpg.program that logs and exits 2, claim rc 0 (0.8 s), sync rc 0, and the gpg log never
    /// created. Like the Python test, it needs /bin/sh for the fake gpg program, and says it skipped
    /// without one.
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
