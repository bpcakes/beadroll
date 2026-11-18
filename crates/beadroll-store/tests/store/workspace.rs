//! What a workspace keeps of the checkout it was made for.

use beadroll_core::json::Object;
use beadroll_store::Workspace;

/// An origin given as empty is none, so that no reader has to tell the two apart; a real one is
/// kept as it was given.
#[test]
fn an_empty_origin_is_none() {
    let home = tempfile::tempdir().unwrap();
    let make = |origin: &str| {
        let settings = crate::settings(home.path(), &[]);
        Workspace::new(
            settings,
            "origins",
            None,
            Some(origin.into()),
            Object::new(),
        )
        .unwrap()
    };
    assert_eq!(make("").origin, None);
    assert_eq!(make("../remote.git").origin, Some("../remote.git".into()));
}
