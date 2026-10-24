//! Reads objects through a persistent `git cat-file --batch-command`, resolving names without
//! loading content when possible. Commit-tree lookup and tree listing are computed in-process
//! using git's format and validation rules.
//!
//! Anything that cannot be answered with certainty returns [`NoAnswer`], triggering a standalone
//! git invocation. Unlike ls-tree, the listing also checks each subtree's content against its
//! name; a caller's fallback must do the same before trusting git's listing.

use std::collections::HashMap;

use crate::hash::object_id;
use crate::runner::Env;
use crate::session::{NoAnswer, Session};

/// How deep a listing follows trees before it leaves the question to git.
const MAX_DEPTH: usize = 256;

/// One object as `cat-file --batch` gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Object {
    /// Its name: 40 hex digits (SHA-1) or 64 (SHA-256).
    pub oid: String,
    /// `blob`, `tree`, `commit` or `tag`.
    pub kind: String,
    /// Its content.
    pub data: Vec<u8>,
}

impl Object {
    /// Whether this object's type and content hash to its name. A successful cat-file read alone
    /// does not establish this: a loose object can hold another object's well-formed content.
    pub fn matches_name(&self) -> bool {
        object_id(&self.kind, &self.data, self.oid.len()).as_deref() == Some(self.oid.as_str())
    }
}

/// `git cat-file --batch-command` in the repository an [`Env`] names, started when first asked and
/// kept until dropped or stopped; a git that answers outside the protocol is dropped, and the next
/// question starts another.
#[derive(Debug)]
pub struct Objects {
    env: Env,
    session: Option<Session>,
}

impl Objects {
    /// A reader of the repository `env` runs git in; no git starts yet.
    pub fn new(env: &Env) -> Objects {
        Objects {
            env: env.clone(),
            session: None,
        }
    }

    /// Starts the git now, if none runs, so that it is ready for the first question: it finds
    /// names and objects afresh at each one. A git that cannot start leaves the question to the
    /// next attempt.
    pub fn start(&mut self) {
        if self.session.is_none() {
            self.session = Session::start(&self.env, ["cat-file", "--batch-command"]).ok();
        }
    }

    /// Ends the git, if one runs: the repository it reads is being set aside.
    pub fn stop(&mut self) {
        self.session = None;
    }

    /// The name of the object `name` names (an object name, a ref, `REV:PATH`), its object found
    /// but not read; no answer when git says it is missing or ambiguous, cannot be asked, or
    /// answers outside the protocol.
    pub fn name(&mut self, name: &str) -> Result<String, NoAnswer> {
        self.ask("info", name).map(|object| object.oid)
    }

    /// The object `name` names, whole; no answer as for [`Objects::name`].
    pub fn get(&mut self, name: &str) -> Result<Object, NoAnswer> {
        self.ask("contents", name)
    }

    fn ask(&mut self, command: &str, name: &str) -> Result<Object, NoAnswer> {
        if name.is_empty() || name.contains(['\n', '\r']) {
            return Err(NoAnswer);
        }
        let session = match &mut self.session {
            Some(session) => session,
            slot @ None => slot.insert(Session::start(&self.env, ["cat-file", "--batch-command"])?),
        };
        match ask(session, command, name) {
            Ok(Some(object)) => Ok(object),
            Ok(None) => Err(NoAnswer), // missing: the git itself is fine
            Err(NoAnswer) => {
                self.session = None;
                Err(NoAnswer)
            }
        }
    }
}

/// One question to `cat-file --batch-command`: `info NAME` (the header alone: the content is
/// empty) or `contents NAME` (the header and the content); `None` when git says the object is
/// missing (or the name ambiguous).
fn ask(session: &mut Session, command: &str, name: &str) -> Result<Option<Object>, NoAnswer> {
    session.send(format!("{command} {name}\n").as_bytes())?;
    let header = String::from_utf8(session.line()?).map_err(|_| NoAnswer)?;
    if let Some(said) = header.strip_prefix(name)
        && matches!(said, " missing" | " ambiguous")
    {
        return Ok(None);
    }
    let fields: Vec<&str> = header.split(' ').collect();
    let [oid, kind, size] = fields.as_slice() else {
        return Err(NoAnswer);
    };
    let known = matches!(*kind, "blob" | "tree" | "commit" | "tag");
    if !known || !lower_hex(oid) || !matches!(oid.len(), 40 | 64) || !digits(size) {
        return Err(NoAnswer);
    }
    let mut data = Vec::new();
    if command == "contents" {
        data = session.exact(size.parse().map_err(|_| NoAnswer)?)?;
        if session.exact(1)? != b"\n" {
            return Err(NoAnswer);
        }
    }
    Ok(Some(Object {
        oid: (*oid).to_owned(),
        kind: (*kind).to_owned(),
        data,
    }))
}

/// What `git rev-parse COMMIT^{tree}` finds for `commit`, read with [`Objects::get`]: its tree
/// object. git reads both whole there and checks each against its name, and so does this.
pub fn root_tree(objects: &mut Objects, commit: &Object) -> Result<Object, NoAnswer> {
    let hex_len = commit.oid.len();
    if commit.kind != "commit" || !commit.matches_name() {
        return Err(NoAnswer);
    }
    let tree_oid = commit_tree(&commit.data, hex_len).ok_or(NoAnswer)?;
    let tree = objects.get(&tree_oid)?;
    if tree.kind != "tree" || tree.oid != tree_oid || !tree.matches_name() {
        return Err(NoAnswer);
    }
    Ok(tree)
}

/// What `git ls-tree -r -z COMMIT` lists for `commit`, read with [`Objects::get`]: every entry of
/// its tree that is not itself a tree, depth first in each tree's own order, as its path and its
/// object's name.
pub fn list_tree(
    objects: &mut Objects,
    commit: &Object,
) -> Result<Vec<(Vec<u8>, String)>, NoAnswer> {
    Ok(list_tree_with_subtrees(objects, commit)?.files)
}

/// A recursive listing together with the subtree objects encountered along its paths.
#[derive(Debug)]
pub struct TreeListing {
    /// Nontree entries, as [`list_tree`] returns them.
    pub files: Vec<(Vec<u8>, String)>,
    /// Subtree names by their paths, without a trailing slash.
    pub subtrees: HashMap<Vec<u8>, String>,
}

/// [`list_tree`], also retaining the names of the subtrees it read. These names are reachable
/// from this commit; a caller may reuse one only if it describes exactly the desired entries.
/// Every tree's content is checked against its name. On [`NoAnswer`], a fallback must check the
/// trees it reads too: ls-tree alone accepts a subtree whose content does not hash to its name.
pub fn list_tree_with_subtrees(
    objects: &mut Objects,
    commit: &Object,
) -> Result<TreeListing, NoAnswer> {
    let root = root_tree(objects, commit)?;
    let mut listing = Listing {
        objects,
        raw_len: root.oid.len() / 2,
        trees: HashMap::new(),
        rows: Vec::new(),
        subtrees: HashMap::new(),
    };
    listing.walk(&root.data, &[], 0)?;
    Ok(TreeListing {
        files: listing.rows,
        subtrees: listing.subtrees,
    })
}

/// A listing under way: the trees read so far, by name, and the rows found.
struct Listing<'a> {
    objects: &'a mut Objects,
    raw_len: usize,
    trees: HashMap<String, Vec<u8>>,
    rows: Vec<(Vec<u8>, String)>,
    subtrees: HashMap<Vec<u8>, String>,
}

impl Listing<'_> {
    fn walk(&mut self, data: &[u8], prefix: &[u8], depth: usize) -> Result<(), NoAnswer> {
        if depth > MAX_DEPTH {
            return Err(NoAnswer);
        }
        for entry in tree_entries(data, self.raw_len).ok_or(NoAnswer)? {
            if entry.mode > 0o177777 {
                return Err(NoAnswer); // git versions keep a mode in 16 bits or 32: leave it to git
            }
            let mut path = prefix.to_vec();
            path.extend_from_slice(entry.name);
            let oid = hex(entry.oid);
            if entry.mode & 0o170000 != 0o040000 {
                self.rows.push((path, oid));
                continue;
            }
            // A directory: follow ls-tree's order, but verify its content before trusting it.
            self.subtrees.insert(path.clone(), oid.clone());
            let tree = match self.trees.get(&oid) {
                Some(tree) => tree.clone(),
                None => {
                    let object = self.objects.get(&oid)?;
                    if object.kind != "tree" || object.oid != oid || !object.matches_name() {
                        return Err(NoAnswer);
                    }
                    self.trees.insert(oid, object.data.clone());
                    object.data
                }
            };
            path.push(b'/');
            self.walk(&tree, &path, depth + 1)?;
        }
        Ok(())
    }
}

/// One entry of a tree object: its mode as read, its name, and its object's name (raw bytes).
#[derive(Debug, PartialEq, Eq)]
struct Entry<'a> {
    mode: u32,
    name: &'a [u8],
    oid: &'a [u8],
}

/// The entries of the tree object `data`, whose object names are `raw_len` bytes, as git's
/// `decode_tree_entry` reads them: octal digits and a space, a name ending at its NUL, the name of
/// its object; with git's own check that what is left ends in a NUL and a name. `None` where git
/// refuses the tree ("too-short tree object", "malformed mode in tree entry", "empty filename in
/// tree entry", "too-short tree file").
fn tree_entries(data: &[u8], raw_len: usize) -> Option<Vec<Entry<'_>>> {
    let mut entries = Vec::new();
    let mut rest = data;
    while !rest.is_empty() {
        let last_nul = rest.len().checked_sub(raw_len + 1)?;
        if rest.len() < raw_len + 3 || rest.get(last_nul) != Some(&0) {
            return None;
        }
        let digits = rest.iter().position(|b| !(b'0'..=b'7').contains(b))?;
        if digits == 0 || rest.get(digits) != Some(&b' ') {
            return None;
        }
        let mode = rest[..digits].iter().fold(0u32, |mode, d| {
            (mode << 3).wrapping_add(u32::from(d - b'0'))
        });
        let named = rest.get(digits + 1..)?;
        let end = named.iter().position(|b| *b == 0)?;
        if end == 0 {
            return None;
        }
        let oid = named.get(end + 1..end + 1 + raw_len)?;
        entries.push(Entry {
            mode,
            name: &named[..end],
            oid,
        });
        rest = named.get(end + 1 + raw_len..)?;
    }
    Some(entries)
}

/// The tree a commit object names, as git's `parse_commit_buffer` reads it: a `tree` line first,
/// then any `parent` lines, each a name of `hex_len` digits and a newline, a parent line never
/// the last thing in the object, and no parent that is the tree. `None` where git refuses the
/// commit, or spells a name in capitals, which git reads and this leaves to git.
fn commit_tree(data: &[u8], hex_len: usize) -> Option<String> {
    let tree_len = hex_len + 5;
    if data.len() <= tree_len + 1 || !data.starts_with(b"tree ") || data[tree_len] != b'\n' {
        return None;
    }
    let tree = std::str::from_utf8(&data[5..tree_len]).ok()?;
    if !lower_hex(tree) {
        return None;
    }
    let parent_len = hex_len + 7;
    let mut rest = &data[tree_len + 1..];
    while rest.len() > parent_len && rest.starts_with(b"parent ") {
        if rest.len() <= parent_len + 1 || rest[parent_len] != b'\n' {
            return None;
        }
        let parent = std::str::from_utf8(&rest[7..parent_len]).ok()?;
        if !lower_hex(parent) || parent == tree {
            return None;
        }
        rest = &rest[parent_len + 1..];
    }
    Some(tree.to_owned())
}

/// Lower-case hex digits only, at least one.
fn lower_hex(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// ASCII digits only, at least one.
fn digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// Raw bytes as lower-case hex.
fn hex(raw: &[u8]) -> String {
    raw.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests;
