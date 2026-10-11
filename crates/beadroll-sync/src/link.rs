//! The coordination ref on one remote, reached through a private bare repository (Python's `Link`).
//! The pushed history holds the manifest and the journals and nothing else: no code commits are
//! its ancestors and no code objects are reachable from it.
//!
//! Python runs one git for each question it asks `sync.git`. A link keeps a few gits instead
//! (`docs/RUST-PORT.md` Appendix B): a `cat-file` for every object it reads (`link/read.rs`), and
//! a `hash-object`, a `mktree` and an `update-ref` for what it writes (`link/write.rs`). Each
//! answer is the one the git Python runs gives, and that git is run whenever a kept one has no
//! certain answer, so every failure reads as before. They end with the link, or when `sync.git` is
//! set aside. The refs a fetch leaves are set by Python's own two calls, at its pace: other
//! processes' fetches are still reading every ref then ([`Link::keep_fetched`]).
//!
//! A link hashes only the journal files it cannot vouch for (roll-lqjj.1): the store's memo
//! (`beadroll_store::Memo`) remembers each file's blob for the identity the file had when it was
//! hashed, and a publish uses a remembered blob while the file keeps that identity and the blob is
//! sure to be in `sync.git`: this link made it, or the tree of the tip it last merged holds it at
//! that path. The tree it publishes is the one hashing every file would make.

mod asked;
mod fetch;
mod merge;
mod publish;
mod read;
mod write;

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt as _;
use std::path::PathBuf;

use beadroll_core::json::{Object, Value};
use beadroll_git::{
    CountRules, Env, Fact, Hasher, IoError, Objects, Refs, Trees, local_damage, sync_env,
    transport_config, transport_settings, with_settings,
};
use beadroll_store::{
    Endpoint, Memo, Workspace, claim_files, dir_entries, has_suffix, hex_token, private_dir,
    relative, segment_files,
};

use crate::error::Error;

/// The coordination ref on one remote, and what the last exchange with it found.
#[derive(Debug)]
pub struct Link<'a> {
    ws: &'a Workspace,
    /// The remote, as git is given it.
    pub url: OsString,
    /// Whether that is the remote's own push URL, which a push goes to as `git push NAME` goes
    /// there, without `pushInsteadOf` ([`Endpoint`]).
    own_push_url: bool,
    /// The name git knows the remote by in this link's commands ([`defining_remote`]).
    remote_name: String,
    /// The name of the remote it was chosen by, when it was ([`Endpoint`]).
    name: Option<OsString>,
    /// The coordination ref.
    pub ref_name: String,
    /// The private bare repository, `<ws>/sync.git`.
    pub git_dir: PathBuf,
    /// The environment its git runs in.
    pub env: Env,
    object_format: Option<String>,
    /// What git said about the last push that did not go through.
    pub last_error: String,
    /// What the remote holds, by path: lines per segment, one per attempt file.
    pub remote_counts: HashMap<String, usize>,
    /// Paths whose local copy differed from the published one: kept beside it, replaced.
    pub forks: Vec<String>,
    /// This machine's pending attempts the last merge found accepted.
    pub confirmed: Vec<String>,
    /// Their events.
    pub confirmed_events: Vec<Value>,
    /// Where `sync.git` was set aside after damage, once per command.
    pub rebuilt: Option<String>,
    /// The commit the last publish left the remote at.
    pub last_commit: Option<String>,
    /// `cat-file --batch-command` in `sync.git`: every object read.
    objects: Objects,
    /// `hash-object --stdin-paths` in `sync.git`, started after a fetch and ended by the next: one
    /// kept across a fetch would not look in the pack the fetch brought.
    hasher: Option<Hasher>,
    /// `mktree --batch` for the next publish's trees, started after a fetch as `hasher` is.
    trees: Option<Trees>,
    /// `update-ref --stdin` in `sync.git`, started once a publish has something to push:
    /// `refs/beads/remote` after the push.
    refs: Option<Refs>,
    /// What is remembered of the store's journal files: read again by each merge, under the store
    /// lock, added to as files are hashed, counted and written, and written back when it changed.
    memo: Option<Memo>,
    /// The blobs this link has made in `sync.git` itself since it was made or last set aside.
    made: HashSet<String>,
    /// The blob the tree of the last merged tip holds at each path.
    listed: HashMap<String, String>,
    /// The store's journal files this link has handed to `hash-object`, by path, in order: a file
    /// whose remembered blob it could use is not among them.
    pub hashed: Vec<String>,
}

/// The environment a link to `ws`'s remote runs git in ([`sync_env`], with `GIT_DIR` its
/// `sync.git`), starting at the top of the checkout when there is one: bead reads a relative remote
/// path from there wherever it runs, and git reads one it makes of a rewritten URL (`insteadOf`,
/// `pushInsteadOf`) from where it starts.
fn link_env(ws: &Workspace) -> Result<Env, Error> {
    let settings = &ws.settings;
    let carried = transport_config(&transport_settings(&settings.git)?);
    let int = |text: &str| beadroll_core::text::py_int(text).map(|n| n.to_string());
    let rules = CountRules {
        is_digit: &beadroll_core::text::py_isdigit,
        int: &int,
    };
    let env = sync_env(&settings.git, &ws.dir.join("sync.git"), &carried, &rules)?;
    let top = settings.fact(Fact::Toplevel)?;
    Ok(if top.success() {
        // the line git prints, without its end: a directory's name may end in a space
        let top = top.stdout.strip_suffix(b"\n").unwrap_or(&top.stdout);
        env.with_dir(PathBuf::from(OsStr::from_bytes(top)))
    } else {
        env
    })
}

/// `env` with `name` a remote at `url` ([`with_settings`]), with `url` as its push URL too when it
/// is a remote's own push URL: git, given `name`, reads `url` as a remote's URL, never as the name
/// of another remote, which a URL spelled like one would be as an argument (one of the checkout's
/// remotes, or one the configuration it inherits defines), and rewrites it as it does that
/// remote's (`insteadOf`, and for a push `pushInsteadOf` unless it is the remote's own push URL),
/// as for `git fetch NAME` and `git push NAME`. `name` is made for the link, so that no
/// configuration defines that remote already: its URLs would be added to.
pub(crate) fn defining_remote(env: &Env, name: &str, url: &OsStr, own_push_url: bool) -> Env {
    let keys: &[&str] = if own_push_url {
        &["url", "pushurl"]
    } else {
        &["url"]
    };
    let settings: Vec<(String, OsString)> = keys
        .iter()
        .map(|key| (format!("remote.{name}.{key}"), url.to_owned()))
        .collect();
    with_settings(env, &settings)
}

impl<'a> Link<'a> {
    /// The link of `ws` to `remote`: its ref checked, its environment built from the checkout's
    /// with the checkout's transport settings carried over, asked of git now, as Python asks them
    /// each time it makes a link.
    pub fn new(ws: &'a Workspace, remote: &Endpoint) -> Result<Link<'a>, Error> {
        let ref_name = ws.ref_name()?;
        private_dir(&ws.dir)?;
        let git_dir = ws.dir.join("sync.git");
        let env = link_env(ws)?;
        Ok(Link {
            ws,
            url: remote.url.clone(),
            own_push_url: remote.own_push_url,
            remote_name: format!("bead-{}", hex_token(12)),
            name: remote.name.clone(),
            ref_name,
            git_dir,
            objects: Objects::new(&env),
            env,
            object_format: None,
            last_error: String::new(),
            remote_counts: HashMap::new(),
            forks: Vec::new(),
            confirmed: Vec::new(),
            confirmed_events: Vec::new(),
            rebuilt: None,
            last_commit: None,
            hasher: None,
            trees: None,
            refs: None,
            memo: None,
            made: HashSet::new(),
            listed: HashMap::new(),
            hashed: Vec::new(),
        })
    }

    /// The remote, for messages.
    pub fn url_text(&self) -> String {
        self.url.to_string_lossy().into_owned()
    }

    /// This link's environment with its remote defined ([`defining_remote`]), for the gits that
    /// reach it.
    fn remote_env(&self) -> Env {
        defining_remote(&self.env, &self.remote_name, &self.url, self.own_push_url)
    }

    /// The endpoint this link reaches, as it was chosen.
    pub fn endpoint(&self) -> Endpoint {
        Endpoint {
            url: self.url.clone(),
            own_push_url: self.own_push_url,
            name: self.name.clone(),
        }
    }

    /// `git ARGS` in `sync.git`, its output stripped; a failure that is damage to `sync.git`'s own
    /// objects is [`Error::SyncRepoDamaged`].
    fn git(&self, args: &[&OsStr], input: Option<Vec<u8>>) -> Result<String, Error> {
        let mut git = self.env.git(args);
        if let Some(data) = input {
            git = git.input(data);
        }
        match git.string() {
            Ok(text) => Ok(text),
            Err(error @ beadroll_git::Error::Failed { .. }) => {
                let said = error.to_string();
                if local_damage(&said) {
                    return Err(self.damage(&said));
                }
                Err(error.into())
            }
            Err(error) => Err(error.into()),
        }
    }

    /// `git ARGS` in `sync.git`, whatever its exit status.
    fn run(&self, args: &[&OsStr]) -> Result<beadroll_git::Output, Error> {
        Ok(self.env.git(args).run()?)
    }

    /// `git ARGS` in `sync.git`; a failure is `GIT_ERROR`, never read as damage.
    fn check(&self, args: &[&OsStr]) -> Result<beadroll_git::Output, Error> {
        Ok(self.env.git(args).check()?)
    }

    fn damage(&self, said: &str) -> Error {
        Error::SyncRepoDamaged(format!(
            "this machine's sync repository {} is damaged; it holds nothing that is not also in \
             events/ or on the remote, so bead makes it again (the damaged one is kept beside it as \
             sync.git.damaged-<time>); if this persists, delete it and run the command again. git \
             said: {said}",
            self.git_dir.display()
        ))
    }

    /// Ends the gits this link keeps in `sync.git`, and forgets which blobs it holds and the memo
    /// kept there: it is being set aside.
    fn stop_gits(&mut self) {
        self.objects.stop();
        self.hasher = None;
        self.trees = None;
        self.refs = None;
        self.memo = None;
        self.made.clear();
        self.listed.clear();
    }

    /// What is remembered of the store's journal files, read when first needed.
    fn memo(&mut self) -> &mut Memo {
        let ws = self.ws;
        self.memo.get_or_insert_with(|| Memo::load(ws))
    }

    /// Forgets every blob remembered and made: each file is hashed again when next needed.
    fn forget_blobs(&mut self) {
        self.memo().forget_blobs();
        self.made.clear();
    }

    /// Writes the memo back if it changed. Lock held.
    fn save_memo(&mut self) {
        if let Some(memo) = &mut self.memo {
            memo.save(self.ws);
        }
    }

    /// `sync.git` is a cache of what the remote holds and of `events/`: a damaged one is set aside,
    /// kept for a look, and made again by the next fetch. Lock held.
    pub fn rebuild(&mut self) -> Result<(), Error> {
        self.stop_gits();
        let aside = self.git_dir.with_file_name(format!(
            "sync.git.damaged-{}",
            self.ws.settings.clock.now_ns()
        ));
        match std::fs::rename(&self.git_dir, &aside) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {} // set aside already
            Err(error) => return Err(IoError::at(&error, &aside).into()),
        }
        self.rebuilt = Some(aside.display().to_string());
        self.object_format = None;
        self.remote_counts.clear();
        Ok(())
    }

    /// Repacks `sync.git` when loose objects or packs have piled up: one pack, loose copies of
    /// packed objects gone, unreachable loose objects older than an hour gone (an in-flight fetch's
    /// are younger). Lock held, so no bead process of this store is writing; git's own maintenance
    /// is off. A blob this link wrote may be gone afterwards, unless a tree it published holds it.
    pub fn maintain(&mut self) -> Result<bool, Error> {
        let objects = self.git_dir.join("objects");
        let mut loose = 0usize;
        for n in 0..256 {
            loose += dir_entries(&objects.join(format!("{n:02x}")))?.len();
        }
        let packs = dir_entries(&objects.join("pack"))?
            .iter()
            .filter(|p| has_suffix(p, ".pack"))
            .count();
        let settings = &self.ws.settings;
        if (loose as i128) < settings.repack_loose && (packs as i128) < settings.repack_packs {
            return Ok(false);
        }
        self.run(&[
            OsStr::new("repack"),
            "-a".as_ref(),
            "-d".as_ref(),
            "-q".as_ref(),
        ])?;
        self.run(&[OsStr::new("prune"), "--expire=1.hour.ago".as_ref()])?;
        self.made.clear();
        Ok(true)
    }

    /// Events of this store the remote does not have, as of the last merge: each segment's lines
    /// less the remote's, and each attempt file the remote lacks. A segment's lines are counted
    /// only when the memo does not know them.
    pub fn unpublished(&mut self) -> Result<i64, Error> {
        let ws = self.ws;
        let mut count: i64 = 0;
        for path in segment_files(ws)? {
            let name = relative(ws, &path);
            let lines = self.memo().lines(&name, &path)?;
            let known = self.remote_counts.get(&name).copied().unwrap_or(0);
            count += lines as i64 - known as i64;
        }
        for path in claim_files(ws)? {
            if !self.remote_counts.contains_key(&relative(ws, &path)) {
                count += 1;
            }
        }
        Ok(count)
    }

    /// What the remote holds of this store the moment a publish went through: lines per journal
    /// segment and one per attempt file, by path (Python's `published_counts`), a segment's lines
    /// counted only when the memo does not know them. Kept in `meta.json` so that the unpublished
    /// count needs no network.
    pub fn published_counts(&mut self) -> Result<Object, Error> {
        let ws = self.ws;
        let mut counts = Object::new();
        for path in segment_files(ws)? {
            let name = relative(ws, &path);
            let lines = self.memo().lines(&name, &path)? as i64;
            counts.insert(name, Value::from(lines));
        }
        for path in claim_files(ws)? {
            counts.insert(relative(ws, &path), Value::from(1));
        }
        Ok(counts)
    }
}

/// How many lines `data` holds (Python's `data.count(b"\n")`).
pub(crate) fn newlines(data: &[u8]) -> usize {
    data.iter().filter(|b| **b == b'\n').count()
}
