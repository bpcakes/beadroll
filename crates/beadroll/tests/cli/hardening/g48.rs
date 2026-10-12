//! G48: an atomic write that fails leaves no temporary file behind, in the checkout or in the
//! store, and its error names the file the user gave.

/// G48: write_atomic leaves `.NAME.PID.tmp` behind when the write, fsync or rename fails: beside an
/// `export -o` target in the user's checkout, and under pending/ and events/ in the store.
mod g48_failed_atomic_write_leaves_nothing_behind {
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    use beadroll_test_support::{Machine, Run, Sandbox, parse_json};

    /// The class's `setUp`: machine g48, its tracker initialised with the prefix `ex`, and the keys
    /// of the six issues it creates, each titled with a hundred `x`.
    fn setup(sandbox: &Sandbox) -> (Machine, Vec<String>) {
        let m = Machine::new(sandbox, "g48");
        m.bead(["init", "--prefix", "ex"]);
        let keys = (0..6)
            .map(|n| {
                let title = format!("issue {n} {}", "x".repeat(100));
                let created = m.bead(["create", &title]).expect("create prints the issue");
                created["id"].as_str().expect("the issue's id").to_owned()
            })
            .collect();
        (m, keys)
    }

    /// Every path under `dir`, symbolic links not followed: `Path.rglob("*")`.
    fn descendants(dir: &Path, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("a directory") {
            let path = entry.expect("a directory entry").path();
            found.push(path.clone());
            if std::fs::symlink_metadata(&path).unwrap().is_dir() {
                descendants(&path, found);
            }
        }
    }

    /// `leftovers`: every file under `root` whose name matches `.*.tmp`, relative to `root`,
    /// sorted.
    fn leftovers(root: &Path) -> Vec<String> {
        let mut paths = Vec::new();
        descendants(root, &mut paths);
        let mut found: Vec<String> = paths
            .iter()
            .filter(|path| {
                let name = path.file_name().unwrap().as_encoded_bytes();
                name.len() >= 5 && name.starts_with(b".") && name.ends_with(b".tmp")
            })
            .map(|path| path.strip_prefix(root).unwrap().display().to_string())
            .collect();
        found.sort();
        found
    }

    /// `_g48_error`: the code and message of a failed `--json` call; a crash or other output is
    /// reported as such, with all of its standard error.
    fn error(run: &Run) -> (String, String) {
        let document = parse_json(&run.stderr).ok();
        let error = document.as_ref().and_then(|d| d.get("error"));
        let code = error.and_then(|e| e.get("code"));
        let message = error.and_then(|e| e.get("message"));
        match (code, message) {
            (Some(code), Some(message)) => {
                let text = |value: &beadroll_test_support::Json| {
                    value
                        .as_str()
                        .map_or_else(|| format!("{value:?}"), str::to_owned)
                };
                (text(code), text(message))
            }
            _ if run.crashed() => ("CRASH".to_owned(), run.stderr.clone()),
            _ => (format!("rc {}", run.returncode()), run.stderr.clone()),
        }
    }

    /// The block `ulimit -f` counts in, in bytes, which the shell decides: 512 in dash and in
    /// bash's POSIX mode, 1024 in the bash 3.2 that is macOS's `/bin/sh`. Measured by a file
    /// written in `dir` under a limit of one block, which stops there.
    fn ulimit_block(dir: &Path) -> u64 {
        let probe = dir.join("ulimit-block");
        // dd ends at the limit, killed by SIGXFSZ or refused with EFBIG: either way the probe is
        // full
        Command::new("/bin/sh")
            .args([
                "-c",
                "ulimit -f 1; exec dd if=/dev/zero of=\"$1\" bs=4096 count=1",
            ])
            .arg("sh")
            .arg(&probe)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        let block = std::fs::metadata(&probe).unwrap().len();
        assert!([512, 1024].contains(&block), "a block of {block} bytes");
        block
    }

    /// Runs `bead ARGS --json` under a file-size limit of `kib` KiB. SIGXFSZ is ignored so a
    /// partial write fails with EFBIG instead of killing bead. Measure the shell's block size in
    /// `dir` before translating the limit, since `ulimit -f` units vary by platform.
    fn limited(machine: &Machine, dir: &Path, kib: u64, args: &[&str]) -> Run {
        let blocks = kib * 1024 / ulimit_block(dir);
        let output = Command::new("/bin/sh")
            .args(["-c", &format!("ulimit -f {blocks} && exec \"$@\""), "sh"])
            .arg(machine.executable())
            .args(args)
            .arg("--json")
            .current_dir(machine.checkout())
            .env_clear()
            .envs(machine.env(None, "codex"))
            .stdin(Stdio::null())
            .output()
            .unwrap();
        Run {
            status: output.status,
            stdout: String::from_utf8(output.stdout).expect("UTF-8 output"),
            stderr: String::from_utf8(output.stderr).expect("UTF-8 errors"),
        }
    }

    /// G48: `export -o DIR` fails at the rename; a full copy of the export stays in the checkout as
    /// `.DIR.PID.tmp`, and the IO message names that temporary file instead of DIR.
    #[test]
    fn g48_export_onto_a_directory_leaves_no_copy_and_names_the_target() {
        let sandbox = crate::sandbox();
        let (m, _) = setup(&sandbox);
        std::fs::create_dir(m.checkout().join("out")).unwrap();
        let failed = m.run(["export", "-o", "out", "--json"]);
        let (code, message) = error(&failed);
        assert_eq!(code, "IO", "{message}");
        assert_eq!(
            leftovers(m.checkout()),
            Vec::<String>::new(),
            "a failed export left a temporary copy in the checkout"
        );
        assert!(
            !message.contains(".tmp"),
            "the error names a temporary file the user never gave: {message}"
        );
        assert!(message.contains("out"), "{message}");
    }

    /// G48: an export that fails part way (file-size limit standing in for a full volume) leaves
    /// the partial `.NAME.PID.tmp` in the checkout, and a later successful export does not remove
    /// it.
    #[test]
    fn g48_export_that_cannot_be_written_whole_leaves_nothing() {
        let sandbox = crate::sandbox();
        let (m, _) = setup(&sandbox);
        let failed = limited(&m, sandbox.root(), 1, &["export", "-o", "big.jsonl"]);
        let (code, message) = error(&failed);
        assert_eq!(code, "IO", "{message}");
        assert!(!m.checkout().join("big.jsonl").exists());
        let again = m.run(["export", "-o", "big.jsonl"]);
        assert_eq!(again.returncode(), 0, "{}", again.stderr);
        assert_eq!(
            leftovers(m.checkout()),
            Vec::<String>::new(),
            "a failed export left a partial copy in the checkout"
        );
    }

    /// G48: a claim whose receipt or attempt cannot be written whole leaves
    /// `.ID.command|json.PID.tmp` under pending/; nothing ever sweeps it, not even a later
    /// successful sync.
    #[test]
    fn g48_failed_store_write_leaves_no_temporary_file() {
        let sandbox = crate::sandbox();
        let (m, keys) = setup(&sandbox);
        let big = sandbox.root().join("description.txt");
        std::fs::write(&big, "d".repeat(100_000)).unwrap();
        let big = big.to_str().expect("a UTF-8 path");
        let args = ["update", &keys[0], "--claim", "--description-file", big];
        let failed = limited(&m, sandbox.root(), 16, &args);
        let (code, message) = error(&failed);
        assert_eq!(code, "IO", "{message}");
        let synced = m.run(["sync", "--json"]);
        assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
        assert_eq!(
            leftovers(&m.store()),
            Vec::<String>::new(),
            "a failed store write left its temporary file"
        );
    }
}
