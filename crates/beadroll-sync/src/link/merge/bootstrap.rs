//! An empty store's first download. At most eight independent files are written and flushed
//! concurrently. Installation and bookkeeping keep the ordinary merge's order, and every worker
//! is joined before returning, including on an error. No existing journal or pending attempt
//! takes this path.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use beadroll_store::{PreparedAtomic, complete_prefix, prepare_atomic};

use super::{Merging, attempt_shaped, newlines, segment_shaped};
use crate::Error;

const WIDTH: usize = 8;
const BUFFER_BYTES: usize = 4 * 1024 * 1024;

struct Download {
    path: String,
    target: PathBuf,
    data: Vec<u8>,
    blob: Option<String>,
    attempt: bool,
    count: usize,
}

impl Merging<'_, '_> {
    pub(super) fn can_bootstrap(&self, listing: &[(Vec<u8>, String)]) -> bool {
        independent_targets(&self.link.ws.events_dir, listing)
    }

    pub(super) fn bootstrap(
        &mut self,
        tip: &str,
        listing: &[(Vec<u8>, String)],
    ) -> Result<i64, Error> {
        let mut pulled = 0;
        let mut listing = listing.iter().peekable();
        while listing.peek().is_some() {
            let mut downloads = Vec::new();
            let mut bytes = 0;
            // One large file may exceed the budget, as in the serial merge, but eight large
            // files cannot multiply that peak: stop once this batch reaches four MiB.
            while downloads.len() < WIDTH && bytes < BUFFER_BYTES {
                let Some((path, sha)) = listing.next() else {
                    break;
                };
                let Ok(path) = std::str::from_utf8(path) else {
                    continue;
                };
                self.link.known.listed_blob(path.to_owned(), sha.clone());
                let attempt = attempt_shaped(path);
                if !attempt && !segment_shaped(path) {
                    continue;
                }
                let blob = self.link.blob(tip, sha)?;
                let (data, written) = if attempt {
                    (blob, Some(sha.clone()))
                } else {
                    let data = complete_prefix(&blob);
                    let written = (data == blob).then(|| sha.clone());
                    (data, written)
                };
                bytes += data.len();
                downloads.push(Download {
                    path: path.to_owned(),
                    target: self.link.ws.dir.join(path),
                    blob: written,
                    count: if attempt { 1 } else { newlines(&data) },
                    data,
                    attempt,
                });
            }
            let prepared = prepare(&downloads);
            for (download, prepared) in downloads.iter().zip(prepared) {
                self.facts
                    .remote_counts
                    .insert(download.path.clone(), download.count);
                if let Some(prepared) = prepared? {
                    prepared.install(&self.link.ws.settings.failpoints)?;
                    self.wrote(
                        &download.path,
                        &download.target,
                        &download.data,
                        download.blob.as_deref(),
                    );
                    if download.attempt {
                        self.link.ws.forget_claims(&download.target);
                    }
                    pulled += download.count as i64;
                }
            }
        }
        Ok(pulled)
    }
}

/// No target or parent may alias another target. Canonical machine names exclude dot components
/// and case aliases on case-insensitive Unix filesystems; duplicate tree entries are refused too.
/// The store lock creates an empty events directory, which is allowed when it is no symlink.
fn independent_targets(events: &Path, listing: &[(Vec<u8>, String)]) -> bool {
    if !unaliased_directory(events) {
        return false;
    }
    let mut paths = HashSet::new();
    listing.iter().all(|(path, _)| {
        let Ok(path) = std::str::from_utf8(path) else {
            return true;
        };
        if !attempt_shaped(path) && !segment_shaped(path) {
            return true;
        }
        let machine = path.split('/').nth(1).unwrap_or_default();
        let folder = events.join(machine);
        let target = events.join(path.strip_prefix("events/").unwrap_or(path));
        machine.len() == 8
            && machine
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && paths.insert(path)
            && unaliased_directory(&folder)
            && (!attempt_shaped(path) || unaliased_directory(&folder.join("claims")))
            && matches!(std::fs::symlink_metadata(target), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
    })
}

fn unaliased_directory(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata.is_dir(), // symlink_metadata never follows a symlink
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

/// Each worker prepares one distinct target. A failure remains in its slot: earlier files are
/// installed first, later prepared files are discarded, as a sequential merge would stop there.
/// Empty segments are not created, matching merge_segment's empty-prefix case.
fn prepare(downloads: &[Download]) -> Vec<Result<Option<PreparedAtomic>, Error>> {
    std::thread::scope(|scope| {
        let workers: Vec<_> = downloads
            .iter()
            .map(|download| {
                std::thread::Builder::new()
                    .name("bead-download".to_owned())
                    .spawn_scoped(scope, move || {
                        if !download.attempt && download.data.is_empty() {
                            Ok(None)
                        } else {
                            prepare_atomic(&download.target, &download.data).map(Some)
                        }
                    })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| match worker {
                Ok(worker) => worker.join().unwrap_or_else(|_| {
                    Err(beadroll_store::Error::io_message(
                        "journal preparation worker failed",
                    ))
                }),
                Err(error) => Err(beadroll_store::Error::io_message(format!(
                    "cannot start journal preparation: {error}"
                ))),
            })
            .map(|result| result.map_err(Error::from))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_download_requires_distinct_destinations_without_existing_aliases() {
        let sandbox = beadroll_test_support::Sandbox::new(beadroll_test_support::built_bead());
        let events = sandbox.root().join("events");
        let listing = |machine: &str| {
            vec![(
                format!("events/{machine}/000001.jsonl").into_bytes(),
                String::new(),
            )]
        };
        let canonical = listing("0000ab01");
        assert!(independent_targets(&events, &canonical));
        let duplicate = [canonical.clone(), canonical.clone()].concat();
        assert!(!independent_targets(&events, &duplicate));
        for machine in [".", "..", "0000AB01", "arbitrary-name"] {
            assert!(!independent_targets(&events, &listing(machine)));
        }
        std::fs::create_dir(&events).unwrap();
        assert!(independent_targets(&events, &canonical));
        let machine = events.join("0000ab01");
        std::os::unix::fs::symlink(sandbox.root().join("absent"), &machine).unwrap();
        assert!(!independent_targets(&events, &canonical));
        std::fs::remove_file(&machine).unwrap();
        std::fs::create_dir(&machine).unwrap();
        assert!(independent_targets(&events, &canonical));
        let file = machine.join("000001.jsonl");
        std::fs::write(&file, b"existing").unwrap();
        assert!(!independent_targets(&events, &canonical));
        std::fs::remove_file(file).unwrap();
        let claim = vec![(b"events/0000ab01/claims/abc.json".to_vec(), String::new())];
        std::os::unix::fs::symlink(sandbox.root().join("absent"), machine.join("claims")).unwrap();
        assert!(!independent_targets(&events, &claim));
        std::fs::remove_file(machine.join("claims")).unwrap();
        std::fs::remove_dir(machine).unwrap();
        std::fs::remove_dir(&events).unwrap();
        std::os::unix::fs::symlink(sandbox.root().join("absent"), &events).unwrap();
        assert!(!independent_targets(&events, &canonical));
    }
}
