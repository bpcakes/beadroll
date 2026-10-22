//! An event's envelope as read (roll-lqjj.53.1): the members every event has, read once into
//! typed members, so that a journal keeps nothing of a line but what replay reads, and validated
//! from what was read. A line is read by serde_json straight into an [`Envelope`] through its
//! `Deserialize`, with no value tree built for the line; an event built in memory walks into one
//! ([`Envelope::from_value`]). The payload stays the object it was written as: every operation
//! reads its own members of it ([`crate::op::Op::read`]).

use std::fmt;

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

use crate::id::EventId;
use crate::json::{Object, Value};
use crate::time::MAX_MILLIS;

/// A member that is text, null, absent, or something else.
#[derive(Clone, Debug, PartialEq, Default)]
pub(crate) enum Text {
    /// Absent.
    #[default]
    Absent,
    /// `null`.
    Null,
    /// A string.
    Str(String),
    /// Anything else.
    Other,
}

impl Text {
    /// The text, if it is one.
    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Text::Str(text) => Some(text),
            _ => None,
        }
    }

    /// `value`'s text, null, or neither.
    fn of(value: Value) -> Text {
        match value {
            Value::Null => Text::Null,
            Value::Str(text) => Text::Str(text),
            _ => Text::Other,
        }
    }
}

/// The `data` member: absent, null, an object, or something else.
#[derive(Clone, Debug, PartialEq, Default)]
pub(crate) enum Data {
    /// Absent.
    #[default]
    Absent,
    /// `null`.
    Null,
    /// An object.
    Object(Object),
    /// Anything else.
    Other,
}

impl Data {
    /// The payload, if it is an object.
    pub(crate) fn as_object(&self) -> Option<&Object> {
        match self {
            Data::Object(data) => Some(data),
            _ => None,
        }
    }

    /// `value` as the `data` member.
    fn of(value: Value) -> Data {
        match value {
            Value::Null => Data::Null,
            Value::Object(data) => Data::Object(*data),
            _ => Data::Other,
        }
    }
}

/// What was read of an event's envelope. A member the event does not have is absent; a member
/// bead does not read is skipped, read as a value and let go of, so that a line nested deeper than
/// any reader takes is refused wherever the nesting is (serde_json's own skip takes any depth); a
/// repeated member keeps its last value, as an object read as a value does.
#[derive(Clone, Debug, PartialEq, Default)]
pub(crate) struct Envelope {
    /// Whether the value was an object at all.
    pub(crate) object: bool,
    /// `id`, when it spells an event id.
    pub(crate) id: Option<EventId>,
    /// `op`, when it is text.
    pub(crate) op: Option<String>,
    /// `replica`, when it is text.
    pub(crate) replica: Option<String>,
    /// The members of `deps` that are event ids.
    pub(crate) deps: Vec<EventId>,
    /// Whether `deps` is absent or a list of event ids only.
    pub(crate) deps_valid: bool,
    /// `data`.
    pub(crate) data: Data,
    /// `v`, as written.
    pub(crate) v: Option<Value>,
    /// `issue`.
    pub(crate) issue: Text,
    /// `actor`, as written.
    pub(crate) actor: Option<Value>,
    /// `where`, as written.
    pub(crate) place: Option<Value>,
}

impl Envelope {
    /// An object's envelope before any member is read.
    fn empty() -> Envelope {
        Envelope {
            object: true,
            deps_valid: true,
            ..Envelope::default()
        }
    }

    /// `value` read as a line of it would be, each member taken from it.
    pub(crate) fn from_value(value: Value) -> Envelope {
        let Value::Object(members) = value else {
            return Envelope::default();
        };
        let mut read = Envelope::empty();
        for (name, value) in *members {
            match Member::named(&name) {
                Member::Id => read.id = Text::of(value).as_str().and_then(EventId::parse),
                Member::Op => read.op = Text::of(value).as_str().map(str::to_owned),
                Member::Replica => read.replica = Text::of(value).as_str().map(str::to_owned),
                Member::Deps => {
                    read.deps.clear();
                    read.deps_valid = false;
                    if let Value::Array(items) = value {
                        read.deps_valid = true;
                        for item in &items {
                            match item.as_str().and_then(EventId::parse) {
                                Some(id) => read.deps.push(id),
                                None => read.deps_valid = false,
                            }
                        }
                    }
                }
                Member::Data => read.data = Data::of(value),
                Member::V => read.v = Some(value),
                Member::Issue => read.issue = Text::of(value),
                Member::Actor => read.actor = Some(value),
                Member::Where => read.place = Some(value),
                Member::Other => {}
            }
        }
        read
    }

    /// Why this envelope cannot be used, or `None`: the rules of `event_problem`, in its order.
    pub(crate) fn problem(&self) -> Option<String> {
        if !self.object {
            return Some("event is not an object".into());
        }
        let Some(id) = self.id else {
            return Some("id is not a UUID".into());
        };
        if id.millis() > MAX_MILLIS {
            return Some("id carries an impossible timestamp".into());
        }
        if self.op.is_none() || self.replica.is_none() {
            return Some("op or replica missing".into());
        }
        if !self.deps_valid {
            return Some("deps is not a list of event ids".into());
        }
        if self.data == Data::Other {
            return Some("data is not an object".into());
        }
        if !self.v.as_ref().is_none_or(super::is_int) {
            return Some("v is not an integer".into());
        }
        if self.issue == Text::Other {
            return Some("issue is not an id".into());
        }
        let actor_valid = match &self.actor {
            None | Some(Value::Null) => true,
            Some(Value::Object(actor)) => ["agent", "thread", "name"]
                .iter()
                .all(|key| str_or_none(actor.get(*key))),
            Some(_) => false,
        };
        if !actor_valid {
            return Some("actor is malformed".into());
        }
        let where_valid = match &self.place {
            None | Some(Value::Null) => true,
            Some(Value::Object(place)) => {
                str_or_none(place.get("branch")) && str_or_none(place.get("commit"))
            }
            Some(_) => false,
        };
        if !where_valid {
            return Some("where is malformed".into());
        }
        None
    }

    /// Why its payload cannot be applied, or `None` (`payload_problem`). Asked only of an
    /// envelope [`Envelope::problem`] passes.
    pub(crate) fn payload_problem(&self) -> Option<String> {
        super::payload_problem_of(
            self.op.as_deref().unwrap_or_default(),
            self.data.as_object(),
            self.id,
            self.issue.as_str(),
        )
    }
}

/// Whether `value` is absent, null or text.
fn str_or_none(value: Option<&Value>) -> bool {
    matches!(value, None | Some(Value::Null | Value::Str(_)))
}

/// The members read, by name.
#[derive(Clone, Copy)]
enum Member {
    Id,
    Op,
    Replica,
    Deps,
    Data,
    V,
    Issue,
    Actor,
    Where,
    Other,
}

impl Member {
    fn named(name: &str) -> Member {
        match name {
            "id" => Member::Id,
            "op" => Member::Op,
            "replica" => Member::Replica,
            "deps" => Member::Deps,
            "data" => Member::Data,
            "v" => Member::V,
            "issue" => Member::Issue,
            "actor" => Member::Actor,
            "where" => Member::Where,
            _ => Member::Other,
        }
    }
}

impl<'de> Deserialize<'de> for Member {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Member, D::Error> {
        struct MemberVisitor;

        impl Visitor<'_> for MemberVisitor {
            type Value = Member;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a member name")
            }

            fn visit_str<E: serde::de::Error>(self, name: &str) -> Result<Member, E> {
                Ok(Member::named(name))
            }
        }

        deserializer.deserialize_str(MemberVisitor)
    }
}

/// A visitor for a member that keeps only a text, and notes anything else: every kind of value
/// serde_json gives is taken, so that a member of any kind reads, and only the shape is kept.
macro_rules! other_kinds {
    ($other:expr) => {
        fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
            Ok($other)
        }

        fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self::Value, E> {
            Ok($other)
        }

        fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self::Value, E> {
            Ok($other)
        }

        fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
            Ok($other)
        }
    };
}

/// A text member, read without keeping anything but the text.
struct TextMember(Text);

impl<'de> Deserialize<'de> for TextMember {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<TextMember, D::Error> {
        struct TextVisitor;

        impl<'de> Visitor<'de> for TextVisitor {
            type Value = TextMember;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("text, null or anything")
            }

            fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<TextMember, E> {
                Ok(TextMember(Text::Str(text.to_owned())))
            }

            fn visit_string<E: serde::de::Error>(self, text: String) -> Result<TextMember, E> {
                Ok(TextMember(Text::Str(text)))
            }

            fn visit_unit<E: serde::de::Error>(self) -> Result<TextMember, E> {
                Ok(TextMember(Text::Null))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<TextMember, A::Error> {
                while seq.next_element::<Value>()?.is_some() {}
                Ok(TextMember(Text::Other))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<TextMember, A::Error> {
                while map.next_entry::<String, Value>()?.is_some() {}
                Ok(TextMember(Text::Other))
            }

            other_kinds!(TextMember(Text::Other));
        }

        deserializer.deserialize_any(TextVisitor)
    }
}

/// The `deps` member: the ids among its items, read from their text without keeping it, and
/// whether every item was one.
struct DepsMember(Vec<EventId>, bool);

impl<'de> Deserialize<'de> for DepsMember {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<DepsMember, D::Error> {
        struct DepsVisitor;

        impl<'de> Visitor<'de> for DepsVisitor {
            type Value = DepsMember;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a list of event ids or anything")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<DepsMember, A::Error> {
                let mut ids = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                let mut valid = true;
                while let Some(TextMember(item)) = seq.next_element::<TextMember>()? {
                    match item.as_str().and_then(EventId::parse) {
                        Some(id) => ids.push(id),
                        None => valid = false,
                    }
                }
                Ok(DepsMember(ids, valid))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<DepsMember, A::Error> {
                while map.next_entry::<String, Value>()?.is_some() {}
                Ok(DepsMember(Vec::new(), false))
            }

            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<DepsMember, E> {
                Ok(DepsMember(Vec::new(), false))
            }

            fn visit_unit<E: serde::de::Error>(self) -> Result<DepsMember, E> {
                Ok(DepsMember(Vec::new(), false))
            }

            other_kinds!(DepsMember(Vec::new(), false));
        }

        deserializer.deserialize_any(DepsVisitor)
    }
}

/// The `data` member: its object read as a value, anything else noted.
struct DataMember(Data);

impl<'de> Deserialize<'de> for DataMember {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<DataMember, D::Error> {
        Value::deserialize(deserializer).map(|value| DataMember(Data::of(value)))
    }
}

impl<'de> Deserialize<'de> for Envelope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Envelope, D::Error> {
        struct EnvelopeVisitor;

        impl<'de> Visitor<'de> for EnvelopeVisitor {
            type Value = Envelope;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an event")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Envelope, A::Error> {
                let mut read = Envelope::empty();
                while let Some(member) = map.next_key::<Member>()? {
                    match member {
                        Member::Id => {
                            let TextMember(text) = map.next_value()?;
                            read.id = text.as_str().and_then(EventId::parse);
                        }
                        Member::Op => {
                            let TextMember(text) = map.next_value()?;
                            read.op = text.as_str().map(str::to_owned);
                        }
                        Member::Replica => {
                            let TextMember(text) = map.next_value()?;
                            read.replica = text.as_str().map(str::to_owned);
                        }
                        Member::Deps => {
                            let DepsMember(ids, valid) = map.next_value()?;
                            read.deps = ids;
                            read.deps_valid = valid;
                        }
                        Member::Data => read.data = map.next_value::<DataMember>()?.0,
                        Member::V => read.v = Some(map.next_value()?),
                        Member::Issue => read.issue = map.next_value::<TextMember>()?.0,
                        Member::Actor => read.actor = Some(map.next_value()?),
                        Member::Where => read.place = Some(map.next_value()?),
                        Member::Other => {
                            map.next_value::<Value>()?;
                        }
                    }
                }
                Ok(read)
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Envelope, A::Error> {
                while seq.next_element::<Value>()?.is_some() {}
                Ok(Envelope::default())
            }

            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Envelope, E> {
                Ok(Envelope::default())
            }

            fn visit_unit<E: serde::de::Error>(self) -> Result<Envelope, E> {
                Ok(Envelope::default())
            }

            other_kinds!(Envelope::default());
        }

        deserializer.deserialize_any(EnvelopeVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::{Format, MAX_DEPTH, parse, write};

    /// `text` read as a line, and read as the value it parses to: both ways must agree.
    fn both_ways(text: &str) -> Envelope {
        let as_line: Envelope = serde_json::from_slice(text.as_bytes()).unwrap();
        let as_value = Envelope::from_value(parse(text.as_bytes()).unwrap().value);
        assert_eq!(as_line, as_value, "{text}");
        as_line
    }

    const ID: &str = "01a11621-e285-7398-ab1c-b9af4864eaa2";
    const OTHER: &str = "01a11621-e285-7398-ab1c-b9af4864eaa3";

    #[test]
    fn every_member_is_read_as_the_value_reads_it() {
        let text = format!(
            r#"{{"v": 1, "id": "{ID}", "replica": "a", "deps": ["{OTHER}"], "actor": {{"agent": "x"}}, "op": "set", "issue": "{ID}", "data": {{"fields": {{"title": "t"}}}}, "where": {{"branch": "main"}}, "extra": [1, {{}}]}}"#
        );
        let read = both_ways(&text);
        assert!(read.object);
        assert_eq!(read.id, EventId::parse(ID));
        assert_eq!(read.op.as_deref(), Some("set"));
        assert_eq!(read.replica.as_deref(), Some("a"));
        assert_eq!(read.deps, [EventId::parse(OTHER).unwrap()]);
        assert!(read.deps_valid);
        assert_eq!(read.v, Some(Value::from(1)));
        assert_eq!(read.issue, Text::Str(ID.to_owned()));
        assert_eq!(
            read.data.as_object().and_then(|d| d.get("fields")),
            parse(br#"{"title": "t"}"#).ok().map(|p| p.value).as_ref()
        );
        assert_eq!(
            read.actor,
            parse(br#"{"agent": "x"}"#).ok().map(|p| p.value)
        );
        assert_eq!(
            read.place,
            parse(br#"{"branch": "main"}"#).ok().map(|p| p.value)
        );
        assert_eq!(read.problem(), None);
        assert_eq!(read.payload_problem(), None);
    }

    #[test]
    fn members_of_other_kinds_are_noted_not_kept() {
        let read = both_ways(
            r#"{"id": 5, "op": null, "replica": [1, 2], "deps": {"a": [1]}, "data": "x", "issue": {"k": null}, "v": "1"}"#,
        );
        assert!(read.object);
        assert_eq!(read.id, None);
        assert_eq!(read.op, None);
        assert_eq!(read.replica, None);
        assert!(read.deps.is_empty() && !read.deps_valid);
        assert_eq!(read.data, Data::Other);
        assert_eq!(read.issue, Text::Other);
        assert_eq!(read.v, Some(Value::from("1")));
        let read = both_ways(r#"{"deps": [true, 1.5, null, "x"], "issue": null, "data": null}"#);
        assert!(!read.deps_valid);
        assert_eq!(read.issue, Text::Null);
        assert_eq!(read.data, Data::Null);
        assert_eq!(read.problem().as_deref(), Some("id is not a UUID"));
    }

    #[test]
    fn the_ids_among_the_deps_are_kept_when_another_item_is_not_one() {
        let read = both_ways(&format!(r#"{{"deps": ["{ID}", 7, "{OTHER}"]}}"#));
        assert_eq!(
            read.deps,
            [EventId::parse(ID).unwrap(), EventId::parse(OTHER).unwrap()]
        );
        assert!(!read.deps_valid);
    }

    #[test]
    fn a_line_that_is_no_object_is_no_event() {
        for text in [
            "[1, 2]",
            "\"x\"",
            "null",
            "true",
            "7",
            "-1.5",
            "18446744073709551615",
        ] {
            let read = both_ways(text);
            assert!(!read.object, "{text}");
            assert_eq!(
                read.problem().as_deref(),
                Some("event is not an object"),
                "{text}"
            );
        }
    }

    #[test]
    fn a_repeated_member_keeps_its_last_value() {
        let read = both_ways(&format!(
            r#"{{"id": "{ID}", "id": "x", "op": 1, "op": "set", "deps": [1], "deps": []}}"#
        ));
        assert_eq!(read.id, None);
        assert_eq!(read.op.as_deref(), Some("set"));
        assert!(read.deps.is_empty() && read.deps_valid);
    }

    #[test]
    fn an_envelope_read_from_a_value_is_the_one_read_from_its_line() {
        // Members of every kind, nested values, escapes, repeated keys and a key bead does not read.
        let texts = [
            format!(
                r#"{{"v": true, "id": "{ID}", "replica": "mé", "deps": [], "actor": null, "op": "claim", "issue": null, "data": {{"generation": 3, "mode": "offline", "x": [1, {{"y": 2.5}}]}}, "where": null, "where": {{"commit": "c"}}}}"#
            ),
            r#"{"v": 1.0, "data": {"a": {"b": {"c": [[[]]]}}, "a": 1}, "unknown": {"deep": [1, 2]}}"#
                .to_owned(),
            r#"{}"#.to_owned(),
        ];
        for text in texts {
            let read = both_ways(&text);
            // and the line bead writes of the value reads back the same
            let written = write(&parse(text.as_bytes()).unwrap().value, Format::COMPACT).unwrap();
            assert_eq!(both_ways(&written), read, "{text}");
        }
    }

    #[test]
    fn a_line_nested_too_deep_is_refused_wherever_the_nesting_is() {
        // What a value refuses, the envelope refuses: a skipped member is read as a value, since
        // serde_json's own skip takes any depth.
        let nested = |depth: usize| format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        let (deep, fine) = (nested(MAX_DEPTH), nested(MAX_DEPTH - 1));
        let lines = [
            format!(r#"{{"unknown": {deep}}}"#),
            format!(r#"{{"issue": {deep}}}"#),
            format!(r#"{{"deps": {{"a": {deep}}}}}"#),
            format!(r#"{{"data": {{"a": {deep}}}}}"#),
            format!(r#"{{"actor": {{"a": {deep}}}}}"#),
            deep.clone(),
            format!(r#"{{"unknown": {fine}}}"#),
            format!(r#"{{"issue": {fine}}}"#),
            format!(r#"{{"id": 1, "unknown": {fine}}}"#),
        ];
        for line in lines {
            let as_value = parse(line.as_bytes()).is_ok();
            let as_envelope = crate::json::deserialize::<Envelope>(line.as_bytes()).is_ok();
            assert_eq!(as_value, as_envelope, "{line}");
        }
        assert!(parse(format!(r#"{{"unknown": {deep}}}"#).as_bytes()).is_err());
        assert!(parse(format!(r#"{{"unknown": {fine}}}"#).as_bytes()).is_ok());
    }
}
