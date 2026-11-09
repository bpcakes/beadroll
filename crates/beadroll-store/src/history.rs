//! A disposable cache of settled history metadata in causal order. A hit checks every journal
//! file's identity; a miss reads and settles the journal exactly as `load_journal` does.

use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;

use beadroll_core::history::HistoryEntry;
use beadroll_core::json::{self, Format, Value};
use beadroll_core::order::causal_order;

use crate::fs::{abandoned, content_sum, read_keyed, write_cache_parts};
use crate::journal::{ContentSums, Loaded, claim_files_afresh, load_listed, segment_files};
use crate::{Error, FileKey, Workspace};

const MAGIC: &[u8] = b"bead history 1\n";

/// Settled events in causal order, keeping only the metadata `log` prints. The journal remains
/// the source: a changed, missing or unreadable file, another build, a damaged cache or an
/// unsettled filesystem timestamp makes this read the journal again. Cache writes are optional
/// and never fail the read. Pending attempts are never included.
pub fn history(ws: &Workspace) -> Result<Vec<HistoryEntry>, Error> {
    let segments = segment_files(ws)?;
    ws.settings.failpoints.hit("journal.after-enumerate");
    let path = ws.dir.join("history");
    if let Some(rows) = cached(ws, &path, &segments) {
        return Ok(rows);
    }
    let mut loaded = load_listed(ws, &segments, ContentSums::Skip)?;
    let journal = loaded.journal.settled_view();
    let (order, _) = causal_order(journal);
    let rows: Vec<_> = order
        .iter()
        .filter_map(|id| journal.event(*id))
        .map(HistoryEntry::from)
        .collect();
    if ws.dir.is_dir()
        && loaded.whole
        && let Some(stamp) = loaded_stamp(ws, &segments, &loaded)
    {
        let value = Value::Array(rows.iter().map(HistoryEntry::cached).collect());
        if let Ok(bytes) = json::write(&value, Format::COMPACT) {
            let mut header = MAGIC.to_vec();
            header.extend_from_slice(&stamp);
            header.extend_from_slice(&content_sum(bytes.as_bytes()));
            abandoned::remove(&[&path]);
            let _ = write_cache_parts(&path, &[&header, bytes.as_bytes()], &ws.settings.failpoints);
        }
    }
    Ok(rows)
}

fn cached(
    ws: &Workspace,
    path: &Path,
    segments: &[std::path::PathBuf],
) -> Option<Vec<HistoryEntry>> {
    let (bytes, file) = read_keyed(path).ok()?;
    let mut stamp = Stamp::new(ws)?;
    let written = file?.written();
    // Take segment identities before the fresh attempt listing: an attempt is placed before
    // a writer appends anything that depends on it, as in the journal reader.
    for path in segments {
        stamp.settled(path, FileKey::of(path)?, written)?;
    }
    let (attempts, _) = claim_files_afresh(ws).ok()?;
    for path in attempts.iter() {
        stamp.settled(path, FileKey::of(path)?, written)?;
    }
    let bytes = bytes.strip_prefix(MAGIC)?;
    let saved: [u8; 16] = bytes.get(..16)?.try_into().ok()?;
    let sum: [u8; 16] = bytes.get(16..32)?.try_into().ok()?;
    let data = bytes.get(32..)?;
    if saved != stamp.finish() || sum != content_sum(data) {
        return None;
    }
    json::parse(data)
        .ok()?
        .value
        .as_array()?
        .iter()
        .map(HistoryEntry::from_cached)
        .collect()
}

#[cfg(test)]
mod tests;

/// Fingerprint the files actually read, never a later stat of them: a concurrent replacement
/// must not label old rows with the replacement's identity.
fn loaded_stamp(
    ws: &Workspace,
    segments: &[std::path::PathBuf],
    loaded: &Loaded,
) -> Option<[u8; 16]> {
    let mut stamp = Stamp::new(ws)?;
    for (path, (key, _)) in segments.iter().zip(&loaded.segments) {
        stamp.add(path, (*key)?);
    }
    for (path, key, _) in &loaded.attempts {
        stamp.add(path, (*key)?);
    }
    Some(stamp.finish())
}

struct Stamp(Vec<u8>);

impl Stamp {
    fn new(ws: &Workspace) -> Option<Self> {
        let mut stamp = Self(Vec::new());
        stamp.bytes(ws.settings.build.as_deref()?.as_bytes());
        Some(stamp)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        self.0
            .extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        self.0.extend_from_slice(bytes);
    }

    fn add(&mut self, path: &Path, key: FileKey) {
        self.bytes(path.as_os_str().as_bytes());
        let (len, modified, changed, inode, device) = key.parts();
        self.0.extend_from_slice(&len.to_le_bytes());
        self.0.extend_from_slice(&modified.to_le_bytes());
        self.0.extend_from_slice(&changed.to_le_bytes());
        self.0.extend_from_slice(&inode.to_le_bytes());
        self.0.extend_from_slice(&device.to_le_bytes());
    }

    fn settled(&mut self, path: &Path, key: FileKey, written: i128) -> Option<()> {
        if !key.settled_before(written) {
            return None;
        }
        self.add(path, key);
        Some(())
    }

    fn finish(self) -> [u8; 16] {
        content_sum(&self.0)
    }
}
