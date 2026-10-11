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

mod merge;
mod publish;
mod read;
mod write;

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt as _;
use std::path::PathBuf;

use beadroll_core::event::FORMAT;
use beadroll_core::json::{self, Value};
use beadroll_core::text::{repr, str_of};
use beadroll_git::{
    CountRules, Env, Fact, Hasher, IoError, Objects, Refs, StoredFormat, Trees, local_damage,
    remote_lacks_objects, stored_format, sync_env, transport_config, transport_settings,
};
use beadroll_store::{
    Endpoint, Fired, Workspace, claim_files, dir_entries, has_suffix, hex_token, object_format,
    private_dir, read, relative, segment_files,
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
        })
    }

    /// Whether `remote` publishes a tracker at `ws`'s coordination ref, asked with `git ls-remote`
    /// as the link asks the remote: in the link's environment, from a bare repository with no
    /// remotes as `sync.git` is, so that a URL is read as the link reads it, never as the name of
    /// one of the checkout's remotes. That repository is made aside, in the temporary directory,
    /// and removed: nothing is written in the store, not even `ws`'s directory. `None` when the
    /// remote cannot be asked.
    pub fn publishes(ws: &Workspace, remote: &Endpoint) -> Result<Option<bool>, Error> {
        let ref_name = ws.ref_name()?;
        let settings = &ws.settings;
        let env = link_env(ws)?;
        let aside = env.tmp().join(format!("bead-asked-{}.git", hex_token(16)));
        let format = object_format(settings)?.unwrap_or_default();
        let mut init: Vec<OsString> = ["init", "--quiet", "--bare", "--template="]
            .map(OsString::from)
            .to_vec();
        if !format.is_empty() {
            init.push(format!("--object-format={format}").into());
        }
        init.push(aside.clone().into_os_string());
        let made = env.git(&init).run()?;
        let listed = made.success().then(|| {
            let mut vars = env.vars().clone();
            vars.insert("GIT_DIR".into(), aside.clone().into_os_string());
            env.with_vars(vars)
                .git([
                    OsStr::new("ls-remote"),
                    "--refs".as_ref(),
                    "--".as_ref(),
                    remote.url.as_os_str(),
                    ref_name.as_ref(),
                ])
                .timeout(settings.net_timeout)
                .run()
        });
        let _ = std::fs::remove_dir_all(&aside);
        Ok(match listed {
            Some(Ok(listed)) if listed.success() => Some(!listed.stdout.is_empty()),
            _ => None,
        })
    }

    /// The remote, for messages.
    pub fn url_text(&self) -> String {
        self.url.to_string_lossy().into_owned()
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

    /// Ends the gits this link keeps in `sync.git`: it is being set aside.
    fn stop_gits(&mut self) {
        self.objects.stop();
        self.hasher = None;
        self.trees = None;
        self.refs = None;
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
    /// is off.
    pub fn maintain(&self) -> Result<bool, Error> {
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
        Ok(true)
    }

    /// Fetches exactly the coordination ref: its head, or `None` if nobody has published. The
    /// manifest must name this workspace in format 1.
    ///
    /// Failpoint: `fetch.result` (`error(CODE)` fails the fetch) at its start.
    pub fn fetch(&mut self) -> Result<Option<String>, Error> {
        if let Some(Fired::Error(code)) = self.ws.settings.failpoints.hit("fetch.result") {
            return Err(Error::injected(code, "fetch.result"));
        }
        // what this fetch brings may be in a pack a `hash-object` or `mktree` started earlier
        // would not see
        self.hasher = None;
        self.trees = None;
        self.prepare_repository()?;
        // the reader starts while the fetch runs, ready when it is done: it finds names and
        // objects afresh at each question
        self.objects.start();
        // Into a ref of this process's own: git fetch updates its destination only if it still
        // holds the value it read first, and every bead process of this store fetches at its own
        // time.
        let mine = format!("refs/beads/fetch/{}-{}", std::process::id(), hex_token(8));
        let refspec = format!("+{}:{mine}", self.ref_name);
        let proc = self
            .env
            .git([
                OsStr::new("fetch"),
                "--quiet".as_ref(),
                "--no-tags".as_ref(),
                "--no-write-fetch-head".as_ref(),
                "--".as_ref(),
                &self.url,
                refspec.as_ref(),
            ])
            .timeout(self.ws.settings.net_timeout)
            .run()?;
        let nothing = !proc.success() && found_nothing(&proc.stderr);
        if proc.success() || nothing {
            // the writers start once the fetch's pack is in place, ready when the round writes
            self.hasher = Hasher::start(&self.env).ok();
            self.trees = Trees::start(&self.env).ok();
        }
        if !proc.success() {
            return self.fetch_failed(&proc.stderr);
        }
        let tip = self.fetched_tip(&mine)?;
        // keeps what was fetched reachable
        self.keep_fetched(&tip, &mine)?;
        self.check_manifest(&tip)?;
        Ok(Some(tip))
    }

    /// Whether `sync.git` was made in another object format than `wanted` (Python's `rev-parse
    /// --show-object-format` there, compared): its config's `extensions.objectformat` when that
    /// names `wanted`, else git's own answer, a failing git meaning no.
    fn stored_format_differs(&self, wanted: &str) -> Result<bool, Error> {
        match stored_format(&self.git_dir) {
            StoredFormat::Absent => Ok(false),
            StoredFormat::Named(have) if have == wanted => Ok(false),
            StoredFormat::Named(_) | StoredFormat::Unsure => {
                let have = self.run(&[OsStr::new("rev-parse"), "--show-object-format".as_ref()])?;
                Ok(have.success() && have.stdout_text() != wanted)
            }
        }
    }

    /// A missing or unfinished `sync.git` (a failed or interrupted init) completed in place, with no
    /// template (the user's hooks are for their repositories) and private modes. Two processes may
    /// race here; whichever loses finds the directory in place.
    /// `sync.git`, in the project's object format: made if it is not there, made again if it was
    /// made in another.
    fn prepare_repository(&mut self) -> Result<(), Error> {
        if self.object_format.is_none() {
            // sync.git hashes like the project, not like this machine's default
            let wanted = object_format(&self.ws.settings)?.unwrap_or_default();
            if !wanted.is_empty() && self.stored_format_differs(&wanted)? {
                // made in another format it can never exchange objects with the remote; it holds
                // nothing that is not also in events/ or on the remote, so it is made again
                self.stop_gits();
                let stale = self
                    .git_dir
                    .with_file_name(format!(".sync.{}.stale", hex_token(32)));
                match std::fs::rename(&self.git_dir, &stale) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                        return Err(IoError::at(&error, &stale).into());
                    }
                    _ => {}
                }
                let _ = std::fs::remove_dir_all(&stale);
            }
            self.object_format = Some(wanted);
        }
        self.ensure_repository()
    }

    fn ensure_repository(&self) -> Result<(), Error> {
        let complete = ["HEAD", "objects", "refs"]
            .iter()
            .all(|part| self.git_dir.join(part).exists());
        if complete {
            return Ok(());
        }
        let format = self.object_format.clone().unwrap_or_default();
        let mut args: Vec<OsString> = ["init", "--quiet", "--bare", "--template=", "--shared=0600"]
            .iter()
            .map(OsString::from)
            .collect();
        if !format.is_empty() {
            args.push(format!("--object-format={format}").into());
        }
        args.push(self.git_dir.clone().into_os_string());
        let created = self.env.git(&args).run()?;
        if !created.success() && !self.git_dir.join("HEAD").exists() {
            let said = String::from_utf8_lossy(&created.stderr);
            return Err(Error::GitError(py_strip(&said).to_owned()));
        }
        Ok(())
    }

    fn fetch_failed(&self, stderr: &[u8]) -> Result<Option<String>, Error> {
        if found_nothing(stderr) {
            return Ok(None);
        }
        let said = String::from_utf8_lossy(stderr);
        let said = py_strip(&said);
        let (shown_ref, url) = (&self.ref_name, self.url_text());
        if remote_lacks_objects(said) {
            return Err(Error::RemoteDamaged(format!(
                "{shown_ref} on {url} names objects that remote no longer has; retrying will not \
                 help. Every machine keeps its own journal: delete the ref there (`git update-ref -d \
                 {shown_ref}` in that repository), then run `bead sync` on every machine to publish \
                 again. git said: {said}"
            )));
        }
        if local_damage(said) {
            return Err(self.damage(said));
        }
        Err(Error::RemoteUnreachable(format!(
            "cannot reach the coordination remote {url}: {said}"
        )))
    }

    fn check_manifest(&mut self, tip: &str) -> Result<(), Error> {
        let (shown_ref, url) = (self.ref_name.clone(), self.url_text());
        let Some(manifest) = self.manifest(tip)? else {
            return Err(Error::ManifestMissing(format!(
                "{shown_ref} on {url} has no manifest; refusing to adopt it"
            )));
        };
        let unreadable =
            || Error::ManifestMissing(format!("{shown_ref} on {url} has an unreadable manifest"));
        let Ok(parsed) = json::parse(&manifest) else {
            return Err(unreadable());
        };
        let Value::Object(parsed) = parsed.value else {
            return Err(unreadable());
        };
        let found = parsed.get("workspace").cloned().unwrap_or(Value::Null);
        if found != Value::from(self.ws.id.as_str()) {
            let shown = str_of(&found);
            return Err(Error::WorkspaceMismatch {
                message: format!(
                    "{shown_ref} on {url} belongs to workspace {shown}, not {}; if that is this \
                     repository's tracker (renamed, or its URL spelled differently here), set \
                     BEAD_WORKSPACE={shown} or commit a .beadroll.json holding {{\"workspace\": \
                     \"{shown}\"}}",
                    self.ws.id
                ),
                found,
            });
        }
        let version = parsed.get("format").cloned().unwrap_or(Value::Null);
        // true and 1.0 equal 1 in Python, but are not format 1
        if !matches!(&version, Value::Int(n) if n.as_i64() == Some(FORMAT)) {
            return Err(Error::FormatUnsupported(format!(
                "{shown_ref} on {url} is in format {}; this version of bead reads format {FORMAT} \
                 only and will not rewrite it",
                repr(&version)
            )));
        }
        Ok(())
    }

    /// Events of this store the remote does not have, as of the last merge: each segment's lines
    /// less the remote's, and each attempt file the remote lacks.
    pub fn unpublished(&self) -> Result<i64, Error> {
        let mut count: i64 = 0;
        for path in segment_files(self.ws)? {
            let lines = newlines(&read(&path)?);
            let known = self
                .remote_counts
                .get(&relative(self.ws, &path))
                .copied()
                .unwrap_or(0);
            count += lines as i64 - known as i64;
        }
        for path in claim_files(self.ws)? {
            if !self.remote_counts.contains_key(&relative(self.ws, &path)) {
                count += 1;
            }
        }
        Ok(count)
    }
}

/// Whether a failed fetch found no coordination ref: nobody has published.
fn found_nothing(stderr: &[u8]) -> bool {
    stderr.windows(15).any(|w| w == b"find remote ref")
}

/// How many lines `data` holds (Python's `data.count(b"\n")`).
pub(crate) fn newlines(data: &[u8]) -> usize {
    data.iter().filter(|b| **b == b'\n').count()
}

/// Python's `str.strip()`.
fn py_strip(text: &str) -> &str {
    beadroll_core::text::strip(text)
}
