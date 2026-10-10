//! G62: in a checkout git refuses for dubious ownership (container mounts, shared machines, CI
//! runners), bead's error keeps git's reason and its `safe.directory` fix.

use std::process::{Command, Stdio};

use beadroll_test_support::{Machine, Sandbox};

/// `_g62_refused_checkout`: a sandbox and its machine `a`, whose checkout git refuses for dubious
/// ownership under `locale`. git's own knob `GIT_TEST_ASSUME_DIFFERENT_OWNER` runs the real
/// ownership check as if another user owned it. `None` when this git does not honour the knob, as
/// git itself answers in the C locale: the Python test skips.
fn refused_checkout(locale: &str) -> Option<(Sandbox, Machine)> {
    let sandbox = crate::sandbox();
    let mut a = Machine::new(&sandbox, "a");
    // never the developer's own config (it may hold safe.directory)
    let scratch = sandbox.root().join("global.gitconfig");
    std::fs::write(&scratch, "").unwrap();
    let extra = [
        ("GIT_CONFIG_GLOBAL", scratch.into_os_string()),
        ("GIT_CONFIG_NOSYSTEM", "1".into()),
        ("GIT_TEST_ASSUME_DIFFERENT_OWNER", "1".into()),
        ("LANG", locale.into()),
        ("LC_ALL", locale.into()),
        ("LANGUAGE", "".into()),
    ];
    a.extra
        .extend(extra.into_iter().map(|(key, value)| (key.into(), value)));
    let probe = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(a.checkout())
        .env_clear()
        .envs(a.env(None, "codex"))
        .envs([("LANG", "C"), ("LC_ALL", "C"), ("LANGUAGE", "")])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&probe.stderr);
    if !stderr.contains("dubious ownership") {
        eprintln!(
            "skipped: this git does not honour GIT_TEST_ASSUME_DIFFERENT_OWNER: {:?}",
            stderr.trim()
        );
        return None;
    }
    Some((sandbox, a))
}

/// `_g62_translates`: whether git speaks `locale`, which its message for a repository that is not
/// one shows: something, without the untranslated "fatal:".
fn translates(locale: &str) -> bool {
    let inherited = std::env::vars_os().filter(|(key, _)| {
        let key = key.as_encoded_bytes();
        !key.starts_with(b"GIT_")
    });
    let probe = Command::new("git")
        .args(["--git-dir", "/dev/null", "rev-parse"])
        .current_dir("/")
        .env_clear()
        .envs(inherited)
        .envs([
            ("LANG", locale),
            ("LC_ALL", locale),
            ("LANGUAGE", ""),
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&probe.stderr);
    !stderr.trim().is_empty() && !stderr.contains("fatal:")
}

mod g62_dubious_ownership {
    use beadroll_test_support::{Run, parse_json};

    use super::{refused_checkout, translates};

    /// `json.loads(proc.stderr)["error"]`'s code and message, which must be there.
    fn error(run: &Run) -> (String, String) {
        let document = parse_json(&run.stderr).unwrap();
        let error = document.get("error").expect("an error");
        let member = |name: &str| {
            let value = error
                .get(name)
                .unwrap_or_else(|| panic!("the error's {name}"));
            value.as_str().expect("text").to_owned()
        };
        (member("code"), member("message"))
    }

    /// G62: in a checkout git refuses for dubious ownership (container mounts, shared machines, CI
    /// runners), `info` and `sync` answer `NO_WORKSPACE` with git's reason and its
    /// `safe.directory` fix, not "not inside a git repository (or set BEAD_WORKSPACE)", which
    /// drops both.
    #[test]
    fn dubious_ownership_gives_gits_reason_and_fix() {
        let Some((_sandbox, a)) = refused_checkout("C") else {
            return;
        };
        for argv in ["info", "sync"] {
            let proc = a.run([argv, "--json"]);
            assert_eq!(proc.returncode(), 1, "{}", proc.stdout);
            let (code, message) = error(&proc);
            assert_eq!(code, "NO_WORKSPACE");
            assert!(
                message.contains("safe.directory"),
                "{argv}: git's reason and fix are dropped: {message:?}"
            );
            assert!(
                !message.contains("not inside a git repository"),
                "{message:?}"
            );
        }
    }

    /// G62: the human error carries git's `safe.directory` command as one copyable line.
    #[test]
    fn human_error_shows_the_fix_command_readably() {
        let Some((_sandbox, a)) = refused_checkout("C") else {
            return;
        };
        let proc = a.run(["info"]);
        assert_eq!(proc.returncode(), 1, "{}", proc.stdout);
        let fix = format!(
            "git config --global --add safe.directory {}",
            a.checkout().display()
        );
        assert!(
            proc.stderr.contains(&fix),
            "the fix git gives is missing or mangled: {:?}",
            proc.stderr
        );
    }

    /// G62: the same under a translated git (the `safe.directory` command itself is never
    /// translated).
    #[test]
    fn reason_survives_a_translated_git() {
        let found = ["de_DE.UTF-8", "fr_FR.UTF-8"]
            .into_iter()
            .find(|name| translates(name));
        let Some(locale) = found else {
            eprintln!("skipped: this git has no German or French messages");
            return;
        };
        let Some((_sandbox, a)) = refused_checkout(locale) else {
            return;
        };
        let proc = a.run(["info", "--json"]);
        assert_eq!(proc.returncode(), 1, "{}", proc.stdout);
        let (code, message) = error(&proc);
        assert_eq!(code, "NO_WORKSPACE");
        assert!(
            message.contains("safe.directory"),
            "under {locale}, git's reason and fix are dropped: {message:?}"
        );
    }
}
