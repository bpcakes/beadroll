//! G13: committed `.beadroll.json` is UTF-8 under every locale, so its bytes always name the same
//! coordination ref. Run under ISO-8859-1, US-ASCII and C when available, reporting absent
//! locales as skips. An alternate interpreter launcher that encodes git arguments using the
//! locale's codeset skips affected cases outside macOS (roll-k2wy.18). Native bead runs every
//! available case.

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

/// Whether the alternate bead is an interpreter launcher on a platform where git arguments use
/// the locale's codeset. Such cases meet roll-k2wy.18 and skip; macOS always uses UTF-8. Native
/// bead is an executable, while the supported legacy launcher is a script.
fn meets_roll_k2wy_18(machine: &Machine) -> bool {
    let mut head = [0u8; 2];
    let script = std::fs::File::open(machine.executable())
        .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut head))
        .is_ok_and(|()| &head == b"#!");
    let meets = script && !cfg!(target_vendor = "apple");
    if meets {
        eprintln!(
            "skipped: Python's bead hands git the ref in the locale's codeset (roll-k2wy.18)"
        );
    }
    meets
}

/// Runs `invocation` with `LC_ALL` and `LANG` set to `locale`. Disable CPython's locale coercion
/// and UTF-8 mode when an alternate launcher is under test (PEP 538 and PEP 540).
fn under<'m>(invocation: Invocation<'m>, locale: &str) -> Invocation<'m> {
    invocation
        .env("LC_ALL", locale)
        .env("LANG", locale)
        .env("PYTHONUTF8", "0")
        .env("PYTHONCOERCECLOCALE", "0")
}

/// Lists refs under refs/beads on the remote, decoding invalid UTF-8 with replacement so a
/// locale-encoded second tracker remains visible to the test.
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
