//! What the state's own helpers answer of an event's actor.

use super::actor_label;
use crate::json::{Object, Value};

/// An actor that is absent, or not an object, names nobody: the label is the machine's human,
/// so no caller has to filter a falsy actor out first.
#[test]
fn an_actor_that_names_nobody_is_the_machines_human() {
    let empty = Value::Object(Box::default());
    for actor in [
        None,
        Some(&Value::Bool(false)),
        Some(&Value::from("")),
        Some(&empty),
    ] {
        assert_eq!(actor_label(actor, "m"), "human@m", "{actor:?}");
    }
    let mut named = Object::new();
    named.insert("agent".into(), Value::from("codex"));
    named.insert("thread".into(), Value::from("t1"));
    assert_eq!(actor_label(Some(&Value::from(named)), "m"), "codex:t1@m");
}
