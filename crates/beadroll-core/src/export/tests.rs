use std::collections::HashSet;

use super::{COMMENT_ID_BASE, comment_id, exported_legacy_id};
use crate::id::EventId;
use crate::json::{Int, Object, Value};

/// A comment made here exports a 48-bit id from 2^47, clear of the small ids `br` mints.
#[test]
fn a_derived_comment_id_lies_in_48_bits_from_two_to_the_47th() {
    assert_eq!(COMMENT_ID_BASE, 1 << 47);
    for n in [0_u64, 1, 2, 0xffff, 0xdead_beef, u64::from(u32::MAX)] {
        let uid = EventId::parse(&format!("019a1111-1111-7000-8000-{n:012x}")).unwrap();
        let id = comment_id(uid);
        assert!((1 << 47..1 << 48).contains(&id), "{uid}: {id}");
    }
}

/// An imported comment's id moved off one taken is taken in its turn: the next imported comment
/// that held that id moves on past it, so no two comments export one id.
#[test]
fn an_imported_id_moved_on_is_taken_in_its_turn() {
    let imported = |id: u64| {
        let mut comment = Object::new();
        comment.insert("legacy_id".into(), Value::Int(Int::from_u64(id)));
        comment
    };
    let ours = 1000;
    let mut taken = HashSet::from([ours]);
    // the first collides with ours and moves to 1001; the second held 1001 and moves to 1002
    let first = exported_legacy_id(&imported(ours), &mut taken);
    let second = exported_legacy_id(&imported(ours + 1), &mut taken);
    assert_eq!(first, Value::Int(Int::from_u64(ours + 1)));
    assert_eq!(second, Value::Int(Int::from_u64(ours + 2)));
    // one that collides with nothing keeps its id
    assert_eq!(
        exported_legacy_id(&imported(7), &mut taken),
        Value::Int(Int::from_u64(7))
    );
}
