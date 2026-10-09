//! Cases as the drivers read them: one JSON object per line, `{"name": ..., "ops": [...]}`.

use beadroll_core::journal::Problem;
use beadroll_core::json::{self, Format, Value};

use super::report::Op;
use crate::workspace::Failure;

/// A journal to replay on both sides.
#[derive(Clone, Debug)]
pub(crate) struct Case {
    pub(crate) name: String,
    pub(crate) ops: Vec<Op>,
}

/// Bytes as a string whose code points are the bytes, as the driver reads them (latin-1).
fn latin1(data: &[u8]) -> Value {
    Value::Str(data.iter().map(|b| char::from(*b)).collect())
}

fn bytes(value: Option<&Value>) -> Option<Vec<u8>> {
    value?
        .as_str()?
        .chars()
        .map(|c| u8::try_from(u32::from(c)).ok())
        .collect()
}

fn text(value: Option<&Value>) -> Option<String> {
    value?.as_str().map(str::to_owned)
}

impl Case {
    /// The case as one line of JSON.
    pub(crate) fn to_line(&self) -> Result<String, Failure> {
        let ops = self.ops.iter().map(|op| {
            let items = match op {
                Op::Segment {
                    replica,
                    origin,
                    data,
                } => {
                    vec![
                        Value::from("segment"),
                        Value::from(replica.as_str()),
                        Value::from(origin.as_str()),
                        latin1(data),
                    ]
                }
                Op::Attempt {
                    replica,
                    origin,
                    data,
                } => {
                    vec![
                        Value::from("attempt"),
                        Value::from(replica.as_str()),
                        Value::from(origin.as_str()),
                        latin1(data),
                    ]
                }
                Op::Add {
                    event,
                    replica,
                    origin,
                    sequenced,
                } => vec![
                    Value::from("add"),
                    event.clone(),
                    Value::from(replica.as_str()),
                    Value::from(origin.as_str()),
                    Value::Bool(*sequenced),
                ],
                Op::Problem(problem) => {
                    let mut entry = json::Object::new();
                    entry.insert("event".into(), problem.event.clone().into());
                    entry.insert("origin".into(), Value::from(problem.origin.as_str()));
                    entry.insert("problem".into(), Value::from(problem.problem.as_str()));
                    vec![Value::from("problem"), Value::Object(entry)]
                }
                Op::Settle => vec![Value::from("settle")],
                Op::Import {
                    replica,
                    path,
                    text,
                } => {
                    vec![
                        Value::from("import"),
                        Value::from(replica.as_str()),
                        Value::from(path.as_str()),
                        Value::from(text.as_str()),
                    ]
                }
            };
            Value::Array(items)
        });
        let mut case = json::Object::new();
        case.insert("name".into(), Value::from(self.name.as_str()));
        case.insert("ops".into(), Value::Array(ops.collect()));
        json::write(&Value::Object(case), Format::COMPACT).map_err(|error| {
            Failure::Failed(format!("case {} cannot be written: {error}", self.name))
        })
    }

    /// A case read back from its line.
    pub(crate) fn from_line(line: &str) -> Result<Case, String> {
        let parsed = json::parse(line.as_bytes()).map_err(|error| error.to_string())?;
        if parsed.lone_surrogates {
            return Err(
                "holds a lone surrogate, which Python keeps and Rust reads as U+FFFD".into(),
            );
        }
        let name = text(parsed.value.get("name")).ok_or("has no name")?;
        let ops = parsed
            .value
            .get("ops")
            .and_then(Value::as_array)
            .ok_or("has no ops")?;
        let ops = ops
            .iter()
            .map(op_of)
            .collect::<Option<Vec<Op>>>()
            .ok_or("has a malformed operation")?;
        Ok(Case { name, ops })
    }
}

fn op_of(op: &Value) -> Option<Op> {
    let items = op.as_array()?;
    let at = |n: usize| items.get(n);
    Some(match at(0)?.as_str()? {
        "segment" => Op::Segment {
            replica: text(at(1))?,
            origin: text(at(2))?,
            data: bytes(at(3))?,
        },
        "attempt" => Op::Attempt {
            replica: text(at(1))?,
            origin: text(at(2))?,
            data: bytes(at(3))?,
        },
        "add" => Op::Add {
            event: at(1)?.clone(),
            replica: text(at(2))?,
            origin: text(at(3))?,
            sequenced: matches!(at(4)?, Value::Bool(true)),
        },
        "problem" => {
            let entry = at(1)?;
            Op::Problem(Problem {
                event: entry
                    .get("event")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                origin: text(entry.get("origin"))?,
                problem: text(entry.get("problem"))?,
            })
        }
        "settle" => Op::Settle,
        "import" => Op::Import {
            replica: text(at(1))?,
            path: text(at(2))?,
            text: text(at(3))?,
        },
        _ => return None,
    })
}
