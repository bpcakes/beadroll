//! Beads JSONL to import: records new and known, with every field Beads writes, and the ways a file
//! is malformed, too deep, or holds what bead refuses.

use beadroll_core::json::{Format, Object, Value, write};

use super::model::World;

fn text(value: &str) -> Value {
    Value::from(value)
}

fn obj(members: Vec<(&str, Value)>) -> Value {
    Value::Object(
        members
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
    )
}

/// Where a hostile token goes in a written record.
const MARK: &str = "\u{1}MARK\u{1}";

impl World<'_> {
    fn record_id(&mut self, number: usize, known: &[String]) -> Value {
        if self.hostile(12) {
            return [Value::Null, text(""), Value::from(5), text("ex-i0")][self.tape.below(4)]
                .clone();
        }
        if self.tape.chance(15) {
            return self
                .tape
                .pick(known)
                .map_or_else(|| text("ex-old"), |key| text(key));
        }
        text(&format!("ex-i{number}"))
    }

    fn record_value(&mut self) -> Value {
        match self.tape.below(10) {
            0 => Value::Null,
            1 => Value::Bool(true),
            2 => Value::from(-3),
            3 => Value::Float(2.5),
            4 => obj(vec![(
                "nested",
                obj(vec![("deeper", Value::Array(vec![Value::from(1)]))]),
            )]),
            5 if self.hostile(40) => text(MARK),
            _ => text(
                super::model::TEXTS
                    .get(self.tape.below(super::model::TEXTS.len()))
                    .copied()
                    .unwrap_or("x"),
            ),
        }
    }

    fn record(&mut self, number: usize, known: &[String], others: &[String]) -> Object {
        let mut record = Object::new();
        if !self.hostile(3) {
            record.insert("id".into(), self.record_id(number, known));
        }
        self.record_fields(&mut record);
        if self.tape.chance(30) {
            let labels = [
                Value::Array(vec![text("bug"), text("p1")]),
                Value::Array(vec![Value::from(1)]),
                text("bug"),
                Value::Null,
            ];
            let plain = if self.hostile(30) {
                self.tape.below(labels.len())
            } else {
                0
            };
            record.insert(
                "labels".into(),
                labels.get(plain).cloned().unwrap_or(Value::Null),
            );
        }
        if self.tape.chance(40) {
            let count = self.tape.below(3);
            let deps: Vec<Value> = (0..count)
                .map(|_| self.record_dependency(number, known, others))
                .collect();
            record.insert(
                "dependencies".into(),
                if self.hostile(4) {
                    text("x")
                } else {
                    Value::Array(deps)
                },
            );
        }
        if self.tape.chance(25) {
            let comments = if self.hostile(5) {
                Value::Array(vec![text("x")])
            } else {
                Value::Array(vec![self.record_comment()])
            };
            record.insert("comments".into(), comments);
        }
        for _ in 0..self.tape.below(3) {
            let names = [
                "source_repo",
                "source_repo_path",
                "compaction_level",
                "x-custom",
                "ünï",
            ];
            let name = names
                .get(self.tape.below(names.len()))
                .copied()
                .unwrap_or("x");
            let value = self.record_value();
            record.insert(name.into(), value);
        }
        record
    }

    /// A record's fields: mostly the plainest of each, now and then any.
    fn record_fields(&mut self, record: &mut Object) {
        let fields = [
            (
                "title",
                vec![text("imported"), text(""), Value::Null, Value::from(1)],
            ),
            (
                "status",
                vec![
                    text("open"),
                    text("closed"),
                    text("review"),
                    text("Bad Status"),
                    Value::Null,
                ],
            ),
            (
                "priority",
                vec![
                    Value::from(2),
                    Value::from(0),
                    Value::Bool(true),
                    Value::from(9),
                    text("1"),
                ],
            ),
            ("issue_type", vec![text("task"), text("epic"), text("bug")]),
            (
                "created_at",
                vec![text("2025-01-01T00:00:00Z"), Value::Null, text("")],
            ),
            ("created_by", vec![text("someone"), Value::Null]),
            (
                "updated_at",
                vec![text("2025-02-01T00:00:00Z"), Value::Null],
            ),
            ("closed_at", vec![text("2025-03-01T00:00:00Z"), Value::Null]),
            ("close_reason", vec![text("done"), Value::Null]),
            (
                "estimated_minutes",
                vec![
                    Value::from(30),
                    Value::from(-1),
                    Value::from(2_147_483_648),
                    Value::Bool(false),
                ],
            ),
            ("assignee", vec![text("ann"), Value::Null]),
            ("description", vec![text("a\nb"), Value::Null]),
            ("external_ref", vec![text("gh-1")]),
        ];
        for (name, values) in fields {
            let often = if name == "title" || name == "status" {
                85
            } else {
                30
            };
            if self.tape.chance(often) {
                let plain = if self.hostile(30) {
                    self.tape.below(values.len())
                } else {
                    0
                };
                record.insert(
                    name.into(),
                    values.get(plain).cloned().unwrap_or(Value::Null),
                );
            }
        }
    }

    /// A comment as Beads writes one, with a member bead does not carry over.
    fn record_comment(&mut self) -> Value {
        let ids = if self.hostile(20) { 4 } else { 3 };
        let id = [Value::from(7), text("c-1"), Value::Null, Value::Bool(true)]
            .get(self.tape.below(ids))
            .cloned();
        let author = [text("bob"), Value::Null].get(self.tape.below(2)).cloned();
        let shown = if self.hostile(30) {
            self.tape.below(3)
        } else {
            0
        };
        let body = [text("hello"), Value::from(3), Value::Null]
            .get(shown)
            .cloned();
        let created = [text("2025-01-02T00:00:00Z"), Value::Null]
            .get(self.tape.below(2))
            .cloned();
        obj(vec![
            ("id", id.unwrap_or(Value::Null)),
            ("issue_id", text("ex-x")),
            ("author", author.unwrap_or(Value::Null)),
            ("text", body.unwrap_or(Value::Null)),
            ("created_at", created.unwrap_or(Value::Null)),
            ("uid", text("not carried")),
        ])
    }

    fn record_dependency(&mut self, number: usize, known: &[String], others: &[String]) -> Value {
        let target = match self.tape.below(8) {
            0 if self.hostile(20) => text(&format!("ex-i{number}")),
            1 | 2 => self
                .tape
                .pick(known)
                .map_or_else(|| text("ex-gone"), |key| text(key)),
            3 => text("ex-late"),
            4 if self.hostile(20) => Value::Null,
            _ => self
                .tape
                .pick(others)
                .filter(|key| **key != format!("ex-i{number}"))
                .map_or_else(|| text("ex-late"), |key| text(key)),
        };
        let kind = [
            text("blocks"),
            text("parent-child"),
            Value::Null,
            text(""),
            Value::from(1),
        ];
        let choice = if self.hostile(20) {
            self.tape.below(kind.len())
        } else {
            self.tape.below(3)
        };
        let kind = kind.get(choice).cloned().unwrap_or(Value::Null);
        let mut dep = vec![
            ("issue_id", text(&format!("ex-i{number}"))),
            ("depends_on_id", target),
            ("type", kind),
        ];
        if self.tape.chance(40) {
            dep.push(("created_at", text("2025-01-01T00:00:00Z")));
            dep.push(("created_by", text("someone")));
            dep.push(("metadata", text("{}")));
            dep.push(("thread_id", text("")));
        }
        obj(dep)
    }

    /// A token where a record's value would be: numbers at the edges, an escaped lone surrogate, or
    /// nesting around the import's limit.
    fn record_token(&mut self) -> String {
        let deep = |depth: usize| format!("{}1{}", "[".repeat(depth), "]".repeat(depth));
        match self.tape.below(10) {
            0 => "7".repeat(4300),
            1 => format!("-{}", "7".repeat(4300)),
            2 => "7".repeat(4301),
            3 => format!("-{}", "7".repeat(4301)),
            4 => "\"\\ud800\"".into(),
            5 => "NaN".into(),
            6 => deep(96),
            7 => deep(97),
            8 => "1e400".into(),
            _ => "0.1".into(),
        }
    }

    /// The text of a Beads JSONL file to import.
    pub(super) fn import_text(&mut self, known: &[String]) -> String {
        let count = 1 + self.tape.below(5);
        let others: Vec<String> = (0..count).map(|n| format!("ex-i{n}")).collect();
        let mut lines = Vec::new();
        for number in 0..count {
            let record = self.record(number, known, &others);
            let format = if self.tape.chance(50) {
                Format::COMPACT
            } else {
                Format::SPACED
            };
            let mut line = write(&Value::Object(record), format).unwrap_or_default();
            let marker = write(&text(MARK), Format::COMPACT).unwrap_or_default();
            while line.contains(&marker) {
                let token = self.record_token();
                line = line.replacen(&marker, &token, 1);
            }
            if self.hostile(3) {
                line = [
                    "not json".to_owned(),
                    "\u{feff}{}".to_owned(),
                    "[]".to_owned(),
                    "5".to_owned(),
                ][self.tape.below(4)]
                .clone();
            }
            lines.push(line);
            if self.tape.chance(10) {
                lines.push(["", "   ", "\u{1c}", "\t"][self.tape.below(4)].to_owned());
            }
        }
        let mut text = lines.join(if self.tape.chance(20) { "\r\n" } else { "\n" });
        if self.tape.chance(50) {
            text.push('\n');
        }
        text
    }
}
