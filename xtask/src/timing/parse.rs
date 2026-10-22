//! `timing --parse N`: what core's JSON reader takes in this process on what bead reads most often
//! at a size (roll-lqjj.49): `meta.json` with a `published` map of N entries, one for every journal
//! file, which every command with a remote parses whole for its unpublished count; and the lines
//! of a history of N events, every segment's and every attempt file's, as the journal reader parses
//! them. Beside the map, what building its members costs on its own: the N members put into an
//! object once, as a reader that builds each object once pays, and twice, by key as written and
//! again by key as read, as the reader built every object. The fastest of [`RUNS`] runs counts.
//! Evidence, never a gate.

use beadroll_core::json::{Format, Object, ParseError, Value, parse, write};
use indexmap::IndexMap;

use super::decode::{fastest, fastest_of};
use super::history::{Files, Shape, event_id, history};
use super::world::profile;
use crate::workspace::{Failure, json_string};

/// How many times each text is parsed; the fastest run counts.
pub(crate) const RUNS: usize = 7;

/// How many machines the published map's files are spread over, as the histories' are.
const MACHINES: usize = 6;

/// The id of machine `n`.
fn replica(n: usize) -> String {
    format!("{:08x}", 0x8a3f_0c11 + n)
}

/// `meta.json` as bead writes it for a store of `count` attempt files: a `published` map with an
/// entry for each, and for every machine's segment, under the tracker's name and its remote.
fn meta(count: usize) -> Result<Vec<u8>, Failure> {
    let mut published = Object::with_capacity(count + MACHINES);
    for machine in 0..MACHINES {
        published.insert(
            format!("events/{}/000001.jsonl", replica(machine)),
            Value::from(1_000),
        );
    }
    for n in 0..count {
        published.insert(
            format!(
                "events/{}/claims/{}.json",
                replica(n % MACHINES),
                event_id(n)
            ),
            Value::from(1),
        );
    }
    let mut meta = Object::new();
    meta.insert("name".into(), Value::from("beadroll"));
    meta.insert("published".into(), Value::from(published));
    meta.insert(
        "remote".into(),
        Value::from("https://example.invalid/beadroll.git"),
    );
    write(&Value::from(meta), Format::INDENTED)
        .map(String::into_bytes)
        .map_err(|error| Failure::Failed(format!("meta.json cannot be written: {error}")))
}

/// The members of `parsed`'s published map, each key with its value.
fn members(parsed: &Value) -> Result<Vec<(String, Value)>, Failure> {
    match parsed.get("published") {
        Some(Value::Object(map)) => Ok(map
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()),
        _ => Err(Failure::Failed(
            "meta.json read back without its published map".into(),
        )),
    }
}

/// The members put into an object once: what a reader that builds each object once pays for them.
fn once(members: Vec<(String, Value)>) -> Object {
    let mut out = Object::with_capacity(members.len());
    for (key, value) in members {
        out.insert(key, value);
    }
    out
}

/// The members put in by key as written, then again by key as read, as the reader built every
/// object until roll-lqjj.49: the key's bytes copied and held with its text, its value and whether
/// either holds a lone surrogate, and the object made from them once the last member is in.
fn twice(members: Vec<(String, Value)>) -> Object {
    let mut raw: IndexMap<Vec<u8>, (String, Value, bool)> = IndexMap::new();
    for (key, value) in members {
        let bytes = key.as_bytes().to_vec();
        match raw.get_mut(&bytes) {
            Some(member) => member.1 = value,
            None => {
                raw.insert(bytes, (key, value, false));
            }
        }
    }
    let mut out = Object::with_capacity(raw.len());
    for (_, (text, value, _)) in raw {
        out.insert(text, value);
    }
    out
}

/// Every line the journal reader parses of `files`: each segment's, then each attempt file's.
fn lines(files: &[Files]) -> Vec<&[u8]> {
    files
        .iter()
        .flat_map(|file| {
            std::iter::once(file.segment.as_slice())
                .chain(file.attempts.iter().map(|(_, data)| data.as_slice()))
        })
        .flat_map(|data| data.split(|byte| *byte == b'\n'))
        .filter(|line| !line.is_empty())
        .collect()
}

/// Measures; returns the beginning of the line to record.
pub(crate) fn measure(count: usize, shape: Shape, label: Option<&str>) -> Result<String, Failure> {
    let unreadable =
        |error: ParseError| Failure::Failed(format!("the text does not parse: {error}"));
    let meta = meta(count)?;
    let published_seconds = fastest(|| parse(&meta).map_err(unreadable))?;
    let parsed = parse(&meta).map_err(unreadable)?;
    let members = members(&parsed.value)?;
    let once_seconds = fastest_of(|| members.clone(), |members| Ok(once(members)))?;
    let twice_seconds = fastest_of(|| members.clone(), |members| Ok(twice(members)))?;
    let files = history(count, shape);
    let lines = lines(&files);
    let lines_bytes: usize = lines.iter().map(|line| line.len()).sum();
    let lines_seconds = fastest(|| {
        lines
            .iter()
            .map(|line| parse(line).map(|parsed| parsed.value).map_err(unreadable))
            .collect::<Result<Vec<Value>, Failure>>()
    })?;
    let label = label.map_or_else(
        || {
            format!(
                "parsing meta.json of {count} files, and the lines of {count} {}",
                shape.events()
            )
        },
        str::to_owned,
    );
    Ok(format!(
        "{{\"label\": {}, \"files\": {count}, \"shape\": {}, \"profile\": {}, \"runs\": {RUNS}, \
         \"published_seconds\": {published_seconds:.4}, \"published_bytes\": {}, \
         \"published_members\": {}, \"once_seconds\": {once_seconds:.4}, \
         \"twice_seconds\": {twice_seconds:.4}, \"lines_seconds\": {lines_seconds:.4}, \
         \"lines\": {}, \"lines_bytes\": {lines_bytes}, ",
        json_string(&label),
        json_string(shape.name()),
        json_string(profile()),
        meta.len(),
        members.len(),
        lines.len(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_meta_names_a_segment_a_machine_and_an_attempt_file_each() {
        let parsed = parse(&meta(5).unwrap()).unwrap();
        let members = members(&parsed.value).unwrap();
        assert_eq!(members.len(), 5 + MACHINES);
        assert_eq!(
            members
                .iter()
                .filter(|(key, _)| key.contains("/claims/"))
                .count(),
            5
        );
        assert!(members.iter().all(|(key, _)| key.starts_with("events/")));
        assert_eq!(parsed.value.get("name"), Some(&Value::from("beadroll")));
    }

    #[test]
    fn once_and_twice_make_the_object_the_reader_made() {
        let parsed = parse(&meta(5).unwrap()).unwrap();
        let members = members(&parsed.value).unwrap();
        let published = Value::from(once(members.clone()));
        assert_eq!(Some(&published), parsed.value.get("published"));
        assert_eq!(Value::from(twice(members)), published);
    }

    #[test]
    fn every_line_of_every_file_is_parsed() {
        let plain = history(40, Shape::Plain);
        assert_eq!(lines(&plain).len(), 40);
        let ownership = history(40, Shape::Ownership);
        assert!(ownership.iter().any(|file| !file.attempts.is_empty()));
        assert_eq!(lines(&ownership).len(), 40);
        for line in lines(&ownership) {
            assert!(matches!(parse(line).unwrap().value, Value::Object(_)));
        }
    }

    #[test]
    fn the_figures_name_every_measure() {
        let line = measure(4, Shape::Plain, Some("label")).unwrap();
        for name in [
            "published_seconds",
            "once_seconds",
            "twice_seconds",
            "lines_seconds",
        ] {
            assert!(line.contains(&format!("\"{name}\": 0.")), "{line}");
        }
        assert!(line.contains("\"files\": 4"), "{line}");
        assert!(line.contains("\"shape\": \"plain\""), "{line}");
        assert!(
            line.contains(&format!("\"published_members\": {}", 4 + MACHINES)),
            "{line}"
        );
        assert!(line.contains("\"lines\": 4"), "{line}");
    }
}
