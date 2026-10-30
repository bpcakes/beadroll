//! The journal's segments: the torn tail an interrupted append leaves, an append killed midway or
//! cut short by the file-size limit, rotation under parallel writers, and a segment deleted from
//! the store.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use beadroll_test_support::{Json, Machine, UNPROFILED, parse_json};

use super::{Worker, jsonl, own_segment, setup, setup_with, wait_marker};
use crate::protocol::sync_audit::ProcessSession;

/// The unterminated fragment an append interrupted mid-line leaves at the end of a segment.
const TORN: &[u8] = br#"{"v":1,"id":"interrupted"#;

/// Appends `bytes` to the file `path`.
fn append(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(bytes).unwrap();
}

/// The size of the file `path`, in bytes.
fn size(path: &Path) -> u64 {
    std::fs::metadata(path).unwrap().len()
}

/// Every segment of every journal in `store`, in
/// order of their paths.
fn segments(store: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(store.join("events")).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(jsonl(&path));
        }
    }
    found.sort();
    found
}

/// Every line of the segment `path`, read as JSON.
fn events(path: &Path) -> Vec<Json> {
    let text = std::fs::read_to_string(path).unwrap();
    text.lines().map(|line| parse_json(line).unwrap()).collect()
}

/// `f` of every job, run on `workers` threads at once, in the order of the jobs: what
/// `list(ThreadPoolExecutor(workers).map(f, jobs))` gives. A job that panics fails the test.
fn pool_map<J, R, F>(workers: usize, jobs: &[J], f: F) -> Vec<R>
where
    J: Sync,
    R: Send,
    F: Fn(&J) -> R + Sync,
{
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let threads: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::SeqCst);
                        let Some(job) = jobs.get(index) else {
                            return done;
                        };
                        done.push((index, f(job)));
                    }
                })
            })
            .collect();
        let mut done: Vec<(usize, R)> = threads
            .into_iter()
            .flat_map(|thread| thread.join().expect("a job panicked"))
            .collect();
        done.sort_by_key(|(index, _)| *index);
        done.into_iter().map(|(_, result)| result).collect()
    })
}

/// The block `ulimit -f` counts in, in bytes, which the shell decides: 512 in dash and in bash's
/// POSIX mode, 1024 in the bash 3.2 that is macOS's `/bin/sh`. Measured by a file written in `dir`
/// under a limit of one block, which stops there.
fn ulimit_block(dir: &Path) -> u64 {
    let probe = dir.join("ulimit-block");
    // dd ends at the limit, killed by SIGXFSZ or refused with EFBIG: either way the probe is full
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
    let block = size(&probe);
    assert!([512, 1024].contains(&block), "a block of {block} bytes");
    block
}

/// A reader skips only the unterminated fragment an interrupted append leaves at the end of a
/// segment: the issue reads as it was, and both events before the fragment still count.
#[test]
fn torn_tail_reader_ignores_only_unterminated_fragment() {
    let sandbox = crate::sandbox();
    let (a, key) = setup(&sandbox);
    let segment = own_segment(&a);
    append(&segment, TORN);
    assert_eq!(a.show(&key)["title"].as_str(), Some("Original issue"));
    assert_eq!(a.bead(["info"]).unwrap()["events"].as_i64(), Some(2));
}

/// An append after a torn tail does not poison the journal: the write succeeds, and a listing
/// reads both issues, the one written before the interruption and the one after it.
#[test]
fn bug_torn_tail_followed_by_successful_append_must_stay_readable() {
    let sandbox = crate::sandbox();
    let (a, key) = setup(&sandbox);
    let segment = own_segment(&a);
    append(&segment, TORN);
    assert_eq!(a.show(&key)["title"].as_str(), Some("Original issue"));
    let created = a.run(["create", "Acknowledged after interruption", "--json"]);
    assert_eq!(created.returncode(), 0, "{}", created.stderr);
    let read = a.run(["list", "--all", "--json"]);
    assert_eq!(
        read.returncode(),
        0,
        "successful append poisoned the journal: {}",
        read.stderr
    );
    assert_eq!(parse_json(&read.stdout).unwrap()["total"].as_i64(), Some(2));
}

/// A file-size limit cuts an append short, leaving a partial line while the previous issue
/// remains readable. The next write repairs it and subsequent reads succeed. A shell sets `ulimit
/// -f` before execing bead; measure its block units and choose the first whole-block limit at
/// least 100 bytes past the segment, well below the 100,000-byte description.
#[test]
fn bug_file_size_limit_partial_append_must_not_poison_next_write() {
    let sandbox = crate::sandbox();
    let (a, key) = setup(&sandbox);
    let segment = own_segment(&a);
    let block = ulimit_block(sandbox.root());
    let blocks = (size(&segment) + 100).div_ceil(block);
    let limit = blocks * block;
    let description = "x".repeat(100_000);
    let proc = Command::new("/bin/sh")
        .args(["-c", &format!("ulimit -f {blocks}; exec \"$@\""), "sh"])
        .arg(a.executable())
        .args([
            "create",
            "disk-full simulation",
            "-d",
            &description,
            "--json",
        ])
        .current_dir(a.checkout())
        .env_clear()
        .envs(a.env(None, "codex"))
        .envs([UNPROFILED])
        .output()
        .unwrap();
    assert!(!proc.status.success(), "{:?}", proc.status);
    assert_eq!(size(&segment), limit);
    assert_eq!(a.show(&key)["title"].as_str(), Some("Original issue"));
    let after = a.run(["create", "After full disk cleared", "--json"]);
    assert_eq!(after.returncode(), 0, "{}", after.stderr);
    let read = a.run(["list", "--all", "--json"]);
    assert_eq!(
        read.returncode(),
        0,
        "partial failed write poisons the next accepted append: {}",
        read.stderr
    );
}

/// Killing a writer during append must leave the journal readable and allow the next write and
/// sync. Pause every append at `append.mid`, after half the line is written and flushed, then
/// kill the process group. The paused writer still holds the store lock while a reader checks the
/// prior issue state.
#[test]
fn bug_kill_mid_append_then_successful_write_must_stay_readable() {
    let sandbox = crate::sandbox();
    let (a, key) = setup_with(&sandbox, crate::rust_only);
    let marker = sandbox.root().join("append-started");
    let spec = format!("append.mid@always=pause-until({})", marker.display());
    let writing = a.cmd(["create", "killed writer", "--json"]);
    let mut worker = Worker::start(writing.env("BEAD_FAILPOINTS", spec), ProcessSession::New);
    wait_marker(&sandbox.root().join("append-started.ready"), &mut worker);
    assert_eq!(a.show(&key)["title"].as_str(), Some("Original issue"));
    worker.kill().expect("the writer's process group");
    let killed = worker.communicate(Some(Duration::from_secs(5)));
    assert_eq!(killed.returncode(), -9, "{}", killed.stderr);
    assert_eq!(a.show(&key)["title"].as_str(), Some("Original issue"));
    let later = a.run(["create", "accepted after killed writer", "--json"]);
    assert_eq!(later.returncode(), 0, "{}", later.stderr);
    let result = a.run(["sync", "--json"]);
    assert_eq!(
        result.returncode(),
        0,
        "killed append and next accepted write prevent sync: {}",
        result.stderr
    );
}

/// Fifty creates from twelve writers at once, with a segment rotated every 600 bytes, lose no event
/// and keep one causal chain across the segments' boundaries: every event names the one before it
/// among its dependencies. Nothing waits, and a new machine reads the same tracker.
#[test]
fn parallel_rotation_preserves_causal_predecessor_and_all_events() {
    let sandbox = crate::sandbox();
    let (mut a, _) = setup(&sandbox);
    a.extra.insert("BEAD_ROTATE_BYTES".into(), "600".into());
    let jobs: Vec<String> = (0..50).map(|i| format!("parallel {i}")).collect();
    let results = pool_map(12, &jobs, |title| a.run(["create", title, "--json"]));
    let failed: Vec<&str> = results
        .iter()
        .filter(|p| p.returncode() != 0)
        .map(|p| p.stderr.as_str())
        .collect();
    assert!(failed.is_empty(), "{failed:?}");
    let segments = segments(&a.store());
    assert!(segments.len() > 10, "{} segments", segments.len());
    let events: Vec<Json> = segments.iter().flat_map(|p| events(p)).collect();
    assert_eq!(events.len(), 52);
    for pair in events.windows(2) {
        let (previous, event) = (&pair[0], &pair[1]);
        let deps = event["deps"].as_array();
        let deps = deps.unwrap_or_else(|| panic!("no deps in {event:?}"));
        assert!(
            deps.contains(&previous["id"]),
            "{previous:?} not in {deps:?}"
        );
    }
    assert_eq!(
        a.bead(["info"]).unwrap()["waiting_events"].as_i64(),
        Some(0)
    );
    a.bead(["sync"]);
    let b = Machine::new(&sandbox, "b");
    b.bead(["sync"]);
    assert_eq!(a.export(), b.export());
}

/// A segment deleted from the store after the journal was published leaves the events after it
/// waiting for their causes; the next sync restores it from the remote, and the tracker reads as
/// it did, with nothing waiting.
#[test]
fn deleted_local_segment_is_restored_from_remote() {
    let sandbox = crate::sandbox();
    let (mut a, _) = setup(&sandbox);
    a.extra.insert("BEAD_ROTATE_BYTES".into(), "200".into());
    for i in 0..4 {
        a.bead(["create", &format!("rotated {i}")]);
    }
    a.bead(["sync"]);
    let expected = a.export();
    let paths = segments(&a.store());
    std::fs::remove_file(&paths[1]).unwrap();
    let waiting = a.bead(["info"]).unwrap()["waiting_events"].as_i64();
    assert!(waiting.is_some_and(|waiting| waiting > 0), "{waiting:?}");
    a.bead(["sync"]);
    assert_eq!(a.export(), expected);
    assert_eq!(
        a.bead(["info"]).unwrap()["waiting_events"].as_i64(),
        Some(0)
    );
}
