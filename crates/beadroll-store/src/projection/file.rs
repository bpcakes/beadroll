//! `<store>/projection` on disk: a fixed header, then what the projection was made from (the build
//! of bead that made it and every journal file's identity), its state and its index. The header
//! gives each part's length and a 128-bit sum of its bytes, so that a part is read only when it is
//! whole and as written: a damaged or truncated file is no projection, and a command that only
//! reads reads the first two parts alone.

use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::Path;

use beadroll_core::projection::{ENCODING, Encoded};

use crate::fs::content_sum as sum;
use crate::memo::FileKey;

/// The first bytes of a projection.
const MAGIC: &[u8; 16] = b"bead projection\n";

/// The version of this layout.
const LAYOUT: u32 = 1;

/// The header's length: the magic, the two versions, three lengths and three sums.
const HEADER: usize = 16 + 4 + 4 + 3 * 8 + 3 * 16;

/// A journal file a projection was made from: its path under the store's `events`, its identity
/// up to what was read of it, whether it is a segment (else an attempt file), and for a segment
/// the sum of its bytes, which a segment that grew since must still begin with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Held {
    pub(super) path: String,
    pub(super) key: FileKey,
    pub(super) segment: bool,
    pub(super) sum: Option<[u8; 16]>,
}

/// What a projection was made from: the build of bead that made it, and the journal's files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Made {
    pub(super) build: String,
    pub(super) files: Vec<Held>,
}

/// A projection file read as far as its state: what it was made from, its state, and where its
/// index is in the file it was read from, which stays open, whatever replaces it on disk.
pub(super) struct Stored {
    pub(super) made: Made,
    pub(super) state: Vec<u8>,
    file: File,
    index_at: u64,
    index_len: usize,
    index_sum: [u8; 16],
}

impl Stored {
    /// Its parts, the index read now. `None` when the index is not as written.
    pub(super) fn encoded(mut self) -> Option<Encoded> {
        self.file.seek(SeekFrom::Start(self.index_at)).ok()?;
        let mut index = vec![0; self.index_len];
        self.file.read_exact(&mut index).ok()?;
        (sum(&index) == self.index_sum).then_some(Encoded {
            state: self.state,
            index,
        })
    }
}

/// The projection at `path` as far as its state, when it is whole, written in this layout and
/// encoding, and its parts are as written. `None` for anything else: no projection.
pub(super) fn read(path: &Path) -> Option<Stored> {
    let mut file = File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let mut header = [0u8; HEADER];
    file.read_exact(&mut header).ok()?;
    let mut at = Fields(&header, 0);
    let ok = at.take::<16>()? == *MAGIC
        && u32::from_le_bytes(at.take()?) == ENCODING
        && u32::from_le_bytes(at.take()?) == LAYOUT;
    if !ok {
        return None;
    }
    let (made_len, state_len, index_len) = (at.number()?, at.number()?, at.number()?);
    let [made_sum, state_sum, index_sum]: [[u8; 16]; 3] = [at.take()?, at.take()?, at.take()?];
    // the lengths are the file's, so that nothing larger than it is read
    let index_at = (HEADER as u64)
        .checked_add(made_len)?
        .checked_add(state_len)?;
    if index_at.checked_add(index_len)? != size {
        return None;
    }
    let mut parts = vec![0; usize::try_from(index_at).ok()? - HEADER];
    file.read_exact(&mut parts).ok()?;
    let state = parts.split_off(usize::try_from(made_len).ok()?);
    if sum(&parts) != made_sum || sum(&state) != state_sum {
        return None;
    }
    Some(Stored {
        made: unmade(&parts)?,
        state,
        file,
        index_at,
        index_len: usize::try_from(index_len).ok()?,
        index_sum,
    })
}

/// The bytes of a projection made from `made`, its parts `encoded`.
pub(super) fn bytes(made: &Made, encoded: &Encoded) -> Vec<u8> {
    let made = made_bytes(made);
    let mut out =
        Vec::with_capacity(HEADER + made.len() + encoded.state.len() + encoded.index.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&ENCODING.to_le_bytes());
    out.extend_from_slice(&LAYOUT.to_le_bytes());
    for part in [&made, &encoded.state, &encoded.index] {
        out.extend_from_slice(&(part.len() as u64).to_le_bytes());
    }
    for part in [&made, &encoded.state, &encoded.index] {
        out.extend_from_slice(&sum(part));
    }
    for part in [&made, &encoded.state, &encoded.index] {
        out.extend_from_slice(part);
    }
    out
}

fn made_bytes(made: &Made) -> Vec<u8> {
    let mut out = Vec::new();
    let text = |out: &mut Vec<u8>, text: &str| {
        out.extend_from_slice(&(text.len() as u64).to_le_bytes());
        out.extend_from_slice(text.as_bytes());
    };
    text(&mut out, &made.build);
    out.extend_from_slice(&(made.files.len() as u64).to_le_bytes());
    for held in &made.files {
        text(&mut out, &held.path);
        out.push(u8::from(held.segment));
        match held.sum {
            Some(sum) => {
                out.push(1);
                out.extend_from_slice(&sum);
            }
            None => out.push(0),
        }
        let (len, modified, changed, inode, device) = held.key.parts();
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&modified.to_le_bytes());
        out.extend_from_slice(&changed.to_le_bytes());
        out.extend_from_slice(&inode.to_le_bytes());
        out.extend_from_slice(&device.to_le_bytes());
    }
    out
}

fn unmade(bytes: &[u8]) -> Option<Made> {
    let mut at = Fields(bytes, 0);
    let build = at.text()?;
    let count = usize::try_from(at.number()?).ok()?;
    let mut files = Vec::with_capacity(count.min(bytes.len()));
    for _ in 0..count {
        let path = at.text()?;
        let segment = match at.take::<1>()? {
            [0] => false,
            [1] => true,
            _ => return None,
        };
        let sum = match at.take::<1>()? {
            [0] => None,
            [1] => Some(at.take::<16>()?),
            _ => return None,
        };
        let len = at.number()?;
        let modified = i128::from_le_bytes(at.take()?);
        let changed = i128::from_le_bytes(at.take()?);
        let (inode, device) = (at.number()?, at.number()?);
        let key = FileKey::from_parts((len, modified, changed, inode, device));
        files.push(Held {
            path,
            key,
            segment,
            sum,
        });
    }
    (at.1 == bytes.len()).then_some(Made { build, files })
}

/// Fixed-width fields read from bytes, from an offset.
struct Fields<'a>(&'a [u8], usize);

impl Fields<'_> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let end = self.1.checked_add(N)?;
        let taken = self.0.get(self.1..end)?.try_into().ok()?;
        self.1 = end;
        Some(taken)
    }

    fn number(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take()?))
    }

    fn text(&mut self) -> Option<String> {
        let len = usize::try_from(self.number()?).ok()?;
        let end = self.1.checked_add(len)?;
        let text = std::str::from_utf8(self.0.get(self.1..end)?)
            .ok()?
            .to_owned();
        self.1 = end;
        Some(text)
    }
}
