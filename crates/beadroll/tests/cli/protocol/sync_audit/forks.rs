//! Forked journals: a copy of another machine's published journal rewritten, and one machine's
//! identity in two places, whose histories part across a segment boundary.

use std::path::Path;
use std::process::Command;

use beadroll_core::json::{Format, Style, write};
use beadroll_test_support::{Json, Machine, Run, Sandbox, code, parse_json};

use super::{own_segment, remote_blob, setup, two};

/// Compact, ASCII-escaped JSON with members in insertion order.
const DUMPS_COMPACT: Format = Format {
    style: Style::Compact,
    ascii: true,
    sorted: false,
};

/// `to`, which must not exist yet, becomes a copy of the directory
/// `from`, its files' modes and times kept.
fn copytree(from: &Path, to: &Path) {
    assert!(!to.exists(), "{} exists", to.display());
    let status = Command::new("cp")
        .arg("-pR")
        .arg(from)
        .arg(to)
        .status()
        .unwrap();
    assert!(status.success(), "cp -pR {} failed", from.display());
}

/// The scenario both copied-identity tests share: a publishes to b, and its home is copied to a
/// twin, a restored backup or a cloned disk with a's identity. The original starts a new segment
/// (`BEAD_ROTATE_BYTES=1`), while the restored copy still appends the old one; each retitles the
/// issue offline, a publishes, then the twin syncs. The twin, the issue's key and the twin's sync.
fn copied_identity_retitles(sandbox: &Sandbox) -> (Machine, String, Run) {
    let (mut a, key) = setup(sandbox);
    two(sandbox, &a);
    let mut twin = Machine::new(sandbox, "twin");
    copytree(a.home(), twin.home());
    // Original starts a new segment, while restored copy still appends the old one.
    a.extra.insert("BEAD_ROTATE_BYTES".into(), "1".into());
    twin.extra
        .insert("BEAD_ROTATE_BYTES".into(), "1000000".into());
    let title = ["--title", "copied identity writer", "--offline"];
    twin.bead(["update", &key].into_iter().chain(title));
    let title = ["--title", "original writer", "--offline"];
    a.bead(["update", &key].into_iter().chain(title));
    a.bead(["sync"]);
    let result = twin.run(["sync", "--json"]);
    (twin, key, result)
}

/// A machine whose copy of another machine's published journal was rewritten either refuses to
/// sync with JOURNAL_FORK or reports that journal among its forked ones; either way the remote keeps
/// the good copy, which the rewritten one never replaces, and the journal's owner syncs on.
#[test]
fn bug_foreign_fork_report_must_not_replace_published_journal() {
    let sandbox = crate::sandbox();
    let (a, _) = setup(&sandbox);
    let b = two(&sandbox, &a);
    let segment = own_segment(&a);
    let path = segment.strip_prefix(a.store()).unwrap();
    let path = path.to_str().unwrap().to_owned();
    let remote_before = remote_blob(&sandbox, &path);
    let foreign = b.store().join(&path);
    let text = std::fs::read_to_string(&foreign).unwrap();
    let mut rows: Vec<Json> = text.lines().map(|row| parse_json(row).unwrap()).collect();
    retitle(rows.last_mut().unwrap(), "rewritten foreign history");
    let rows: Vec<String> = rows
        .iter()
        .map(|row| write(row, DUMPS_COMPACT).unwrap() + "\n")
        .collect();
    std::fs::write(&foreign, rows.concat()).unwrap();
    let result = b.run(["sync", "--json"]);
    if result.returncode() != 0 {
        assert_eq!(code(&result), "JOURNAL_FORK");
    } else {
        let report = parse_json(&result.stdout).unwrap();
        let forked = Json::Array(vec![Json::from(path.as_str())]);
        assert_eq!(report["forked_journals"], forked);
    }
    assert_eq!(
        remote_blob(&sandbox, &path),
        remote_before,
        "sync reported a foreign fork but then replaced the good remote copy with its local fork"
    );
    assert_eq!(a.run(["sync", "--json"]).returncode(), 0);
}

/// Assigns `row.data.fields.title`; missing members fail the test.
fn retitle(row: &mut Json, title: &str) {
    let fields = match row {
        Json::Object(row) => match row.get_mut("data") {
            Some(Json::Object(data)) => data.get_mut("fields"),
            _ => None,
        },
        _ => None,
    };
    let Some(Json::Object(fields)) = fields else {
        panic!("no data.fields in the last row");
    };
    fields.insert("title".into(), Json::from(title));
}

/// A copy of a machine's home that writes on in the segment the original has since rotated away
/// from forks one machine's journal over two segments: the copy's sync is refused with
/// JOURNAL_FORK, never accepted without an integrity warning.
#[test]
fn bug_copied_machine_id_fork_across_segments_is_detected() {
    let sandbox = crate::sandbox();
    let (_, _, result) = copied_identity_retitles(&sandbox);
    assert_ne!(
        result.returncode(),
        0,
        "a shared-machine causal fork split over two segments was accepted without an integrity \
         warning"
    );
    assert_eq!(code(&result), "JOURNAL_FORK");
}

/// The same fork, both sides retitling one issue: the copy's sync is refused with JOURNAL_FORK,
/// or, accepted, shows the two titles as a conflict. The order of the segments never invents an
/// ancestry that would let one title erase the other.
#[test]
fn bug_segment_boundary_fork_must_not_erase_concurrent_title() {
    let sandbox = crate::sandbox();
    let (twin, key, result) = copied_identity_retitles(&sandbox);
    if result.returncode() != 0 {
        assert_eq!(code(&result), "JOURNAL_FORK");
        return;
    }
    let shown = twin.show(&key);
    // Conflicts are an object keyed by field; the title must be disputed.
    let conflicts = shown.get("conflicts").and_then(Json::as_object);
    assert!(
        conflicts.is_some_and(|conflicts| conflicts.contains_key("title")),
        "concurrent title vanished because segment order invented ancestry: {}",
        write(&shown, Format::ASCII).unwrap()
    );
}
