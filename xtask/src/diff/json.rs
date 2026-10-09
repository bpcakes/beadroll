//! `diff json`: bytes read as a journal line is read, and values written in every dialect, against
//! CPython's `json` through `xtask/python/json_dump.py`.

mod corpus;

use std::process::ExitCode;

use beadroll_core::json::{Format, Int, Object, ParseError, Value, parse, write};
use proptest::collection::vec;
use proptest::prelude::any;
use proptest::strategy::{Strategy, ValueTree};
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};

use super::generate::{self, Tape};
use super::report::Op;
use super::{SHOWN, in_parallel, run};
use crate::workspace::{self, Failure};

/// One case: bytes to read, or an integer built in memory (the digits, plus one) to write.
#[derive(Clone, Debug)]
pub(crate) enum JsonCase {
    Bytes(Vec<u8>),
    PlusOne(String),
}

const DIALECTS: [(&str, Format); 6] = [
    ("compact", Format::COMPACT),
    ("spaced", Format::SPACED),
    ("ascii", Format::ASCII),
    ("indented", Format::INDENTED),
    ("canonical", Format::CANONICAL),
    ("sorted_ascii", Format::SORTED_ASCII),
];

impl JsonCase {
    fn to_line(&self) -> String {
        let mut case = Object::new();
        match self {
            JsonCase::Bytes(bytes) => case.insert(
                "bytes".into(),
                Value::Str(bytes.iter().map(|b| char::from(*b)).collect()),
            ),
            JsonCase::PlusOne(digits) => {
                case.insert("plus_one".into(), Value::from(digits.as_str()))
            }
        };
        write(&Value::Object(case), Format::COMPACT).unwrap_or_default()
    }

    /// What the Rust core answers, as the driver writes its answer.
    fn answer(&self) -> String {
        let mut out = Object::new();
        match self {
            JsonCase::PlusOne(digits) => {
                let value = Int::parse(digits).map_or(Value::Null, |n| Value::Int(n.plus_one()));
                out.insert("value".into(), written(&value));
            }
            JsonCase::Bytes(bytes) => match parse(bytes) {
                Ok(parsed) => {
                    out.insert("value".into(), written(&parsed.value));
                    out.insert(
                        "lone_surrogates".into(),
                        Value::Bool(parsed.lone_surrogates),
                    );
                }
                Err(ParseError::Utf8) => {
                    out.insert("error".into(), Value::from("UnicodeDecodeError"));
                    out.insert("message".into(), Value::Null);
                }
                Err(error) => {
                    let class = if matches!(error, ParseError::Syntax { .. }) {
                        "JSONDecodeError"
                    } else {
                        "ValueError"
                    };
                    out.insert("error".into(), Value::from(class));
                    out.insert("message".into(), Value::from(error.to_string()));
                }
            },
        }
        write(&Value::Object(out), Format::COMPACT)
            .unwrap_or_else(|error| format!("unwritable answer: {error}"))
    }
}

fn written(value: &Value) -> Value {
    let dialects = DIALECTS.iter().map(|(name, format)| {
        let text = match write(value, *format) {
            Ok(text) => Value::Str(text),
            Err(error) => Value::Object(
                [("error".to_owned(), Value::from(error.to_string()))]
                    .into_iter()
                    .collect(),
            ),
        };
        ((*name).to_owned(), text)
    });
    Value::Object(dialects.collect())
}

/// The corpus, then random values in random dress, then the lines of generated journals: `count`
/// cases in all, from `seed`.
fn cases(count: usize, seed: u64) -> Vec<JsonCase> {
    let mut out = corpus::all();
    let mut bytes = [0u8; 32];
    for (slot, byte) in bytes.iter_mut().zip(seed.to_le_bytes()) {
        *slot = byte;
    }
    let mut runner = TestRunner::new_with_rng(
        Config::default(),
        TestRng::from_seed(RngAlgorithm::ChaCha, &bytes),
    );
    let tapes = vec(any::<u64>(), 0..400);
    let mut journals = generate::cases(usize::MAX, seed).flat_map(|case| {
        case.ops.into_iter().filter_map(|op| match op {
            Op::Segment { data, .. } | Op::Attempt { data, .. } => Some(data),
            _ => None,
        })
    });
    while out.len() < count {
        if out.len().is_multiple_of(3) {
            let Some(segment) = journals.next() else {
                break;
            };
            out.extend(
                segment
                    .split(|b| *b == b'\n')
                    .map(|line| JsonCase::Bytes(line.to_vec())),
            );
            continue;
        }
        let Ok(tree) = tapes.new_tree(&mut runner) else {
            break;
        };
        let mut tape = Tape::new(tree.current());
        out.push(JsonCase::Bytes(corpus::random(&mut tape)));
    }
    out.truncate(count.max(corpus::all().len()));
    out
}

pub(crate) fn main(count: usize, seed: u64, jobs: usize) -> Result<ExitCode, Failure> {
    let cases = cases(count, seed);
    let driver = workspace::root().join("xtask/python/json_dump.py");
    let dir = workspace::output_dir("diff")?;
    let mut failures = 0;
    for (start, chunk) in (0..).step_by(2000).zip(cases.chunks(2000)) {
        let lines: Vec<String> = chunk.iter().map(JsonCase::to_line).collect();
        let python = run::python(&driver, &lines, jobs)?;
        let rust = in_parallel(chunk, jobs, JsonCase::answer);
        for (index, ((line, python), rust)) in lines.iter().zip(&python).zip(&rust).enumerate() {
            let Some(difference) = run::difference(python, rust) else {
                continue;
            };
            failures += 1;
            if failures <= SHOWN {
                let path = dir.join(format!("json-{}.jsonl", start + index));
                std::fs::write(&path, format!("{line}\n"))
                    .map_err(|e| Failure::io("cannot write", &path, &e))?;
                println!(
                    "case {} differs at {difference}\n  kept in {}",
                    start + index,
                    path.display()
                );
            }
        }
    }
    println!(
        "diff json: {} cases (seed {seed}), {failures} different",
        cases.len()
    );
    Ok(if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
