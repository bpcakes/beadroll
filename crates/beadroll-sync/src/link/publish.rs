//! Publishing (Python's `Link.tree` and `Link.publish`): the tree of the manifest and the journals,
//! a commit on the fetched head, and a push whose outcome is git's own status line.

use std::collections::BTreeMap;
use std::ffi::OsStr;

use beadroll_core::event::FORMAT;
use beadroll_core::json::{self, Format, Object, Value};
use beadroll_git::{PushOutcome, push_outcome};
use beadroll_store::{
    Fired, REMOTE_REF, attempt_path, encode_events, hex_token, relative, repair_tail, segment_files,
};

use super::Link;
use crate::error::Error;

impl Link<'_> {
    /// The tree to publish: the manifest and every journal file, with `pending`'s attempt at the
    /// place it will live if accepted. An interrupted write is never published.
    fn tree(&self, pending: &[Value]) -> Result<String, Error> {
        for path in segment_files(self.ws)? {
            repair_tail(self.ws, &path)?;
        }
        let mut blobs = self.local_blobs()?;
        if !pending.is_empty() {
            let place = relative(self.ws, &attempt_path(self.ws, pending));
            let args = ["hash-object", "-w", "--stdin"].map(OsStr::new);
            blobs.insert(place, self.git(&args, Some(encode_events(pending)?))?);
        }
        let mut folders: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
        for (path, sha) in &blobs {
            let parts: Vec<&str> = path.split('/').collect();
            let group = if parts.len() == 4 { parts[2] } else { "" };
            folders
                .entry(parts.get(1).copied().unwrap_or_default().to_owned())
                .or_default()
                .entry(group.to_owned())
                .or_default()
                .push(format!(
                    "100644 blob {sha}\t{}\n",
                    parts.last().copied().unwrap_or_default()
                ));
        }
        let mktree = ["mktree"].map(OsStr::new);
        let mut rows = String::new();
        for (replica, groups) in &folders {
            let mut entries = groups.get("").cloned().unwrap_or_default().concat();
            if let Some(claims) = groups.get("claims") {
                let tree = self.git(&mktree, Some(claims.concat().into_bytes()))?;
                entries.push_str(&format!("040000 tree {tree}\tclaims\n"));
            }
            let tree = self.git(&mktree, Some(entries.into_bytes()))?;
            rows.push_str(&format!("040000 tree {tree}\t{replica}\n"));
        }
        let events = self.git(&mktree, Some(rows.into_bytes()))?;
        let mut manifest = Object::new();
        manifest.insert("format".into(), Value::from(FORMAT));
        manifest.insert("workspace".into(), Value::from(self.ws.id.as_str()));
        let mut manifest = json::write(&Value::Object(manifest), Format::SORTED_ASCII)
            .map_err(|e| Error::Validation(e.to_string()))?;
        manifest.push('\n');
        let args = ["hash-object", "-w", "--stdin"].map(OsStr::new);
        let manifest_sha = self.git(&args, Some(manifest.into_bytes()))?;
        let top =
            format!("040000 tree {events}\tevents\n100644 blob {manifest_sha}\tmanifest.json\n");
        self.git(&mktree, Some(top.into_bytes()))
    }

    /// A conditional update: the new commit's parent is the head fetched, and the push is a plain
    /// fast-forward, so the remote refuses it if anyone else got there first. With `fence`, a
    /// commit is pushed even when nothing changed, so that any earlier in-flight update on the same
    /// head dies. [`Link::last_error`] keeps what git said when it did not go through.
    ///
    /// Failpoints: `publish.before` and `publish.result` (`error(CODE)` fails the publish) at its
    /// start; `push.before`, `push.reply` (`fail-reply` turns an accepted push into a lost
    /// acknowledgement) and `push.after-ok` around the push.
    pub fn publish(
        &mut self,
        tip: Option<&str>,
        note: &str,
        pending: &[Value],
        fence: bool,
    ) -> Result<PushOutcome, Error> {
        let failpoints = std::sync::Arc::clone(&self.ws.settings.failpoints);
        failpoints.hit("publish.before");
        if let Some(Fired::Error(code)) = failpoints.hit("publish.result") {
            return Err(Error::injected(code, "publish.result"));
        }
        let tree = self.tree(pending)?;
        if let Some(tip) = tip
            && !fence
        {
            let current = format!("{tip}^{{tree}}");
            if self.git(&[OsStr::new("rev-parse"), current.as_ref()], None)? == tree {
                self.last_commit = Some(tip.to_owned());
                return Ok(PushOutcome::Ok);
            }
        }
        let subject = format!("bead {note} {}", hex_token(12));
        let mut args: Vec<&OsStr> = vec![OsStr::new("commit-tree"), OsStr::new(&tree)];
        if let Some(tip) = tip {
            args.extend([OsStr::new("-p"), OsStr::new(tip)]);
        }
        args.extend([OsStr::new("-m"), OsStr::new(&subject)]);
        let commit = self.git(&args, None)?;
        let refspec = format!("{commit}:{}", self.ref_name);
        // --no-verify: the user's client pre-push hooks are written for their code pushes; the
        // remote's own hooks still apply. `--`: a repository-controlled remote string is never a
        // git option.
        failpoints.hit("push.before");
        let pushed = self
            .env
            .git([
                OsStr::new("push"),
                "--porcelain".as_ref(),
                "--no-verify".as_ref(),
                "--".as_ref(),
                &self.url,
                refspec.as_ref(),
            ])
            .timeout(self.ws.settings.net_timeout)
            .reap(false)
            .run();
        let mut push = match pushed {
            Ok(push) => push,
            Err(error @ beadroll_git::Error::Timeout { .. }) => {
                self.last_error = error.to_string();
                return Ok(PushOutcome::Uncertain);
            }
            Err(error) => return Err(error.into()),
        };
        if failpoints.hit("push.reply") == Some(Fired::FailReply) && push.returncode == 0 {
            // the remote took it; the answer never arrived
            push.returncode = 1;
            push.stdout.clear();
            push.stderr = b"connection closed after remote accepted".to_vec();
        }
        let (outcome, said) = push_outcome(push.returncode, &push.stdout, &push.stderr, &refspec);
        if outcome == PushOutcome::Ok {
            // the remote holds it, whatever went wrong afterwards
            self.git(
                &[
                    OsStr::new("update-ref"),
                    REMOTE_REF.as_ref(),
                    commit.as_ref(),
                ],
                None,
            )?;
            self.last_commit = Some(commit);
            failpoints.hit("push.after-ok");
            return Ok(PushOutcome::Ok);
        }
        self.last_error = said;
        Ok(outcome)
    }
}
