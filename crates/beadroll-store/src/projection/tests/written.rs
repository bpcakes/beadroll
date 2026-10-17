//! What a write names of the journal's files it leaves (roll-lqjj.51, [`after_write`]): the files
//! its projection was made from as they were, its own segment grown by the events it appended, in
//! their order, or begun with them, and the attempt file of the ownership events it confirmed, each
//! with its identity now and the sum of its bytes. Anything else, and it names nothing.

use super::super::{Basis, after_write, projection_sourced};
use super::*;

/// The store's projection's basis, as a write starts from it.
fn basis(ws: &Workspace) -> Basis {
    projection_sourced(ws)
        .unwrap()
        .into_parts()
        .1
        .expect("a basis")
}

/// Each file a basis names: its identity's length and its sum, by path.
fn named(basis: &Basis) -> BTreeMap<String, (u64, Option<[u8; 16]>)> {
    (basis.made.files.iter())
        .map(|held| (held.path.clone(), (held.key.len(), held.sum)))
        .collect()
}

fn eid(n: u128) -> EventId {
    EventId::parse(&id(n)).unwrap()
}

#[test]
fn a_write_names_its_own_segment_as_it_left_it() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    begun(&ws);
    let found = basis(&ws);
    // nothing written: the files as they were
    assert_eq!(
        named(&after_write(&ws, &found, &[], &[]).unwrap()),
        named(&found)
    );
    append_events(
        &ws,
        HERE,
        &[title(5, &[4], 2, "five"), title(6, &[5], 3, "six")],
    )
    .unwrap();
    let left = after_write(&ws, &found, &[eid(5), eid(6)], &[]).expect("its own");
    let path = format!("{HERE}/000001.jsonl");
    let bytes = std::fs::read(segment(&ws)).unwrap();
    let mut expected = named(&found);
    expected.insert(
        path.clone(),
        (bytes.len() as u64, Some(content_sum(&bytes))),
    );
    assert_eq!(named(&left), expected);
    let held = (left.made.files.iter())
        .find(|held| held.path == path)
        .unwrap();
    assert_eq!(Some(held.key), FileKey::of(&segment(&ws)));
    // not its events, not all of them, not in their order, or more than its own
    for appended in [
        &[eid(5)][..],
        &[eid(6), eid(5)],
        &[eid(5), eid(6), eid(7)],
        &[eid(7), eid(8)],
    ] {
        assert!(
            after_write(&ws, &found, appended, &[]).is_none(),
            "{appended:?}"
        );
    }
    // nothing it says it wrote, where the journal grew
    assert!(after_write(&ws, &found, &[], &[]).is_none());
}

#[test]
fn a_write_names_nothing_when_another_file_changed() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    begun(&ws);
    let found = basis(&ws);
    let mut data = Object::new();
    data.insert("mode".into(), Value::from("online"));
    data.insert("generation".into(), Value::from(1));
    let claim = event(5, &[4], "claim", 2, data);
    let claims = ws.events_dir.join(HERE).join("claims");
    let attempt = claims.join(format!("{}.json", id(5)));
    // an attempt file it did not confirm
    write_attempt_file(&ws, &attempt, std::slice::from_ref(&claim)).unwrap();
    ws.forget_claims(&attempt);
    assert!(after_write(&ws, &found, &[], &[]).is_none());
    // the one it did: named, with the sum of its bytes
    let left = after_write(&ws, &found, &[], std::slice::from_ref(&claim)).expect("its own");
    let bytes = std::fs::read(&attempt).unwrap();
    let path = format!("{HERE}/claims/{}.json", id(5));
    assert_eq!(
        named(&left).get(&path),
        Some(&(bytes.len() as u64, Some(content_sum(&bytes))))
    );
    // another machine's segment begun, beside its own record
    let found = basis(&ws);
    let theirs = ws.events_dir.join("0000bb02");
    std::fs::create_dir_all(&theirs).unwrap();
    let mut other = title(6, &[5], 3, "theirs");
    if let Value::Object(object) = &mut other {
        object.insert("replica".into(), Value::from("0000bb02"));
    }
    append_events(&ws, "0000bb02", &[other]).unwrap();
    append_events(&ws, HERE, &[title(7, &[5], 2, "ours")]).unwrap();
    assert!(after_write(&ws, &found, &[eid(7)], &[]).is_none());
    // its own segment's bytes before the ones it appended rewritten in place
    let ws = store(tempfile::tempdir().unwrap().path(), Some("this build"));
    begun(&ws);
    let found = basis(&ws);
    let data = std::fs::read_to_string(segment(&ws)).unwrap();
    std::fs::write(segment(&ws), data.replace("renamed", "RENAMED")).unwrap();
    append_events(&ws, HERE, &[title(5, &[4], 2, "five")]).unwrap();
    assert!(after_write(&ws, &found, &[eid(5)], &[]).is_none());
}
