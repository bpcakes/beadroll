//! The coordination ref on one remote, reached through a private bare repository (Python's `Link`).
//! The pushed history holds the manifest and the journals and nothing else: no code commits are
//! its ancestors and no code objects are reachable from it.

mod merge;
mod publish;

use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt as _;
use std::path::PathBuf;

use beadroll_core::event::FORMAT;
use beadroll_core::json::{self, Value};
use beadroll_core::text::{repr, str_of};
use beadroll_git::{
    CountRules, Env, IoError, local_damage, remote_lacks_objects, sync_env, transport_config,
};
use beadroll_store::{
    REMOTE_REF, Workspace, claim_files, dir_entries, has_suffix, hex_token, object_format,
    private_dir, read, relative, segment_files,
};

use crate::error::Error;

/// The coordination ref on one remote, and what the last exchange with it found.
#[derive(Debug)]
pub struct Link<'a> {
    ws: &'a Workspace,
    /// The remote, as git is given it.
    pub url: OsString,
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
}

impl<'a> Link<'a> {
    /// The link of `ws` to `remote`: its ref checked, its environment built from the checkout's
    /// with the checkout's transport settings carried over.
    pub fn new(ws: &'a Workspace, remote: &OsStr) -> Result<Link<'a>, Error> {
        let ref_name = ws.ref_name()?;
        private_dir(&ws.dir)?;
        let git_dir = ws.dir.join("sync.git");
        let carried = transport_config(&ws.settings.git)?;
        let int = |text: &str| beadroll_core::text::py_int(text).map(|n| n.to_string());
        let rules = CountRules {
            is_digit: &beadroll_core::text::py_isdigit,
            int: &int,
        };
        let env = sync_env(&ws.settings.git, &git_dir, &carried, &rules)?;
        Ok(Link {
            ws,
            url: remote.to_owned(),
            ref_name,
            git_dir,
            env,
            object_format: None,
            last_error: String::new(),
            remote_counts: HashMap::new(),
            forks: Vec::new(),
            confirmed: Vec::new(),
            confirmed_events: Vec::new(),
            rebuilt: None,
            last_commit: None,
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

    /// `sync.git` is a cache of what the remote holds and of `events/`: a damaged one is set aside,
    /// kept for a look, and made again by the next fetch. Lock held.
    pub fn rebuild(&mut self) -> Result<(), Error> {
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
    pub fn fetch(&mut self) -> Result<Option<String>, Error> {
        if self.object_format.is_none() {
            // sync.git hashes like the project, not like this machine's default
            let wanted = object_format(&self.ws.settings)?.unwrap_or_default();
            let have = self.run(&[OsStr::new("rev-parse"), "--show-object-format".as_ref()])?;
            if !wanted.is_empty() && have.success() && have.stdout_text() != wanted {
                // made in another format it can never exchange objects with the remote; it holds
                // nothing that is not also in events/ or on the remote, so it is made again
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
        self.ensure_repository()?;
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
        if !proc.success() {
            return self.fetch_failed(&proc.stderr);
        }
        let tip = self.git(&[OsStr::new("rev-parse"), mine.as_ref()], None)?;
        // keeps what was fetched reachable
        self.run(&[OsStr::new("update-ref"), REMOTE_REF.as_ref(), tip.as_ref()])?;
        self.run(&[OsStr::new("update-ref"), "-d".as_ref(), mine.as_ref()])?;
        self.check_manifest(&tip)?;
        Ok(Some(tip))
    }

    /// A missing or unfinished `sync.git` (a failed or interrupted init) completed in place, with no
    /// template (the user's hooks are for their repositories) and private modes. Two processes may
    /// race here; whichever loses finds the directory in place.
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
        if stderr.windows(15).any(|w| w == b"find remote ref") {
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

    fn check_manifest(&self, tip: &str) -> Result<(), Error> {
        let (shown_ref, url) = (&self.ref_name, self.url_text());
        let object = format!("{tip}:manifest.json");
        let manifest = self.run(&[OsStr::new("cat-file"), "blob".as_ref(), object.as_ref()])?;
        if !manifest.success() {
            return Err(Error::ManifestMissing(format!(
                "{shown_ref} on {url} has no manifest; refusing to adopt it"
            )));
        }
        let unreadable =
            || Error::ManifestMissing(format!("{shown_ref} on {url} has an unreadable manifest"));
        let Ok(parsed) = json::parse(&manifest.stdout) else {
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

    /// Every local journal file in `sync.git` as a blob, by its path in the store.
    fn local_blobs(&self) -> Result<BTreeMap<String, String>, Error> {
        let mut files = segment_files(self.ws)?;
        files.extend(claim_files(self.ws)?);
        if files.is_empty() {
            return Ok(BTreeMap::new());
        }
        let mut input = Vec::new();
        for file in &files {
            input.extend_from_slice(file.as_os_str().as_bytes());
            input.push(b'\n');
        }
        let args = ["hash-object", "-w", "--no-filters", "--stdin-paths"].map(OsStr::new);
        let shas = self.git(&args, Some(input))?;
        Ok(files
            .iter()
            .zip(shas.split_whitespace())
            .map(|(file, sha)| (relative(self.ws, file), sha.to_owned()))
            .collect())
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

    /// The `sync.git` objects of `sha`, read with `cat-file blob`.
    fn blob(&self, sha: &str) -> Result<Vec<u8>, Error> {
        Ok(self
            .check(&[OsStr::new("cat-file"), "blob".as_ref(), sha.as_ref()])?
            .stdout)
    }
}

/// How many lines `data` holds (Python's `data.count(b"\n")`).
pub(crate) fn newlines(data: &[u8]) -> usize {
    data.iter().filter(|b| **b == b'\n').count()
}

/// Python's `str.strip()`.
fn py_strip(text: &str) -> &str {
    beadroll_core::text::strip(text)
}
