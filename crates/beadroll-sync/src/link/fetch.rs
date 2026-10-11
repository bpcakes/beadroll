//! Fetching the coordination ref into `sync.git`. Every bead process of a store fetches at its own
//! time, outside the store lock or under it, each into a ref of its own under `refs/beads/fetch/`
//! that it deletes once it has kept what it fetched. git's checks of a fetch read every ref, so a
//! fetch can find another process's fetch ref gone between listing it and reading it, or the
//! directory of its own removed while it makes it, which git reports as a bad object or a ref it
//! cannot lock. Such a fetch lost nothing and is tried again: the other fetch is over by then.

use std::ffi::{OsStr, OsString};
use std::time::Duration;

use beadroll_core::event::FORMAT;
use beadroll_core::json::{self, Value};
use beadroll_core::text::{repr, str_of};
use beadroll_git::{
    Hasher, IoError, Output, StoredFormat, Trees, local_damage, lost_to_other_fetch,
    remote_lacks_objects, stored_format,
};
use beadroll_store::{Failpoints, Fired, hex_token, object_format};

use super::Link;
use crate::error::Error;

/// Where each bead process of a store fetches the coordination ref, into a ref of its own.
const FETCH_REFS: &str = "refs/beads/fetch/";

/// How many times a fetch is tried that loses to other processes' fetches each time.
const FETCH_ATTEMPTS: u32 = 5;

impl Link<'_> {
    /// Fetches exactly the coordination ref: its head, or `None` if nobody has published. The
    /// manifest must name this workspace in format 1. A fetch that lost to another process's
    /// fetch is tried again, `FETCH_ATTEMPTS` times in all.
    ///
    /// Failpoints: `fetch.result` (`error(CODE)` fails the fetch) at its start; `fetch.retry`
    /// before each wait to try again (`skip` waits not at all).
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
        let mut attempt = 1;
        let (proc, mine) = loop {
            // Into a ref of this process's own: git fetch updates its destination only if it still
            // holds the value it read first, and every bead process of this store fetches at its
            // own time.
            let mine = format!("{FETCH_REFS}{}-{}", std::process::id(), hex_token(8));
            let proc = self.fetch_into(&mine)?;
            let said = String::from_utf8_lossy(&proc.stderr);
            if proc.success()
                || attempt == FETCH_ATTEMPTS
                || !lost_to_other_fetch(&said, FETCH_REFS)
            {
                break (proc, mine);
            }
            // that other fetch is over in a moment, its ref gone from the next listing
            wait_to_fetch_again(attempt, &self.ws.settings.failpoints);
            attempt += 1;
        };
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

    /// `git fetch` of the coordination ref into `mine`, from the remote defined for the link,
    /// whatever its exit status.
    fn fetch_into(&self, mine: &str) -> Result<Output, Error> {
        let refspec = format!("+{}:{mine}", self.ref_name);
        Ok(self
            .remote_env()
            .git([
                OsStr::new("fetch"),
                "--quiet".as_ref(),
                "--no-tags".as_ref(),
                "--no-write-fetch-head".as_ref(),
                "--".as_ref(),
                self.remote_name.as_ref(),
                refspec.as_ref(),
            ])
            .timeout(self.ws.settings.net_timeout)
            .run()?)
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
}

/// The wait before a fetch that lost to another process's fetch is tried again: `U(10, 50) ms ×
/// attempt`, enough for that fetch to be over, short even under the store lock. The `fetch.retry`
/// failpoint passes here (`skip` waits not at all).
fn wait_to_fetch_again(attempt: u32, failpoints: &Failpoints) {
    if failpoints.hit("fetch.retry") == Some(Fired::Skip) {
        return;
    }
    let seconds = (0.01 + fastrand::f64() * 0.04) * f64::from(attempt);
    std::thread::sleep(Duration::from_secs_f64(seconds));
}

/// Whether a failed fetch found no coordination ref: nobody has published.
fn found_nothing(stderr: &[u8]) -> bool {
    stderr.windows(15).any(|w| w == b"find remote ref")
}

/// Python's `str.strip()`.
fn py_strip(text: &str) -> &str {
    beadroll_core::text::strip(text)
}
