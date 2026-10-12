//! `timing --read N`: how long the reading commands take on a store of N events, published once
//! to a local bare remote as a store in use is (`bead list`, `bead show` of an issue, `bead
//! ready`): with the store's projection in place (`*_seconds`), and with it removed before each
//! (`cold_*_seconds`), so that each replays the journal, as every command did before roll-lqjj.2,
//! and writes the projection back. Evidence, never a gate.

use super::history::{ROTATE_BYTES, Shape, history};
use super::world::{RUNS, World, bead, profile, write_store};
use crate::workspace::{self, Failure, json_string};

/// The commands timed, and how a record names each.
const READS: [(&str, &[&str]); 3] = [
    ("list", &["list", "--json"]),
    ("show", &["show", "ex-1", "--json"]),
    ("ready", &["ready", "--json"]),
];

/// Measures; returns the beginning of the line to record.
pub(crate) fn measure(count: usize, shape: Shape, label: Option<&str>) -> Result<String, Failure> {
    let bead = bead()?;
    let dir = workspace::output_dir("timing")?.join(format!("read-{}-{count}", shape.name()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|error| Failure::io("cannot remove", &dir, &error))?;
    }
    let files = history(count, shape);
    let world = World::new(&dir, bead, ROTATE_BYTES)?;
    let layout = write_store(&world.store(), &files, ROTATE_BYTES)?;
    drop(files);
    world.bead(&["sync"])?;
    world.forget("projection")?;
    let first = world.timed(&["list", "--json"])?;
    let projection = world.store().join("projection");
    let bytes = std::fs::metadata(&projection)
        .map_err(|error| Failure::io("cannot read", &projection, &error))?
        .len();
    let mut fields = Vec::new();
    for (name, args) in READS {
        let (mut warm, mut cold) = (f64::INFINITY, f64::INFINITY);
        for _ in 0..RUNS {
            warm = warm.min(world.timed(args)?);
        }
        for _ in 0..RUNS {
            world.forget("projection")?;
            cold = cold.min(world.timed(args)?);
        }
        fields.push(format!(
            "\"{name}_seconds\": {warm:.4}, \"cold_{name}_seconds\": {cold:.4}, "
        ));
    }
    // An ownership-heavy store is a hundred thousand files: nothing to leave lying about.
    std::fs::remove_dir_all(&dir).map_err(|error| Failure::io("cannot remove", &dir, &error))?;
    let label = label.map_or_else(
        || format!("reads of {count} {}", shape.events()),
        str::to_owned,
    );
    Ok(format!(
        "{{\"label\": {}, \"shape\": {}, \"events\": {count}, \"segments\": {}, \
         \"attempt_files\": {}, \"journal_bytes\": {}, \"projection_bytes\": {bytes}, \
         \"profile\": {}, \"runs\": {RUNS}, \"first_seconds\": {first:.4}, {}",
        json_string(&label),
        json_string(shape.name()),
        layout.segments,
        layout.attempts,
        layout.bytes,
        json_string(profile()),
        fields.concat(),
    ))
}
