//! V7: spellings of one remote share an id; ports stay distinct.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::path::Path;

use beadroll_git::normalize_remote;

fn distinct(urls: &[&str]) -> usize {
    let set: BTreeSet<OsString> = urls
        .iter()
        .map(|u| normalize_remote(OsStr::new(u), Path::new("/")))
        .collect();
    set.len()
}

/// normalize_remote reduces one repository's usual spellings (scp, ssh://, https with .git and a
/// trailing slash, an https user part, host case, git://, git+ssh://) to one string, and keeps
/// distinct ports apart.
#[test]
fn spellings_of_one_remote_share_an_id_and_ports_separate() {
    let same = [
        "git@github.com:Org/Repo.git",
        "ssh://git@github.com/org/repo",
        "https://github.com/org/repo.git/",
        "https://alice@GitHub.com/org/repo",
        "git://github.com/org/repo.git",
        "git+ssh://git@github.com/org/repo",
    ];
    assert_eq!(distinct(&same), 1);
    let ports = [
        "ssh://git@host.example/org/repo",
        "ssh://git@host.example:2222/org/repo",
        "https://host.example:8443/org/repo",
    ];
    assert_eq!(distinct(&ports), 3);
}
