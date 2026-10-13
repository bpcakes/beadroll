//! `timing --import N`: what importing a generated Beads export of N records takes (roll-lqjj.9),
//! the export shaped as real trackers' are (a description of 1.2-2.4 KB, notes on half the issues,
//! up to three comments, labels, a parent or a blocker among the earlier issues, most closed): how
//! many bytes it is, and the time, the peak resident set and the heap peak of `bead init --import`
//! into a scratch store, recorded locally only (`--offline`) and published to a local bare remote,
//! as a first import is, and of the first `bead sync` after an offline import, which publishes it
//! and makes the projection of it, and how many bytes the projection and its text file hold
//! after the published import. Bead runs in xtask (`cargo xtask bead`), whose heap is counted,
//! unless `BEAD_UNDER_TEST` names another bead, whose heap is then not known. Evidence, never a
//! gate.

use std::fmt::Write as _;
use std::path::PathBuf;

use beadroll_core::json::{Format, Object, Value, write};

use super::history::ROTATE_BYTES;
use super::world::{World, bead, profile};
use crate::workspace::{self, Failure, json_string};

/// A fixed sequence of numbers: every run generates the same export.
struct Draws(u64);

impl Draws {
    /// A number below `n`.
    fn below(&mut self, n: usize) -> usize {
        self.0 = (self.0)
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        usize::try_from((self.0 >> 33) % u64::try_from(n.max(1)).unwrap_or(1)).unwrap_or(0)
    }

    /// Prose of `at_least` bytes and up to twice that.
    fn prose(&mut self, at_least: usize) -> String {
        const WORDS: [&str; 16] = [
            "the", "import", "keeps", "every", "record", "and", "its", "comments", "so", "that",
            "each", "machine", "agrees", "on", "what", "issues",
        ];
        let target = at_least + self.below(at_least + 1);
        let mut text = String::with_capacity(target + 16);
        while text.len() < target {
            if !text.is_empty() {
                text.push(if self.below(12) == 0 { '\n' } else { ' ' });
            }
            text.push_str(WORDS[self.below(WORDS.len())]);
        }
        text
    }
}

/// A generated export of `records` records, as Beads writes one: a JSON object a line.
pub(crate) fn export(records: usize) -> String {
    let mut draws = Draws(0x1A9);
    let mut out = String::new();
    let at = |n: usize| format!("2026-09-{:02}T{:02}:{:02}:00Z", 1 + n % 28, n % 24, n % 60);
    for n in 0..records {
        let mut record = Object::new();
        record.insert("id".into(), Value::from(format!("ex-{n}")));
        record.insert("title".into(), Value::from(format!("issue {n}")));
        record.insert("description".into(), Value::from(draws.prose(1200)));
        if draws.below(2) == 0 {
            record.insert("notes".into(), Value::from(draws.prose(600)));
        }
        let closed = draws.below(10) < 8;
        record.insert(
            "status".into(),
            Value::from(if closed { "closed" } else { "open" }),
        );
        record.insert("priority".into(), Value::from(draws.below(5) as i64));
        record.insert("issue_type".into(), Value::from("task"));
        record.insert("created_at".into(), Value::from(at(n)));
        record.insert("updated_at".into(), Value::from(at(n + 1)));
        if closed {
            record.insert("closed_at".into(), Value::from(at(n + 2)));
            record.insert("close_reason".into(), Value::from(draws.prose(80)));
        }
        let labels = ["ops", "db", "ui", "infra"];
        let label = Value::from(labels[draws.below(labels.len())]);
        record.insert("labels".into(), Value::Array(vec![label]));
        if n > 0 && draws.below(3) > 0 {
            let mut dep = Object::new();
            dep.insert("issue_id".into(), Value::from(format!("ex-{n}")));
            let to = draws.below(n);
            dep.insert("depends_on_id".into(), Value::from(format!("ex-{to}")));
            let kind = if draws.below(2) == 0 {
                "parent-child"
            } else {
                "blocks"
            };
            dep.insert("type".into(), Value::from(kind));
            record.insert("dependencies".into(), Value::Array(vec![Value::from(dep)]));
        }
        let comments: Vec<Value> = (0..draws.below(4))
            .map(|c| {
                let mut comment = Object::new();
                comment.insert("id".into(), Value::from((n * 4 + c) as i64));
                comment.insert("issue_id".into(), Value::from(format!("ex-{n}")));
                comment.insert("author".into(), Value::from("agent"));
                comment.insert("text".into(), Value::from(draws.prose(300)));
                comment.insert("created_at".into(), Value::from(at(n + 1)));
                Value::from(comment)
            })
            .collect();
        if !comments.is_empty() {
            record.insert("comments".into(), Value::Array(comments));
        }
        let line = write(&Value::from(record), Format::COMPACT).unwrap_or_default();
        let _ = writeln!(out, "{line}");
    }
    out
}

/// What runs bead so that its heap is counted: xtask itself, unless `BEAD_UNDER_TEST` names
/// another bead.
fn counted() -> Option<PathBuf> {
    let other = std::env::var_os("BEAD_UNDER_TEST").is_some_and(|bead| !bead.is_empty());
    if other {
        return None;
    }
    std::env::current_exe().ok()
}

/// A peak in bytes, or `null`.
fn peak_bytes(peak: Option<u64>) -> String {
    peak.map_or_else(|| "null".to_owned(), |peak| peak.to_string())
}

/// Measures; returns the beginning of the line to record.
pub(crate) fn measure(records: usize, label: Option<&str>) -> Result<String, Failure> {
    let bead = bead()?;
    let counted = counted();
    let dir = workspace::output_dir("timing")?.join(format!("import-{records}"));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|error| Failure::io("cannot remove", &dir, &error))?;
    }
    std::fs::create_dir_all(&dir).map_err(|error| Failure::io("cannot create", &dir, &error))?;
    let path = dir.join("issues.jsonl");
    let exported = export(records);
    std::fs::write(&path, &exported).map_err(|error| Failure::io("cannot write", &path, &error))?;
    let bytes = exported.len();
    drop(exported);
    let shown = path.to_string_lossy().into_owned();
    let mut fields = Vec::new();
    for (name, offline) in [("offline", true), ("published", false)] {
        let world = World::new(&dir.join(name), bead.clone(), ROTATE_BYTES)?;
        let mut args = vec!["init", "--prefix", "ex", "--import", shown.as_str()];
        if offline {
            args.push("--offline");
        }
        let mut runs = vec![(name, world.peaks(&args, counted.as_deref())?)];
        if offline {
            runs.push(("sync", world.peaks(&["sync"], counted.as_deref())?));
        }
        for (name, (seconds, peaks)) in runs {
            fields.push(format!(
                "\"{name}_seconds\": {seconds:.3}, \"{name}_peak_bytes\": {}, \
                 \"{name}_heap_peak_bytes\": {}, ",
                peak_bytes(peaks.resident),
                peak_bytes(peaks.heap),
            ));
        }
        // the projection the import's publish made, every text in its text file
        if !offline {
            fields.push(format!(
                "\"projection_bytes\": {}, \"texts_bytes\": {}, ",
                world.stored_bytes("projection")?,
                world.stored_bytes("projection-texts")?,
            ));
        }
    }
    std::fs::remove_dir_all(&dir).map_err(|error| Failure::io("cannot remove", &dir, &error))?;
    let label = label.map_or_else(|| format!("import of {records} records"), str::to_owned);
    Ok(format!(
        "{{\"label\": {}, \"records\": {records}, \"export_bytes\": {bytes}, \"profile\": {}, {}",
        json_string(&label),
        json_string(profile()),
        fields.concat(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    use beadroll_core::import::read_records;

    #[test]
    fn the_export_is_one_bead_reads_and_the_size_of_a_real_one() {
        let text = export(300);
        let records = read_records(&text, "issues.jsonl").unwrap();
        assert_eq!(records.len(), 300);
        // about 3.5 KB a record, as real trackers' are
        let per = text.len() / 300;
        assert!((2_500..5_000).contains(&per), "{per} bytes a record");
        assert_eq!(export(300), text, "the same every run");
    }
}
