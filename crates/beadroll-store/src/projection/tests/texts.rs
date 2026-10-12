//! Each issue's texts, kept apart from the projection's state (roll-lqjj.6): read in only where a
//! command needs them, a write adding its own to the text file, and a text file damaged, cut, gone
//! or another's never trusted, while what a list shows needs none of it.

use super::*;

use crate::projection::{SLACK, outline, state, texts_path};

fn described(n: u128, deps: &[u128], issue: u128, text: &str) -> Value {
    let mut fields = Object::new();
    fields.insert("description".into(), Value::from(text));
    let mut data = Object::new();
    data.insert("fields".into(), Value::from(fields));
    event(n, deps, "set", issue, data)
}

fn comment(n: u128, deps: &[u128], issue: u128, text: &str) -> Value {
    let mut data = Object::new();
    data.insert("text".into(), Value::from(text));
    event(n, deps, "comment", issue, data)
}

/// [`begun`], its first issue described and commented on: the frontier is event 6.
fn written(ws: &Workspace) {
    begun(ws);
    let events = [
        described(5, &[4], 2, "why it matters, at length"),
        comment(6, &[5], 2, "noted"),
    ];
    append_events(ws, HERE, &events).unwrap();
}

/// What a list shows of each issue of `state`, by key.
fn listed(state: &State) -> Vec<String> {
    (state.issues_by_key().iter())
        .map(|issue| format!("{:?}", state.brief(issue)))
        .collect()
}

fn uid(n: u128) -> EventId {
    EventId::parse(&id(n)).unwrap()
}

#[test]
fn texts_are_read_in_only_where_a_command_needs_them() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    written(&ws);
    assert!(replays(&ws));
    assert!(texts_path(&ws).is_file());
    let outlined = outline(&ws).unwrap();
    assert!(outlined.issues().all(|issue| !issue.texts().here()));
    let whole = state(&ws).unwrap();
    assert!(whole.issues().all(|issue| issue.texts().here()));
    assert_eq!(listed(&outlined), listed(&whole));
    // one issue's read in, and the other's still there
    let mut one = outline(&ws).unwrap();
    assert!(read_texts(&ws, &mut one, [uid(2)]));
    let first = one.issue(uid(2)).unwrap();
    assert!(first.texts().here());
    assert_eq!(first.comments().len(), 1);
    assert_eq!(first, whole.issue(uid(2)).unwrap());
    assert!(!one.issue(uid(3)).unwrap().texts().here());
}

#[test]
fn a_write_adds_its_texts_to_the_file() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    written(&ws);
    assert!(replays(&ws));
    let before = std::fs::read(texts_path(&ws)).unwrap();
    append_events(&ws, HERE, &[comment(7, &[6], 3, "another")]).unwrap();
    assert!(replays(&ws));
    let after = std::fs::read(texts_path(&ws)).unwrap();
    assert!(after.len() > before.len());
    assert_eq!(
        after[..before.len()],
        before[..],
        "appended to, nothing written again"
    );
}

#[test]
fn a_text_file_damaged_cut_gone_or_anothers_is_never_trusted() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    written(&ws);
    assert!(replays(&ws));
    let replayed = listed(&replay(load_journal(&ws).unwrap()).unwrap());
    let path = texts_path(&ws);
    let good = std::fs::read(&path).unwrap();
    // every cut, and a byte changed anywhere: its generation changed is another's file
    let mut damaged: Vec<Vec<u8>> = (0..good.len()).map(|cut| good[..cut].to_vec()).collect();
    for at in 0..good.len() {
        let mut flipped = good.clone();
        flipped[at] ^= 0x01;
        damaged.push(flipped);
    }
    for bytes in damaged {
        std::fs::write(&path, &bytes).unwrap();
        // what a list shows needs none of it
        assert_eq!(listed(&outline(&ws).unwrap()), replayed);
        // texts that cannot be read in are no texts: a write about them would replay
        let mut projected = projection(&ws).unwrap().into_state();
        assert!(!read_texts(&ws, &mut projected, [uid(2), uid(3)]));
        // a command that reads them all replays, and writes both files again
        assert_eq!(listed(&view(&ws).unwrap()), replayed);
        assert_eq!(std::fs::read(&path).unwrap(), good);
        assert!(replays(&ws));
    }
    std::fs::remove_file(&path).unwrap();
    assert_eq!(listed(&view(&ws).unwrap()), replayed);
    assert_eq!(std::fs::read(&path).unwrap(), good);
    assert!(replays(&ws));
}

#[test]
fn a_text_file_mostly_no_longer_read_is_started_again() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    written(&ws);
    assert!(replays(&ws));
    // the first issue described again and again, at length, each time caught up alone
    let long = "x".repeat(64 * 1024);
    let rewrites = 2 * SLACK / (64 * 1024) + 4;
    for n in 0..rewrites {
        let at = 7 + n as u128;
        append_events(&ws, HERE, &[described(at, &[at - 1], 2, &long)]).unwrap();
        projection(&ws).unwrap();
    }
    let len = std::fs::metadata(texts_path(&ws)).unwrap().len();
    assert!(
        len < 2 * SLACK,
        "{len} bytes after {rewrites} descriptions of 64 KiB"
    );
    assert!(replays(&ws));
}
