//! A repository's objects read through one long-lived `git cat-file --batch-command`
//! (`docs/RUST-PORT.md` Appendix B: `--batch`'s answers, with `info` beside `contents`, so that a
//! name is resolved without its object being read, as `rev-parse` resolves it), and what bead used
//! to ask other gits to work out from them worked out here as those gits work it out: a commit's
//! tree as `rev-parse COMMIT^{tree}` finds it, and a listing as `ls-tree -r -z COMMIT` gives it.
//!
//! Anything git would refuse, and anything this code does not read exactly as git reads it, is
//! [`NoAnswer`]: the caller then runs the git command Python runs, whose answer, or failure, is the
//! one bead gives. So a damaged or hostile object never changes what bead says, only how many gits
//! it took to find out.

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
    let named = |object: &Object| object_id(&object.kind, &object.data, hex_len);
    if commit.kind != "commit" || named(commit).as_deref() != Some(commit.oid.as_str()) {
        return Err(NoAnswer);
    }
    let tree_oid = commit_tree(&commit.data, hex_len).ok_or(NoAnswer)?;
    let tree = objects.get(&tree_oid)?;
    if tree.kind != "tree" || tree.oid != tree_oid || named(&tree).as_deref() != Some(&tree_oid) {
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
    let root = root_tree(objects, commit)?;
    let mut listing = Listing {
        objects,
        raw_len: root.oid.len() / 2,
        trees: HashMap::new(),
        rows: Vec::new(),
    };
    listing.walk(&root.data, &[], 0)?;
    Ok(listing.rows)
}

/// A listing under way: the trees read so far, by name, and the rows found.
struct Listing<'a> {
    objects: &'a mut Objects,
    raw_len: usize,
    trees: HashMap<String, Vec<u8>>,
    rows: Vec<(Vec<u8>, String)>,
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
            // a directory: ls-tree -r goes into it there and then, without checking its name
            let tree = match self.trees.get(&oid) {
                Some(tree) => tree.clone(),
                None => {
                    let object = self.objects.get(&oid)?;
                    if object.kind != "tree" || object.oid != oid {
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
mod tests {
    use super::*;

    fn raw(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
            .collect()
    }

    fn entry(mode: &str, name: &str, oid: &str) -> Vec<u8> {
        [mode.as_bytes(), b" ", name.as_bytes(), b"\0", &raw(oid)].concat()
    }

    const BLOB: &str = "ce013625030ba8dba906f756967f9e9ca394464a";

    #[test]
    fn tree_entries_are_read_as_git_reads_them() {
        let data = [
            entry("100644", "a.json", BLOB),
            entry("40000", "claims", BLOB),
        ]
        .concat();
        let entries = tree_entries(&data, 20).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].mode, 0o100644);
        assert_eq!(entries[1].name, b"claims");
        assert_eq!(hex(entries[1].oid), BLOB);
        assert_eq!(tree_entries(b"", 20), Some(Vec::new()));
        // what git refuses
        for bad in [
            entry(" 100644", "a", BLOB),
            entry("100648", "a", BLOB),
            entry("", "a", BLOB),
            entry("100644", "", BLOB),
            entry("100644", "a", BLOB)[..20].to_vec(),
            [entry("100644", "a", BLOB), b"1".to_vec()].concat(),
        ] {
            assert_eq!(tree_entries(&bad, 20), None, "{bad:?}");
        }
    }

    #[test]
    fn a_commit_names_its_tree_as_git_reads_it() {
        let tree = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
        let good = format!("tree {tree}\nparent {BLOB}\nauthor a <a> 1 +0000\n\nm\n");
        assert_eq!(commit_tree(good.as_bytes(), 40).as_deref(), Some(tree));
        let bare = format!("tree {tree}\nauthor a\n");
        assert_eq!(commit_tree(bare.as_bytes(), 40).as_deref(), Some(tree));
        for bad in [
            format!("tree {tree}\n"),
            format!("tree {}\nauthor a\n", tree.to_uppercase()),
            format!("tree {tree} \nauthor a\n"),
            format!("parent {BLOB}\ntree {tree}\n"),
            format!("tree {tree}\nparent {BLOB}\n"),
            format!("tree {tree}\nparent {BLOB}x\nauthor a\n"),
            format!("tree {tree}\nparent {tree}\nauthor a\n"),
        ] {
            assert_eq!(commit_tree(bad.as_bytes(), 40), None, "{bad:?}");
        }
    }

    /// `ls-tree -r -z` of `commit` in `env`, read back as rows.
    fn ls_tree(env: &Env, commit: &str) -> Vec<(Vec<u8>, String)> {
        let listing = env.git(["ls-tree", "-r", "-z", commit]).check().unwrap();
        let rows = listing
            .stdout
            .split(|b| *b == 0)
            .filter(|row| !row.is_empty());
        rows.map(|row| {
            let tab = row.iter().position(|b| *b == b'\t').unwrap();
            let meta = String::from_utf8(row[..tab].to_vec()).unwrap();
            let sha = meta.split_whitespace().nth(2).unwrap().to_owned();
            (row[tab + 1..].to_vec(), sha)
        })
        .collect()
    }

    /// A commit of a tree shaped like a published journal, with an entry of every kind git has
    /// (executable, link, gitlink, a subtree twice), in `env`'s repository; its name.
    fn journal_commit(env: &Env) -> String {
        let blob = |data: &str| {
            let hashed = env.git(["hash-object", "-w", "--stdin"]).input(data);
            hashed.string().unwrap()
        };
        let mktree = |entries: String| env.git(["mktree"]).input(entries).string().unwrap();
        let (a, b) = (blob("a\n"), blob("b\n"));
        let missing = "1".repeat(a.len());
        let claims = mktree(format!("100644 blob {a}\t0190-ab.json\n"));
        let replica = mktree(format!(
            "100644 blob {a}\t000001.jsonl\n100755 blob {b}\trun\n120000 blob {b}\tlink\n\
             040000 tree {claims}\tclaims\n160000 commit {missing}\tmodule\n"
        ));
        let events = mktree(format!(
            "040000 tree {replica}\tm1\n040000 tree {replica}\tm\u{e9}2\n"
        ));
        let top = mktree(format!(
            "040000 tree {events}\tevents\n100644 blob {a}\tmanifest.json\n"
        ));
        let identity = ["-c", "user.name=t", "-c", "user.email=t@example.invalid"];
        let args = identity.into_iter().chain(["commit-tree", &top, "-m", "m"]);
        env.git(args).string().unwrap()
    }

    #[test]
    fn listings_trees_and_names_are_the_ones_git_gives() {
        for format in ["sha1", "sha256"] {
            let (_dir, env) = crate::testing::bare(format);
            let commit = journal_commit(&env);
            let mut objects = Objects::new(&env);
            let tip = objects.get(&commit).unwrap();
            assert_eq!(
                list_tree(&mut objects, &tip).unwrap(),
                ls_tree(&env, &commit)
            );
            let tree = env.git(["rev-parse", &format!("{commit}^{{tree}}")]);
            assert_eq!(
                root_tree(&mut objects, &tip).unwrap().oid,
                tree.string().unwrap()
            );
            env.git(["update-ref", "refs/beads/x", &commit])
                .check()
                .unwrap();
            assert_eq!(objects.name("refs/beads/x"), Ok(commit.clone()));
            let manifest = objects.get(&format!("{commit}:manifest.json")).unwrap();
            assert_eq!(
                (manifest.kind.as_str(), &manifest.data[..]),
                ("blob", &b"a\n"[..])
            );
            // missing: no answer, and the same git answers the next question
            assert_eq!(objects.get(&"2".repeat(commit.len())), Err(NoAnswer));
            assert_eq!(objects.name("refs/beads/none"), Err(NoAnswer));
            assert_eq!(objects.get(&commit).unwrap().oid, commit);
        }
    }

    #[test]
    fn an_object_that_is_not_what_its_name_says_is_left_to_git() {
        let (dir, env) = crate::testing::bare("sha1");
        let commit = journal_commit(&env);
        let identity = ["-c", "user.name=t", "-c", "user.email=t@example.invalid"];
        let other = env
            .git(identity.into_iter().chain([
                "commit-tree",
                &format!("{commit}^{{tree}}"),
                "-m",
                "x",
            ]))
            .string()
            .unwrap();
        let loose = |name: &str| {
            dir.path()
                .join("r.git/objects")
                .join(&name[..2])
                .join(&name[2..])
        };
        // the commit's file now holds another commit, whole and readable
        std::fs::remove_file(loose(&commit)).unwrap();
        std::fs::copy(loose(&other), loose(&commit)).unwrap();
        let mut objects = Objects::new(&env);
        let tip = objects.get(&commit).unwrap();
        assert_eq!(list_tree(&mut objects, &tip), Err(NoAnswer));
        assert_eq!(root_tree(&mut objects, &tip), Err(NoAnswer));
        // git refuses both: the caller's git gives bead the same failure as before
        assert!(
            !env.git(["ls-tree", "-r", "-z", &commit])
                .run()
                .unwrap()
                .success()
        );
        let peeled = env.git(["rev-parse", &format!("{commit}^{{tree}}")]);
        assert!(!peeled.run().unwrap().success());
    }
}
