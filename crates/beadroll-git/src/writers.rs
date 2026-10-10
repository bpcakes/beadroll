//! Objects and refs written through long-lived gits (`docs/RUST-PORT.md` Appendix B): one `git
//! hash-object -w --no-filters --stdin-paths` for every blob, one `git mktree --batch` for every
//! tree of a publish, one `git update-ref --stdin` whose transactions do the ref bookkeeping.
//!
//! Each writes exactly what the one-shot command Python runs writes. A question it cannot ask in
//! the same words (a name holding a newline, which would end a line early) is [`NoAnswer`] before
//! anything is sent, and so is any answer outside the protocol; the caller then runs the one-shot
//! command, whose answer, or failure, is the one bead gives.

use std::io::Write as _;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;

use crate::runner::Env;
use crate::session::{NoAnswer, Session};

/// Paths asked of `hash-object` at a time: their answers (one name a line) always fit in a pipe's
/// buffer, so git never waits for bead to read while bead waits for git to read.
const PATHS_AT_A_TIME: usize = 32;

/// `git hash-object -w --no-filters --stdin-paths`: each file named becomes a blob, written into
/// the repository unless it is there already, as the one-shot command writes it. A `hash-object`
/// sees the packs present when it first looks for an object; one kept across a fetch would write
/// loose copies of what the fetch brought, which a fresh one does not, so a caller keeps one only
/// between fetches.
#[derive(Debug)]
pub struct Hasher {
    session: Session,
}

impl Hasher {
    /// A `hash-object` in the repository `env` runs git in.
    pub fn start(env: &Env) -> Result<Hasher, NoAnswer> {
        let args = ["hash-object", "-w", "--no-filters", "--stdin-paths"];
        Ok(Hasher {
            session: Session::start(env, args)?,
        })
    }

    /// The blob names of the files at `paths`, in order: what `hash-object -w --no-filters
    /// --stdin-paths` prints for them, one a line.
    pub fn hash(&mut self, paths: &[&Path]) -> Result<Vec<String>, NoAnswer> {
        let mut names = Vec::with_capacity(paths.len());
        for chunk in paths.chunks(PATHS_AT_A_TIME) {
            let mut request = Vec::new();
            for path in chunk {
                let bytes = path.as_os_str().as_bytes();
                // a newline ends the line early, a leading quote is unquoted: not the same question
                if bytes.is_empty() || bytes.contains(&b'\n') || bytes.starts_with(b"\"") {
                    return Err(NoAnswer);
                }
                request.extend_from_slice(bytes);
                request.push(b'\n');
            }
            self.session.send(&request)?;
            for _ in chunk {
                names.push(object_name(self.session.line()?)?);
            }
        }
        Ok(names)
    }

    /// The blob name of `data`, written as `hash-object -w --stdin` writes it: through a file of its
    /// own in `tmp`, removed once git has read it.
    pub fn hash_data(&mut self, data: &[u8], tmp: &Path) -> Result<String, NoAnswer> {
        let mut file = tempfile::NamedTempFile::new_in(tmp).map_err(|_| NoAnswer)?;
        file.write_all(data)
            .and_then(|()| file.flush())
            .map_err(|_| NoAnswer)?;
        let names = self.hash(&[file.path()])?;
        names.into_iter().next().ok_or(NoAnswer)
    }
}

/// `git mktree --batch`: the trees of one publish, each written as one-shot `mktree` writes it. A
/// `mktree` looks objects up without looking for packs again, so one is started after the fetch
/// whose packs its trees name.
#[derive(Debug)]
pub struct Trees {
    session: Session,
}

impl Trees {
    /// A `mktree --batch` in the repository `env` runs git in.
    pub fn start(env: &Env) -> Result<Trees, NoAnswer> {
        Ok(Trees {
            session: Session::start(env, ["mktree", "--batch"])?,
        })
    }

    /// The name of the tree `entries` make (`mktree`'s input: one entry a line, each ending in a
    /// newline; none for the empty tree), written.
    pub fn make(&mut self, entries: &[u8]) -> Result<String, NoAnswer> {
        // a blank line ends a tree in batch mode, where one-shot mktree refuses it
        let blank = entries.starts_with(b"\n") || entries.windows(2).any(|w| w == b"\n\n");
        if blank || !(entries.is_empty() || entries.ends_with(b"\n")) {
            return Err(NoAnswer);
        }
        let mut request = entries.to_vec();
        request.push(b'\n');
        self.session.send(&request)?;
        object_name(self.session.line()?)
    }
}

/// `git update-ref --stdin`: ref updates made as transactions, one after another in one git, each
/// confirmed before the next.
#[derive(Debug)]
pub struct Refs {
    session: Session,
}

impl Refs {
    /// An `update-ref --stdin` in the repository `env` runs git in.
    pub fn start(env: &Env) -> Result<Refs, NoAnswer> {
        Ok(Refs {
            session: Session::start(env, ["update-ref", "--stdin"])?,
        })
    }

    /// Applies `commands` (`update-ref --stdin` lines such as `update REF NEW` and `delete REF`) as
    /// one transaction: all of them, or none and no answer. git ends at a transaction it could not
    /// commit; this one cannot be used again.
    pub fn apply(&mut self, commands: &[String]) -> Result<(), NoAnswer> {
        let mut request = String::from("start\n");
        for command in commands {
            if command.is_empty() || command.contains(['\n', '\r']) {
                return Err(NoAnswer);
            }
            request.push_str(command);
            request.push('\n');
        }
        request.push_str("commit\n");
        self.session.send(request.as_bytes())?;
        for expected in [&b"start: ok"[..], b"commit: ok"] {
            if self.session.line()? != expected {
                return Err(NoAnswer);
            }
        }
        Ok(())
    }
}

/// An object name as git prints it on a line of its own.
fn object_name(line: Vec<u8>) -> Result<String, NoAnswer> {
    let name = String::from_utf8(line).map_err(|_| NoAnswer)?;
    let hex = name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if hex && matches!(name.len(), 40 | 64) {
        Ok(name)
    } else {
        Err(NoAnswer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bare repository in a temporary directory, and the environment git runs in there.
    fn repository() -> (tempfile::TempDir, Env) {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join("r.git");
        let mut vars: std::collections::BTreeMap<_, _> = std::env::vars_os().collect();
        vars.insert("GIT_DIR".into(), git_dir.into_os_string());
        let env = Env::new(vars, None, std::env::temp_dir());
        env.git(["init", "-q", "--bare"]).check().unwrap();
        (dir, env)
    }

    #[test]
    fn blobs_trees_and_refs_are_written_as_the_one_shot_commands_write_them() {
        let (dir, env) = repository();
        let file = dir.path().join("hello");
        std::fs::write(&file, b"hello\n").unwrap();
        let mut hasher = Hasher::start(&env).unwrap();
        let paths: Vec<&Path> = std::iter::repeat_n(file.as_path(), 70).collect();
        let names = hasher.hash(&paths).unwrap();
        assert_eq!(names.len(), 70);
        assert!(
            names
                .iter()
                .all(|n| n == "ce013625030ba8dba906f756967f9e9ca394464a")
        );
        let blob = hasher.hash_data(b"hello\n", &std::env::temp_dir()).unwrap();
        assert_eq!(blob, names[0]);
        let mut trees = Trees::start(&env).unwrap();
        let entry = format!("100644 blob {blob}\thello\n");
        let tree = trees.make(entry.as_bytes()).unwrap();
        let once = env.git(["mktree"]).input(entry).string().unwrap();
        assert_eq!(tree, once);
        assert_eq!(
            trees.make(b"").unwrap(),
            "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
        );
        assert_eq!(trees.make(b"x\n\ny\n"), Err(NoAnswer));
        let commit = env
            .git([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit-tree",
                &tree,
                "-m",
                "m",
            ])
            .string()
            .unwrap();
        let mut refs = Refs::start(&env).unwrap();
        refs.apply(&[format!("update refs/beads/remote {commit}")])
            .unwrap();
        refs.apply(&["delete refs/beads/nothing-here".to_owned()])
            .unwrap();
        let tip = env
            .git(["rev-parse", "refs/beads/remote"])
            .string()
            .unwrap();
        assert_eq!(tip, commit);
        // a transaction git cannot commit ends that git: no answer, and none after it
        refs.apply(&[format!("update refs/beads/remote {blob} {blob}")])
            .unwrap_err();
        assert_eq!(
            refs.apply(&["delete refs/beads/x".to_owned()]),
            Err(NoAnswer)
        );
        assert_eq!(hasher.hash(&[Path::new("/no\nsuch")]), Err(NoAnswer));
    }
}
