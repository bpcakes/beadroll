//! `timing --read N`: how long the reading commands take on a store of N events, published once
//! to a local bare remote as a store in use is (`bead list`, `bead show` of an issue, `bead
//! ready`, and `bead log` of the last event, `bead info` and `bead machines`, which answered from
//! the whole journal before roll-lqjj.31), and a local write (`bead comments add --offline`, so that only what the store does is
//! timed): with the store's projection in place (`*_seconds`), and with it removed before each
//! (`cold_*_seconds`), so that each replays the journal, as every command did before roll-lqjj.2,
//! and writes the projection back; a write cycle (roll-lqjj.36): a write after a read that left
//! nothing to take in (`write_after_read_seconds`), the `bead show` after it, which takes the
//! write's record in and writes the projection back (`read_after_write_seconds`), and a write after
//! a write, which takes the record of the one before in (`write_after_write_seconds`); a claim of
//! an issue created for it and its release, each published at once to the remote as an ownership
//! command is (`claim_seconds`, `release_seconds`): the release ends the claim and so adds to the
//! issue's ownership history, which its texts hold (roll-lqjj.40); a write after a read once the
//! store has no remote, not `--offline` (`no_remote_write_seconds`, roll-lqjj.50); and how many
//! bytes the projection and its text file hold. Evidence, never a gate.

use super::history::{ROTATE_BYTES, Shape, history};
use super::world::{RUNS, World, bead, profile, write_store};
use crate::workspace::{self, Failure, json_string};

/// The commands timed, and how a record names each.
const READS: [(&str, &[&str]); 6] = [
    ("list", &["list", "--json"]),
    ("show", &["show", "ex-1", "--json"]),
    ("ready", &["ready", "--json"]),
    ("log", &["log", "--limit", "1", "--json"]),
    ("info", &["info", "--json"]),
    ("machines", &["machines", "--json"]),
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
    // made whole after a replay: every issue's texts are in the text file
    let bytes = world.stored_bytes("projection")?;
    let texts = world.stored_bytes("projection-texts")?;
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
    let (mut warm, mut cold) = (f64::INFINITY, f64::INFINITY);
    for run in 0..RUNS {
        let text = format!("write {run}");
        warm = warm.min(world.timed(&["comments", "add", "ex-1", &text, "--offline"])?);
    }
    for run in 0..RUNS {
        world.forget("projection")?;
        let text = format!("cold write {run}");
        cold = cold.min(world.timed(&["comments", "add", "ex-1", &text, "--offline"])?);
    }
    fields.push(format!(
        "\"write_seconds\": {warm:.4}, \"cold_write_seconds\": {cold:.4}, "
    ));
    let [mut write, mut read, mut again] = [f64::INFINITY; 3];
    for run in 0..RUNS {
        world.bead(&["list", "--json"])?;
        let comment = |what: &str| -> Result<f64, Failure> {
            let text = format!("cycle {run}, {what}");
            world.timed(&["comments", "add", "ex-1", &text, "--offline"])
        };
        write = write.min(comment("after a read")?);
        read = read.min(world.timed(&["show", "ex-1", "--json"])?);
        comment("first")?;
        again = again.min(comment("after a write")?);
    }
    fields.push(format!(
        "\"write_after_read_seconds\": {write:.4}, \"read_after_write_seconds\": {read:.4}, \
         \"write_after_write_seconds\": {again:.4}, "
    ));
    fields.push(claimed_and_released(&world)?);
    // the same write after a read, not `--offline`, once the store has no remote to publish to
    world.without_remote()?;
    let mut alone = f64::INFINITY;
    for run in 0..RUNS {
        world.bead(&["list", "--json"])?;
        let text = format!("no remote {run}");
        alone = alone.min(world.timed(&["comments", "add", "ex-1", &text])?);
    }
    fields.push(format!("\"no_remote_write_seconds\": {alone:.4}, "));
    // An ownership-heavy store is a hundred thousand files: nothing to leave lying about.
    std::fs::remove_dir_all(&dir).map_err(|error| Failure::io("cannot remove", &dir, &error))?;
    let label = label.map_or_else(
        || format!("reads of {count} {}", shape.events()),
        str::to_owned,
    );
    Ok(format!(
        "{{\"label\": {}, \"shape\": {}, \"events\": {count}, \"segments\": {}, \
         \"attempt_files\": {}, \"journal_bytes\": {}, \"projection_bytes\": {bytes}, \
         \"texts_bytes\": {texts}, \"profile\": {}, \"runs\": {RUNS}, \"first_seconds\": {first:.4}, \
         {}",
        json_string(&label),
        json_string(shape.name()),
        layout.segments,
        layout.attempts,
        layout.bytes,
        json_string(profile()),
        fields.concat(),
    ))
}

/// The fastest of [`RUNS`] claims of an issue created for it and of its releases, each published at
/// once to the remote as an ownership command is: the release ends the claim and so adds to the
/// issue's ownership history, which its texts hold (roll-lqjj.40). Its fields, as JSON members.
fn claimed_and_released(world: &World) -> Result<String, Failure> {
    let created = world.output(&["create", "claimed and released", "--json"], None)?;
    let key = (beadroll_core::json::from_text(&created).ok())
        .and_then(|value| {
            value
                .get("id")
                .and_then(|id| id.as_str())
                .map(str::to_owned)
        })
        .ok_or_else(|| Failure::Failed(format!("bead create printed no key: {created}")))?;
    let (mut claim, mut release) = (f64::INFINITY, f64::INFINITY);
    for _ in 0..RUNS {
        claim = claim.min(world.timed(&["claim", &key])?);
        release = release.min(world.timed(&["release", &key])?);
    }
    Ok(format!(
        "\"claim_seconds\": {claim:.4}, \"release_seconds\": {release:.4}, "
    ))
}
