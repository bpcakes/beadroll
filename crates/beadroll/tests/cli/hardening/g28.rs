//! G28: reading pending attempts must not delete an accepted attempt or its receipt when the file
//! is unreadable, nor race a locked sync that confirms it after listing. A read-only store with a
//! junk pending file must remain readable. The lost acknowledgement uses
//! `BEAD_TEST_CRASH_AFTER_PUSH` (exit 99); the listing race uses `pending.after-list`.

mod g28_only_the_lock_holder_sweeps_pending {
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use beadroll_test_support::{Json, Machine, Run, Sandbox, code};

    use crate::hardening::gate::{Gate, beside};

    /// The hook that stops a command after the remote took its push, before bead hears back.
    const CRASH_AFTER_PUSH: &str = "BEAD_TEST_CRASH_AFTER_PUSH";

    /// `_g28_unprivileged`: whether file modes bind the user the tests run as. Root reads a
    /// mode-000 file and writes a read-only store, so the tests that rely on modes skip under it.
    fn unprivileged() -> bool {
        !rustix::process::geteuid().is_root()
    }

    /// The id of the issue a `--json` result is: `result["id"]`.
    fn id(result: Option<Json>) -> String {
        let result = result.expect("bead printed a result");
        result["id"].as_str().expect("an id").to_owned()
    }

    /// The class's `setUp`: machine a (made by `make`) publishes its tracker (prefix `gp`), which
    /// holds the issue `x`; its key.
    fn set_up(sandbox: &Sandbox, make: fn(&Sandbox, &str) -> Machine) -> (Machine, String) {
        let a = make(sandbox, "a");
        a.bead(["init", "--prefix", "gp"]);
        let key = id(a.bead(["create", "x"]));
        a.bead(["sync"]);
        (a, key)
    }

    /// The entries of `dir` whose names end with `suffix`, in directory order, as
    /// `Path.glob("*SUFFIX")` lists them: none when `dir` is not there.
    fn glob(dir: &Path, suffix: &str) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .map(|entry| entry.expect("a directory entry").path())
            .filter(|path| {
                let name = path.file_name().expect("a name");
                name.as_encoded_bytes().ends_with(suffix.as_bytes())
            })
            .collect()
    }

    /// `argv` followed by `--json`.
    fn with_json(argv: &[String]) -> Vec<&str> {
        argv.iter().map(String::as_str).chain(["--json"]).collect()
    }

    /// `lose_ack`: runs `argv`, a command the remote accepts, dying before it hears so: its
    /// attempt and receipt stay. The attempt, `pending/<id>.json`.
    fn lose_ack(a: &Machine, argv: &[String]) -> PathBuf {
        let crashed = a.cmd(with_json(argv)).env(CRASH_AFTER_PUSH, "1").run();
        assert_eq!(crashed.returncode(), 99, "{}", crashed.stderr);
        let attempts = glob(&a.store().join("pending"), ".json");
        assert_eq!(attempts.len(), 1);
        assert!(attempts[0].with_extension("command").exists());
        attempts[0].clone()
    }

    /// `release_with_lost_ack`: a new issue, `held`, claimed, then released with the
    /// acknowledgement lost; the release's arguments and its attempt.
    fn release_with_lost_ack(a: &Machine) -> (Vec<String>, PathBuf) {
        let key = id(a.bead(["create", "held"]));
        a.bead(["claim", &key]);
        let argv = vec!["release".to_owned(), key];
        let attempt = lose_ack(a, &argv);
        (argv, attempt)
    }

    /// The mode of the file `path` set to `mode`, as `Path.chmod` sets it.
    fn chmod(path: &Path, mode: u32) {
        let set = std::fs::set_permissions(path, Permissions::from_mode(mode));
        set.unwrap_or_else(|error| panic!("chmod {mode:o} {}: {error}", path.display()));
    }

    /// `chmod -R MODE PATH`, which must succeed.
    fn chmod_tree(mode: &str, path: &Path) {
        let status = Command::new("chmod")
            .args(["-R", mode])
            .arg(path)
            .status()
            .expect("chmod starts");
        assert!(status.success(), "chmod -R {mode} {}", path.display());
    }

    /// G28: `show` (no lock) deletes an accepted attempt it cannot read (EACCES) and its receipt.
    #[test]
    fn g28_reader_keeps_an_unreadable_accepted_attempt_and_its_receipt() {
        if !unprivileged() {
            return; // root can read a mode-000 file
        }
        let sandbox = crate::sandbox();
        let (a, key) = set_up(&sandbox, Machine::new);
        for name in ["update --claim --status closed", "release"] {
            let (argv, attempt) = if name == "release" {
                release_with_lost_ack(&a)
            } else {
                let argv = ["update", &key, "--claim", "--status", "closed"];
                let argv: Vec<String> = argv.map(str::to_owned).into();
                let attempt = lose_ack(&a, &argv);
                (argv, attempt)
            };
            let receipt = attempt.with_extension("command");
            chmod(&attempt, 0);
            let shown = a.run(["show", &key, "--json"]);
            if attempt.exists() {
                chmod(&attempt, 0o600);
            }
            assert_eq!(shown.returncode(), 0, "{name}: {}", shown.stderr);
            assert_eq!(
                (attempt.exists(), receipt.exists()),
                (true, true),
                "{name}: a read-only command deleted an accepted attempt it could not read, and its \
                 receipt"
            );
            let retry = a.cmd(with_json(&argv)).run();
            assert_eq!(retry.returncode(), 0, "{name}: {}", retry.stderr);
        }
    }

    /// G28: a reader racing confirmation must retain the receipt kept by sync. Pause show at its
    /// first pending.after-list, after listing the attempt but before reading it, while a locked
    /// sync confirms it and removes the pending file.
    #[test]
    fn g28_reader_racing_a_sync_keeps_the_receipt_the_sync_kept() {
        let sandbox = crate::sandbox();
        let (a, _) = set_up(&sandbox, crate::rust_only);
        let (argv, attempt) = release_with_lost_ack(&a);
        let receipt = attempt.with_extension("command");
        let listed = Gate::new(&sandbox, "g28-pending-listed");
        let failpoints = format!("pending.after-list={}", listed.action());
        let (shown, paused) = beside(
            || {
                let show = a.cmd(["show", &argv[1], "--json"]).thread("reader");
                show.env("BEAD_FAILPOINTS", &failpoints).run()
            },
            |finished| {
                // listed, not yet read: a locked writer runs now
                listed.meet(finished, || {
                    let synced = a.run(["sync", "--json"]);
                    let left = (attempt.exists(), receipt.exists());
                    (synced.returncode(), synced.stderr, left.0, left.1)
                })
            },
        );
        assert_eq!(shown.returncode(), 0, "{}{}", shown.stdout, shown.stderr);
        let paused = paused.expect("the reader never reached pending/");
        assert_eq!(
            paused,
            (0, String::new(), false, true),
            "the sync confirms the attempt and keeps its receipt"
        );
        assert!(
            receipt.exists(),
            "a reader (show, no lock) deleted the receipt the locked sync kept"
        );
        let retry = a.cmd(with_json(&argv)).run();
        assert_eq!(retry.returncode(), 0, "{}", retry.stderr);
    }

    /// G28: readers try to delete a junk pending file, so every read fails with IO on a read-only
    /// store.
    #[test]
    fn g28_readers_work_on_a_read_only_store_with_a_junk_pending_file() {
        if !unprivileged() {
            return; // root can write a read-only store
        }
        let sandbox = crate::sandbox();
        let (a, key) = set_up(&sandbox, Machine::new);
        let store = a.store();
        let pending = store.join("pending");
        std::fs::create_dir_all(&pending).expect("the store's pending/");
        let junk = pending.join("01900000-0000-7000-8000-0000000000aa.json");
        std::fs::write(junk, "{").expect("a junk pending file");
        chmod_tree("a-w", &store);
        let readers: [&[&str]; 5] = [
            &["show", &key],
            &["list"],
            &["ready"],
            &["export"],
            &["info"],
        ];
        let results: Vec<(&str, Run)> = readers
            .iter()
            .map(|argv| (argv[0], a.cmd(argv.iter().chain(&["--json"])).run()))
            .collect();
        // The `finally`: should a run fail, the sandbox's drop makes the store writable again.
        chmod_tree("u+w", &store);
        for (name, run) in results {
            assert_eq!(run.returncode(), 0, "{name}: {}", run.stderr);
        }
    }

    /// G28: under the lock too, an attempt that cannot be read is not junk: `sync` must not sweep
    /// it (and its receipt) away; it reports IO, and once the mode is fixed the retry is
    /// recognised.
    #[test]
    fn g28_locked_sweep_does_not_delete_an_unreadable_attempt() {
        if !unprivileged() {
            return; // root can read a mode-000 file
        }
        let sandbox = crate::sandbox();
        let (a, _) = set_up(&sandbox, Machine::new);
        let (argv, attempt) = release_with_lost_ack(&a);
        let receipt = attempt.with_extension("command");
        chmod(&attempt, 0);
        let synced = a.run(["sync", "--json"]);
        if attempt.exists() {
            chmod(&attempt, 0o600);
        }
        assert_eq!(
            (attempt.exists(), receipt.exists()),
            (true, true),
            "sync (exit {}) swept away an accepted attempt it could not read, and its receipt",
            synced.returncode()
        );
        assert_eq!(code(&synced), "IO", "{}", synced.stderr);
        let retry = a.cmd(with_json(&argv)).run();
        assert_eq!(retry.returncode(), 0, "{}", retry.stderr);
    }
}
