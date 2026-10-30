//! What `init` asks the remote before it starts a tracker: whether it publishes one already.

use std::ffi::OsStr;

use beadroll_store::{Endpoint, Workspace, hex_token};

use super::{Link, defining_remote, link_env};
use crate::error::Error;
use crate::link::PushUrl;

impl Link<'_> {
    /// Whether `remote` publishes a tracker at `ws`'s coordination ref, asked with `git ls-remote`
    /// as a link asks the remote, through a remote defined for the question ([`defining_remote`]),
    /// in the link's environment but in the checkout, without `sync.git`, which it does not need:
    /// nothing is written in the store, not even `ws`'s directory. `None` when the remote cannot
    /// be asked.
    pub fn publishes(ws: &Workspace, remote: &Endpoint) -> Result<Option<bool>, Error> {
        let ref_name = ws.ref_name()?;
        let settings = &ws.settings;
        let env = link_env(ws)?;
        let mut vars = env.vars().clone();
        vars.remove(OsStr::new("GIT_DIR"));
        let name = format!("bead-{}", hex_token(12));
        let listed = defining_remote(
            &env.with_vars(vars),
            &name,
            &remote.url,
            if remote.own_push_url {
                PushUrl::Explicit
            } else {
                PushUrl::Rewrite
            },
        )
        .git([
            OsStr::new("ls-remote"),
            "--refs".as_ref(),
            "--".as_ref(),
            name.as_ref(),
            ref_name.as_ref(),
        ])
        .timeout(settings.net_timeout)
        .run();
        Ok(match listed {
            Ok(listed) if listed.success() => Some(!listed.stdout.is_empty()),
            _ => None,
        })
    }
}
