//! Jobs run on a few threads at once, for tests of what concurrent bead processes do.

use std::sync::atomic::{AtomicUsize, Ordering};

/// `f` of every job, run on `workers` threads at once, each taking the next job not yet taken:
/// the results in the order of the jobs. A job that panics fails the test.
pub fn pool_map<J, R, F>(workers: usize, jobs: &[J], f: F) -> Vec<R>
where
    J: Sync,
    R: Send,
    F: Fn(&J) -> R + Sync,
{
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let threads: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::SeqCst);
                        let Some(job) = jobs.get(index) else {
                            return done;
                        };
                        done.push((index, f(job)));
                    }
                })
            })
            .collect();
        let mut done: Vec<(usize, R)> = threads
            .into_iter()
            .flat_map(|thread| thread.join().expect("a job panicked"))
            .collect();
        done.sort_by_key(|(index, _)| *index);
        done.into_iter().map(|(_, result)| result).collect()
    })
}
