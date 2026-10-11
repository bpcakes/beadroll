//! What `init` asks the remote before it starts a tracker: whether it publishes one already.

use std::ffi::{OsStr, OsString};

use beadroll_store::{Endpoint, Workspace, hex_token, object_format};

use super::{Link, link_env};
use crate::error::Error;

impl Link<'_> {
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
}
