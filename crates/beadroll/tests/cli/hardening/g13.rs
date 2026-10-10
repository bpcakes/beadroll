//! G13: the committed `.beadroll.json` is UTF-8 whatever the locale, so the same bytes name one
//! coordination ref everywhere. Read in the locale's encoding, its `refs/beads/équipe` became
//! another ref under ISO-8859-1, a second tracker on which two agents could both hold a claim, and
//! "not valid JSON" under US-ASCII.
//!
//! D-class (`docs/RUST-PORT.md` §4.6): the Python tests asked `python3` which encoding it would
//! read text in, and skipped unless it was the locale's. These ask the system which locales it has
//! (`locale -a`) and run under ISO-8859-1, US-ASCII and C whichever bead is under test; where the
//! system lacks the locale, the test says so on standard error and returns.
//!
//! Outside macOS, Python's bead hands git the committed ref in the locale's codeset, so that under
//! ISO-8859-1 its claim lands on a second ref and under C it fails with a traceback (roll-k2wy.18):
//! the cases that pass the ref to git say so and return when run by Python's launcher there, and
//! run whole against Rust everywhere and against Python on macOS.

use std::path::Path;
use std::process::{Command, Stdio};

use beadroll_test_support::{Invocation, Machine, Sandbox, parse_json};

/// The ref the committed `.beadroll.json` names.
const REF: &str = "refs/beads/équipe";

/// A locale's codeset as glibc compares codesets: its letters and digits, lower-cased, so that
/// macOS's `en_US.ISO8859-1` and glibc's `en_US.iso88591` are one locale. `None` for a locale
/// named without one, such as `C`.
fn codeset(name: &str) -> Option<String> {
    let (_, codeset) = name.split_once('.')?;
    let kept = codeset.chars().filter(char::is_ascii_alphanumeric);
    Some(kept.map(|c| c.to_ascii_lowercase()).collect())
}

/// The name under which `locale -a` lists `wanted`: the same language with the same codeset. When
/// the system has no such locale, or no `locale` to ask, the test has nothing to run under: this
/// says so on standard error and answers `None`.
fn installed(wanted: &str) -> Option<String> {
    let language = |name: &str| name.split('.').next().map(str::to_owned);
    let listed = Command::new("locale")
        .arg("-a")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default();
    let found = listed
        .lines()
        .find(|name| language(name) == language(wanted) && codeset(name) == codeset(wanted));
    if found.is_none() {
        eprintln!("skipped: locale -a lists no {wanted} locale here");
    }
    found.map(str::to_owned)
}

/// Whether `machine` runs Python's launcher outside macOS, where CPython encodes the arguments it
/// hands git in the locale's codeset (macOS's filesystem encoding is UTF-8 whatever the locale): a
/// case that passes the committed ref to git then meets roll-k2wy.18, says so and returns.
fn meets_roll_k2wy_18(machine: &Machine) -> bool {
    let launcher = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bead");
    let python = match (machine.executable().canonicalize(), launcher.canonicalize()) {
        (Ok(bead), Ok(launcher)) => bead == launcher,
        _ => false,
    };
    let meets = python && !cfg!(target_vendor = "apple");
    if meets {
        eprintln!(
            "skipped: Python's bead hands git the ref in the locale's codeset (roll-k2wy.18)"
        );
    }
    meets
}

/// `_g13_locale`: `invocation` under `locale`, in `LC_ALL` and `LANG`. As in the Python test,
/// CPython's own switches are off, so that it reads text in the locale's codeset instead of
/// coercing C to C.UTF-8 (PEP 538) or reading UTF-8 whatever the locale (PEP 540).
fn under<'m>(invocation: Invocation<'m>, locale: &str) -> Invocation<'m> {
    invocation
        .env("LC_ALL", locale)
        .env("LANG", locale)
        .env("PYTHONUTF8", "0")
        .env("PYTHONCOERCECLOCALE", "0")
}

/// The refs under `refs/beads` on the sandbox's remote, as `support.git("for-each-ref", ...)`
/// lists them, but read with replacement: a ref named in the locale's bytes rather than UTF-8 is the
/// second tracker the claim test looks for, and is shown, not a failure to decode.
fn coordination_refs(sandbox: &Sandbox) -> Vec<String> {
    let listed = Command::new("git")
        .args(["for-each-ref", "--format=%(refname)", "refs/beads"])
        .current_dir(sandbox.origin())
        .stdin(Stdio::null())
        .output()
        .expect("git starts");
    let said = String::from_utf8_lossy(&listed.stderr);
    assert!(listed.status.success(), "git for-each-ref: {said}");
    let refs = String::from_utf8_lossy(&listed.stdout);
    refs.split_whitespace().map(str::to_owned).collect()
}

/// The class's `setUp`: machines `g13-a` and `g13-b`, whose clones hold the same `.beadroll.json`
/// bytes (a workspace id and [`REF`], in UTF-8); the tracker made on `a` with one issue, and both
/// synced. The issue's id.
fn set_up(sandbox: &Sandbox) -> (Machine, Machine, String) {
    let config = format!(r#"{{"workspace": "ws-g13-0001", "ref": "{REF}"}}"#);
    let (a, b) = (
        Machine::new(sandbox, "g13-a"),
        Machine::new(sandbox, "g13-b"),
    );
    for machine in [&a, &b] {
        // the same committed bytes in both clones
        std::fs::write(machine.checkout().join(".beadroll.json"), &config).unwrap();
    }
    a.bead(["init", "--prefix", "ex"]);
    let created = a.bead(["create", "shared work"]).expect("the new issue");
    let key = created["id"].as_str().expect("an id").to_owned();
    a.bead(["sync"]);
    b.bead(["sync"]);
    (a, b, key)
}

/// `test_ascii_locale_reads_a_valid_utf8_config` under `wanted`: `info` reads the valid UTF-8
/// config, instead of refusing it as not valid JSON, and answers its ref.
fn reads_a_valid_utf8_config(wanted: &str) {
    let Some(locale) = installed(wanted) else {
        return;
    };
    let sandbox = crate::sandbox();
    let (_a, b, _key) = set_up(&sandbox);
    if meets_roll_k2wy_18(&b) {
        return;
    }
    let info = under(b.cmd(["info", "--json"]), &locale).run();
    assert_eq!(
        info.returncode(),
        0,
        "a valid UTF-8 config was refused under {locale}: {}",
        info.stderr
    );
    let info = parse_json(&info.stdout).unwrap();
    assert_eq!(info["ref"].as_str(), Some(REF), "{info:?}");
}

mod g13_tracked_config_is_utf8_whatever_the_locale {
    use beadroll_test_support::parse_json;

    use super::{
        REF, coordination_refs, installed, meets_roll_k2wy_18, reads_a_valid_utf8_config, set_up,
        under,
    };

    /// G13: under an ISO-8859-1 locale bead reads the UTF-8 `.beadroll.json` as UTF-8 and uses the
    /// committed ref, instead of decoding it as Latin-1 and using another.
    #[test]
    fn latin1_session_resolves_the_committed_ref() {
        let Some(latin1) = installed("en_US.ISO8859-1") else {
            return;
        };
        let sandbox = crate::sandbox();
        let (_a, b, _key) = set_up(&sandbox);
        let info = under(b.cmd(["info", "--json"]), &latin1).run();
        assert_eq!(info.returncode(), 0, "{}", info.stderr);
        let info = parse_json(&info.stdout).unwrap();
        assert_eq!(
            info["ref"].as_str(),
            Some(REF),
            "the committed ref was decoded in the locale's encoding"
        );
    }

    /// G13: a claim made under ISO-8859-1 lands on the one coordination ref, so another agent's
    /// claim under UTF-8 is refused, instead of landing on a second ref beside which it also
    /// succeeds.
    #[test]
    fn latin1_claim_excludes_a_utf8_claim() {
        let Some(latin1) = installed("en_US.ISO8859-1") else {
            return;
        };
        let sandbox = crate::sandbox();
        let (a, b, key) = set_up(&sandbox);
        if meets_roll_k2wy_18(&b) {
            return;
        }
        let claim = b.cmd(["claim", &key, "--json"]).thread("g13-b-agent");
        let first = under(claim, &latin1).run();
        assert_eq!(first.returncode(), 0, "{}", first.stderr);
        let second = a.cmd(["claim", &key, "--json"]).thread("g13-a-agent").run();
        let refs = coordination_refs(&sandbox);
        assert_ne!(
            second.returncode(),
            0,
            "two agents both hold a confirmed claim on {key}; the remote has refs {refs:?}"
        );
        assert_eq!(refs, [REF], "one tracker, one coordination ref");
    }

    /// G13: under a US-ASCII locale a valid UTF-8 `.beadroll.json` is read, not rejected as "not
    /// valid JSON".
    #[test]
    fn ascii_locale_reads_a_valid_utf8_config() {
        reads_a_valid_utf8_config("en_US.US-ASCII");
    }

    /// G13: the same under the C locale, which every system has and whose codeset is ASCII on
    /// macOS and glibc alike; few systems but macOS list `en_US.US-ASCII`.
    #[test]
    fn c_locale_reads_a_valid_utf8_config() {
        reads_a_valid_utf8_config("C");
    }
}
