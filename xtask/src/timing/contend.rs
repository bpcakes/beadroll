//! `timing --contend M`: M machines writing one tracker at once, each write published as bead
//! publishes it, against a local bare remote that `--latency MS` makes as slow as a real one (each
//! fetch and each push waits MS milliseconds, through bead's `fetch.result` and `push.before`
//! failpoints), `--writes W` writes each: how long a write takes, how many were left unpublished,
//! and whether every machine holds the same tracker once all have synced (roll-lqjj.7). Evidence,
//! never a gate.

use std::time::Instant;

use super::history::{ROTATE_BYTES, Shape, history};
use super::world::{World, bead, profile, write_store};
use crate::workspace::{self, Failure, json_string};

/// How many writes each machine makes when `--writes` does not say.
pub(crate) const WRITES: usize = 25;

/// How many milliseconds each fetch and push waits when `--latency` does not say: about what a
/// fetch or a push to GitHub takes.
pub(crate) const LATENCY: u64 = 300;

/// The events of the tracker the machines write to.
const EVENTS: usize = 1000;

/// Measures; returns the beginning of the line to record.
pub(crate) fn measure(
    machines: usize,
    writes: usize,
    latency: u64,
    label: Option<&str>,
) -> Result<String, Failure> {
    let bead = bead()?;
    let dir = workspace::output_dir("timing")?.join(format!("contend-{machines}"));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|error| Failure::io("cannot remove", &dir, &error))?;
    }
    let first = World::new(&dir.join("m0"), bead, ROTATE_BYTES)?;
    write_store(&first.store(), &history(EVENTS, Shape::Plain), ROTATE_BYTES)?;
    first.bead(&["sync"])?;
    let mut fleet = vec![first];
    for m in 1..machines.max(1) {
        let world = fleet[0].another(&dir.join(format!("m{m}")))?;
        world.bead(&["sync"])?;
        fleet.push(world);
    }
    let slow = format!("fetch.result@always=sleep({latency});push.before@always=sleep({latency})");
    let started = Instant::now();
    let runs: Vec<Vec<(f64, bool)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (fleet.iter().enumerate())
            .map(|(m, world)| {
                let slow = slow.as_str();
                scope.spawn(move || -> Result<Vec<(f64, bool)>, Failure> {
                    let mut out = Vec::with_capacity(writes);
                    for n in 0..writes {
                        let text = format!("machine {m}, write {n}");
                        let clock = Instant::now();
                        let args = ["comments", "add", "ex-1", &text, "--json"];
                        let printed = world.output(&args, Some(slow))?;
                        out.push((
                            clock.elapsed().as_secs_f64(),
                            printed.contains("\"unpublished\""),
                        ));
                    }
                    Ok(out)
                })
            })
            .collect();
        (handles.into_iter())
            .map(|handle| {
                (handle.join()).unwrap_or_else(|_| Err(Failure::Failed("a writer panicked".into())))
            })
            .collect::<Result<Vec<_>, Failure>>()
    })?;
    let wall = started.elapsed().as_secs_f64();
    // every machine syncs until each has what all wrote
    for _ in 0..2 {
        for world in &fleet {
            world.bead(&["sync"])?;
        }
    }
    let exported = (fleet.iter())
        .map(|world| world.output(&["export"], None))
        .collect::<Result<Vec<_>, Failure>>()?;
    let converged = exported.windows(2).all(|pair| pair[0] == pair[1]);
    std::fs::remove_dir_all(&dir).map_err(|error| Failure::io("cannot remove", &dir, &error))?;
    let mut seconds: Vec<f64> = runs.iter().flatten().map(|(seconds, _)| *seconds).collect();
    seconds.sort_by(f64::total_cmp);
    let at = |fraction: f64| {
        let last = seconds.len().saturating_sub(1);
        let index = ((last as f64) * fraction).round() as usize;
        seconds.get(index).copied().unwrap_or(0.0)
    };
    let unpublished = runs.iter().flatten().filter(|(_, left)| *left).count();
    let slow_writes = seconds.iter().filter(|seconds| **seconds > 1.0).count();
    let label = label.map_or_else(
        || format!("{machines} machines writing at once, {latency} ms a fetch or a push"),
        str::to_owned,
    );
    Ok(format!(
        "{{\"label\": {}, \"machines\": {machines}, \"writes\": {}, \"latency_ms\": {latency}, \
         \"profile\": {}, \"median_seconds\": {:.3}, \"p90_seconds\": {:.3}, \"p99_seconds\": {:.3}, \
         \"max_seconds\": {:.3}, \"over_a_second\": {slow_writes}, \"unpublished\": {unpublished}, \
         \"wall_seconds\": {wall:.1}, \"converged\": {converged}, ",
        json_string(&label),
        seconds.len(),
        json_string(profile()),
        at(0.5),
        at(0.9),
        at(0.99),
        seconds.last().copied().unwrap_or(0.0),
    ))
}
