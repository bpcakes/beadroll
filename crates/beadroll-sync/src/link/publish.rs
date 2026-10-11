//! Publishing (Python's `Link.tree` and `Link.publish`): the tree of the manifest and the journals,
//! a commit on the fetched head, and a push whose outcome is git's own status line.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};

use beadroll_core::event::FORMAT;
use beadroll_core::json::{self, Format, Object, Value};
use beadroll_git::{PushOutcome, Trees, push_outcome};
use beadroll_store::{
    Fired, attempt_path, encode_events, hex_token, relative, repair_tail, segment_files,
};

use super::Link;
use crate::error::Error;

impl Link<'_> {
    /// The tree to publish: the manifest and every journal file, with `pending`'s attempt at the
    /// place it will live if accepted. An interrupted write is never published. Its trees are made
    /// by one `mktree --batch`: the one the last fetch started, or one started now, ready when the
    /// blobs are.
    fn tree(&mut self, pending: &[Value]) -> Result<String, Error> {
        let mut trees = self.trees.take().or_else(|| Trees::start(&self.env).ok());
        for path in segment_files(self.ws)? {
            repair_tail(self.ws, &path)?;
        }
        let mut blobs = self.local_blobs()?;
        if !pending.is_empty() {
            let place = relative(self.ws, &attempt_path(self.ws, pending));
            blobs.insert(place, self.blob_of(encode_events(pending)?)?);
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
        let mut rows = String::new();
        for (replica, groups) in &folders {
            let mut entries = groups.get("").cloned().unwrap_or_default().concat();
            if let Some(claims) = groups.get("claims") {
                let tree = self.make_tree(&mut trees, claims.concat())?;
                entries.push_str(&format!("040000 tree {tree}\tclaims\n"));
            }
            let tree = self.make_tree(&mut trees, entries)?;
            rows.push_str(&format!("040000 tree {tree}\t{replica}\n"));
        }
        let events = self.make_tree(&mut trees, rows)?;
        let mut manifest = Object::new();
        manifest.insert("format".into(), Value::from(FORMAT));
        manifest.insert("workspace".into(), Value::from(self.ws.id.as_str()));
        let mut manifest = json::write(&Value::Object(manifest), Format::SORTED_ASCII)
            .map_err(|e| Error::Validation(e.to_string()))?;
        manifest.push('\n');
        let manifest_sha = self.blob_of(manifest.into_bytes())?;
        let top =
            format!("040000 tree {events}\tevents\n100644 blob {manifest_sha}\tmanifest.json\n");
        self.make_tree(&mut trees, top)
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
            && self.tip_tree(tip)? == tree
        {
            self.last_commit = Some(tip.to_owned());
            return Ok(PushOutcome::Ok);
        }
        // there is a push to make: the `update-ref` for the ref after it starts while the commit is
        // made, and is ready when the push is done
        self.ready_refs();
        let subject = format!("bead {note} {}", hex_token(12));
        let mut args: Vec<&OsStr> = vec![OsStr::new("commit-tree"), OsStr::new(&tree)];
        if let Some(tip) = tip {
            args.extend([OsStr::new("-p"), OsStr::new(tip)]);
        }
        args.extend([OsStr::new("-m"), OsStr::new(&subject)]);
        let commit = self.git(&args, None)?;
        let refspec = format!("{commit}:{}", self.ref_name);
        // a remote's own push URL is pushed to as `git push NAME` pushes there, without the
        // `pushInsteadOf` rewrites git applies to a URL given to it: through a remote defined for
        // this push alone, with that URL as its push URL, named for this push so that no
        // configuration it inherits defines that remote already (its URLs would be added to)
        let mut own_push_url = false;
        for pushed in &self.push_urls {
            if self.ws.resolve_remote(pushed, false)? == self.url {
                own_push_url = true;
                break;
            }
        }
        let name = format!("bead-push-{}", hex_token(12));
        let defined = ["url", "pushurl"].map(|key| {
            let mut setting = OsString::from(format!("remote.{name}.{key}="));
            setting.push(&self.url);
            setting
        });
        let mut args: Vec<&OsStr> = Vec::new();
        if own_push_url {
            for setting in &defined {
                args.extend([OsStr::new("-c"), setting.as_os_str()]);
            }
        }
        // --no-verify: the user's client pre-push hooks are written for their code pushes; the
        // remote's own hooks still apply. `--`: a repository-controlled remote string is never a
        // git option.
        args.extend(["push", "--porcelain", "--no-verify", "--"].map(OsStr::new));
        args.push(if own_push_url {
            OsStr::new(&name)
        } else {
            &self.url
        });
        args.push(refspec.as_ref());
        failpoints.hit("push.before");
        let pushed = self
            .env
            .git(args)
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
        let mut judged = push_outcome(push.returncode, &push.stdout, &push.stderr, &refspec);
        if failpoints.hit("push.reply") == Some(Fired::FailReply) && judged.0 == PushOutcome::Ok {
            // the remote took it, whatever the exit status said; the answer never arrived, git's
            // status line with it
            push.returncode = 1;
            push.stdout.clear();
            push.stderr = b"connection closed after remote accepted".to_vec();
            judged = push_outcome(push.returncode, &push.stdout, &push.stderr, &refspec);
        }
        let (outcome, said) = judged;
        if outcome == PushOutcome::Ok {
            // the remote holds it, whatever went wrong afterwards
            self.keep_published(&commit)?;
            self.last_commit = Some(commit);
            failpoints.hit("push.after-ok");
            return Ok(PushOutcome::Ok);
        }
        self.last_error = said;
        Ok(outcome)
    }
}
