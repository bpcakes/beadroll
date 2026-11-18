use super::{COMMENT_ID_BASE, comment_id};
use crate::id::EventId;

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
