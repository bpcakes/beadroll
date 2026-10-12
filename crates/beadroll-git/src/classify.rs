//! What git's messages mean to bead (transliterated from `sync.py`): whether a complaint is about
//! `sync.git`'s own objects, whether a fetch found the remote missing objects or lost to another
//! process's fetch, and what a push's porcelain status line says happened.

use std::sync::LazyLock;

use regex_lite::Regex;

use crate::text::{split_lines, strip};

/// Git's own read errors about local repository objects.
static LOCAL_DAMAGE: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(concat!(
        r"object file .* is empty|corrupt or missing|cannot read existing object info|",
        r"invalid index-pack output|unable to mmap|object .* is corrupt|bad object header|",
        r"unable to read [0-9a-f]{40,64}|missing object [0-9a-f]{40,64} for refs/|",
        r"packfile .* (?:cannot be accessed|is corrupt)|bad packed object|corrupted pack|",
        r"index file .* is corrupt",
    ))
    .ok()
});

/// Remote errors reporting objects missing from the remote's repository.
static REMOTE_LACKS: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(concat!(
        r"not our ref|bad (?:tree |blob |commit )?object|unable to read [0-9a-f]{40}|",
        r"does not point to a valid object",
    ))
    .ok()
});

/// A rejection that means someone else updated the ref first.
static RACE: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(concat!(
        r"cannot lock ref|failed to update ref|failed to lock|",
        r"incorrect old value|non-fast-forward|fetch first|stale info",
    ))
    .ok()
});

/// A `[remote rejected]` that is the remote's own verdict (a hook, a rule): final.
static VERDICT: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"declined|prohibited|not allowed|denied|deny|checked out|funny|protected").ok()
});

fn search(pattern: &LazyLock<Option<Regex>>, text: &str) -> bool {
    pattern.as_ref().is_some_and(|re| re.is_match(text))
}

/// Whether a line came from the remote, relayed by git (`remote:` or `upload-pack:`), rather than
/// from git about this repository.
fn relayed(line: &str) -> bool {
    line.starts_with("remote:") || line.contains("upload-pack:")
}

/// Whether git reports damage to local objects. Relayed remote lines concern remote objects and
/// are excluded.
pub fn local_damage(said: &str) -> bool {
    let local: Vec<&str> = split_lines(said)
        .into_iter()
        .filter(|line| !relayed(line))
        .collect();
    search(&LOCAL_DAMAGE, &local.join("\n"))
}

/// Whether a failed fetch's relayed lines say the remote lacks objects its ref names.
pub fn remote_lacks_objects(said: &str) -> bool {
    let relayed: Vec<&str> = split_lines(said)
        .into_iter()
        .filter(|line| relayed(line))
        .collect();
    search(&REMOTE_LACKS, &relayed.join("\n"))
}

/// Whether a failed fetch lost to another process's fetch, each of which lands in a ref of its own
/// under `namespace` and deletes it after: git listed such a ref that was gone when it read it
/// (`bad object REF`, ref paranoia being on), or the directory of the fetch's own ref vanished
/// while git made it (`cannot lock ref 'REF'`, after git's own tries). Never a line the remote
/// relayed: its refs are not this repository's.
pub fn lost_to_other_fetch(said: &str, namespace: &str) -> bool {
    let (listed, locked) = (
        format!("bad object {namespace}"),
        format!("cannot lock ref '{namespace}"),
    );
    split_lines(said)
        .into_iter()
        .filter(|line| !relayed(line))
        .any(|line| line.contains(&listed) || line.contains(&locked))
}

/// What became of a push, as `Link.publish` reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushOutcome {
    /// The remote holds the commit.
    Ok,
    /// The remote did not apply it: someone else got there first, or it could not take the pack
    /// this time. Decide again against the new head.
    Rejected,
    /// The remote did not apply it and will not on a retry: a hook, a rule.
    Refused,
    /// Unknown: the update may still be applied.
    Uncertain,
}

/// The outcome of `git push --porcelain` of `refspec` (`<commit>:<ref>`): git's own status line
/// for that ref (`<flag>\t<from>:<to>\t<summary>`), never what stderr says, since a server hook can
/// print anything there, even after the ref was updated. Also what to keep as the error when it did
/// not go through: the status summary and every stderr line but `To ...` and hints, or the exit
/// status.
pub fn push_outcome(
    returncode: i32,
    stdout: &[u8],
    stderr: &[u8],
    refspec: &str,
) -> (PushOutcome, String) {
    let stdout = String::from_utf8_lossy(stdout);
    let status: Option<Vec<&str>> = split_lines(&stdout)
        .into_iter()
        .map(|line| line.split('\t').collect::<Vec<&str>>())
        .find(|fields| fields.get(1) == Some(&refspec));
    let flag = status.as_ref().and_then(|s| s.first()).copied();
    if returncode == 0 || matches!(flag, Some(" " | "+" | "*" | "=")) {
        return (PushOutcome::Ok, String::new());
    }
    let said = String::from_utf8_lossy(stderr);
    let summary = status
        .as_ref()
        .and_then(|s| s.get(2))
        .copied()
        .unwrap_or_default();
    let mut parts = vec![summary.to_owned()];
    for line in split_lines(&said) {
        let trimmed = strip(line);
        if !trimmed.is_empty() && !line.starts_with("To ") && !line.starts_with("hint:") {
            parts.push(trimmed.to_owned());
        }
    }
    let joined = parts.join(" ");
    let mut error = strip(&joined).to_owned();
    if error.is_empty() {
        error = format!("git push exited {returncode}");
    }
    let outcome = if flag == Some("!") {
        if summary.starts_with("[rejected]") || search(&RACE, summary) {
            PushOutcome::Rejected
        } else if summary.starts_with("[remote rejected]") {
            // The remote's own verdict on this ref is final; one that could not take the pack
            // (maintenance there, our sync repository pruned under us) answers differently after a
            // fresh fetch.
            if search(&VERDICT, summary) {
                PushOutcome::Refused
            } else {
                PushOutcome::Rejected
            }
        } else {
            PushOutcome::Uncertain
        }
    } else {
        PushOutcome::Uncertain
    };
    (outcome, error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_patterns_compile() {
        for pattern in [&LOCAL_DAMAGE, &REMOTE_LACKS, &RACE, &VERDICT] {
            assert!(pattern.is_some());
        }
    }

    #[test]
    fn a_fetch_lost_to_another_is_told_by_its_own_fetch_refs() {
        let fetch = "refs/beads/fetch/";
        let listed = "fatal: bad object refs/beads/fetch/4242-0a1b2c3d\n\
                      error: /srv/r.git did not send all necessary objects\n";
        assert!(lost_to_other_fetch(listed, fetch));
        let made = "error: cannot lock ref 'refs/beads/fetch/4243-0a1b2c3d': unable to create \
                    directory for /s/sync.git/refs/beads/fetch/4243-0a1b2c3d\n \
                    ! [new ref] refs/beads/journal -> refs/beads/fetch/4243-0a1b2c3d (unable to \
                    update local ref)\n";
        assert!(lost_to_other_fetch(made, fetch));
        let relayed = "remote: fatal: bad object refs/beads/fetch/1-00\n";
        assert!(!lost_to_other_fetch(relayed, fetch));
        let elsewhere = "fatal: bad object refs/beads/remote\n";
        assert!(!lost_to_other_fetch(elsewhere, fetch));
        let unreachable = "fatal: '/srv/gone.git' does not appear to be a git repository\n";
        assert!(!lost_to_other_fetch(unreachable, fetch));
    }

    #[test]
    fn a_status_line_decides_a_push() {
        let refspec = "abc:refs/beads/journal";
        let ok = push_outcome(
            1,
            b"To x\n \tabc:refs/beads/journal\t[new]\nDone\n",
            b"",
            refspec,
        );
        assert_eq!(ok.0, PushOutcome::Ok);
        let raced = push_outcome(
            1,
            b"!\tabc:refs/beads/journal\t[rejected] (fetch first)\n",
            b"To /x\nhint: pull\nerror: failed to push\n",
            refspec,
        );
        assert_eq!(
            raced,
            (
                PushOutcome::Rejected,
                "[rejected] (fetch first) error: failed to push".into()
            )
        );
        let refused = push_outcome(
            1,
            b"!\tabc:refs/beads/journal\t[remote rejected] (pre-receive hook declined)\n",
            b"",
            refspec,
        );
        assert_eq!(refused.0, PushOutcome::Refused);
        let unpacker = push_outcome(
            1,
            b"!\tabc:refs/beads/journal\t[remote rejected] (unpacker error)\n",
            b"",
            refspec,
        );
        assert_eq!(unpacker.0, PushOutcome::Rejected);
        assert_eq!(
            push_outcome(128, b"", b"", refspec),
            (PushOutcome::Uncertain, "git push exited 128".into())
        );
    }
}
